use anchor_lang::{prelude::*, solana_program::program_option::COption};
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};
use crate::{
    errors::QBitFlowError,
    instructions::permit::PermitRegistry,
    state::{Authority, OwnerUpdated, CosignerUpdated},
    AUTHORITY_PDA_SEED,
    PERMIT_REGISTRY_PDA_SEED,
};

#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(
        init,
        payer = signer,
        space = Authority::LEN,
        seeds = [AUTHORITY_PDA_SEED],
        bump
    )]
    pub authority: Account<'info, Authority>,

    #[account(mut)]
    pub signer: Signer<'info>,

    pub system_program: Program<'info, System>,
}

pub fn initialize(ctx: Context<Initialize>, co_signer: Pubkey) -> Result<()> {
    require!(co_signer != Pubkey::default(), QBitFlowError::InvalidArgument);
    require!(co_signer != ctx.accounts.signer.key(), QBitFlowError::InvalidArgument);

    let authority = &mut ctx.accounts.authority;
    authority.owner = ctx.accounts.signer.key();
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
        has_one = owner @ QBitFlowError::Unauthorized,
        has_one = co_signer @ QBitFlowError::Unauthorized,
    )]
    pub authority: Account<'info, Authority>,

    #[account(mut, address = authority.owner)]
    pub owner: Signer<'info>,

    pub co_signer: Signer<'info>,
}

pub fn update_owner(ctx: Context<UpdateOwner>, new_owner: Pubkey) -> Result<()> {
    require!(new_owner != Pubkey::default(), QBitFlowError::InvalidArgument);
    require!(new_owner != ctx.accounts.authority.co_signer, QBitFlowError::InvalidArgument);

    let authority = &mut ctx.accounts.authority;
    authority.owner = new_owner;
    emit!(OwnerUpdated { new_owner });
    Ok(())
}


// Rotate the co-signer. Multi-sig: current owner + current co-signer must both sign.
// The co-signer is intended to be a cold wallet, used only for this and `update_owner`.
#[derive(Accounts)]
pub struct UpdateCoSigner<'info> {
    #[account(
        mut,
        seeds = [AUTHORITY_PDA_SEED],
        bump = authority.bump,
        has_one = owner @ QBitFlowError::Unauthorized,
        has_one = co_signer @ QBitFlowError::Unauthorized,
    )]
    pub authority: Account<'info, Authority>,

    pub owner: Signer<'info>,

    pub co_signer: Signer<'info>,
}

pub fn update_co_signer(ctx: Context<UpdateCoSigner>, new_co_signer: Pubkey) -> Result<()> {
    require!(new_co_signer != Pubkey::default(), QBitFlowError::InvalidArgument);
    require!(new_co_signer != ctx.accounts.authority.owner, QBitFlowError::InvalidArgument);
    require!(new_co_signer != ctx.accounts.authority.co_signer, QBitFlowError::InvalidArgument);

    let authority = &mut ctx.accounts.authority;
    authority.co_signer = new_co_signer;
    emit!(CosignerUpdated { new_cosigner: new_co_signer });
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

    #[account(
        mut,
        seeds = [PERMIT_REGISTRY_PDA_SEED, subscriber.key().as_ref(), mint.key().as_ref()],
        bump = permit_registry.bump
    )]
    pub permit_registry: Account<'info, PermitRegistry>,

    #[account(mut)]
    pub subscriber: Signer<'info>,

    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = subscriber
    )]
    pub subscriber_token_account: InterfaceAccount<'info, TokenAccount>,

    pub mint: InterfaceAccount<'info, Mint>,

    pub token_program: Interface<'info, TokenInterface>,
}


pub fn set_delegate(ctx: Context<SetDelegate>) -> Result<()> {
    let permit_registry = &mut ctx.accounts.permit_registry;

    let current_delegate = ctx.accounts.subscriber_token_account.delegate;
    let already_set = matches!(current_delegate, COption::Some(d) if d == ctx.accounts.authority.key());
    if already_set {
        return Ok(());
    }

    permit_registry.set_permit(
        &ctx.accounts.token_program,
        &ctx.accounts.subscriber,
        &ctx.accounts.authority,
        &ctx.accounts.subscriber_token_account,
    )
}
