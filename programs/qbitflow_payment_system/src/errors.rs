use anchor_lang::error_code;


#[error_code]
pub enum QBitFlowError {
    #[msg("Amount must be greater than zero")]
    ZeroAmount,

    #[msg("Fee percentage exceeds maximum")]
    InvalidFeePercentage,

    #[msg("Payment is not due yet")]
    PaymentNotDueYet,

    #[msg("Frequency is below the minimum allowed")]
    InvalidFrequency,

    #[msg("Insufficient allowance for this payment")]
    InsufficientAllowance,

    #[msg("Signer is not authorized for this operation")]
    Unauthorized,

    #[msg("Arithmetic overflow")]
    Overflow,

    #[msg("Subscription parameters do not match the stored commitment")]
    InvalidSubscriptionParameters,

    #[msg("Cannot cancel an active subscription past its next payment due date")]
    CannotCancelActiveSubscription,

    #[msg("Amount exceeds the per-period maximum")]
    MaxAmountExceeded,

    #[msg("Amount is invalid for this operation")]
    InvalidAmount,

    #[msg("New max amount is lower than the last payment amount")]
    MaxAmountInvalid,

    #[msg("Compute refund exceeds the configured cap")]
    RefundExceedsCap,

    #[msg("Argument is invalid (zero / default pubkey / equal-to-existing)")]
    InvalidArgument,

    #[msg("Subscription is stopped; pending final billing or already settled")]
    SubscriptionStopped,

    #[msg("Token mint uses an unsupported extension (transfer-fee or confidential-transfer)")]
    UnsupportedTokenExtension,

    #[msg("Requested max-amount increase exceeds the per-call factor cap")]
    MaxIncreaseExceeded,


	#[msg("Organization fee percentage exceeds maximum")]
    InvalidOrganizationFeePercentage,

    #[msg("Referral fee percentage exceeds maximum")]
    InvalidReferralFeePercentage,

    #[msg("Referral deadline is in the past")]
    InvalidReferralDeadline,

    #[msg("Fee recipients (owner / organization / referrer) must be distinct")]
    DuplicateFeeRecipient,

    #[msg("Referrer account required when referral_fee_bps > 0")]
    MissingReferrerAccount,

    #[msg("Organization account required when organization_fee_bps > 0")]
    MissingOrganizationAccount,

    #[msg("Settle function called on subscription with a different mode")]
    WrongSettleMode,

    #[msg("Next payment period has not yet opened")]
    SettleTooEarly,

    #[msg("Nonce range does not match the next expected nonce")]
    InvalidNonceRange,

    #[msg("Receipt or report expiry has passed")]
    PayloadExpired,

    #[msg("x402 batch attestation signature invalid")]
    UserAttestationInvalid,

    #[msg("Merchant report signature invalid")]
    MerchantSignatureInvalid,

    #[msg("Caller-asserted amount does not match the signed payload")]
    AmountMismatch,

    #[msg("Required Ed25519Program precompile not found")]
    MissingEd25519Instruction,

    #[msg("Ed25519Program precompile contents do not match expected signer/message")]
    InvalidEd25519Instruction,

    #[msg("Compute refund params invalid (token_price or compute_cost is zero)")]
    InvalidComputeRefundParams,

    #[msg("Atomic settle payload requires the full set of settle accounts")]
    MissingSettleAccount,

    #[msg("New value equals the current value (no-op)")]
    NoChange,

	#[msg("Session signer does not match the subscription's session_signer")]
	InvalidSessionSigner
}
