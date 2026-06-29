use anchor_lang::prelude::*;
use anchor_spl::associated_token::AssociatedToken;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface, TransferChecked};
use crate::instructions::compute_refund::{compute_refund, compute_refund_best_effort, ComputeRefundData};
use crate::instructions::mint_check::reject_unsupported_extensions;
use crate::instructions::shared::{
    check_subscription_allowance, close_subscription, distribute_payment_pda,
    verify_subscription_digest,
};
use crate::{
    state::*,
    MAX_INCREASE_FACTOR_BPS, FEE_DENOMINATOR, MIN_FREQUENCY, MAX_REFERRAL_FEE_BPS,
    AUTHORITY_PDA_SEED, PERMIT_REGISTRY_PDA_SEED, SUBSCRIPTION_PDA_SEED,
};
use crate::errors::*;
use crate::permit::PermitRegistry;

#[derive(Accounts)]
#[instruction(uuid: [u8; 16])]
pub struct CreateSubscription<'info> {
    #[account()]
    pub authority_and_owner: AuthorityAndOwner<'info>,

    // Permit registry per (subscriber, mint).
    #[account(
        init_if_needed,
        payer = authority_and_owner.owner,
        space = PermitRegistry::LEN,
        seeds = [PERMIT_REGISTRY_PDA_SEED, subscriber.key().as_ref(), mint.key().as_ref()],
        bump
    )]
    pub permit_registry: Box<Account<'info, PermitRegistry>>,

    // Subscription PDA scoped to (subscriber, uuid) so UUIDs cannot collide across
    // different subscribers and cannot be front-run by another user creating one first.
    #[account(
        init,
        payer = authority_and_owner.owner,
        space = Subscription::LEN,
        seeds = [SUBSCRIPTION_PDA_SEED, subscriber.key().as_ref(), uuid.as_ref()],
        bump
    )]
    pub subscription: Box<Account<'info, Subscription>>,

    #[account(mut)]
    pub subscriber: Signer<'info>,

    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = subscriber
    )]
    pub subscriber_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    /// CHECK: Merchant payee. Validated only via the subscription_hash; the merchant_token_account
    /// is what actually receives funds.
    #[account(mut)]
    pub merchant: UncheckedAccount<'info>,

    #[account(
        init_if_needed,
        payer = authority_and_owner.owner,
        associated_token::mint = mint,
        associated_token::authority = merchant
    )]
    pub merchant_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        init_if_needed,
        payer = authority_and_owner.owner,
        associated_token::mint = mint,
        associated_token::authority = authority_and_owner.owner
    )]
    pub fee_recipient_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    pub mint: Box<InterfaceAccount<'info, Mint>>,

    /// CHECK: Organization fee payee; same binding model as `merchant`.
    #[account(mut)]
    pub organization: Option<UncheckedAccount<'info>>,

    #[account(
        init_if_needed,
        payer = authority_and_owner.owner,
        associated_token::mint = mint,
        associated_token::authority = organization
    )]
    pub organization_token_account: Option<Box<InterfaceAccount<'info, TokenAccount>>>,

    /// CHECK: Referrer payee. Bound via subscription_hash on creation. Required when
    /// `referral_fee_bps > 0`; passed as None when no referrer.
    #[account(mut)]
    pub referrer: Option<UncheckedAccount<'info>>,

    #[account(
        init_if_needed,
        payer = authority_and_owner.owner,
        associated_token::mint = mint,
        associated_token::authority = referrer,
    )]
    pub referrer_token_account: Option<Box<InterfaceAccount<'info, TokenAccount>>>,

    pub system_program: Program<'info, System>,
    pub token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
}

