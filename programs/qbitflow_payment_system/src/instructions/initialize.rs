use anchor_spl::token::{Token, TokenAccount};
use anchor_lang::{prelude::*, solana_program::program_option::COption};
use crate::{errors::QBitFlowError, instructions::permit::PermitRegistry, state::Authority, AUTHORITY_PDA_SEED, PERMIT_REGISTRY_PDA_SEED};

#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(
        init,
        payer = signer,
        space = Authority::LEN, // discriminator + authority + bump
        seeds = [AUTHORITY_PDA_SEED], // Seed for PDA
        bump
    )]
    pub authority: Account<'info, Authority>,
    
    #[account(mut)]
    pub signer: Signer<'info>,

    pub system_program: Program<'info, System>,
}

pub fn initialize(ctx: Context<Initialize>, co_signer: Pubkey) -> Result<()> {
	let authority = &mut ctx.accounts.authority;
	authority.owner = ctx.accounts.signer.key(); // Set the authority to the signer
	authority.co_signer = co_signer;
	authority.bump = ctx.bumps.authority;

	Ok(())
}


#[derive(Accounts)]
pub struct UpdateOwner<'info> {
	#[account(
		mut,
		seeds = [AUTHORITY_PDA_SEED],
		bump = authority.bump,
		has_one = owner @ QBitFlowError::Unauthorized, // Ensure the signer is the current owner
		has_one = co_signer @ QBitFlowError::Unauthorized,
	)]
	pub authority: Account<'info, Authority>,

	#[account(mut, address = authority.owner)]
	pub owner: Signer<'info>, // current owner must sign

	pub co_signer: Signer<'info>,  // Co-signer must sign (no account needed!)
}

pub fn update_owner(ctx: Context<UpdateOwner>, new_owner: Pubkey) -> Result<()> {
	let authority = &mut ctx.accounts.authority;
	authority.owner = new_owner; // Update the owner to the new owner
	Ok(())
}


#[derive(Accounts)]
pub struct SetDelegate<'info> {
	#[account(
		seeds = [AUTHORITY_PDA_SEED],
		bump = authority.bump,
	)]
	pub authority: Account<'info, Authority>,

	#[account(mut, address = authority.owner @ QBitFlowError::Unauthorized)]
	pub authority_owner: Signer<'info>,

	// Permit registry, must match the subscriber of the subscription
	#[account(
		mut,
		seeds = [PERMIT_REGISTRY_PDA_SEED, subscriber.key().as_ref(), mint.key().as_ref()],
		bump = permit_registry.bump
	)]
	pub permit_registry: Account<'info, PermitRegistry>,

	// The user creating the subscription
	// Needs to sign the transaction
    #[account(mut)]
    pub subscriber: Signer<'info>,

	// The subscriber's token account from which payments will be made
	// This ensures the token account belongs to the subscriber (no need to provide)
    #[account(
		mut,
		associated_token::mint = mint,
		associated_token::authority = subscriber
	)]
    pub subscriber_token_account: Account<'info, TokenAccount>,

	// The token mint used for this subscription
    pub mint: Account<'info, anchor_spl::token::Mint>,

	pub token_program: Program<'info, Token>,
}


pub fn set_delegate(ctx: Context<SetDelegate>) -> Result<()> {
    let permit_registry = &mut ctx.accounts.permit_registry;

	// Get the current delegate of the susbcriber's token account (ctx.accounts.subscriber_token_account.delegate). If the current delegate is the authority PDA, nothing to do.
	// Otherwise (if it's None or a different delegate), we can proceed to set the delegate to the authority PDA
	match ctx.accounts.subscriber_token_account.delegate {
		COption::None => {
			// No delegate set, proceed to set the delegate to the authority PDA
			permit_registry.set_permit(
				&ctx.accounts.token_program,
				&ctx.accounts.subscriber,
				&ctx.accounts.authority,
				&ctx.accounts.subscriber_token_account
			)?;
		}
		COption::Some(current_delegate) => {
			if current_delegate == ctx.accounts.authority.key() {
				// Delegate is already set to the authority PDA, nothing to do
				return Ok(());
			} else {
				// Different delegate set, proceed to update the delegate to the authority PDA
				permit_registry.set_permit(
				&ctx.accounts.token_program,
				&ctx.accounts.subscriber,
				&ctx.accounts.authority,
				&ctx.accounts.subscriber_token_account
			)?;
			}
		}
	}


    Ok(())
}

