use anchor_lang::prelude::*;
use anchor_spl::token_interface::{self, TransferChecked};

use crate::constants::MAX_COMPUTE_COST_LAMPORTS;
use crate::errors::*;
use crate::state::GasRefundProcessed;


#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct ComputeRefundData {
    /// Lamports per SMALLEST token unit of the specific mint being used. The backend
    /// pre-scales by the mint's decimals before passing this value — the contract does
    /// NOT look at `mint.decimals` for the conversion. Convention only; backend tests
    /// must validate it.
    pub token_price_in_lamports: u64,
    /// SOL cost (lamports) the backend wants reimbursed for this tx. Hard-capped by
    /// `MAX_COMPUTE_COST_LAMPORTS`.
    pub compute_cost_in_lamports: u64,
}


/// Convert a backend-claimed SOL compute cost into the equivalent token amount and pull
/// it from the user's token account to the fee-recipient account.
///
/// ### Caps
/// - `compute_cost_in_lamports > MAX_COMPUTE_COST_LAMPORTS` → hard-fail with
///   `RefundExceedsCap`. The ceiling is the user's sole protection against a compromised
///   backend on this path.
/// - `max_amount_refund` (caller-side) bounds the resulting token amount. Always applied:
///   pass `u64::MAX` to disable (used for one-time payments where the payer signs the tx
///   live and the wallet shows them the refund leg). Passing `0` means "no refund headroom"
///   — any positive refund the backend asks for fails with `MaxAmountExceeded`.
///
/// ### Failure semantics
/// Hard revert across the board. Zero params, cap overrun, or transfer failure all bubble
/// up and abort the enclosing instruction. The protocol does NOT silently absorb refund
/// failures — callers that don't want a refund must not be on a code path that calls this
/// (`cancel_subscription` is an example of such a path).
///
/// ### Rounding
/// Ceiling-division: `tokens = ceil(lamports × price / 1e9)`. Favors the authority owner
/// over the user by at most 1 base unit per call.
/// Pure conversion + cap checks (no CPI, no event). Returns the token amount owed to the
/// operator for `params.compute_cost_in_lamports`, bounded by `MAX_COMPUTE_COST_LAMPORTS`
/// and `max_amount_refund`. Used directly by paths that bundle the refund into another
/// transfer leg (e.g. the x402 one-shot, which rolls gas into the signed amount).
pub fn compute_refund_amount(max_amount_refund: u64, params: &ComputeRefundData) -> Result<u64> {
    // Every refund call must produce a real refund. Both inputs must be non-zero.
    if params.token_price_in_lamports == 0 || params.compute_cost_in_lamports == 0 {
        return err!(QBitFlowError::InvalidComputeRefundParams);
    }

    if params.compute_cost_in_lamports > MAX_COMPUTE_COST_LAMPORTS {
        return err!(QBitFlowError::RefundExceedsCap);
    }

    // Ceiling division using u128 to avoid the u64-multiply overflow that the original
    // code was vulnerable to.
    let product = (params.compute_cost_in_lamports as u128)
        .checked_mul(params.token_price_in_lamports as u128)
        .ok_or(QBitFlowError::Overflow)?;
    let numerator = product
        .checked_add(999_999_999u128)
        .ok_or(QBitFlowError::Overflow)?;
    let result_u128 = numerator / 1_000_000_000u128;
    if result_u128 > u64::MAX as u128 {
        return err!(QBitFlowError::Overflow);
    }
    let compute_cost_in_tokens = result_u128 as u64;

    if compute_cost_in_tokens > max_amount_refund {
        return err!(QBitFlowError::MaxAmountExceeded);
    }

    Ok(compute_cost_in_tokens)
}

#[inline(never)]
pub fn compute_refund<'info>(
    uuid: [u8; 16],
    max_amount_refund: u64,
    params: ComputeRefundData,
    cpi_context: CpiContext<'_, '_, '_, 'info, TransferChecked<'info>>,
    decimals: u8,
) -> Result<u64> {
    let compute_cost_in_tokens = compute_refund_amount(max_amount_refund, &params)?;

    token_interface::transfer_checked(cpi_context, compute_cost_in_tokens, decimals)?;
    emit!(GasRefundProcessed { uuid, amount: compute_cost_in_tokens });
    Ok(compute_cost_in_tokens)
}

/// Best-effort variant of [`compute_refund`]. Used by `update_max_amount_per_cycle` and
/// `extend_allowance` (parity with EVM's `bestEffort=true`): a user must never be locked out of
/// editing their subscription just because the compute refund would breach the per-cycle cap or
/// fail to pull. Returns the token amount actually taken (`0` if the refund was skipped).
///
/// Unlike the hard-revert [`compute_refund`], every failure mode here — invalid params, cap
/// overrun, or a failed transfer — results in "no refund taken" rather than aborting the
/// enclosing instruction. A failed `transfer_checked` CPI is rolled back atomically by the
/// runtime, so swallowing it leaves no partial state.
#[inline(never)]
pub fn compute_refund_best_effort<'info>(
    uuid: [u8; 16],
    max_amount_refund: u64,
    params: ComputeRefundData,
    cpi_context: CpiContext<'_, '_, '_, 'info, TransferChecked<'info>>,
    decimals: u8,
) -> u64 {
    let compute_cost_in_tokens = match compute_refund_amount(max_amount_refund, &params) {
        Ok(amount) => amount,
        Err(_) => return 0,
    };
    if compute_cost_in_tokens == 0 {
        return 0;
    }
    if token_interface::transfer_checked(cpi_context, compute_cost_in_tokens, decimals).is_err() {
        return 0;
    }
    emit!(GasRefundProcessed { uuid, amount: compute_cost_in_tokens });
    compute_cost_in_tokens
}