#[derive(Accounts)]
#[instruction(amount: u64, uuid: [u8; 16])]
pub struct ExecuteSubscription<'info> {
    #[account()]
    pub authority_and_owner: AuthorityAndOwner<'info>,

    /// CHECK: Used only to re-derive the subscription PDA. The subscription PDA's seeds bind
    /// this account to `subscription.subscriber`; if they disagree, derivation fails.
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

    // Verified by the subscription_hash comparison below.
    #[account(mut)]
    pub merchant_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    pub mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = authority_and_owner.owner
    )]
    pub fee_recipient_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    // Verified by the subscription_hash comparison below.
    #[account(mut)]
    pub organization_token_account: Option<Box<InterfaceAccount<'info, TokenAccount>>>,

    // Bound by subscription_hash. None when the subscription was created without a referrer
    // (in which case the hash commits to `Pubkey::default()`).
    #[account(mut)]
    pub referrer_token_account: Option<Box<InterfaceAccount<'info, TokenAccount>>>,

    pub system_program: Program<'info, System>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub token_program: Interface<'info, TokenInterface>,
}

// User-initiated cancel — the SUBSCRIBER signs, NOT the operator (parity with EVM, where a user
// can cancel unilaterally). `authority` is read-only (to locate the rent-refund destination) and
// `owner` is the non-signer rent destination (the operator funded account creation, so closed-
// account rent returns to them).
#[derive(Accounts)]
#[instruction(uuid: [u8; 16])]
pub struct CancelSubscription<'info> {
    #[account(seeds = [AUTHORITY_PDA_SEED], bump = authority.bump)]
    pub authority: Account<'info, Authority>,

    /// CHECK: rent-refund destination; must equal `authority.owner`. NOT a signer.
    #[account(mut, address = authority.owner @ QBitFlowError::Unauthorized)]
    pub owner: UncheckedAccount<'info>,

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
        address = subscription.subscriber @ QBitFlowError::Unauthorized
    )]
    pub subscriber: Signer<'info>,

    pub mint: Box<InterfaceAccount<'info, Mint>>,
}


#[derive(Accounts)]
#[instruction(uuid: [u8; 16])]
pub struct UpdateMaxAmount<'info> {
    #[account()]
    pub authority_and_owner: AuthorityAndOwner<'info>,

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

    #[account(mut, address = subscription.subscriber @ QBitFlowError::Unauthorized)]
    pub subscriber: Signer<'info>,

    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = subscription.subscriber
    )]
    pub subscriber_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    pub mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = authority_and_owner.owner
    )]
    pub fee_recipient_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    pub token_program: Interface<'info, TokenInterface>,
}


// `force_cancel_subscription` is the merchant-requested escape hatch. Signed by the
// authority owner ONLY (no co-signer). QBitFlow's backend exposes this to merchants who
// need to cancel on a user's behalf (refunds, abuse, etc.). The co-signer is kept cold
// and reserved for ownership rotation only.
#[derive(Accounts)]
#[instruction(uuid: [u8; 16])]
pub struct ForceCancelSubscription<'info> {
    #[account()]
    pub authority_and_owner: AuthorityAndOwner<'info>,

    /// CHECK: Used only to re-derive the subscription PDA.
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

    pub mint: Box<InterfaceAccount<'info, Mint>>,
}


#[derive(Accounts)]
#[instruction(uuid: [u8; 16], new_allowance: u64)]
pub struct ExtendAllowance<'info> {
    #[account()]
    pub authority_and_owner: AuthorityAndOwner<'info>,

    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = authority_and_owner.owner
    )]
    pub fee_recipient_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

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

    #[account(mut, address = subscription.subscriber @ QBitFlowError::Unauthorized)]
    pub subscriber: Signer<'info>,

    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = subscriber
    )]
    pub subscriber_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    pub mint: Box<InterfaceAccount<'info, Mint>>,

    pub token_program: Interface<'info, TokenInterface>,
}


