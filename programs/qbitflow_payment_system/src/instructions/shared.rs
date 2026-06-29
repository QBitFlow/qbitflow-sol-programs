// Shared building blocks for the subscription settle / rotate / cancel paths.
//
// These helpers exist purely to kill copy-paste across `execute_subscription`, `settle_inner`,
// `rotate_inner`, `cancel_subscription`, and `force_cancel_subscription`. Each block below was
// previously duplicated across 3–4 instruction handlers — every copy is one more place for the
// Solana and EVM behaviors to drift apart, and one more contribution to deploy-time bytecode.
// Keeping the logic in one place keeps parity honest and shrinks the program.

use anchor_lang::prelude::*;
use solana_sha256_hasher::hashv;
use anchor_spl::token_interface::{self, TransferChecked};

use crate::instructions::compute_refund::{compute_refund, ComputeRefundData};
use crate::permit::PermitRegistry;
use crate::state::{Amounts, Fee, Subscription, SubscriptionPaymentProcessed, create_subscription_hash};
use crate::{errors::*, BATCH_ATTESTATION_HASH_DOMAIN};


/// Domain-tagged batch-attestation message the authority signs off-chain:
/// `hash(domain || uuid || first_nonce || last_nonce || amount || merkle_root)`.
/// Shared by both settle entrypoints and the rotation atomic-settle.
pub fn batch_attestation_message(
    uuid: &[u8; 16],
    first_nonce: u64,
    last_nonce: u64,
    amount: u64,
    merkle_root: &[u8; 32],
) -> [u8; 32] {
    hashv(&[
        BATCH_ATTESTATION_HASH_DOMAIN,
        uuid,
        &first_nonce.to_le_bytes(),
        &last_nonce.to_le_bytes(),
        &amount.to_le_bytes(),
        merkle_root,
    ])
    .to_bytes()
}


/// Recompute the subscription hash from caller-supplied params and reject any drift from the
/// stored digest. `max_amount_per_cycle` is deliberately NOT part of the hash (see
/// `create_subscription_hash`), so it is not a parameter here.
#[allow(clippy::too_many_arguments)]
pub fn verify_subscription_digest(
    subscription: &Subscription,
    merchant_token_account: &Pubkey,
    subscriber_token_account: &Pubkey,
    frequency: u32,
    organization_token_account: &Pubkey,
    referrer_token_account: &Pubkey,
    fee: &Fee,
    session_signer: &Pubkey,
    mode: u8,
) -> Result<()> {
    let computed = create_subscription_hash(
        merchant_token_account,
        subscriber_token_account,
        frequency,
        organization_token_account,
        referrer_token_account,
        fee.referral_fee_bps,
        fee.referral_deadline,
        session_signer,
        mode,
    );
    if subscription.digest != computed {
        return err!(QBitFlowError::InvalidSubscriptionParameters);
    }
    Ok(())
}


/// Reject a spend that would exceed either the per-subscription allowance or the registry-wide
/// (per subscriber+mint) headroom.
pub fn check_subscription_allowance(
    subscription: &Subscription,
    permit_registry: &PermitRegistry,
    amount: u64,
) -> Result<()> {
    let new_used = subscription
        .used_allowance
        .checked_add(amount)
        .ok_or(QBitFlowError::Overflow)?;
    if new_used > subscription.allowance {
        return err!(QBitFlowError::InsufficientAllowance);
    }
    if !permit_registry.has_enough_allowance(amount) {
        return err!(QBitFlowError::InsufficientAllowance);
    }
    Ok(())
}


#[inline(never)]
fn transfer_checked_pda<'info>(
    token_program: &AccountInfo<'info>,
    from: &AccountInfo<'info>,
    mint: &AccountInfo<'info>,
    to: &AccountInfo<'info>,
    authority: &AccountInfo<'info>,
    signer_seeds: &[&[&[u8]]],
    amount: u64,
    decimals: u8,
) -> Result<()> {
    token_interface::transfer_checked(
        CpiContext::new_with_signer(
            token_program.key(),
            TransferChecked {
                from: from.clone(),
                mint: mint.clone(),
                to: to.clone(),
                authority: authority.clone(),
            },
            signer_seeds,
        ),
        amount,
        decimals,
    )
}


