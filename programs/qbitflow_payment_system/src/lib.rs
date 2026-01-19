#![allow(unexpected_cfgs)]

use anchor_lang::prelude::*;

// Re-export constants from constants.rs
pub mod constants;
pub use constants::*; // This makes them available at the top level for IDL

declare_id!("48xuDnaYoAgo7dEZaJUt5xxrkfUYBbySWBWwNrydHEhU");

pub mod instructions;
pub mod errors;
pub mod state;



use instructions::*;
use crate::instructions::compute_refund::ComputeRefundData;



#[program]
pub mod qbitflow_payment_system {

    use super::*;

    /// Initialize the payment system, and sets the authority
	/// The authority is a PDA owned by the program, and is used to sign transactions on behalf of the program
	/// The authority's owner is the signer of this transaction and is the "owner" of the payment system. it's also the fee recipient
    pub fn initialize(ctx: Context<Initialize>, co_signer: Pubkey) -> Result<()> {
        instructions::initialize(ctx, co_signer)
    }

	// Update the owner (and fee recipient) of the payment system (only current owner can do this)
	pub fn update_owner(ctx: Context<UpdateOwner>, new_owner: Pubkey) -> Result<()> {
		instructions::update_owner(ctx, new_owner)
	}


	/**
	 * This function sets the delegate allowance on the user's token account to the current effective allowance
	 * It effectively approve the current allowance (permit registry), and sets the delegate to the program's PDA
	 * This is useful if the user has modified the delegate unintentionally, and wants to reset it to the correct value
	 */
	pub fn set_delegate(ctx: Context<SetDelegate>) -> Result<()> {
		instructions::set_delegate(ctx)
	}

    // Process a one-time payment in SOL
    pub fn process_sol_payment(
        ctx: Context<ProcessSolPayment>,
        amount: u64,
        fee_bps: u16,
        uuid: [u8; 16],
        organization_fee_bps: u16,
    ) -> Result<()> {
        instructions::process_sol_payment(ctx, amount, fee_bps, uuid, organization_fee_bps)
    }

	// Functions with a "ComputeRefundData" parameter are paid in SPL tokens, but the authority owner pays the compute fees in SOL, and is refunded in tokens by the user
	// This enables gasless transactions for the user, as the authority owner pays the compute fees
	// The refund is a best-effort basis, if it fails, the authority owner will have to pay the compute fees in SOL without being refunded in tokens. Shouldn't happen often

    /// Process a one-time payment in SPL tokens
    pub fn process_token_payment(
        ctx: Context<ProcessTokenPayment>,
        amount: u64,
        fee_bps: u16,
        uuid: [u8; 16],
        organization_fee_bps: u16,
		compute_refund_params: ComputeRefundData,
    ) -> Result<()> {
        instructions::process_token_payment(ctx, amount, fee_bps, uuid, organization_fee_bps, compute_refund_params)
    }

    /// Create a subscription
    pub fn create_subscription(
        ctx: Context<CreateSubscription>,
        uuid: [u8; 16],
		amount: u64,
		max_amount: u64,
        frequency: u32,
        allowance: u64,
		compute_refund_params: ComputeRefundData,
		is_payg: bool,
    ) -> Result<()> {
        instructions::create_subscription(ctx, uuid, amount, max_amount, frequency, allowance, compute_refund_params, is_payg)
    }

    /// Execute a subscription payment
    pub fn execute_subscription(
        ctx: Context<ExecuteSubscription>,
        amount: u64,
        fee_bps: u16,
        uuid: [u8; 16],
		frequency: u32,
        organization_fee_bps: u16,
		compute_refund_params: ComputeRefundData,
		is_payg: bool,
    ) -> Result<()> {
        instructions::execute_subscription(ctx, amount, fee_bps, uuid, frequency, organization_fee_bps, compute_refund_params, is_payg)
    }

    /// Cancel a subscription
    pub fn cancel_subscription(
        ctx: Context<CancelSubscription>,
        uuid: [u8; 16],
		is_payg: bool
    ) -> Result<()> {
        instructions::cancel_subscription(ctx, uuid, is_payg)
    }

	// Force cancel a subscription (admin only)
	pub fn force_cancel_subscription(
		ctx: Context<ForceCancelSubscription>,
		uuid: [u8; 16],
	) -> Result<()> {
		instructions::force_cancel_subscription(ctx, uuid)
	}

	pub fn update_max_amount(
		ctx: Context<UpdateMaxAmount>,
		uuid: [u8; 16],
		new_max_amount: u64,
		compute_refund_params: ComputeRefundData,
	) -> Result<()> {
		instructions::update_max_amount(ctx, uuid, new_max_amount, compute_refund_params)
	}

    // Increase allowance for a subscription (only subscriber can do this)
    pub fn increase_allowance(
        ctx: Context<IncreaseAllowance>,
        uuid: [u8; 16],
        new_allowance: u64,
		compute_refund_params: ComputeRefundData,
    ) -> Result<()> {
        instructions::increase_allowance(ctx, uuid, new_allowance, compute_refund_params)
    }
}