/// Create a subscription. Mode is bound at creation and immutable thereafter.
///
/// Subscribers commit to a per-period `max_amount` cap, a `fee_bps` / `organization_fee_bps`
/// rate, the referral terms, **the settle mode**, and the **off-chain session signer** — all
/// baked into a domain-tagged hash on the subscription PDA. Every settle (regular or PAYG)
/// re-derives this hash and rejects any drift, so even a compromised relayer cannot vary the
/// fee schedule, switch the mode, or substitute the receipt-signing key.
///
/// Mode:
///   - `Regular` (0): billed via `execute_subscription`. First bill is due immediately.
///   - `X402` (1): off-chain receipts batched into `settle_x402_batch`. First bill deferred
///     by `frequency` so the merchant can accrue a period of usage.
///   - `MerchantX402` (2): merchant-attested batches settled via `settle_merchant_x402_batch`.
///     The merkle leaves are merchant-signed usage reports. First bill deferred by `frequency`.
///
/// `session_signer == Pubkey::default()` means "subscriber signs receipts directly" (mostly
/// useful for `Regular`; X402 / MerchantX402 normally delegate to a hot key — the merchant's
/// key for MerchantX402).
pub fn create_subscription(
    ctx: Context<CreateSubscription>,
    uuid: [u8; 16],
    amount: u64,
    max_amount_per_cycle: u64,
    frequency: u32,
    fee: Fee,
    allowance: u64,
    compute_refund_params: ComputeRefundData,
    mode: u8,
    session_signer: Pubkey,
) -> Result<()> {
    if frequency < MIN_FREQUENCY {
        return err!(QBitFlowError::InvalidFrequency);
    }
    if amount == 0 {
        return err!(QBitFlowError::ZeroAmount);
    }
    if max_amount_per_cycle <= amount {
        return err!(QBitFlowError::InvalidAmount);
    }
    if allowance == 0 || allowance < max_amount_per_cycle {
        return err!(QBitFlowError::InvalidAmount);
    }

    // Reject garbage SettleMode values (anything outside 0/1/2).
    let settle_mode = SettleMode::from_u8(mode)?;

	// Reject a non-default session_signer for Regular subscriptions. The session_signer is
	if settle_mode == SettleMode::Regular && session_signer != Pubkey::default() {
		return err!(QBitFlowError::InvalidSessionSigner);
	}

    // Token-2022 extension policy: reject mints we can't account for safely.
    reject_unsupported_extensions(&ctx.accounts.mint.to_account_info())?;

    // Referral validation. The referrer ATA is committed into the subscription_hash
    // (Pubkey::default() when absent), so the referrer terms become immutable for the
    // life of the subscription.
    if fee.referral_fee_bps > MAX_REFERRAL_FEE_BPS {
        return err!(QBitFlowError::InvalidReferralFeePercentage);
    }
    let now = Clock::get()?.unix_timestamp;
    let referrer_ata_key = if fee.referral_fee_bps > 0 {
        let ata = ctx.accounts.referrer_token_account.as_ref()
            .ok_or(QBitFlowError::MissingReferrerAccount)?;
        if fee.referral_deadline <= now {
            return err!(QBitFlowError::InvalidReferralDeadline);
        }
        ata.key()
    } else {
		if fee.referral_deadline != 0 {
			return err!(QBitFlowError::InvalidReferralDeadline);
		}
        Pubkey::default()
    };

    // Full fee-recipient distinctness (parity with EVM `_validateFeeRecipients`) + merchant must
    // differ from the subscriber. Compares ATA keys (1:1 with (owner, mint)).
    let org_key = if fee.organization_fee_bps > 0 {
        Some(ctx.accounts.organization_token_account.as_ref().ok_or(QBitFlowError::MissingOrganizationAccount)?.key())
    } else {
        None
    };
    let referrer_key = if fee.referral_fee_bps > 0 { Some(referrer_ata_key) } else { None };
    validate_fee_recipients(
        &ctx.accounts.subscriber_token_account.key(),
        &ctx.accounts.merchant_token_account.key(),
        &ctx.accounts.fee_recipient_token_account.key(),
        org_key.as_ref(),
        referrer_key.as_ref(),
    )?;

    let permit_registry = &mut ctx.accounts.permit_registry;

    // Idempotent — fine for both first-time init and existing-registry re-use.
    permit_registry.bump = ctx.bumps.permit_registry;

    permit_registry.add_allowance(
        allowance,
        &ctx.accounts.subscriber,
        &ctx.accounts.authority_and_owner.authority,
        &ctx.accounts.token_program,
        &ctx.accounts.subscriber_token_account,
    )?;

    let subscription = &mut ctx.accounts.subscription;

    // Regular mode bills the first cycle atomically below (parity with the EVM `Regular` path),
    // so its next bill is a full period out. PAYG modes defer the first bill: usage accrues
    // off-chain and is settled at period end. Either way the next due is `now + frequency`.
    let next_payment_due = now.checked_add(frequency as i64).ok_or(QBitFlowError::Overflow)?;

    subscription.subscriber = ctx.accounts.subscriber.key();
    subscription.next_payment_due = next_payment_due;
    subscription.allowance = allowance;
    subscription.used_allowance = 0;
    subscription.stopped = false;
    subscription.max_amount_per_cycle = max_amount_per_cycle;
    subscription.bump = ctx.bumps.subscription;
    subscription.mode = mode;
    subscription.next_nonce = 0;
    subscription.current_cycle_used = 0;
    // session_signer is committed in subscription_hash; not stored as a field.

    subscription.digest = create_subscription_hash(
        &ctx.accounts.merchant_token_account.key(),
        &ctx.accounts.subscriber_token_account.key(),
        frequency,
        &org_key.unwrap_or_default(),
        &referrer_ata_key,
        fee.referral_fee_bps,
        fee.referral_deadline,
        &session_signer,
        mode,
    );

    let decimals = ctx.accounts.mint.decimals;

    let refund_result = if settle_mode == SettleMode::Regular {
        // Regular: bill the first cycle atomically at creation (parity with EVM `createSubscription`,
        // which settles cycle 1 in-body). Pull `amount` via the just-approved PDA delegate, split it
        // 4 ways, and take the compute refund — same path as `execute_subscription`.
        let amounts = calculate_fee(
            amount,
            fee.platform_fee_bps,
            fee.organization_fee_bps,
            fee.referral_fee_bps,
            fee.referral_deadline,
        )?;
        let refund_cap = max_amount_per_cycle.checked_sub(amount).ok_or(QBitFlowError::Overflow)?;

        let token_program_ai = ctx.accounts.token_program.to_account_info();
        let mint_ai = ctx.accounts.mint.to_account_info();
        let from_ai = ctx.accounts.subscriber_token_account.to_account_info();
        let authority_ai = ctx.accounts.authority_and_owner.authority.to_account_info();
        let fee_recipient_ai = ctx.accounts.fee_recipient_token_account.to_account_info();
        let organization_ai = ctx.accounts.organization_token_account.as_ref().map(|a| a.to_account_info());
        let merchant_ai = ctx.accounts.merchant_token_account.to_account_info();
        let referrer_ai = ctx.accounts.referrer_token_account.as_ref().map(|a| a.to_account_info());
        let authority_seeds = ctx.accounts.authority_and_owner.authority.get_seeds();

        let refund = distribute_payment_pda(
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
            compute_refund_params,
        )?;

        let total_spent = amount.checked_add(refund).ok_or(QBitFlowError::Overflow)?;
        subscription.current_cycle_used = total_spent;

		emit!(SubscriptionPaymentProcessed {
			uuid,
			subscriber: subscription.subscriber,
			mint: ctx.accounts.mint.key(),
			next_payment_due,
			remaining_allowance: allowance.checked_sub(total_spent).ok_or(QBitFlowError::Overflow)?,
			amounts: amounts,
			refund_amount: refund,
		});

        total_spent
    } else {
        // PAYG: no first bill (usage accrues off-chain, billed at period end). Just pull the
        // creation-tx compute refund — the subscriber is the live signer, so authority = subscriber.
        compute_refund(
            uuid,
            max_amount_per_cycle.checked_sub(amount).ok_or(QBitFlowError::Overflow)?,
            compute_refund_params,
            CpiContext::new(
                ctx.accounts.token_program.key(),
                TransferChecked {
                    from: ctx.accounts.subscriber_token_account.to_account_info(),
                    mint: ctx.accounts.mint.to_account_info(),
                    to: ctx.accounts.fee_recipient_token_account.to_account_info(),
                    authority: ctx.accounts.subscriber.to_account_info(),
                },
            ),
            decimals,
        )?
    };

    permit_registry.use_allowance(refund_result)?;
    subscription.used_allowance = refund_result;

    emit!(SubscriptionCreated {
        uuid,
        subscriber: subscription.subscriber,
        merchant_token_account: ctx.accounts.merchant_token_account.key(),
        mint: ctx.accounts.mint.key(),
        next_payment_due,
        initial_allowance: subscription.allowance.saturating_sub(subscription.used_allowance),
        max_amount_per_cycle,
        frequency,
        fee_bps: fee.platform_fee_bps,
        organization_fee_bps: fee.organization_fee_bps,
        referral_fee_bps: fee.referral_fee_bps,
        referral_deadline: fee.referral_deadline,
        mode,
        session_signer,
    });

    Ok(())
}



