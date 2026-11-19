use anchor_lang::prelude::*;
use solana_sha256_hasher::hashv;
use crate::{AUTHORITY_PDA_SEED, FEE_DENOMINATOR, MAX_FEE_PLATFORM_BPS, MAX_FEE_ORGANIZATION_BPS, MAX_REFERRAL_FEE_BPS, MIN_FEE_FOR_CONTRACT_BPS, SUBSCRIPTION_HASH_DOMAIN};
use crate::errors::*;


// Settlement mode bound at subscription creation. Immutable for the life of the subscription
// (committed into `subscription_hash`).
//
// - `Regular`: classic recurring subscription, settled via `execute_subscription`.
// - `X402`: pay-as-you-go with off-chain micropayments. Settled in batches via
//   `settle_x402_batch`; an optional user attestation can co-sign the batch summary.
// - `MerchantX402`: merchant-attested pay-as-you-go. Same batch machinery as X402, but the
//   merkle leaves are merchant-signed usage reports (`session_signer` is the merchant's key).
//   Settled via `settle_merchant_x402_batch`; key rotation is merchant-authorized.
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, AnchorSerialize, AnchorDeserialize)]
#[borsh(use_discriminant = true)]
pub enum SettleMode {
    Regular = 0,
    X402 = 1,
    MerchantX402 = 2,
}

impl SettleMode {
    pub fn from_u8(v: u8) -> Result<Self> {
        match v {
            0 => Ok(SettleMode::Regular),
            1 => Ok(SettleMode::X402),
            2 => Ok(SettleMode::MerchantX402),
            _ => err!(QBitFlowError::WrongSettleMode),
        }
    }

    pub fn is_payg(self) -> bool {
        matches!(self, SettleMode::X402 | SettleMode::MerchantX402)
    }
}


// Fee schedule, passed as a single argument — mirrors the EVM `Fee` /`OrganizationFee` /
// `ReferrerFee` structs. On Solana the recipient *addresses* travel as accounts (ATAs), so this
// struct only carries the numeric / temporal fields. `referral_deadline` is ignored for one-time
// payments (same as EVM).
#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct Fee {
    pub platform_fee_bps: u16,
    pub organization_fee_bps: u16,
    pub referral_fee_bps: u16,
    pub referral_deadline: i64,
}

// Resolved payout legs — mirrors the EVM `Amounts` struct. The four legs sum to the gross billed
// amount: `gross = platform_fee_amount + referral_fee_amount + org_fee_amount + merchant_amount`.
#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct Amounts {
    pub platform_fee_amount: u64,
    pub org_fee_amount: u64,
    pub referral_fee_amount: u64,
    pub merchant_amount: u64,
}



#[account]
pub struct Authority {
    pub owner: Pubkey,
    pub co_signer: Pubkey,
    pub bump: u8,
}

impl Authority {
    pub const LEN: usize = 8 + 32 + 32 + 1; // discriminator + owner + co_signer + bump

    pub fn get_seeds(&self) -> [&[u8]; 2] {
        [
            AUTHORITY_PDA_SEED,
            std::slice::from_ref(&self.bump),
        ]
    }
}

#[derive(Accounts)]
pub struct AuthorityAndOwner<'info> {
    #[account(
        seeds = [AUTHORITY_PDA_SEED],
        bump = authority.bump
    )]
    pub authority: Account<'info, Authority>,

    #[account(mut, address = authority.owner @ QBitFlowError::Unauthorized)]
    pub owner: Signer<'info>,
}



#[account]
pub struct Subscription {
    pub subscriber: Pubkey,
    pub next_payment_due: i64,

    pub allowance: u64,
    pub used_allowance: u64,

    pub digest: [u8; 32],
    pub stopped: bool,
    pub max_amount_per_cycle: u64,
    pub bump: u8,

    // Per-cycle spend accumulator (mirrors EVM `currentCycleUsed`). Reset to 0 by the
    // period-guarded normal settle; accumulated (NOT reset) by the rotation atomic-settle,
    // which also does not advance `next_payment_due`. This bounds total billing within a
    // period to `max_amount_per_cycle` even across unlimited rotate+atomic-settle calls.
    pub current_cycle_used: u64,

    // Settlement mode (SettleMode discriminant; bound in digest).
    pub mode: u8,
    // Next expected nonce for the active settlement path. Advances on every settle:
    //   - settle_x402_batch:           next_nonce = last_nonce + 1
    //   - settle_merchant_x402_batch:  next_nonce = last_nonce + 1
    // The on-chain check is `first_nonce >= subscription.next_nonce` — the relayer may skip
    // burned receipts by starting a later batch. Replay is prevented by monotonic advance.
    // Unused (stays 0) for `Regular`.
    pub next_nonce: u64,
}

