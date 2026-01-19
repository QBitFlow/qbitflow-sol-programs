use anchor_lang::prelude::*;
use anchor_lang::solana_program::hash::{hashv};
use crate::{AUTHORITY_PDA_SEED, FEE_DENOMINATOR, MAX_FEE_BPS, MIN_FEE_FOR_CONTRACT_BPS};
use crate::errors::*;



#[account]
pub struct Authority {
    pub owner: Pubkey, // The authority's public key
	pub co_signer: Pubkey,  // Required co-signer for updates

    pub bump: u8, // Bump for PDA (used to derive the PDA address)
}

impl Authority {
    pub const LEN: usize = 8 + 32 + 32 + 1; // discriminator + authority + co_signer + bump

	/// Helper to return PDA seeds for signing
	pub fn get_seeds(&self) -> [&[u8]; 2] {
		[
			AUTHORITY_PDA_SEED,  // must be &'static [u8]
			std::slice::from_ref(&self.bump),
		]
	}
}

#[derive(Accounts)]
pub struct AuthorityAndOwner<'info> {
	// Verify the authority PDA. 
	// The program expects the authority PDA to be initialized (otherwise raises a 'AccountNotInitialized' error)
	// The only way to do initialize it is by calling the initialize function with the correct PDA
	// Since we ensured the program cannot be reinitialized, it means the PDA must be the correct one (eg the program blocks any other PDA)
    #[account(
        seeds = [AUTHORITY_PDA_SEED], // Must match the seed used during initialization
        bump = authority.bump
    )]
    pub authority: Account<'info, Authority>,
    
	// Require that the authority.owner also signs the transaction
	// If a different signer is provided, the program will raise a 'ConstraintAddress' error
	// If the authority_owner is not provided, but the program detects a signer that matches authority.owner, it will work as well
    #[account(mut, address = authority.owner @ QBitFlowError::Unauthorized)]
    pub owner: Signer<'info>,
}



#[account]
pub struct Subscription {
    pub subscriber: Pubkey,
    pub next_payment_due: i64,

	pub allowance: u64, // Maximum allowance for the subscription
	pub used_allowance: u64, // Cumulative used allowance

	pub subscription_hash: [u8; 32],
	pub stopped: bool, // Whether the subscription is stopped (for pay-as-you-go)
	pub max_amount: u64, // Maximum amount allowed per period
	pub last_payment_amount: u64, // Last payment amount
    pub bump: u8,
}

impl Subscription {
	pub const LEN: usize = 8 // discriminator
		 + 32  // subscriber
		 + 8  // next_payment_due
		 + 8  // allowance
		 + 8  // used_allowance
		 + 32  // subscription_hash
		 + 1   // stopped
		 + 8   // max_amount
		 + 8   // last_payment_amount
		 + 1; // bump
}


// Events
#[event]
pub struct PaymentProcessed {
    pub uuid: [u8; 16],
}


#[event]
pub struct SubscriptionCreated {
    pub uuid: [u8; 16],
    pub next_payment_due: i64,
    pub initial_allowance: u64,
}

#[event]
pub struct SubscriptionPaymentProcessed {
    pub uuid: [u8; 16],
    pub next_payment_due: i64,
    pub remaining_allowance: u64,
}

#[event]
pub struct SubscriptionCancelled {
	pub uuid: [u8; 16],
}

#[event]
pub struct AllowanceIncreased {
    pub new_allowance: u64,
    pub uuid: [u8; 16],
}

#[event]
pub struct ComputeRefundFailed {
	pub uuid: [u8; 16],
}

#[event]
pub struct MaxAmountUpdated {
	pub uuid: [u8; 16],
	pub new_max_amount: u64,
}

// Helper functions
pub fn calculate_fee(amount: u64, fee_bps: u16, organization_fee_bps: u16) -> Result<(u64, u64)> {
    if amount == 0 {
        return err!(crate::errors::QBitFlowError::ZeroAmount);
    }

    if fee_bps > MAX_FEE_BPS || organization_fee_bps > MAX_FEE_BPS {
        return err!(crate::errors::QBitFlowError::InvalidFeePercentage);
    }

    let effective_fee_bps = if fee_bps < MIN_FEE_FOR_CONTRACT_BPS {
        MIN_FEE_FOR_CONTRACT_BPS
    } else {
        fee_bps
    };

    let owner_fee_amount = (amount as u128 * effective_fee_bps as u128 / FEE_DENOMINATOR as u128) as u64;
    
    let organization_fee_amount = if organization_fee_bps > 0 {
        let remaining_amount = amount.checked_sub(owner_fee_amount)
            .ok_or(crate::errors::QBitFlowError::Overflow)?;
        (remaining_amount as u128 * organization_fee_bps as u128 / FEE_DENOMINATOR as u128) as u64
    } else {
        0
    };

    Ok((owner_fee_amount, organization_fee_amount))
}



pub fn create_subscription_hash(
	merchant_token_account: &Pubkey,
	subscriber_token_account: &Pubkey,
	frequency: u32,
	organization_token_account: &Pubkey,
) -> [u8; 32] {
	let freq_bytes = frequency.to_le_bytes();
    return hashv(&[
        merchant_token_account.as_ref(),
        subscriber_token_account.as_ref(),
        &freq_bytes,
        organization_token_account.as_ref(),
    ]).to_bytes()
}