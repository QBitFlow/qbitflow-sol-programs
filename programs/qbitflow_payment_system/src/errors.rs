use anchor_lang::{error_code};


#[error_code]
pub enum QBitFlowError {    
    ZeroAmount,
    InvalidFeePercentage,
    
    // #[msg("Transfer failed")]
    // TransferFailed,
    
    // #[msg("Subscription not active")]
    // SubscriptionNotActive,
    
    PaymentNotDueYet,
    
    InvalidFrequency,
    
    InsufficientAllowance,
    
    // #[msg("Subscription already exists")]
    // SubscriptionAlreadyExists,
    
    // #[msg("Subscription not found")]
    // SubscriptionNotFound,
    
    Unauthorized,
    
    // #[msg("Account not initialized")]
    // AccountNotInitialized,

	// #[msg("Invalid fee recipient")]
	// InvalidFeeRecipient,

	Overflow,

	InvalidSubscriptionParameters,

	CannotCancelActiveSubscription,

	#[msg("Amount exceeds maximum")]
	MaxAmountExceeded,

	InvalidAmount,

	#[msg("Max amount lower than last payment")]
	MaxAmountInvalid,
}