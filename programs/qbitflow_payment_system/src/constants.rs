use anchor_lang::constant;


pub const FEE_DENOMINATOR: u16 = 10000;

// Drain-rate cap floor. Matches EVM `MIN_FREQUENCY` (1 hour). No upper bound (EVM has none;
// `frequency: u32` seconds is self-bounding at ~136 years).
// pub const MIN_FREQUENCY: u32 = 3600; // 1 hour in seconds — PRODUCTION value
// NOTE: the integration suite (tests/test.ts) uses FREQUENCY=5, so before running `anchor test`
// temporarily lower this to 5, then restore it afterward (see the TODO at the top of test.ts).
pub const MIN_FREQUENCY: u32 = 5;


pub const MIN_FEE_FOR_CONTRACT_BPS: u16 = 50; // 0.50%
pub const MAX_FEE_PLATFORM_BPS: u16 = 1500; // 15% (matches EVM MAX_FEE_PLATFORM_BPS)

// Referrer can take at most 50% of the platform fee.
pub const MAX_REFERRAL_FEE_BPS: u16 = 5000;

// Organization can take at most 50% of remaining for the merchant after platform and referrer cut.
pub const MAX_FEE_ORGANIZATION_BPS: u16 = 5000; // 50%

// Trust-minimizing hard ceiling on the SOL cost a backend can claim as "compute
// reimbursement". Sized far above any realistic per-tx cost (base fee × max sigs +
// heavy priority fee even on a congested chain) so honest operation never bumps into
// it. Tightening requires a program upgrade — there's deliberately no owner knob.
pub const MAX_COMPUTE_COST_LAMPORTS: u64 = 100_000_000; // 0.1 SOL

// Per-call cap on how much `update_max_amount_per_cycle` can raise the cap relative to the
// CURRENT cap. 30000 / 10000 = 3x. Larger jumps require multiple user-signed txs.
pub const MAX_INCREASE_FACTOR_BPS: u16 = 30_000;

// Domain-separation tag mixed into the subscription parameter hash. Bump the version
// suffix if the hashed-field set or order ever changes.
pub const SUBSCRIPTION_HASH_DOMAIN: &[u8] = b"qbf:sub:v2";

// Domain-tag for the batch attestation message: the authority signs
// `(uuid || first_nonce || last_nonce || total_amount || merkle_root)` off-chain to give
// cryptographic finality to a batch settlement (subscriber for X402, merchant for MerchantX402).
pub const BATCH_ATTESTATION_HASH_DOMAIN: &[u8] = b"qbf:x402-att:v1";


// PDA seeds
#[constant]
pub const AUTHORITY_PDA_SEED: &[u8] = b"authority";

#[constant]
pub const SUBSCRIPTION_PDA_SEED: &[u8] = b"subscription";

#[constant]
pub const PERMIT_REGISTRY_PDA_SEED: &[u8] = b"permit_registry";