impl Subscription {
    pub const LEN: usize = 8 // discriminator
        + 32  // subscriber
        + 8   // next_payment_due
        + 8   // allowance
        + 8   // used_allowance
        + 32  // digest
        + 1   // stopped
        + 8   // max_amount_per_cycle
        + 1   // bump
        + 8   // current_cycle_used
        + 1   // mode
        + 8;  // next_nonce
        // session_signer + session_signer_version DROPPED — single source of truth is
        // the subscription_hash. Rotation rewrites the hash; off-chain readers reconstruct
        // the current signer from the SubscriptionCreated + SessionSignerRotated event log.
}


// Events
#[event]
pub struct PaymentProcessed {
    pub uuid: [u8; 16],
    pub from: Pubkey,
    pub to: Pubkey,
    pub mint: Option<Pubkey>, // None for native SOL
    pub amounts: Amounts,
	pub refund_amount: u64
}

#[event]
pub struct SubscriptionCreated {
    pub uuid: [u8; 16],
    pub subscriber: Pubkey,
    pub merchant_token_account: Pubkey,
    pub mint: Pubkey,
    pub next_payment_due: i64,
    pub initial_allowance: u64,
    pub max_amount_per_cycle: u64,
    pub frequency: u32,
    pub fee_bps: u16,
    pub organization_fee_bps: u16,
    pub referral_fee_bps: u16,
    pub referral_deadline: i64,
    pub mode: u8,
    pub session_signer: Pubkey,
}

#[event]
pub struct SubscriptionPaymentProcessed {
    pub uuid: [u8; 16],
    pub subscriber: Pubkey,
    pub mint: Pubkey,
    pub next_payment_due: i64,
    pub remaining_allowance: u64,
    pub amounts: Amounts,
    pub refund_amount: u64,
}

#[event]
pub struct SubscriptionCancelled {
    pub uuid: [u8; 16],
}

// Owner-forced cancel — distinct from SubscriptionCancelled so off-chain monitoring can
// tell a user-initiated cancel apart from a merchant/operator-forced one (parity with EVM).
#[event]
pub struct SubscriptionForceCancelled {
    pub uuid: [u8; 16],
}

#[event]
pub struct SubscriptionStopped {
    pub uuid: [u8; 16],
}

#[event]
pub struct AllowanceExtended {
    pub uuid: [u8; 16],
    pub new_amount: u64,
}

#[event]
pub struct GasRefundProcessed {
    pub uuid: [u8; 16],
    pub amount: u64,
}

#[event]
pub struct MaxAmountPerCycleUpdated {
    pub uuid: [u8; 16],
    pub new_max_amount_per_cycle: u64,
}

#[event]
pub struct OwnerUpdated {
    pub new_owner: Pubkey,
}

#[event]
pub struct CosignerUpdated {
    pub new_cosigner: Pubkey,
}

#[event]
pub struct X402BatchSettled {
    pub uuid: [u8; 16],
    pub subscriber: Pubkey,
    pub mint: Pubkey,
    pub merkle_root: [u8; 32],
    pub first_nonce: u64,
    pub last_nonce: u64,
    pub amounts: Amounts,
    pub refund_amount: u64,
    pub next_payment_due: i64,
    pub remaining_allowance: u64,
}

// Merchant-attested batch settlement. Same shape as X402BatchSettled; distinct event so
// off-chain indexers can tell merchant-attested settlements apart. The merkle root commits
// to the merchant-signed usage-report set.
#[event]
pub struct MerchantX402BatchSettled {
    pub uuid: [u8; 16],
    pub subscriber: Pubkey,
    pub mint: Pubkey,
    pub merkle_root: [u8; 32],
    pub first_nonce: u64,
    pub last_nonce: u64,
    pub amounts: Amounts,
    pub refund_amount: u64,
    pub next_payment_due: i64,
    pub remaining_allowance: u64,
}

#[event]
pub struct SessionSignerRotated {
    pub uuid: [u8; 16],
    pub old_session_signer: Pubkey,
    pub new_session_signer: Pubkey,
    pub settled_in_same_tx: bool,
}


// Helper functions
pub fn calculate_fee(amount: u64, platform_fee_bps: u16, organization_fee_bps: u16, referral_fee_bps: u16, referral_deadline: i64) -> Result<Amounts> {
    if amount == 0 {
        return err!(QBitFlowError::ZeroAmount);
    }

    if platform_fee_bps > MAX_FEE_PLATFORM_BPS {
        return err!(QBitFlowError::InvalidFeePercentage);
    }

	if organization_fee_bps > MAX_FEE_ORGANIZATION_BPS {
		return err!(QBitFlowError::InvalidOrganizationFeePercentage);
	}

	if referral_fee_bps > MAX_REFERRAL_FEE_BPS {
		return err!(QBitFlowError::InvalidReferralFeePercentage);
	}

    let effective_fee_bps = if platform_fee_bps < MIN_FEE_FOR_CONTRACT_BPS {
        MIN_FEE_FOR_CONTRACT_BPS
    } else {
        platform_fee_bps
    };

    let gross_platform_fee = (amount as u128 * effective_fee_bps as u128 / FEE_DENOMINATOR as u128) as u64;


	// Referrer fee is a portion of the platform fee, so it doesn't dilute the org fee, nor the merchant's net amount.
	let referral_fee_amount = if referral_fee_bps > 0 && referral_deadline >= Clock::get()?.unix_timestamp {
		(gross_platform_fee as u128 * referral_fee_bps as u128 / FEE_DENOMINATOR as u128) as u64
	} else {
		0
	};

	let platform_fee_amount = gross_platform_fee.checked_sub(referral_fee_amount)
		.ok_or(QBitFlowError::Overflow)?;

    let org_fee_amount = if organization_fee_bps > 0 {
        let remaining_amount = amount.checked_sub(gross_platform_fee)
            .ok_or(QBitFlowError::Overflow)?;
        (remaining_amount as u128 * organization_fee_bps as u128 / FEE_DENOMINATOR as u128) as u64
    } else {
        0
    };

	let merchant_amount = amount.checked_sub(gross_platform_fee)
		.and_then(|amt| amt.checked_sub(org_fee_amount))
		.ok_or(QBitFlowError::Overflow)?;

	Ok(Amounts {
        platform_fee_amount,
        org_fee_amount,
        referral_fee_amount,
        merchant_amount,
    })
}


