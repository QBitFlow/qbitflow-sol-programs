use anchor_lang::prelude::*;
use anchor_spl::token_2022::spl_token_2022::{
    extension::{BaseStateWithExtensions, StateWithExtensions},
    state::Mint as SplMint,
};

use crate::errors::QBitFlowError;


// Reject Token-2022 mints that use extensions incompatible with our flow.
//
// - `TransferFeeConfig`: receiver gets less than `amount`, breaking every transfer leg
//   (fee, org-fee, refund, merchant). Accounting becomes uncertain.
// - `ConfidentialTransferMint`: requires confidential-balance bookkeeping incompatible with
//   the delegate + approve model the permit registry relies on.
//
// Legacy SPL Token mints have no extensions and are always accepted. For Token-2022 mints,
// any other extension is allowed (interest-bearing, metadata-pointer, transfer-hook, etc.).
//
// Important: this is enforced at the program boundary (create_subscription / process_token_payment)
// rather than relying on the underlying CPI to fail. A CPI failure mid-transaction would
// not give us a clean error code to surface to the user.
pub fn reject_unsupported_extensions(mint_account: &AccountInfo<'_>) -> Result<()> {
    let owner = mint_account.owner;
    if owner != &anchor_spl::token_2022::ID {
        // Legacy SPL Token: no extensions possible.
        return Ok(());
    }

    let mint_data = mint_account.try_borrow_data()?;
    let mint_state = StateWithExtensions::<SplMint>::unpack(&mint_data)
        .map_err(|_| error!(QBitFlowError::UnsupportedTokenExtension))?;

    use anchor_spl::token_2022::spl_token_2022::extension::{
        confidential_transfer::ConfidentialTransferMint,
        transfer_fee::TransferFeeConfig,
    };

    if mint_state.get_extension::<TransferFeeConfig>().is_ok() {
        return err!(QBitFlowError::UnsupportedTokenExtension);
    }
    if mint_state.get_extension::<ConfidentialTransferMint>().is_ok() {
        return err!(QBitFlowError::UnsupportedTokenExtension);
    }

    Ok(())
}
