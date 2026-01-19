
use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount};
use anchor_spl::token::Approve;
use crate::errors::*;
use crate::state::{Authority, Subscription};




// Permit registry is per subscriber, and per token mint
// This keeps track of the total allowance, and the total used across all subscriptions for a given subscriber and token mint
// This allows the program to manage multiple subscriptions for the same user
// This way, when a new subscription is added, the new allowance is added to the total remaining allowance (and does not override the existing allowance, so the previous allowances remain valid)

#[account]
pub struct PermitRegistry {
    pub total_allowance: u64, // sum of all subscription max allowances
    pub total_used: u64,      // cumulative tokens spent
	pub bump: u8,
}

impl PermitRegistry {
	pub const LEN: usize = 8  // discriminator
			+ 8  // total_allowance
			+ 8 // total_used
			+ 1; // bump

	// Add a new allowance when a subscription is created
	// This computes the new effective allowance (total_allowance - total_used + new allowance)
	// and approves the program's delegate PDA to spend that amount from the user's token account
	#[inline(never)]
	pub fn add_allowance<'info>(
		&mut self, 
		allowance_amount: u64, 
		subscriber: &Signer<'info>, 
		authority: &Account<'info, Authority>,
		token_program: &Program<'info, Token>,
		subscriber_token_account: &Account<'info, TokenAccount>
	) -> Result<()> {
		// Now, we need to approve the new allowance with the token program
		// This new allowance increases the total budget available for subscriptions
		// So the new allowance to approve is the remaining allowance (= current total allowance - used allowance) + the new allowance
 
		// Compute the new effective allowance (total_allowance - total_used)
		let effective_allowance = self.total_allowance.checked_sub(self.total_used).ok_or(QBitFlowError::Overflow)?;

		// New effective allowance after adding this subscription
		let new_effective_allowance = effective_allowance.checked_add(allowance_amount).ok_or(QBitFlowError::Overflow)?;


		// Approve the program's delegate PDA to spend the new effective allowance
		// Note: The actual CPI to approve should be done in the instruction handler where the
		// user's token account and authority are available.
		token::approve(
			CpiContext::new(
				// The token program account should be passed in the instruction context
				// Here we just use a placeholder; replace with actual account info in the handler
				token_program.to_account_info(), 
				Approve {
					to: subscriber_token_account.to_account_info(), // User's token account (placeholder)
					delegate: authority.to_account_info(), // Program's delegate PDA (placeholder)
					authority: subscriber.to_account_info(), // User as authority
				},
			),
			new_effective_allowance,
		)?;


		// Allowance approved successfully, now update the registry
		self.total_allowance = self.total_allowance.checked_add(allowance_amount).ok_or(QBitFlowError::Overflow)?;
		Ok(())
	}


	// Check if there is enough allowance to cover a payment of the given amount
	pub fn has_enough_allowance(&self, amount: u64) -> bool {
		self.total_used.checked_add(amount).unwrap_or(u64::MAX) <= self.total_allowance
	}


	// Use some of the allowance when a subscription payment is executed
	// This does NOT modify the token account's delegate or allowance, it only updates the registry
	pub fn use_allowance(&mut self, amount: u64) -> Result<()> {
		// Update used amounts
		let used: u64 = self.total_used.checked_add(amount).ok_or(QBitFlowError::Overflow)?;

		if used > self.total_allowance {
			return err!(QBitFlowError::InsufficientAllowance);
		}
		self.total_used = used;

		Ok(())
	}


	// Revoke the allowance associated with a cancelled subscription
	// This does NOT modify the token account's delegate or allowance, it only updates the registry, by reducing the total allowance and used amounts (removing the subscription's allowance and used amounts from the registry)
	pub fn revoke_allowance(&mut self, subscription: &Subscription) -> Result<()> {
		self.total_allowance = self.total_allowance.checked_sub(subscription.allowance).ok_or(QBitFlowError::Overflow)?;
		self.total_used = self.total_used.checked_sub(subscription.used_allowance).ok_or(QBitFlowError::Overflow)?;

		Ok(())
	}



	/**
	 * This function sets the delegate allowance on the user's token account to the current effective allowance
	 * It effectively approve the current allowance (permit registry), and sets the delegate to the program's PDA
	 * This is useful if the user has modified the delegate unintentionally, and wants to reset it to the correct value
	 * Note: This function does NOT modify the permit registry itself, it only sets the delegate
	 */
	#[inline(never)]
	pub fn set_permit<'info>(&mut self, token_program: &Program<'info, Token>, subscriber: &Signer<'info>, authority: &Account<'info, Authority>, subscriber_token_account: &Account<'info, TokenAccount>) -> Result<()> {

		// Compute the new effective allowance (total_allowance - total_used)
		let allowance = self.total_allowance.checked_sub(self.total_used).ok_or(QBitFlowError::Overflow)?;

		// Approve the program's delegate PDA to spend the new effective allowance
		// Note: The actual CPI to approve should be done in the instruction handler where the
		// user's token account and authority are available.
		token::approve(
			CpiContext::new(
				// The token program account should be passed in the instruction context
				// Here we just use a placeholder; replace with actual account info in the handler
				token_program.to_account_info(), 
				Approve {
					to: subscriber_token_account.to_account_info(), // User's token account (placeholder)
					delegate: authority.to_account_info(), // Program's delegate PDA (placeholder)
					authority: subscriber.to_account_info(), // User as authority
				},
			),
			allowance,
		)?;

		Ok(())
	}

}
