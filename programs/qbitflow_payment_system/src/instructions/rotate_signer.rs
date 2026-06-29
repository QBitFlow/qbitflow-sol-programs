// `rotate_session_signer` — subscriber-signed session-signer rotation, optionally
// settling the final x402 batch under the OLD signer in the same tx.
//
// Why optional atomic settle: when the relayer has accumulated x402 receipts signed by
// the OLD session signer and the period hasn't elapsed yet, plain rotation would
// invalidate those receipts (off-chain validation flips to the new signer). The atomic
// path bypasses the period guard — safe because the subscriber's signature on the
// rotation tx is itself the authorization, and the nonce range still advances atomically
// to prevent replay.
//
// Rotation rebinds the subscription_hash with the new signer. The session_signer is NOT
// stored as a struct field — the hash is the single source of truth. Off-chain readers
// reconstruct the current signer by tracking SubscriptionCreated + SessionSignerRotated
// events.
//
// Account requirements:
//   - Always required: merchant/subscriber/organization ATAs (needed to reconstruct the
//     OLD hash for verification). Referrer ATA is optional in the usual sense.
//   - Required only when `settle.is_some()`: mint, permit_registry, fee_recipient ATA,
//     token_program, instructions_sysvar.

use anchor_lang::prelude::*;
use solana_instructions_sysvar::ID as INSTRUCTIONS_ID;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

use crate::instructions::compute_refund::ComputeRefundData;
use crate::instructions::ed25519::verify_ed25519_signature;
use crate::instructions::shared::{
    batch_attestation_message, check_subscription_allowance, close_subscription,
    distribute_payment_pda, verify_subscription_digest,
};
use crate::{
    state::*,
    PERMIT_REGISTRY_PDA_SEED, SUBSCRIPTION_PDA_SEED,
};
use crate::errors::*;
use crate::permit::PermitRegistry;


#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct RotateSettleArgs {
    pub amount: u64,
    pub first_nonce: u64,
    pub last_nonce: u64,
    pub merkle_root: [u8; 32],
    /// Instruction index of the Ed25519Program precompile carrying the user attestation
    /// for the batch summary `(uuid || first_nonce || last_nonce || amount || merkle_root)`.
    /// REQUIRED when settle is used — atomic rotation cannot be unattested.
    pub attestation_ix_index: u16,
    pub compute_refund_params: ComputeRefundData,
}


#[derive(Accounts)]
#[instruction(uuid: [u8; 16])]
pub struct RotateSessionSigner<'info> {
    #[account()]
    pub authority_and_owner: AuthorityAndOwner<'info>,

    /// CHECK: derives the subscription PDA; bound by the seeds constraint.
    pub subscriber: UncheckedAccount<'info>,

    #[account(
        mut,
        seeds = [SUBSCRIPTION_PDA_SEED, subscriber.key().as_ref(), uuid.as_ref()],
        bump = subscription.bump
    )]
    pub subscription: Box<Account<'info, Subscription>>,

    /// Authorization for the rotation. Subscriber for X402, merchant for MerchantX402 — the
    /// handler checks this key against the mode-appropriate authority (no static address
    /// constraint because it depends on the subscription's mode).
    #[account(mut)]
    pub authority_signer: Signer<'info>,

    // ATAs needed for OLD-hash reconstruction (always required). Marked `mut` so the
    // atomic-settle path can move funds into / out of them; harmless on the plain-rotation
    // path (no transfers happen there).
    #[account(mut)]
    pub merchant_token_account: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut)]
    pub subscriber_token_account: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut)]
    pub organization_token_account: Option<Box<InterfaceAccount<'info, TokenAccount>>>,
    /// Optional: None when the subscription was created without a referrer.
    #[account(mut)]
    pub referrer_token_account: Option<Box<InterfaceAccount<'info, TokenAccount>>>,

    // Settle-only accounts. Must ALL be Some when `settle.is_some()`.

    pub mint: Option<Box<InterfaceAccount<'info, Mint>>>,

    #[account(mut)]
    pub permit_registry: Option<Box<Account<'info, PermitRegistry>>>,

    #[account(mut)]
    pub fee_recipient_token_account: Option<Box<InterfaceAccount<'info, TokenAccount>>>,

    /// CHECK: Solana Instructions sysvar. Address-checked when present.
    #[account(address = INSTRUCTIONS_ID)]
    pub instructions_sysvar: Option<UncheckedAccount<'info>>,

    pub token_program: Option<Interface<'info, TokenInterface>>,
}