/// Execute a billing on a Regular subscription.
///
/// Re-derives and compares the subscription hash to reject any drift in merchant /
/// subscriber / org ATA / frequency / fee rates / referral terms / mode / session signer.
/// Pulls `amount` from the subscriber via the program PDA delegate, splits into owner fee +
/// referral fee (if active) + org fee + merchant payment, and updates the registry.
/// Rejects PAYG modes — use `settle_x402_batch` or `settle_merchant_x402_batch` instead.
pub fn execute_subscription(
    ctx: Context<ExecuteSubscription>,
    amount: u64,
    uuid: [u8; 16],
    frequency: u32,
    fee: Fee,
    compute_refund_params: ComputeRefundData,
) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let subscription = &mut ctx.accounts.subscription;

	// PAYG subscriptions settle via the dedicated handlers. If the caller passed mode != 0,
    // the hash check below would already fail — but bail early with a clearer error.
    if SettleMode::from_u8(subscription.mode)? != SettleMode::Regular {
        return err!(QBitFlowError::WrongSettleMode);
    }

    if now < subscription.next_payment_due {
        return err!(QBitFlowError::PaymentNotDueYet);
    }
    if amount == 0 {
        return err!(QBitFlowError::ZeroAmount);
    }
    if amount >= subscription.max_amount_per_cycle {
        // Strict `>=`: the refund leg needs headroom below max_amount.
        return err!(QBitFlowError::MaxAmountExceeded);
    }


    // Re-derive the hash from caller-supplied referral terms. Hash mismatch reverts.
    let referrer_ata_key_for_hash = ctx.accounts.referrer_token_account
        .as_ref()
        .map(|a| a.key())
        .unwrap_or_default();
    verify_subscription_digest(
        subscription,
        &ctx.accounts.merchant_token_account.key(),
        &ctx.accounts.subscriber_token_account.key(),
        frequency,
        &ctx.accounts.organization_token_account.as_ref().map(|a| a.key()).unwrap_or_default(),
        &referrer_ata_key_for_hash,
        &fee,
        &Pubkey::default(),
        subscription.mode,
    )?;

    check_subscription_allowance(subscription, &ctx.accounts.permit_registry, amount)?;

    let amounts = calculate_fee(amount, fee.platform_fee_bps, fee.organization_fee_bps, fee.referral_fee_bps, fee.referral_deadline)?;

    let decimals = ctx.accounts.mint.decimals;
    // Per-cycle refund headroom (max_amount - amount); after the reset below this equals
    // max_amount - current_cycle_used (EVM `_refundGas` headroom model).
    let refund_cap = subscription.max_amount_per_cycle
        .checked_sub(amount)
        .ok_or(QBitFlowError::Overflow)?;

    let token_program_ai = ctx.accounts.token_program.to_account_info();
    let mint_ai = ctx.accounts.mint.to_account_info();
    let from_ai = ctx.accounts.subscriber_token_account.to_account_info();
    let authority_ai = ctx.accounts.authority_and_owner.authority.to_account_info();
    let fee_recipient_ai = ctx.accounts.fee_recipient_token_account.to_account_info();
    let organization_ai = ctx.accounts.organization_token_account.as_ref().map(|a| a.to_account_info());
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
        organization_ai.as_ref(),
        &merchant_ai,
        &amounts,
        decimals,
        uuid,
        refund_cap,
        compute_refund_params,
    )?;

    let total_spent = amount.checked_add(refund_result).ok_or(QBitFlowError::Overflow)?;
    let permit_registry = &mut ctx.accounts.permit_registry;
    permit_registry.use_allowance(total_spent)?;
    subscription.used_allowance = subscription.used_allowance
        .checked_add(total_spent)
        .ok_or(QBitFlowError::Overflow)?;

    // Period roll: reset the per-cycle accumulator to this settle's spend (parity with EVM
    // `_settle`, which sets currentCycleUsed = 0 then adds amount + gas refund, and with
    // settle_inner). The previous code accumulated across periods — a divergence.
    subscription.current_cycle_used = total_spent;

    // Regular subscriptions always advance by a full period from the previously-scheduled
    // due time (the PAYG drift-correction branch lived here in the old code; PAYG modes now
    // route through settle_x402_batch / settle_merchant_x402_batch instead).
    let next_payment_due = subscription.next_payment_due
        .checked_add(frequency as i64)
        .ok_or(QBitFlowError::Overflow)?;
    subscription.next_payment_due = next_payment_due;

    let remaining_allowance = subscription.allowance.saturating_sub(subscription.used_allowance);

    emit!(SubscriptionPaymentProcessed {
        uuid,
        subscriber: subscription.subscriber,
        mint: ctx.accounts.mint.key(),
        next_payment_due,
        remaining_allowance,
        amounts: amounts,
        refund_amount: refund_result,
    });

    Ok(())
}


