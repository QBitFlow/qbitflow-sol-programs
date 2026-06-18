#![allow(unexpected_cfgs)]

use anchor_lang::prelude::*;

pub mod constants;
pub use constants::*;

declare_id!("47Xz4vVaw1Bi45goXDanywRW1HzC1zYgEVMzVFDkCbiu");

pub mod instructions;
pub mod errors;
pub mod state;



use instructions::*;
use crate::instructions::compute_refund::ComputeRefundData;
use crate::state::Fee;



#[program]
pub mod qbitflow_payment_system {

    use super::*;

    /// Initialize the payment system and set the authority PDA.
    /// `signer` becomes the owner (= protocol fee recipient + relayer key). `co_signer` is
    /// the cold-wallet key required for owner / co-signer / pause changes.
    pub fn initialize(ctx: Context<Initialize>, co_signer: Pubkey) -> Result<()> {
        instructions::initialize(ctx, co_signer)
    }

    /// Rotate the protocol owner (relayer key). Multi-sig: current owner + co-signer.
    pub fn update_owner(ctx: Context<UpdateOwner>, new_owner: Pubkey) -> Result<()> {
        instructions::update_owner(ctx, new_owner)
    }

    /// Rotate the cold-wallet co-signer. Multi-sig: owner + current co-signer.
    pub fn update_co_signer(ctx: Context<UpdateCoSigner>, new_co_signer: Pubkey) -> Result<()> {
        instructions::update_co_signer(ctx, new_co_signer)
    }

    /// Restore the program PDA as delegate on a subscriber's token account in case the user
    /// (or some unrelated dapp) overwrote it. Recomputes the allowance to set from the
    /// registry — no privilege escalation here.
    pub fn set_delegate(ctx: Context<SetDelegate>) -> Result<()> {
        instructions::set_delegate(ctx)
    }

    /// One-time native SOL payment (EVM `processPayment`). Operator (owner) + payer both sign.
    /// If `fee.referral_fee_bps > 0`, a `referrer` account must be provided and receives that
    /// percentage of the platform fee. One-time payments ignore the referrer deadline.
    pub fn process_payment(
        ctx: Context<ProcessPayment>,
        amount: u64,
        uuid: [u8; 16],
        fee: Fee,
    ) -> Result<()> {
        instructions::process_payment(ctx, amount, uuid, fee)
    }

    // Token-payment / subscription entrypoints take `ComputeRefundData`: the authority owner
    // pays the SOL compute fees and is reimbursed in tokens by the user (gasless tx). The
    // refund is hard-capped by `MAX_COMPUTE_COST_LAMPORTS` regardless of what the backend
    // claims.

    /// One-time SPL token payment (EVM `processTokenPayment`). Operator (owner) + payer both sign.
    pub fn process_token_payment(
        ctx: Context<ProcessTokenPayment>,
        amount: u64,
        uuid: [u8; 16],
        fee: Fee,
        compute_refund_params: ComputeRefundData,
    ) -> Result<()> {
        instructions::process_token_payment(ctx, amount, uuid, fee, compute_refund_params)
    }

