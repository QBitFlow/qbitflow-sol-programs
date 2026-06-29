use anchor_lang::prelude::*;
use anchor_lang::system_program;
use anchor_spl::associated_token::AssociatedToken;
use anchor_spl::token_interface::{self, Mint, TokenAccount, TokenInterface, TransferChecked};
use crate::instructions::compute_refund::{compute_refund, ComputeRefundData};
use crate::instructions::mint_check::reject_unsupported_extensions;
use crate::state::*;
use crate::errors::*;

#[derive(Accounts)]
#[instruction(amount: u64, uuid: [u8; 16], fee: Fee)]
pub struct ProcessPayment<'info> {
    #[account()]
    pub authority_and_owner: AuthorityAndOwner<'info>,

    #[account(mut)]
    pub payer: Signer<'info>,

    /// CHECK: address-constrained to authority.owner — receives the protocol fee.
    #[account(mut, address = authority_and_owner.authority.owner @ QBitFlowError::Unauthorized)]
    pub fee_recipient: UncheckedAccount<'info>,

    /// CHECK: The payer is a tx signer and reviews the merchant in their wallet before signing,
    /// and the operator (authority owner) co-signs — so both parties authorize the fee split.
    #[account(mut)]
    pub merchant: UncheckedAccount<'info>,

    /// CHECK: Same rationale as `merchant`.
    #[account(mut)]
    pub organization_fee_recipient: Option<UncheckedAccount<'info>>,

    /// CHECK: Same rationale as `merchant`. Required when `fee.referral_fee_bps > 0`.
    #[account(mut)]
    pub referrer: Option<UncheckedAccount<'info>>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(amount: u64, uuid: [u8; 16], fee: Fee)]
pub struct ProcessTokenPayment<'info> {
    #[account()]
    pub authority_and_owner: AuthorityAndOwner<'info>,

    #[account(mut)]
    pub payer: Signer<'info>,

    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = payer
    )]
    pub payer_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    /// CHECK: Payer-signed + operator co-signed (see `ProcessPayment`).
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

    /// CHECK: Payer-signed.
    #[account(mut)]
    pub organization: Option<UncheckedAccount<'info>>,

    #[account(
        init_if_needed,
        payer = authority_and_owner.owner,
        associated_token::mint = mint,
        associated_token::authority = organization
    )]
    pub organization_token_account: Option<Box<InterfaceAccount<'info, TokenAccount>>>,

    /// CHECK: Payer-signed. Required when `fee.referral_fee_bps > 0`.
    #[account(mut)]
    pub referrer: Option<UncheckedAccount<'info>>,

    /// Referrer ATA. Caller must ensure it exists (e.g. by prepending an
    /// `AssociatedToken::create` ix in the same tx) when `fee.referral_fee_bps > 0`.
    #[account(
		init_if_needed,
        payer = authority_and_owner.owner,
        associated_token::mint = mint,
        associated_token::authority = referrer,
    )]
    pub referrer_token_account: Option<Box<InterfaceAccount<'info, TokenAccount>>>,

    pub mint: Box<InterfaceAccount<'info, Mint>>,

    pub system_program: Program<'info, System>,
    pub token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
}

