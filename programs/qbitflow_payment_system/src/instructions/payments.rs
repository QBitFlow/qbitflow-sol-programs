use anchor_lang::prelude::*;
use anchor_lang::system_program;
use anchor_spl::associated_token::AssociatedToken;
use anchor_spl::token::Mint;
use anchor_spl::token::{self, Token, TokenAccount, Transfer};
use crate::instructions::compute_refund::compute_refund;
use crate::instructions::compute_refund::ComputeRefundData;
use crate::state::*;
use crate::errors::*;

#[derive(Accounts)]
#[instruction(amount: u64, fee_bps: u16, uuid: [u8; 16], organization_fee_bps: u16)]
pub struct ProcessSolPayment<'info> {
	#[account()]
	pub authority_and_owner: AuthorityAndOwner<'info>,

	// The payer is the one initiating the payment
    #[account(mut)]
    pub payer: Signer<'info>,

	/// CHECK: This must match the stored owner in the authority account. The address == authority.owner ensures it, and raises a 'ConstraintAddress' otherwise
	#[account(mut, address = authority_and_owner.authority.owner @ QBitFlowError::Unauthorized)]
	pub fee_recipient: UncheckedAccount<'info>,

    /// CHECK: This is the merchant receiving the payment, must be initialized
    #[account(mut)]
    pub merchant: UncheckedAccount<'info>,
    
    /// CHECK: This is the organization fee recipient (optional). If provided, must be initialized
    #[account(mut)]
    pub organization_fee_recipient: Option<UncheckedAccount<'info>>,
    
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(amount: u64, fee_bps: u16, uuid: [u8; 16], organization_fee_bps: u16)]
pub struct ProcessTokenPayment<'info> {
    #[account()]
	pub authority_and_owner: AuthorityAndOwner<'info>,

	// The payer is the one initiating the payment
    #[account(mut)]
    pub payer: Signer<'info>,

	// ATA of the payer
    #[account(
		mut,
		// Automatically sets the address to the associated token account for (payer, mint)
		// No need to pass the address from the client side (and even though we do, it will be ignored)
		associated_token::mint = mint,
		associated_token::authority = payer
	)]
    pub payer_token_account: Account<'info, TokenAccount>,
    

	/// CHECK: The merchant receiving the payments for this subscription (main account). Must be initialized
	#[account(mut)] // needs to be initialized
	pub merchant: UncheckedAccount<'info>,

    // The merchant is the one receiving the payment
    #[account(
		init_if_needed,
		payer = authority_and_owner.owner, // authority pays for token account initialization (merchant)
		associated_token::mint = mint,
		associated_token::authority = merchant
	)]
    pub merchant_token_account: Account<'info, TokenAccount>,
    
    // Ensures this token account belongs to authority.owner
	// No need to pass the address from the client side (and even though we do, it will be ignored)
    #[account(
        init_if_needed,
		payer = authority_and_owner.owner,
		associated_token::mint = mint,
		associated_token::authority = authority_and_owner.owner // fee recipient is authority owner
    )]
    pub fee_recipient_token_account: Account<'info, TokenAccount>,

	/// CHECK: This is the organization receiving a portion of the fees (optional). If provided, must be initialized
	#[account(mut)]
	pub organization: UncheckedAccount<'info>,
    
    #[account(
		init_if_needed,
		payer = authority_and_owner.owner, // authority pays for token account initialization (organization fee recipient)
		associated_token::mint = mint,
		associated_token::authority = organization
	)]
    pub organization_token_account: Account<'info, TokenAccount>,
    
	// The mint of the token being transferred
	// #[account()]
	pub mint: Account<'info, Mint>,
	
    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>,
	pub associated_token_program: Program<'info, AssociatedToken>,
}