/// Cancel a Regular subscription. PAYG subscriptions use `cancel_pay_as_you_go_subscription`.
///
/// - **Regular**: closes immediately, provided `now < next_payment_due` (this stops users from
///   dodging a bill that is already due). If the cancel comes in after the due date, the
///   merchant has to route through `force_cancel_subscription`.
/// - **X402** / **MerchantX402**: two-phase. This call marks `stopped = true`; the next
///   `settle_x402_batch` or `settle_merchant_x402_batch` runs the final billing for accrued
///   usage and then closes the subscription. The subscriber still owes for whatever has
///   accrued off-chain in the current period.
pub fn cancel_subscription(
    ctx: Context<CancelSubscription>,
    uuid: [u8; 16],
) -> Result<()> {
    let subscription = &mut ctx.accounts.subscription;

    // Regular only — PAYG routes through `cancel_pay_as_you_go_subscription`.
    if SettleMode::from_u8(subscription.mode)? != SettleMode::Regular {
        return err!(QBitFlowError::WrongSettleMode);
    }

    let current_time = Clock::get()?.unix_timestamp;
    if current_time >= subscription.next_payment_due {
        return err!(QBitFlowError::CannotCancelActiveSubscription);
    }

    emit!(SubscriptionCancelled { uuid });

    let owner_info = ctx.accounts.owner.to_account_info();
    close_subscription(
        &mut ctx.accounts.subscription,
        &mut ctx.accounts.permit_registry,
        owner_info,
    )?;

    Ok(())
}


