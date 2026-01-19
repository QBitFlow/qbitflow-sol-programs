use anchor_lang::constant;


pub const FEE_DENOMINATOR: u16 = 10000;

pub const MIN_FREQUENCY: u32 = 7 * 86400; // 7 days in seconds. u32 since maximum freq is 1 year < 2^32
// pub const MIN_FREQUENCY: u32 = 60*10; // 10 minutes in seconds. u32 since maximum freq is 1 year < 2^32


pub const MIN_FEE_FOR_CONTRACT_BPS: u16 = 75; // 0.75%
pub const MAX_FEE_BPS: u16 = 1000; // 10%



// PDA seeds
#[constant]
pub const AUTHORITY_PDA_SEED: &[u8] = b"authority";

#[constant]
pub const SUBSCRIPTION_PDA_SEED: &[u8] = b"subscription";

#[constant]
pub const PERMIT_REGISTRY_PDA_SEED: &[u8] = b"permit_registry";