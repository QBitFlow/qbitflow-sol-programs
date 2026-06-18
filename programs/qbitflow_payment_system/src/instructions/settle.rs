// PAYG batch settlement instructions: x402 + merchant-attested x402.
//
// Both mirror the EVM `settleX402Batch` / `settleMerchantX402Batch` flows:
//   - Bound to a specific `SettleMode` via the subscription_hash (set at creation).
//   - Nonce-sequenced (first/last range) to prevent replay or out-of-order settlement.
//   - `next_payment_due` is advanced by `frequency` each settle so a relayer cannot dust-drain
//     the per-period cap by stacking multiple settles in one window.
//   - Same 4-leg fund routing (fee → owner / referrer / org / merchant) as `execute_subscription`.
//
// The on-chain footprint of a batch is a single `merkle_root` commitment in the event log
// — per-receipt detail (user-signed receipts for X402, merchant-signed usage reports for
// MerchantX402) stays off-chain. On-chain authorization is the subscription digest + per-cycle
// cap + nonce range (parity with EVM, which carries no batch-settle attestation either — the
// attestation lives only on the rotation atomic-settle path, where it authorizes bypassing the
// period guard).

use anchor_lang::prelude::*;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

use crate::instructions::compute_refund::ComputeRefundData;
use crate::instructions::shared::{
    check_subscription_allowance, close_subscription,
    distribute_payment_pda, verify_subscription_digest,
};
use crate::{
    state::*,
    PERMIT_REGISTRY_PDA_SEED, SUBSCRIPTION_PDA_SEED,
};
use crate::errors::*;
use crate::permit::PermitRegistry;


// Accounts struct shared by both settle instructions. Identical layout to `ExecuteSubscription`.
#[derive(Accounts)]
#[instruction(uuid: [u8; 16])]
pub struct SettlePayg<'info> {
    #[account()]
    pub authority_and_owner: AuthorityAndOwner<'info>,

    /// CHECK: Used only to re-derive the subscription PDA. The PDA seeds bind this account to
    /// `subscription.subscriber`; mismatched values fail derivation.
    pub subscriber: UncheckedAccount<'info>,

    #[account(
        mut,
        seeds = [SUBSCRIPTION_PDA_SEED, subscriber.key().as_ref(), uuid.as_ref()],
        bump = subscription.bump
    )]
    pub subscription: Box<Account<'info, Subscription>>,

    #[account(
        mut,
        seeds = [PERMIT_REGISTRY_PDA_SEED, subscription.subscriber.as_ref(), mint.key().as_ref()],
        bump = permit_registry.bump
    )]
    pub permit_registry: Box<Account<'info, PermitRegistry>>,

    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = subscription.subscriber
    )]
    pub subscriber_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    // Verified by the subscription_hash comparison.
    #[account(mut)]
    pub merchant_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    pub mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = authority_and_owner.owner
    )]
    pub fee_recipient_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    // Verified by the subscription_hash comparison.
    #[account(mut)]
    pub organization_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    // Bound by subscription_hash. None when the subscription was created without a referrer.
    #[account(mut)]
    pub referrer_token_account: Option<Box<InterfaceAccount<'info, TokenAccount>>>,

    pub system_program: Program<'info, System>,
    pub token_program: Interface<'info, TokenInterface>,
}


/// Settle a batch of x402 micropayments.
///
/// The caller asserts the aggregate `amount` and commits a `merkle_root` of the receipt set.
/// Per-receipt detail stays off-chain; on-chain only the root + total + nonce range are
/// persisted (via the event log, not in account state). On-chain authorization is the
/// subscription digest + per-cycle cap + nonce range (parity with EVM — no batch-settle
/// attestation; cryptographic batch finality is available via the rotation atomic-settle).
pub fn settle_x402_batch(
    ctx: Context<SettlePayg>,
    uuid: [u8; 16],
    amount: u64,
    fee: Fee,
    frequency: u32,
    session_signer: Pubkey,
    first_nonce: u64,
    last_nonce: u64,
    merkle_root: [u8; 32],
    compute_refund_params: ComputeRefundData,
) -> Result<()> {
    settle_inner(
        ctx,
        SettleInnerArgs {
            uuid,
            amount,
            fee,
            frequency,
            session_signer,
            expected_mode: SettleMode::X402,
            compute_refund_params,
            first_nonce,
            last_nonce,
            merkle_root,
        },
    )
}