/// One-time native SOL payment. Operator (authority owner) + payer both sign; the operator can
/// broadcast as fee payer. Mirrors the EVM `processPayment` (native) — both parties authorize the
/// fee split, which on EVM is enforced via the owner signature and on Solana via the live owner
/// signer.
pub fn process_payment(
    ctx: Context<ProcessPayment>,
    amount: u64,
    uuid: [u8; 16],
    fee: Fee,
) -> Result<()> {
    let amounts = calculate_fee(amount, fee.platform_fee_bps, fee.organization_fee_bps, fee.referral_fee_bps, fee.referral_deadline)?;

	let org_key = if fee.organization_fee_bps > 0 {
        // Parity with EVM: an org fee with no recipient would silently shortchange the
        // merchant (fee deducted but never paid out). Require the recipient.
        Some(
            ctx.accounts.organization_fee_recipient.as_ref()
                .ok_or(QBitFlowError::MissingOrganizationAccount)?
                .key(),
        )
    } else {
        None
    };
    let referrer_key = if fee.referral_fee_bps > 0 {
        Some(
            ctx.accounts.referrer.as_ref()
                .ok_or(QBitFlowError::MissingReferrerAccount)?
                .key(),
        )
    } else {
        None
    };

    validate_fee_recipients(
        &ctx.accounts.payer.key(),
        &ctx.accounts.merchant.key(),
        &ctx.accounts.fee_recipient.key(),
        org_key.as_ref(),
        referrer_key.as_ref(),
    )?;

    // Platform fee leg (net of the referrer cut)
    system_program::transfer(
        CpiContext::new(
            ctx.accounts.system_program.key(),
            system_program::Transfer {
                from: ctx.accounts.payer.to_account_info(),
                to: ctx.accounts.fee_recipient.to_account_info(),
            },
        ),
        amounts.platform_fee_amount,
    )?;

    // Referrer leg
    if amounts.referral_fee_amount > 0 {
        let referrer = ctx.accounts.referrer.as_ref()
            .ok_or(QBitFlowError::MissingReferrerAccount)?;
        system_program::transfer(
            CpiContext::new(
                ctx.accounts.system_program.key(),
                system_program::Transfer {
                    from: ctx.accounts.payer.to_account_info(),
                    to: referrer.to_account_info(),
                },
            ),
            amounts.referral_fee_amount,
        )?;
    }

	// Organization fee leg
    if amounts.org_fee_amount > 0 {
		let organization_fee_recipient = ctx.accounts.organization_fee_recipient.as_ref()
			.ok_or(QBitFlowError::MissingOrganizationAccount)?;
        system_program::transfer(
            CpiContext::new(
                ctx.accounts.system_program.key(),
                system_program::Transfer {
                    from: ctx.accounts.payer.to_account_info(),
                    to: organization_fee_recipient.to_account_info(),
                },
            ),
            amounts.org_fee_amount,
        )?;
    }

	// Merchant leg
    system_program::transfer(
        CpiContext::new(
            ctx.accounts.system_program.key(),
            system_program::Transfer {
                from: ctx.accounts.payer.to_account_info(),
                to: ctx.accounts.merchant.to_account_info(),
            },
        ),
        amounts.merchant_amount,
    )?;

    emit!(PaymentProcessed {
        uuid,
        from: ctx.accounts.payer.key(),
        to: ctx.accounts.merchant.key(),
        mint: None,
        amounts,
		refund_amount: 0, // No gas refund for native SOL payments
    });

    Ok(())
}

