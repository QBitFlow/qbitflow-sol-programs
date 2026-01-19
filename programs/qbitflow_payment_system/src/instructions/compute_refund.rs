
use anchor_lang::prelude::*;
use anchor_spl::token::{self, Transfer};

use crate::{errors::*, state::ComputeRefundFailed};

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct ComputeRefundData {
	pub token_price_in_lamports: u64,
	pub compute_cost_in_lamports: u64,
}


// Compute the refund amount for a given user
// This is used for token-based payments. The owner of the authority pays the compute fees in SOL, the is refunded in tokens by the user to enable gasless transactions
#[inline(never)]
pub fn compute_refund<'info>(uuid: [u8; 16], max_amount_refund: u64, params: ComputeRefundData, cpi_context: CpiContext<'_, '_, '_, 'info, Transfer<'info>>) -> Result<u64> {
	// Compute the number of tokens to refund based on the compute cost and token price
	if params.token_price_in_lamports == 0 {
		return Ok(0);
	}
	if params.compute_cost_in_lamports == 0 {
		return Ok(0);
	}

	let compute_cost_in_tokens = params.compute_cost_in_lamports * params.token_price_in_lamports / 1_000_000_000;


	// For safety, ensure the refund does not exceed the max amount allowed
	// Only used for subscriptions
	// For one-time-payments, the compute_refund_data is signed by the user, so no need to limit it
	// For subscriptions however, we need to ensure the refund does not exceed the max amount allowed per period (because the subscriber signed a transaction for several periods, so we need to ensure execution + refund does not exceed the max amount allowed per period)
	if max_amount_refund > 0 && compute_cost_in_tokens > max_amount_refund {
		return err!(QBitFlowError::MaxAmountExceeded);
	}


	// This function doesn't fail if the compute refund fails, it just emits an event
	// This is to avoid the whole transaction from failing if the compute refund fails
	// The refund is a best-effort basis, if it fails, the owner of the authority will have to pay the compute fees in SOL without being refunded in tokens. Shouldn't happen often
	match token::transfer(
		cpi_context,
		compute_cost_in_tokens,
	) {
		Ok(_) => Ok(compute_cost_in_tokens),
		Err(_) => {
			emit!(ComputeRefundFailed { uuid });
			Ok(0)
		}
	}
}