pub fn process_sol_payment(
    ctx: Context<ProcessSolPayment>,
    amount: u64,
    fee_bps: u16,
    uuid: [u8; 16],
    organization_fee_bps: u16,
) -> Result<()> {
    if amount == 0 {
        return err!(QBitFlowError::ZeroAmount);
    }

    let (fee_amount, org_fee_amount) = calculate_fee(amount, fee_bps, organization_fee_bps)?;
    let remaining_amount = amount
        .checked_sub(fee_amount)
        .and_then(|x| x.checked_sub(org_fee_amount))
        .ok_or(QBitFlowError::Overflow)?;


    // Transfer fee to fee recipient
    system_program::transfer(
        CpiContext::new(
            ctx.accounts.system_program.to_account_info(),
            system_program::Transfer {
                from: ctx.accounts.payer.to_account_info(),
                to: ctx.accounts.fee_recipient.to_account_info(),
            },
        ),
        fee_amount,
    )?;


    // Transfer organization fee if applicable
    if org_fee_amount > 0 && ctx.accounts.organization_fee_recipient.is_some() {
        system_program::transfer(
            CpiContext::new(
                ctx.accounts.system_program.to_account_info(),
                system_program::Transfer {
                    from: ctx.accounts.payer.to_account_info(),
                    to: ctx.accounts.organization_fee_recipient.as_ref().unwrap().to_account_info(),
                },
            ),
            org_fee_amount,
        )?;

    }

    // Transfer remaining amount to merchant
    system_program::transfer(
        CpiContext::new(
            ctx.accounts.system_program.to_account_info(),
            system_program::Transfer {
                from: ctx.accounts.payer.to_account_info(),
                to: ctx.accounts.merchant.to_account_info(),
            },
        ),
        remaining_amount,
    )?;


    emit!(PaymentProcessed {
        uuid,
		// from: ctx.accounts.payer.key(),
        // to: ctx.accounts.merchant.key(),
        // amount: remaining_amount,
        // token_address: None, // Add this line to avoid errors

    });

    Ok(())
}

pub fn process_token_payment(
    ctx: Context<ProcessTokenPayment>,
    amount: u64,
    fee_bps: u16,
    uuid: [u8; 16],
    organization_fee_bps: u16,
	compute_refund_params: ComputeRefundData,
) -> Result<()> {
    if amount == 0 {
        return err!(QBitFlowError::ZeroAmount);
    }

    let (fee_amount, org_fee_amount) = calculate_fee(amount, fee_bps, organization_fee_bps)?;
    let remaining_amount = amount
        .checked_sub(fee_amount)
        .and_then(|x| x.checked_sub(org_fee_amount))
        .ok_or(QBitFlowError::Overflow)?;


    // Transfer fee to fee recipient
    token::transfer(
        CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.payer_token_account.to_account_info(),
                to: ctx.accounts.fee_recipient_token_account.to_account_info(),
                authority: ctx.accounts.payer.to_account_info(),
            },
        ),
        fee_amount,
    )?;
													

    // Transfer organization fee if applicable
    if org_fee_amount > 0 {
        token::transfer(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.payer_token_account.to_account_info(),
                    to: ctx.accounts.organization_token_account.to_account_info(),
                    authority: ctx.accounts.payer.to_account_info(),
                },
            ),
            org_fee_amount,
        )?;
    }


    // Transfer remaining amount to merchant
    token::transfer(
        CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.payer_token_account.to_account_info(),
                to: ctx.accounts.merchant_token_account.to_account_info(),
                authority: ctx.accounts.payer.to_account_info(),
            },
        ),
        remaining_amount,
    )?;

	// Compute refund in tokens for the authority owner to refund the compute cost paid in SOL
	let _ = compute_refund(uuid, 0, compute_refund_params, CpiContext::new(
		ctx.accounts.token_program.to_account_info(),
		Transfer {
			from: ctx.accounts.payer_token_account.to_account_info(),
			to: ctx.accounts.fee_recipient_token_account.to_account_info(),
			authority: ctx.accounts.payer.to_account_info(),
		},
	));

    emit!(PaymentProcessed {
        uuid,
        // from: ctx.accounts.payer.key(),
        // to: ctx.accounts.merchant_token_account.owner,
        // token_address: Some(ctx.accounts.payer_token_account.mint),
        // amount: remaining_amount,
    });

    Ok(())
}