/// Cancel a PAYG (X402 / MerchantX402) subscription. Two-phase: marks `stopped = true`; the
/// next `settle_x402_batch` / `settle_merchant_x402_batch` runs the final billing for accrued usage
/// and then closes the subscription. Mirrors EVM `cancelPayAsYouGoSubscription`.
pub fn cancel_pay_as_you_go_subscription(
    ctx: Context<CancelSubscription>,
    uuid: [u8; 16],
) -> Result<()> {
    let subscription = &mut ctx.accounts.subscription;

    if !SettleMode::from_u8(subscription.mode)?.is_payg() {
        return err!(QBitFlowError::WrongSettleMode);
    }

    // Idempotent: already stopped → no-op (parity with EVM's early-return).
    if subscription.stopped {
        return Ok(());
    }

    subscription.stopped = true;
    emit!(SubscriptionStopped { uuid });
    Ok(())
}


/// Force-cancel a subscription (authority-only).
///
/// Signed by the authority owner ONLY — no co-signer required. This is the merchant-or-user
/// escape hatch routed through QBitFlow's backend, NOT a cold-wallet operation. Bypasses the
/// `next_payment_due` check that `cancel_subscription` enforces, so a stuck or compromised
/// subscriber can still have their state cleaned up.
pub fn force_cancel_subscription(
    ctx: Context<ForceCancelSubscription>,
    uuid: [u8; 16],
) -> Result<()> {
    emit!(SubscriptionForceCancelled { uuid });

    let owner_info = ctx.accounts.authority_and_owner.owner.to_account_info();
    close_subscription(
        &mut ctx.accounts.subscription,
        &mut ctx.accounts.permit_registry,
        owner_info,
    )?;

    Ok(())
}