    /// Create a subscription. `mode` (Regular / X402 / MerchantX402) is bound at creation
    /// and immutable thereafter; `session_signer` is the off-chain receipt-signing key
    /// (`Pubkey::default()` means the subscriber signs receipts directly). All of mode,
    /// session_signer, fee schedule, frequency, ATAs, and referral terms are committed into
    /// the on-chain subscription_hash so the relayer cannot vary them between settlements.
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
        instructions::create_subscription(
            ctx, uuid, amount, max_amount_per_cycle, frequency,
            fee, allowance,
            compute_refund_params, mode, session_signer,
        )
    }

    /// Execute a billing on a Regular subscription. PAYG subscriptions (X402 / MerchantX402)
    /// settle through `settle_x402_batch` / `settle_merchant_x402_batch` and reject here.
    pub fn execute_subscription(
        ctx: Context<ExecuteSubscription>,
        amount: u64,
        uuid: [u8; 16],
        frequency: u32,
        fee: Fee,
        compute_refund_params: ComputeRefundData,
        mode: u8,
        session_signer: Pubkey,
    ) -> Result<()> {
        instructions::execute_subscription(
            ctx, amount, uuid, frequency,
            fee, compute_refund_params, mode, session_signer,
        )
    }

    /// Cancel a Regular subscription. Closes immediately, only before the next due date
    /// (EVM `cancelSubscription`).
    pub fn cancel_subscription(
        ctx: Context<CancelSubscription>,
        uuid: [u8; 16],
    ) -> Result<()> {
        instructions::cancel_subscription(ctx, uuid)
    }

    /// Cancel a PAYG (X402 / MerchantX402) subscription — marks `stopped`, finalizes on the next
    /// settle (EVM `cancelPayAsYouGoSubscription`).
    pub fn cancel_pay_as_you_go_subscription(
        ctx: Context<CancelSubscription>,
        uuid: [u8; 16],
    ) -> Result<()> {
        instructions::cancel_pay_as_you_go_subscription(ctx, uuid)
    }

    /// Settle a batch of x402 micropayments. The caller commits a merkle root of the receipt
    /// set + aggregate amount; optionally a user attestation over the batch summary. The
    /// per-receipt detail stays off-chain. See `settle.rs` for the validation prologue and
    /// the Ed25519 sysvar pattern used for the attestation.
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
        instructions::settle_x402_batch(
            ctx, uuid, amount, fee, frequency,
            session_signer,
            first_nonce, last_nonce, merkle_root,
            compute_refund_params,
        )
    }

    /// Settle a batch of merchant-attested usage reports (MerchantX402). Same shape as
    /// `settle_x402_batch`; the optional attestation is co-signed by the merchant.
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
        instructions::settle_merchant_x402_batch(
            ctx, uuid, amount, fee, frequency,
            session_signer,
            first_nonce, last_nonce, merkle_root,
            compute_refund_params,
        )
    }

    /// Rotate the off-chain session signer (X402 — subscriber-signed). Recomputes the
    /// subscription_hash with the new signer. Optionally settles a final batch under the OLD
    /// signer in the same tx: the atomic settle bypasses the period guard (the attestation
    /// authorizes it) but uses a cumulative per-cycle cap AND rolls the period
    /// (next_payment_due = now + frequency), so total billing stays bounded by max_amount per
    /// frequency window even across a boundary. Pass `settle = None` for plain rotation.
    pub fn rotate_session_signer(
        ctx: Context<RotateSessionSigner>,
        uuid: [u8; 16],
        frequency: u32,
        fee: Fee,
        mode: u8,
        old_session_signer: Pubkey,
        new_session_signer: Pubkey,
        settle: Option<RotateSettleArgs>,
    ) -> Result<()> {
        instructions::rotate_session_signer(
            ctx, uuid, frequency, fee, mode,
            old_session_signer, new_session_signer, settle,
        )
    }

    /// Rotate the merchant's signing key (MerchantX402 — merchant-signed). Same shape and
    /// atomic-settle semantics as `rotate_session_signer`, but the rotation and the
    /// atomic-settle attestation are authorized by the merchant wallet.
    pub fn rotate_merchant_signer(
        ctx: Context<RotateSessionSigner>,
        uuid: [u8; 16],
        frequency: u32,
        fee: Fee,
        mode: u8,
        old_session_signer: Pubkey,
        new_session_signer: Pubkey,
        settle: Option<RotateSettleArgs>,
    ) -> Result<()> {
        instructions::rotate_merchant_signer(
            ctx, uuid, frequency, fee, mode,
            old_session_signer, new_session_signer, settle,
        )
    }

    /// Force-cancel a subscription (authority-only — merchant-requested escape hatch).
    pub fn force_cancel_subscription(
        ctx: Context<ForceCancelSubscription>,
        uuid: [u8; 16],
    ) -> Result<()> {
        instructions::force_cancel_subscription(ctx, uuid)
    }

    /// Update the per-cycle cap. Subscriber-signed. Increase bounded at 3× current (phishing
    /// guard); decrease floored above the last billed amount (EVM `updateMaxAmountPerCycle`).
    pub fn update_max_amount_per_cycle(
        ctx: Context<UpdateMaxAmount>,
        uuid: [u8; 16],
        new_max_amount: u64,
        compute_refund_params: ComputeRefundData,
    ) -> Result<()> {
        instructions::update_max_amount_per_cycle(ctx, uuid, new_max_amount, compute_refund_params)
    }

    /// Extend (= replace) the subscription's allowance. Subscriber-signed (EVM `extendAllowance`).
    pub fn extend_allowance(
        ctx: Context<ExtendAllowance>,
        uuid: [u8; 16],
        new_allowance: u64,
        compute_refund_params: ComputeRefundData,
    ) -> Result<()> {
        instructions::extend_allowance(ctx, uuid, new_allowance, compute_refund_params)
    }
}