/// Run the full 4-leg payout (platform fee → owner, referrer, organization, merchant) plus the
/// compute-refund leg, all signed by the Authority PDA. Returns the refund amount actually taken.
///
/// This is the shared settlement core for `execute_subscription`, `settle_inner`, and the
/// rotation atomic-settle. The referrer leg is only paid when `amounts.referral_fee_amount > 0`
/// (deadline-gated upstream by `calculate_fee`); the org leg only when `amounts.org_fee_amount > 0`.
#[allow(clippy::too_many_arguments)]
pub fn distribute_payment_pda<'info>(
    token_program: &AccountInfo<'info>,
    mint: &AccountInfo<'info>,
    from: &AccountInfo<'info>,
    authority: &AccountInfo<'info>,
    authority_seeds: &[&[u8]],
    fee_recipient: &AccountInfo<'info>,
    referrer: Option<&AccountInfo<'info>>,
    organization: Option<&AccountInfo<'info>>,
    merchant: &AccountInfo<'info>,
    amounts: &Amounts,
    decimals: u8,
    uuid: [u8; 16],
    refund_cap: u64,
    compute_refund_params: ComputeRefundData,
) -> Result<u64> {
    let signer_seeds: &[&[&[u8]]] = &[authority_seeds];

    // Platform fee leg → owner ATA (net of the referrer cut).
    transfer_checked_pda(
        token_program, from, mint, fee_recipient, authority, signer_seeds,
        amounts.platform_fee_amount, decimals,
    )?;

    // Referrer leg (deadline-gated upstream).
    if amounts.referral_fee_amount > 0 {
        let referrer = referrer.ok_or(QBitFlowError::MissingReferrerAccount)?;
        transfer_checked_pda(
            token_program, from, mint, referrer, authority, signer_seeds,
            amounts.referral_fee_amount, decimals,
        )?;
    }

    // Organization fee leg.
    if amounts.org_fee_amount > 0 {
		let organization = organization.ok_or(QBitFlowError::MissingOrganizationAccount)?;
        transfer_checked_pda(
            token_program, from, mint, organization, authority, signer_seeds,
            amounts.org_fee_amount, decimals,
        )?;
    }

    // Merchant leg.
    transfer_checked_pda(
        token_program, from, mint, merchant, authority, signer_seeds,
        amounts.merchant_amount, decimals,
    )?;

    // Compute-refund leg → owner ATA (hard-revert; settle paths must collect the refund).
    let refund_amount = compute_refund(
        uuid,
        refund_cap,
        compute_refund_params,
        CpiContext::new_with_signer(
            token_program.key(),
            TransferChecked {
                from: from.clone(),
                mint: mint.clone(),
                to: fee_recipient.clone(),
                authority: authority.clone(),
            },
            signer_seeds,
        ),
        decimals,
    )?;

    Ok(refund_amount)
}


/// Close a subscription: drop its slice from the permit registry, close the subscription account
/// (rent → `rent_dest`), and close the registry too if no other subscription is using it.
/// Shared by `cancel_subscription`, `force_cancel_subscription`, and the stopped final-billing
/// path in both `settle_inner` and `rotate_inner`. The distinct cancellation event stays at the
/// callsite.
pub fn close_subscription<'info>(
    subscription: &mut Box<Account<'info, Subscription>>,
    permit_registry: &mut Box<Account<'info, PermitRegistry>>,
    rent_dest: AccountInfo<'info>,
) -> Result<()> {
    permit_registry.revoke_allowance(subscription)?;
    let registry_empty = permit_registry.total_allowance == 0;
    subscription.close(rent_dest.clone())?;
    if registry_empty {
        permit_registry.close(rent_dest)?;
    }
    Ok(())
}