/// Settle a batch of merchant-attested usage reports (MerchantX402).
///
/// Same machinery as `settle_x402_batch`: the caller asserts the aggregate `amount` and commits
/// a `merkle_root` of the off-chain merchant-signed usage-report set. On-chain authorization is the
/// subscription digest + per-cycle cap + nonce range (no batch-settle attestation; parity with EVM).
pub fn settle_merchant_x402_batch(
    ctx: Context<SettlePayg>,
    uuid: [u8; 16],
    amount: u64,
    fee: Fee,
    frequency: u32,
    session_signer: Pubkey,
    first_nonce: u64,
    last_nonce: u64,
    merkle_root: [u8; 32],
    compute_refund_params: ComputeRefundData,
) -> Result<()> {
    settle_inner(
        ctx,
        SettleInnerArgs {
            uuid,
            amount,
            fee,
            frequency,
            session_signer,
            expected_mode: SettleMode::MerchantX402,
            compute_refund_params,
            first_nonce,
            last_nonce,
            merkle_root,
        },
    )
}


// ---------- Internal helper (shared by both settle entrypoints) ----------

// Both settle entrypoints share this single path. The two PAYG modes differ in only one place:
// the emitted event type (X402BatchSettled vs MerchantX402BatchSettled). `expected_mode` carries
// the discriminant; everything else is identical.
struct SettleInnerArgs {
    uuid: [u8; 16],
    amount: u64,
    fee: Fee,
    frequency: u32,
    session_signer: Pubkey,
    expected_mode: SettleMode,
    compute_refund_params: ComputeRefundData,
    first_nonce: u64,
    last_nonce: u64,
    merkle_root: [u8; 32],
}