// Body for the SPL one-time payment path (`process_token_payment`). Runs all four transfer legs
// + the gas refund and returns the resolved payout legs + refund so the caller can emit the
// event. (Solana has no x402 one-shot equivalent — the x402 SVM "exact" scheme can't carry
// QBitFlow's on-chain fee split; see the design notes — so this is the sole caller.)
fn run_token_payment(
    ctx: &Context<ProcessTokenPayment>,
    amount: u64,
    uuid: [u8; 16],
    fee: &Fee,
    compute_refund_params: ComputeRefundData,
) -> Result<(Amounts, u64)> {
    reject_unsupported_extensions(&ctx.accounts.mint.to_account_info())?;

    let amounts = calculate_fee(amount, fee.platform_fee_bps, fee.organization_fee_bps, fee.referral_fee_bps, fee.referral_deadline)?;

    let org_key = if fee.organization_fee_bps > 0 {
        Some(ctx.accounts.organization_token_account.as_ref()
            .ok_or(QBitFlowError::MissingOrganizationAccount)?
            .key())
    } else {
        None
    };
    let referrer_key = if fee.referral_fee_bps > 0 {
        Some(
            ctx.accounts.referrer_token_account.as_ref()
                .ok_or(QBitFlowError::MissingReferrerAccount)?
                .key(),
        )
    } else {
        None
    };
    validate_fee_recipients(
        &ctx.accounts.payer_token_account.key(),
        &ctx.accounts.merchant_token_account.key(),
        &ctx.accounts.fee_recipient_token_account.key(),
        org_key.as_ref(),
        referrer_key.as_ref(),
    )?;

    let decimals = ctx.accounts.mint.decimals;

    // Platform fee leg (net of the referrer cut)
    token_interface::transfer_checked(
        CpiContext::new(
            ctx.accounts.token_program.key(),
            TransferChecked {
                from: ctx.accounts.payer_token_account.to_account_info(),
                mint: ctx.accounts.mint.to_account_info(),
                to: ctx.accounts.fee_recipient_token_account.to_account_info(),
                authority: ctx.accounts.payer.to_account_info(),
            },
        ),
        amounts.platform_fee_amount,
        decimals,
    )?;

    // Referrer leg
    if amounts.referral_fee_amount > 0 {
        let referrer_ata = ctx.accounts.referrer_token_account.as_ref()
            .ok_or(QBitFlowError::MissingReferrerAccount)?;
        token_interface::transfer_checked(
            CpiContext::new(
                ctx.accounts.token_program.key(),
                TransferChecked {
                    from: ctx.accounts.payer_token_account.to_account_info(),
                    mint: ctx.accounts.mint.to_account_info(),
                    to: referrer_ata.to_account_info(),
                    authority: ctx.accounts.payer.to_account_info(),
                },
            ),
            amounts.referral_fee_amount,
            decimals,
        )?;
    }

    // Organization fee leg
    if amounts.org_fee_amount > 0 {
        token_interface::transfer_checked(
            CpiContext::new(
                ctx.accounts.token_program.key(),
                TransferChecked {
                    from: ctx.accounts.payer_token_account.to_account_info(),
                    mint: ctx.accounts.mint.to_account_info(),
                    to: ctx.accounts.organization_token_account.as_ref()
                        .ok_or(QBitFlowError::MissingOrganizationAccount)?
                        .to_account_info(),
                    authority: ctx.accounts.payer.to_account_info(),
                },
            ),
            amounts.org_fee_amount,
            decimals,
        )?;
    }

    // Merchant leg
    token_interface::transfer_checked(
        CpiContext::new(
            ctx.accounts.token_program.key(),
            TransferChecked {
                from: ctx.accounts.payer_token_account.to_account_info(),
                mint: ctx.accounts.mint.to_account_info(),
                to: ctx.accounts.merchant_token_account.to_account_info(),
                authority: ctx.accounts.payer.to_account_info(),
            },
        ),
        amounts.merchant_amount,
        decimals,
    )?;

    // Gas refund (best-effort; payer signs the tx so they review the refund leg in their wallet).
    // Cap disabled (u64::MAX); the SOL-side `MAX_COMPUTE_COST_LAMPORTS` ceiling still bounds it.
    let refund_amount = compute_refund(
        uuid,
        u64::MAX,
        compute_refund_params,
        CpiContext::new(
            ctx.accounts.token_program.key(),
            TransferChecked {
                from: ctx.accounts.payer_token_account.to_account_info(),
                mint: ctx.accounts.mint.to_account_info(),
                to: ctx.accounts.fee_recipient_token_account.to_account_info(),
                authority: ctx.accounts.payer.to_account_info(),
            },
        ),
        decimals,
    )?;

    Ok((amounts, refund_amount))
}

/// One-time SPL token payment. Operator (authority owner) + payer both sign.
pub fn process_token_payment(
    ctx: Context<ProcessTokenPayment>,
    amount: u64,
    uuid: [u8; 16],
    fee: Fee,
    compute_refund_params: ComputeRefundData,
) -> Result<()> {
    let (amounts, refund_amount) = run_token_payment(&ctx, amount, uuid, &fee, compute_refund_params)?;

    emit!(PaymentProcessed {
        uuid,
        from: ctx.accounts.payer.key(),
        to: ctx.accounts.merchant_token_account.key(),
        mint: Some(ctx.accounts.mint.key()),
        amounts,
        refund_amount
    });

    Ok(())
}