/// Set a new allowance (replaces the current one). Subscriber-signed.
///
/// Modeled on the EVM `permit` "increase = replace" semantic. Revokes the prior allowance
/// in the registry, sets the new one, and resets `used_allowance` for this subscription
/// (the registry's `total_used` is decremented accordingly so the overall accounting is
/// consistent).
pub fn extend_allowance(
    ctx: Context<ExtendAllowance>,
    uuid: [u8; 16],
    new_allowance: u64,
    compute_refund_params: ComputeRefundData,
) -> Result<()> {
    if new_allowance == 0 {
        return err!(QBitFlowError::ZeroAmount);
    }

    let subscription = &mut ctx.accounts.subscription;

    if new_allowance <= subscription.allowance {
        return err!(QBitFlowError::InvalidAmount);
    }
    if new_allowance < subscription.max_amount_per_cycle {
        return err!(QBitFlowError::InvalidAmount);
    }

    let permit_registry = &mut ctx.accounts.permit_registry;
    permit_registry.revoke_allowance(subscription)?;

    subscription.allowance = new_allowance;
    subscription.used_allowance = 0;

    permit_registry.add_allowance(
        new_allowance,
        &ctx.accounts.subscriber,
        &ctx.accounts.authority_and_owner.authority,
        &ctx.accounts.token_program,
        &ctx.accounts.subscriber_token_account,
    )?;

    let decimals = ctx.accounts.mint.decimals;
    // Best-effort compute refund (parity with EVM `bestEffort=true`): never lock the user out of
    // extending their allowance just because the refund would breach a cap. Headroom is bounded
    // by BOTH the per-cycle cap and the registry-wide allowance so the accounting below always
    // fits. (Bug fix: this was `max - max = 0`, which made every refund revert.)
    let registry_headroom = permit_registry.total_allowance.saturating_sub(permit_registry.total_used);
    let refund_cap = subscription.max_amount_per_cycle
        .checked_sub(subscription.current_cycle_used)
        .ok_or(QBitFlowError::Overflow)?
        .min(registry_headroom);
    let refund_result = compute_refund_best_effort(
        uuid,
        refund_cap,
        compute_refund_params,
        CpiContext::new(
            ctx.accounts.token_program.key(),
            TransferChecked {
                from: ctx.accounts.subscriber_token_account.to_account_info(),
                mint: ctx.accounts.mint.to_account_info(),
                to: ctx.accounts.fee_recipient_token_account.to_account_info(),
                authority: ctx.accounts.subscriber.to_account_info(),
            },
        ),
        decimals,
    );

    subscription.used_allowance = refund_result;
    permit_registry.use_allowance(refund_result)?;
    subscription.current_cycle_used = subscription.current_cycle_used
        .checked_add(refund_result)
        .ok_or(QBitFlowError::Overflow)?;

    emit!(AllowanceExtended {
        uuid,
        new_amount: new_allowance,
    });

    Ok(())
}