// Enforce the EVM `_validateFeeRecipients` distinctness set: the merchant must differ from the
// payer/subscriber, and every active fee recipient (organization when org bps > 0, referrer when
// referral bps > 0) must be distinct from the platform fee recipient, the merchant, and each
// other. Prevents an operator from disguising a higher composite fee as separate legs routed to
// the same wallet. Keys are token-account (ATA) keys for SPL paths and wallet keys for the native
// path — both work because an ATA is 1:1 with (owner, mint).
pub fn validate_fee_recipients(
    payer: &Pubkey,
    merchant: &Pubkey,
    fee_recipient: &Pubkey,
    organization: Option<&Pubkey>,
    referrer: Option<&Pubkey>,
) -> Result<()> {
    if merchant == payer {
        return err!(QBitFlowError::InvalidArgument);
    }
    if let Some(org) = organization {
        if org == fee_recipient || org == merchant {
            return err!(QBitFlowError::DuplicateFeeRecipient);
        }
    }
    if let Some(referrer) = referrer {
        if referrer == fee_recipient || referrer == merchant {
            return err!(QBitFlowError::DuplicateFeeRecipient);
        }
        if let Some(org) = organization {
            if referrer == org {
                return err!(QBitFlowError::DuplicateFeeRecipient);
            }
        }
    }
    Ok(())
}


// Domain-tagged hash of the subscription parameters the subscriber consented to at create time.
// `amount` is intentionally EXCLUDED — USD-pegged subscriptions vary the token amount each cycle.
// `max_amount_per_cycle` is also EXCLUDED — unlike EVM (where the subscriber signs only a param
// subset, so the cap must be committed in the digest), on Solana the subscriber signs the whole
// transaction. The cap lives as a trusted on-chain field (`subscription.max_amount_per_cycle`),
// is never caller-supplied at settle, and is only ever changed via the subscriber-signed
// `update_max_amount_per_cycle` — so committing it in the digest is unnecessary (and would force
// a digest rewrite on every cap update).
// `mint` is implicitly bound via the ATA addresses below (an ATA is derived from (owner, mint),
// so swapping the mint produces a different ATA address that would fail to match this hash).
//
// Referral fields are always hashed — when there's no referrer, they are zero/default. That
// commits "no referrer" into the subscription so the relayer can't inject one later.
//
// `platform_fee_bps` and `organization_fee_bps` are deliberately EXCLUDED: the relayer passes them
// on every bill so QBitFlow (volume-based pricing) and the organization can change their rates
// during a subscription's life, in either direction, without every subscriber re-signing. This is
// an operator trust assumption, bounded on-chain: the rates are capped (`MAX_FEE_PLATFORM_BPS`,
// `MAX_FEE_ORGANIZATION_BPS`), every recipient is committed here (so a rate change can't pay a
// new party), and the subscriber never pays more than the per-cycle cap. Within those caps a
// compromised relayer can shift part of each bill from the merchant to the fee accounts
// (SPEC §0.1, P-61).
pub fn create_subscription_hash(
    merchant_token_account: &Pubkey,
    subscriber_token_account: &Pubkey,
    frequency: u32,
    organization_token_account: &Pubkey,
    referrer_token_account: &Pubkey,
    referral_fee_bps: u16,
    referral_deadline: i64,
    session_signer: &Pubkey,
    mode: u8,
) -> [u8; 32] {
    let freq_bytes = frequency.to_le_bytes();
    let referral_fee_bytes = referral_fee_bps.to_le_bytes();
    let referral_deadline_bytes = referral_deadline.to_le_bytes();
    let mode_bytes = [mode];
    hashv(&[
        SUBSCRIPTION_HASH_DOMAIN,
        merchant_token_account.as_ref(),
        subscriber_token_account.as_ref(),
        &freq_bytes,
        organization_token_account.as_ref(),
        referrer_token_account.as_ref(),
        &referral_fee_bytes,
        &referral_deadline_bytes,
        session_signer.as_ref(),
        &mode_bytes,
    ]).to_bytes()
}