fn settle_inner(
    ctx: Context<SettlePayg>,
    args: SettleInnerArgs,
) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;

    // 1. Mode binding — the hash recomputation below would catch a mismatched mode too, but
    //    a clean error message helps off-chain debugging.
    if SettleMode::from_u8(ctx.accounts.subscription.mode)? != args.expected_mode {
        return err!(QBitFlowError::WrongSettleMode);
    }

    // 2. Period gate. One settle per period — prevents dust-attack drain of `max_amount`.
    if now < ctx.accounts.subscription.next_payment_due {
        return err!(QBitFlowError::SettleTooEarly);
    }

    // 3. Amount limits.
    if args.amount == 0 {
        return err!(QBitFlowError::ZeroAmount);
    }
    if args.amount >= ctx.accounts.subscription.max_amount_per_cycle {
        // Strict `>=`: the refund leg needs headroom below max_amount.
        return err!(QBitFlowError::MaxAmountExceeded);
    }

    // 4. Recompute the subscription hash from caller-supplied params. Drift on any field —
    //    fees, frequency, referrer, mode, session_signer — reverts.
    let referrer_ata_key_for_hash = ctx.accounts.referrer_token_account
        .as_ref()
        .map(|a| a.key())
        .unwrap_or_default();
    let mode_byte = args.expected_mode as u8;
    verify_subscription_digest(
        &ctx.accounts.subscription,
        &ctx.accounts.merchant_token_account.key(),
        &ctx.accounts.subscriber_token_account.key(),
        args.frequency,
        &ctx.accounts.organization_token_account.key(),
        &referrer_ata_key_for_hash,
        &args.fee,
        &args.session_signer,
        mode_byte,
    )?;

    // 5. Nonce range (identical for both modes). Monotonic-greater check: relayer may skip past
    //    burned/lost nonces by submitting a later batch. Replay is prevented by the unconditional
    //    advance below.
    if args.first_nonce < ctx.accounts.subscription.next_nonce || args.last_nonce < args.first_nonce {
        return err!(QBitFlowError::InvalidNonceRange);
    }

    // 6. Allowance check (registry-wide + per-subscription).
    check_subscription_allowance(&ctx.accounts.subscription, &ctx.accounts.permit_registry, args.amount)?;

    // 8. Fee math (identical to execute_subscription).
    let amounts = calculate_fee(
        args.amount,
        args.fee.platform_fee_bps,
        args.fee.organization_fee_bps,
        args.fee.referral_fee_bps,
        args.fee.referral_deadline,
    )?;

    // 9. Fund routing + compute refund — all signed by the Authority PDA (shared helper).
    let decimals = ctx.accounts.mint.decimals;
    let refund_cap = ctx.accounts.subscription.max_amount_per_cycle
        .checked_sub(args.amount)
        .ok_or(QBitFlowError::Overflow)?;

    let token_program_ai = ctx.accounts.token_program.to_account_info();
    let mint_ai = ctx.accounts.mint.to_account_info();
    let from_ai = ctx.accounts.subscriber_token_account.to_account_info();
    let authority_ai = ctx.accounts.authority_and_owner.authority.to_account_info();
    let fee_recipient_ai = ctx.accounts.fee_recipient_token_account.to_account_info();
    let organization_ai = ctx.accounts.organization_token_account.to_account_info();
    let merchant_ai = ctx.accounts.merchant_token_account.to_account_info();
    let referrer_ai = ctx.accounts.referrer_token_account.as_ref().map(|a| a.to_account_info());
    let authority_seeds = ctx.accounts.authority_and_owner.authority.get_seeds();

    let refund_result = distribute_payment_pda(
        &token_program_ai,
        &mint_ai,
        &from_ai,
        &authority_ai,
        &authority_seeds,
        &fee_recipient_ai,
        referrer_ai.as_ref(),
        &organization_ai,
        &merchant_ai,
        &amounts,
        decimals,
        args.uuid,
        refund_cap,
        args.compute_refund_params,
    )?;

    let total_spent = args.amount.checked_add(refund_result).ok_or(QBitFlowError::Overflow)?;

    let subscription = &mut ctx.accounts.subscription;
    ctx.accounts.permit_registry.use_allowance(total_spent)?;
    subscription.used_allowance = subscription.used_allowance
        .checked_add(total_spent)
        .ok_or(QBitFlowError::Overflow)?;

    // Period-guarded normal settle == a period roll: reset the per-cycle accumulator to this
    // settle's spend (mirrors EVM `_settle` resetting `currentCycleUsed` then adding amount +
    // gas refund). Rotation atomic-settles accumulate on top of this without resetting.
    subscription.current_cycle_used = total_spent;

    // Advance nonce past the settled batch range.
    subscription.next_nonce = args.last_nonce
        .checked_add(1)
        .ok_or(QBitFlowError::Overflow)?;

    // Next period due. Anchored to `now` (not the previous due) so a relayer that lets billing
    // slip cannot drift the cadence forward.
    let next_payment_due = now
        .checked_add(args.frequency as i64)
        .ok_or(QBitFlowError::Overflow)?;
    subscription.next_payment_due = next_payment_due;

    let stopped = subscription.stopped;
    let remaining_allowance = subscription.allowance.saturating_sub(subscription.used_allowance);
    let emitted_remaining = if stopped { 0 } else { remaining_allowance };
    let subscriber_pubkey = subscription.subscriber;
    let mint_pubkey = ctx.accounts.mint.key();

    // Emit the mode-specific event before any close().
    match args.expected_mode {
        SettleMode::MerchantX402 => {
            emit!(MerchantX402BatchSettled {
                uuid: args.uuid,
                subscriber: subscriber_pubkey,
                mint: mint_pubkey,
                merkle_root: args.merkle_root,
                first_nonce: args.first_nonce,
                last_nonce: args.last_nonce,
                amounts,
                refund_amount: refund_result,
                next_payment_due,
                remaining_allowance: emitted_remaining,
            });
        }
        _ => {
            emit!(X402BatchSettled {
                uuid: args.uuid,
                subscriber: subscriber_pubkey,
                mint: mint_pubkey,
                merkle_root: args.merkle_root,
                first_nonce: args.first_nonce,
                last_nonce: args.last_nonce,
                amounts,
                refund_amount: refund_result,
                next_payment_due,
                remaining_allowance: emitted_remaining,
            });
        }
    }

    // Final-billing path for cancelled (stopped) subscriptions: this settle is the final bill,
    // then the subscription closes.
    if stopped {
        emit!(SubscriptionCancelled { uuid: args.uuid });
        let owner_info = ctx.accounts.authority_and_owner.owner.to_account_info();
        close_subscription(
            &mut ctx.accounts.subscription,
            &mut ctx.accounts.permit_registry,
            owner_info,
        )?;
    }

    Ok(())
}


// bump_session_signer_version was removed — superseded by rotate_session_signer in
// `rotate_signer.rs`. The session signer is now solely committed in subscription_hash;
// rotation rebinds the hash with the new signer (and optionally settles the final batch
// atomically).
