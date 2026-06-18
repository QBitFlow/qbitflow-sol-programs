use anchor_lang::prelude::*;
use anchor_spl::token_interface::{self, Approve, TokenAccount, TokenInterface};
use crate::errors::*;
use crate::state::{Authority, Subscription};


// Permit registry is per (subscriber, token mint). Tracks the SUM of allowances across all
// of a subscriber's subscriptions on a given mint, plus the cumulative amount actually
// pulled. Lets multiple subscriptions share / coexist without overriding each other's
// SPL delegate approval.
#[account]
pub struct PermitRegistry {
    pub total_allowance: u64, // sum of all subscription max allowances
    pub total_used: u64,      // cumulative tokens spent across all subscriptions
    pub bump: u8,
}

impl PermitRegistry {
    pub const LEN: usize = 8  // discriminator
        + 8                   // total_allowance
        + 8                   // total_used
        + 1;                  // bump


    // CPI helper: re-approve the program's PDA as delegate for `effective_allowance` units.
    // Signs as the subscriber (who owns the token account). Idempotent — `token::approve`
    // overwrites the previous delegate/amount.
    #[inline(never)]
    fn approve_effective<'info>(
        token_program: &Interface<'info, TokenInterface>,
        subscriber: &Signer<'info>,
        authority: &Account<'info, Authority>,
        subscriber_token_account: &InterfaceAccount<'info, TokenAccount>,
        effective_allowance: u64,
    ) -> Result<()> {
        token_interface::approve(
            CpiContext::new(
                token_program.to_account_info(),
                Approve {
                    to: subscriber_token_account.to_account_info(),
                    delegate: authority.to_account_info(),
                    authority: subscriber.to_account_info(),
                },
            ),
            effective_allowance,
        )
    }


    // Add a new subscription's allowance to the registry and (re-)approve the program PDA
    // as delegate for the resulting effective allowance. Validates state BEFORE the CPI;
    // mutates registry state only AFTER the CPI succeeds — if either step fails, the
    // registry's internal accounting and the on-chain delegate stay consistent.
    pub fn add_allowance<'info>(
        &mut self,
        allowance_amount: u64,
        subscriber: &Signer<'info>,
        authority: &Account<'info, Authority>,
        token_program: &Interface<'info, TokenInterface>,
        subscriber_token_account: &InterfaceAccount<'info, TokenAccount>,
    ) -> Result<()> {
        let new_total_allowance = self.total_allowance
            .checked_add(allowance_amount)
            .ok_or(QBitFlowError::Overflow)?;

        let new_effective_allowance = new_total_allowance
            .checked_sub(self.total_used)
            .ok_or(QBitFlowError::Overflow)?;

        Self::approve_effective(token_program, subscriber, authority, subscriber_token_account, new_effective_allowance)?;

        self.total_allowance = new_total_allowance;
        Ok(())
    }


    // True iff there's enough headroom to pull `amount` more from the registry.
    // Overflow is treated as "no headroom".
    pub fn has_enough_allowance(&self, amount: u64) -> bool {
        self.total_used.checked_add(amount).is_some_and(|used| used <= self.total_allowance)
    }


    // Account for a spend against the registry. Does NOT touch the SPL delegate.
    pub fn use_allowance(&mut self, amount: u64) -> Result<()> {
        let used = self.total_used.checked_add(amount).ok_or(QBitFlowError::Overflow)?;
        if used > self.total_allowance {
            return err!(QBitFlowError::InsufficientAllowance);
        }
        self.total_used = used;
        Ok(())
    }


    // Remove a cancelled subscription's slice from the registry totals. Both totals are
    // reduced by the subscription's own remembered values, so the registry continues to
    // reflect the correct sum across the remaining subscriptions.
    pub fn revoke_allowance(&mut self, subscription: &Subscription) -> Result<()> {
        self.total_allowance = self.total_allowance
            .checked_sub(subscription.allowance)
            .ok_or(QBitFlowError::Overflow)?;
        self.total_used = self.total_used
            .checked_sub(subscription.used_allowance)
            .ok_or(QBitFlowError::Overflow)?;
        Ok(())
    }


    // Re-approve the program PDA as delegate for the CURRENT effective allowance, without
    // changing the registry. Used by `set_delegate` to recover from a user who accidentally
    // (or maliciously) cleared / replaced the delegate on their token account.
    #[inline(never)]
    pub fn set_permit<'info>(
        &mut self,
        token_program: &Interface<'info, TokenInterface>,
        subscriber: &Signer<'info>,
        authority: &Account<'info, Authority>,
        subscriber_token_account: &InterfaceAccount<'info, TokenAccount>,
    ) -> Result<()> {
        let effective_allowance = self.total_allowance
            .checked_sub(self.total_used)
            .ok_or(QBitFlowError::Overflow)?;

        Self::approve_effective(token_program, subscriber, authority, subscriber_token_account, effective_allowance)
    }
}