/// X402 rotation — subscriber-authorized.
pub fn rotate_session_signer(
    ctx: Context<RotateSessionSigner>,
    uuid: [u8; 16],
    frequency: u32,
    fee: Fee,
    mode: u8,
    old_session_signer: Pubkey,
    new_session_signer: Pubkey,
    settle: Option<RotateSettleArgs>,
) -> Result<()> {
    rotate_inner(ctx, uuid, frequency, fee, mode, old_session_signer, new_session_signer, settle, SettleMode::X402)
}

/// MerchantX402 rotation — merchant-authorized.
pub fn rotate_merchant_signer(
    ctx: Context<RotateSessionSigner>,
    uuid: [u8; 16],
    frequency: u32,
    fee: Fee,
    mode: u8,
    old_session_signer: Pubkey,
    new_session_signer: Pubkey,
    settle: Option<RotateSettleArgs>,
) -> Result<()> {
    rotate_inner(ctx, uuid, frequency, fee, mode, old_session_signer, new_session_signer, settle, SettleMode::MerchantX402)
}

#[allow(clippy::too_many_arguments)]
fn rotate_inner(
    ctx: Context<RotateSessionSigner>,
    uuid: [u8; 16],
    frequency: u32,
    fee: Fee,
    mode: u8,
    old_session_signer: Pubkey,
    new_session_signer: Pubkey,
    settle: Option<RotateSettleArgs>,
    expected_mode: SettleMode,
) -> Result<()> {
    // The mode arg is committed into the subscription_hash (verified below); it must also match
    // the entrypoint's expected mode so rotate_session_signer can't act on a MerchantX402 sub.
    if SettleMode::from_u8(mode)? != expected_mode {
        return err!(QBitFlowError::WrongSettleMode);
    }

    let merchant_ata_key = ctx.accounts.merchant_token_account.key();
    let subscriber_ata_key = ctx.accounts.subscriber_token_account.key();
    let organization_ata_key = ctx.accounts.organization_token_account.as_ref().map(|a| a.key()).unwrap_or_default();
    let referrer_ata_key = ctx.accounts.referrer_token_account
        .as_ref()
        .map(|a| a.key())
        .unwrap_or_default();

    // ---- 1. Verify the OLD hash matches the stored hash ----
    // This is the on-chain proof that the caller knows the current subscription state
    // (and, crucially, knows the OLD session signer). Mismatch on any immutable param
    // including the old signer aborts the call.
    verify_subscription_digest(
        &ctx.accounts.subscription,
        &merchant_ata_key,
        &subscriber_ata_key,
        frequency,
        &organization_ata_key,
        &referrer_ata_key,
        &fee,
        &old_session_signer,
        mode,
    )?;

    // ---- Authorization: the key being rotated belongs to the subscriber (X402) or the
    // merchant (MerchantX402). The signer (and any atomic-settle attestation) is checked
    // against this authority. ----
    let authority = if expected_mode == SettleMode::X402 {
        ctx.accounts.subscription.subscriber
    } else {
        ctx.accounts.merchant_token_account.owner
    };
    if ctx.accounts.authority_signer.key() != authority {
        return err!(QBitFlowError::Unauthorized);
    }

    let mut settled_in_same_tx = false;
    let mut subscription_closed_during_settle = false;

    if let Some(settle_args) = settle {
        // ---- 2. Atomic settle path (X402 and MerchantX402). ----
        // Bypasses the period guard (the rotation attestation authorizes it) and uses a CUMULATIVE
        // per-cycle cap so repeated rotate+settle within a window can't exceed max_amount. It DOES
        // roll the period (next_payment_due = now + frequency) below — combined with the cumulative
        // cap this caps billing at max_amount per frequency window across boundaries too.

        // All settle-only accounts must be present.
        let mint = ctx.accounts.mint.as_ref()
            .ok_or(QBitFlowError::MissingSettleAccount)?;
        let fee_recipient_token_account = ctx.accounts.fee_recipient_token_account.as_ref()
            .ok_or(QBitFlowError::MissingSettleAccount)?;
        let instructions_sysvar = ctx.accounts.instructions_sysvar.as_ref()
            .ok_or(QBitFlowError::MissingSettleAccount)?;
        let token_program = ctx.accounts.token_program.as_ref()
            .ok_or(QBitFlowError::MissingSettleAccount)?;

        // permit_registry PDA: verify the derivation manually (Option<> accounts can't
        // use seeds constraints that reference other Option<> accounts cleanly).
        let permit_registry = ctx.accounts.permit_registry.as_ref()
            .ok_or(QBitFlowError::MissingSettleAccount)?;
        let (expected_permit_pda, _) = Pubkey::find_program_address(
            &[
                PERMIT_REGISTRY_PDA_SEED,
                ctx.accounts.subscription.subscriber.as_ref(),
                mint.key().as_ref(),
            ],
            &crate::ID,
        );
        if permit_registry.key() != expected_permit_pda {
            return err!(QBitFlowError::InvalidSubscriptionParameters);
        }

        // Amount bounds — CUMULATIVE against the per-cycle cap (strict `>=` leaves refund
        // headroom). Because we don't roll the period below, all rotate+atomic-settle calls
        // within a period share this single ceiling.
        if settle_args.amount == 0 {
            return err!(QBitFlowError::ZeroAmount);
        }
        let cycle_before = ctx.accounts.subscription.current_cycle_used;
        if cycle_before
            .checked_add(settle_args.amount)
            .ok_or(QBitFlowError::Overflow)?
            >= ctx.accounts.subscription.max_amount_per_cycle
        {
            return err!(QBitFlowError::MaxAmountExceeded);
        }

        // Nonce range (loosened: `first_nonce >= next_nonce`).
        if settle_args.first_nonce < ctx.accounts.subscription.next_nonce
            || settle_args.last_nonce < settle_args.first_nonce
        {
            return err!(QBitFlowError::InvalidNonceRange);
        }

        // Attestation REQUIRED for atomic settle — authorizes bypassing the period guard.
        // Signed by the authority (subscriber for X402, merchant for MerchantX402) over the
        // OLD signer's batch summary.
        let attestation_message = batch_attestation_message(
            &uuid,
            settle_args.first_nonce,
            settle_args.last_nonce,
            settle_args.amount,
            &settle_args.merkle_root,
        );
        verify_ed25519_signature(
            &instructions_sysvar.to_account_info(),
            settle_args.attestation_ix_index,
            &authority,
            &attestation_message,
        ).map_err(|_| error!(QBitFlowError::UserAttestationInvalid))?;

        // Allowance checks (registry-wide + per-subscription).
        check_subscription_allowance(&ctx.accounts.subscription, permit_registry, settle_args.amount)?;

        // Fee math.
        let amounts = calculate_fee(settle_args.amount, fee.platform_fee_bps, fee.organization_fee_bps, fee.referral_fee_bps, fee.referral_deadline)?;

        // Fund routing + compute refund — all signed by the Authority PDA (shared helper).
        // CUMULATIVE refund headroom: max - (already-billed this cycle + this settle's amount),
        // mirroring EVM `_settleMem` + `_refundGas` (currentCycleUsed already includes amount).
        let decimals = mint.decimals;
        let refund_cap = ctx.accounts.subscription.max_amount_per_cycle
            .checked_sub(cycle_before)
            .ok_or(QBitFlowError::Overflow)?
            .checked_sub(settle_args.amount)
            .ok_or(QBitFlowError::Overflow)?;

        let token_program_ai = token_program.to_account_info();
        let mint_ai = mint.to_account_info();
        let from_ai = ctx.accounts.subscriber_token_account.to_account_info();
        let authority_ai = ctx.accounts.authority_and_owner.authority.to_account_info();
        let fee_recipient_ai = fee_recipient_token_account.to_account_info();
        let organization_ai = ctx.accounts.organization_token_account.as_ref().map(|a| a.to_account_info());
        let merchant_ai = ctx.accounts.merchant_token_account.to_account_info();
        let referrer_ai = ctx.accounts.referrer_token_account.as_ref().map(|a| a.to_account_info());
        let authority_seeds = ctx.accounts.authority_and_owner.authority.get_seeds();

        let refund_amount = distribute_payment_pda(
            &token_program_ai,
            &mint_ai,
            &from_ai,
            &authority_ai,
            &authority_seeds,
            &fee_recipient_ai,
            referrer_ai.as_ref(),
            organization_ai.as_ref(),
            &merchant_ai,
            &amounts,
            decimals,
            uuid,
            refund_cap,
            settle_args.compute_refund_params,
        )?;

        let total_spent = settle_args.amount
            .checked_add(refund_amount)
            .ok_or(QBitFlowError::Overflow)?;

        let mint_key = mint.key();

        let subscription = &mut ctx.accounts.subscription;
        ctx.accounts.permit_registry.as_mut().unwrap().use_allowance(total_spent)?;
        subscription.used_allowance = subscription.used_allowance
            .checked_add(total_spent)
            .ok_or(QBitFlowError::Overflow)?;

        // Accumulate the per-cycle counter (NOT a reset) so further rotate+atomic-settles in this
        // period stay bounded by max_amount. Counts amount + refund EXACTLY ONCE — the prior code
        // also added `amount` before this, double-counting it against the cumulative cap.
        subscription.current_cycle_used = cycle_before
            .checked_add(total_spent)
            .ok_or(QBitFlowError::Overflow)?;

        subscription.next_nonce = settle_args.last_nonce
            .checked_add(1)
            .ok_or(QBitFlowError::Overflow)?;

        // Roll the period from now. Combined with the cumulative (non-reset) cap above, this bounds
        // total billing to `max_amount_per_cycle` per `frequency` window even across a period
        // boundary: a normal settle can't fire right after this rotation (it's gated until the new
        // due), closing the prior ~2× cap leak (relevant to MerchantX402, where the merchant signs
        // the attestation). The accrued usage was billed here, so the shifted next-due is not a loss.
        let now = Clock::get()?.unix_timestamp;
        let next_payment_due = now
            .checked_add(frequency as i64)
            .ok_or(QBitFlowError::Overflow)?;
        subscription.next_payment_due = next_payment_due;

        let stopped = subscription.stopped;
        let remaining_allowance = subscription.allowance.saturating_sub(subscription.used_allowance);
        let subscriber_pubkey = subscription.subscriber;

        let emitted_remaining = if stopped { 0 } else { remaining_allowance };
        if expected_mode == SettleMode::X402 {
            emit!(X402BatchSettled {
                uuid,
                subscriber: subscriber_pubkey,
                mint: mint_key,
                merkle_root: settle_args.merkle_root,
                first_nonce: settle_args.first_nonce,
                last_nonce: settle_args.last_nonce,
                amounts: amounts.clone(),
                refund_amount,
                next_payment_due,
                remaining_allowance: emitted_remaining,
            });
        } else {
            emit!(MerchantX402BatchSettled {
                uuid,
                subscriber: subscriber_pubkey,
                mint: mint_key,
                merkle_root: settle_args.merkle_root,
                first_nonce: settle_args.first_nonce,
                last_nonce: settle_args.last_nonce,
                amounts: amounts.clone(),
                refund_amount,
                next_payment_due,
                remaining_allowance: emitted_remaining,
            });
        }

        // Stopped final-billing path: if cancel was called before this rotation, settle
        // is the final bill and the subscription closes. No rotation possible after that.
        if stopped {
            emit!(SubscriptionCancelled { uuid });
            let owner_info = ctx.accounts.authority_and_owner.owner.to_account_info();
            close_subscription(
                &mut ctx.accounts.subscription,
                ctx.accounts.permit_registry.as_mut().unwrap(),
                owner_info,
            )?;
            subscription_closed_during_settle = true;
        }

        settled_in_same_tx = true;
    }

    // ---- 3. Rotation: rewrite the stored hash to bind the new signer. ----
    if !subscription_closed_during_settle {
        let new_hash = create_subscription_hash(
            &merchant_ata_key,
            &subscriber_ata_key,
            frequency,
            &organization_ata_key,
            &referrer_ata_key,
            fee.referral_fee_bps,
            fee.referral_deadline,
            &new_session_signer,
            mode,
        );
        ctx.accounts.subscription.digest = new_hash;

        emit!(SessionSignerRotated {
            uuid,
            old_session_signer,
            new_session_signer,
            settled_in_same_tx,
        });
    }

    Ok(())
}