/// Update the per-period cap. Subscriber-signed. Supports both directions (parity with EVM
/// `updateMaxAmountPerCycle`):
///   - **Increase**: bounded at 3× the CURRENT `max_amount` (phishing guard) — larger jumps need
///     multiple txs across periods.
///   - **Decrease**: allowed down to (strictly above) `last_payment_amount`, leaving headroom for
///     this tx's own compute-refund leg (capped at `new_max_amount - last_payment_amount`).
pub fn update_max_amount_per_cycle(
    ctx: Context<UpdateMaxAmount>,
    uuid: [u8; 16],
    new_max_amount: u64,
    compute_refund_params: ComputeRefundData,
) -> Result<()> {
    let subscription = &mut ctx.accounts.subscription;

    if new_max_amount == 0 {
        return err!(QBitFlowError::ZeroAmount);
    }
    if new_max_amount == subscription.max_amount_per_cycle {
        return err!(QBitFlowError::NoChange);
    }

    if new_max_amount > subscription.max_amount_per_cycle {
        // Increase: bounded at 3× the current cap (phishing guard).
        let max_allowed = (subscription.max_amount_per_cycle as u128)
            .checked_mul(MAX_INCREASE_FACTOR_BPS as u128)
            .ok_or(QBitFlowError::Overflow)?
            .checked_div(FEE_DENOMINATOR as u128)
            .ok_or(QBitFlowError::Overflow)?;
        if (new_max_amount as u128) > max_allowed {
            return err!(QBitFlowError::MaxIncreaseExceeded);
        }
    } else {
        // Decrease: floor strictly above what's already been billed this cycle so this tx's own
        // compute-refund leg still has headroom (parity with EVM `MaxBelowCurrentCycle`).
        if new_max_amount <= subscription.current_cycle_used {
            return err!(QBitFlowError::MaxAmountInvalid);
        }
    }

    // The cap is NOT part of the subscription_hash on Solana (the subscriber signs the whole tx),
    // so updating it requires no digest rewrite — just write the field.
    subscription.max_amount_per_cycle = new_max_amount;

    let decimals = ctx.accounts.mint.decimals;
    let permit_registry = &mut ctx.accounts.permit_registry;
    // Best-effort compute refund (parity with EVM `bestEffort=true`): never lock the user out of
    // changing their cap — important for the security-relevant DECREASE path. Headroom is bounded
    // by BOTH the (new) per-cycle cap and the registry-wide allowance so the accounting fits.
    // (Bug fix: refund_cap was computed AFTER overwriting the field → always 0 → always reverted.)
    let registry_headroom = permit_registry.total_allowance.saturating_sub(permit_registry.total_used);
    let refund_cap = new_max_amount
        .checked_sub(subscription.current_cycle_used)
        .ok_or(QBitFlowError::Overflow)?
        .min(registry_headroom);
    let refund_result = compute_refund_best_effort(
        uuid,
        refund_cap,
        compute_refund_params,
        CpiContext::new(
            ctx.accounts.token_program.key(),
            TransferChecked {
                from: ctx.accounts.subscriber_token_account.to_account_info(),
                mint: ctx.accounts.mint.to_account_info(),
                to: ctx.accounts.fee_recipient_token_account.to_account_info(),
                authority: ctx.accounts.subscriber.to_account_info(),
            },
        ),
        decimals,
    );

    permit_registry.use_allowance(refund_result)?;
    subscription.used_allowance = subscription.used_allowance
        .checked_add(refund_result)
        .ok_or(QBitFlowError::Overflow)?;
    subscription.current_cycle_used = subscription.current_cycle_used
        .checked_add(refund_result)
        .ok_or(QBitFlowError::Overflow)?;

    emit!(MaxAmountPerCycleUpdated {
        uuid,
        new_max_amount_per_cycle: new_max_amount,
    });

    Ok(())
}
