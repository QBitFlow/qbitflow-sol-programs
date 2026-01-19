use anchor_lang::prelude::*;
use anchor_spl::associated_token::AssociatedToken;
use anchor_spl::token::{self, Token, TokenAccount, Transfer};
use crate::instructions::compute_refund::{compute_refund, ComputeRefundData};
use crate::{state::*, MIN_FREQUENCY, PERMIT_REGISTRY_PDA_SEED, SUBSCRIPTION_PDA_SEED};
use crate::errors::*;
use crate::permit::{PermitRegistry};

#[derive(Accounts)]
#[instruction(uuid: [u8; 16], frequency: u32, allowance: u64)]
pub struct CreateSubscription<'info> {
    #[account()]
	pub authority_and_owner: AuthorityAndOwner<'info>,

	// Permit registry to track allowances for this subscriber (per subscriber, and per mint)
	#[account(
		init_if_needed, // First time initialization if needed
		payer = authority_and_owner.owner, // authority owner pays for the permit registry account creation (and is reimbursed if it's closed later)
		space = PermitRegistry::LEN,
		seeds = [PERMIT_REGISTRY_PDA_SEED, subscriber.key().as_ref(), mint.key().as_ref()],
		bump
	)]
	pub permit_registry: Account<'info, PermitRegistry>,
    

	// Unique subscription account derived from uuid
	// Raises 'AccountAlreadyInitialized' if the subscription with the same uuid already exists
    #[account(
        init,
        payer = authority_and_owner.owner, // authority owner pays for the subscription account creation (and is reimbursed when the subscription is closed)
        space = Subscription::LEN,
        seeds = [SUBSCRIPTION_PDA_SEED, uuid.as_ref()],
        bump
    )]
    pub subscription: Account<'info, Subscription>,
    

	// The user creating the subscription
	// Needs to sign the transaction
    #[account(mut)]
    pub subscriber: Signer<'info>,
    
	// The subscriber's token account from which payments will be made
	// This one needs to be initiated and funded by the subscriber beforehand
	// This ensures the token account belongs to the subscriber
    #[account(
		mut,
		associated_token::mint = mint,
		associated_token::authority = subscriber
	)]
    pub subscriber_token_account: Account<'info, TokenAccount>,


	/// CHECK: The merchant receiving the payments for this subscription (main account). Must be initialized
	#[account(mut)] // needs to be initialized
	pub merchant: UncheckedAccount<'info>,

	// The merchant receiving the payments for this subscription (token account)
	#[account(
		init_if_needed,
		payer = authority_and_owner.owner, // authority pays for token account initialization (merchant)
		associated_token::mint = mint,
		associated_token::authority = merchant
	)]
    pub merchant_token_account: Account<'info, TokenAccount>,

	// Ensures this token account belongs to authority.owner
	// No need to pass the address from the client side (and even though we do, it will be ignored)
    #[account(
        init_if_needed,
		payer = authority_and_owner.owner,
		associated_token::mint = mint,
		associated_token::authority = authority_and_owner.owner // fee recipient is authority owner
    )]
    pub fee_recipient_token_account: Account<'info, TokenAccount>,


    
	// The token mint used for this subscription
    pub mint: Account<'info, anchor_spl::token::Mint>,
    

	/// CHECK: This is the organization receiving a portion of the fees (optional). Must be initialized
	#[account(mut)]
	pub organization: UncheckedAccount<'info>,

	#[account(
		init_if_needed,
		payer = authority_and_owner.owner, // organization pays for their token own account initialization
		associated_token::mint = mint,
		associated_token::authority = organization
	)]
    pub organization_token_account: Account<'info, TokenAccount>,

    
    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>,
	pub associated_token_program: Program<'info, AssociatedToken>,
}

#[derive(Accounts)]
#[instruction(amount: u64, fee_bps: u16, uuid: [u8; 16], frequency: u32, organization_fee_bps: u16)]
pub struct ExecuteSubscription<'info> {
    #[account()]
	pub authority_and_owner: AuthorityAndOwner<'info>,
    
	// Subscription PDA derived from uuid of the subscription
    #[account(
        mut,
        seeds = [SUBSCRIPTION_PDA_SEED, uuid.as_ref()],
        bump = subscription.bump
    )]
    pub subscription: Account<'info, Subscription>,

	// Permit registry, must match the subscriber of the subscription
	#[account(
		mut,
		seeds = [PERMIT_REGISTRY_PDA_SEED, subscription.subscriber.key().as_ref(), mint.key().as_ref()],
		bump = permit_registry.bump
	)]
	pub permit_registry: Account<'info, PermitRegistry>,

	/// CHECK: The user creating the subscription
	// Needs to sign the transaction
    #[account(mut, address = subscription.subscriber @ QBitFlowError::Unauthorized)]
    pub subscriber: UncheckedAccount<'info>,
    

	// The subscriber's token account from which payments will be made
	// This ensures the token account belongs to the subscriber (no need to provide)
    #[account(
		mut,
		associated_token::mint = mint,
		associated_token::authority = subscription.subscriber
	)]
    pub subscriber_token_account: Account<'info, TokenAccount>,

    
	// Merchant's token account (will be verified comparing hash)
    #[account(mut)]
    pub merchant_token_account: Account<'info, TokenAccount>,

	pub mint: Account<'info, anchor_spl::token::Mint>,
    

	// Fee recipient token account (ATA of the authority.owner)
	// Verifies that the token account belongs to authority.owner
	// Creates the ATA if it doesn't exist
    #[account(
		mut,
		associated_token::mint = mint,
		associated_token::authority = authority_and_owner.owner // fee recipient is authority owner
	)]
    pub fee_recipient_token_account: Account<'info, TokenAccount>,
    

	// Organization token account (ATA of the organization_account)
	// Will be verified during hash comparison
    #[account(mut)]
    pub organization_token_account: Account<'info, TokenAccount>,
    
	pub system_program: Program<'info, System>,
	pub associated_token_program: Program<'info, AssociatedToken>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
#[instruction(uuid: [u8; 16])]
pub struct CancelSubscription<'info> {
	#[account()]
	pub authority_and_owner: AuthorityAndOwner<'info>,

    #[account(
        mut,
        seeds = [SUBSCRIPTION_PDA_SEED, uuid.as_ref()],
        bump = subscription.bump
    )]
    pub subscription: Account<'info, Subscription>,

	#[account(
		mut,
		seeds = [PERMIT_REGISTRY_PDA_SEED, subscription.subscriber.key().as_ref(), mint.key().as_ref()],
		bump = permit_registry.bump
	)]
	pub permit_registry: Account<'info, PermitRegistry>,
    
    #[account(
		mut,
		address = subscription.subscriber @ QBitFlowError::Unauthorized
	)]
    pub subscriber: Signer<'info>,


	pub mint: Account<'info, anchor_spl::token::Mint>,
}


#[derive(Accounts)]
#[instruction(uuid: [u8; 16])]
pub struct UpdateMaxAmount<'info> {
	#[account()]
	pub authority_and_owner: AuthorityAndOwner<'info>,

    
	// Subscription PDA derived from uuid of the subscription
    #[account(
        mut,
        seeds = [SUBSCRIPTION_PDA_SEED, uuid.as_ref()],
        bump = subscription.bump
    )]
    pub subscription: Account<'info, Subscription>,

	#[account(
		mut,
		seeds = [PERMIT_REGISTRY_PDA_SEED, subscription.subscriber.key().as_ref(), mint.key().as_ref()],
		bump = permit_registry.bump
	)]
	pub permit_registry: Account<'info, PermitRegistry>,

	#[account(mut, address = subscription.subscriber @ QBitFlowError::Unauthorized)]
    pub subscriber: Signer<'info>,

	// The subscriber's token account from which payments will be made
	// This ensures the token account belongs to the subscriber (no need to provide)
    #[account(
		mut,
		associated_token::mint = mint,
		associated_token::authority = subscription.subscriber
	)]
    pub subscriber_token_account: Account<'info, TokenAccount>,

	pub mint: Account<'info, anchor_spl::token::Mint>,

	// Fee recipient token account (ATA of the authority.owner)
	// Verifies that the token account belongs to authority.owner
	// Creates the ATA if it doesn't exist
    #[account(
		mut,
		associated_token::mint = mint,
		associated_token::authority = authority_and_owner.owner // fee recipient is authority owner
	)]
    pub fee_recipient_token_account: Account<'info, TokenAccount>,

	pub token_program: Program<'info, Token>,
}


#[derive(Accounts)]
#[instruction(uuid: [u8; 16])]
pub struct ForceCancelSubscription<'info> {
	#[account()]
	pub authority_and_owner: AuthorityAndOwner<'info>,

    #[account(
        mut,
        seeds = [SUBSCRIPTION_PDA_SEED, uuid.as_ref()],
        bump = subscription.bump
    )]
    pub subscription: Account<'info, Subscription>,

	#[account(
		mut,
		seeds = [PERMIT_REGISTRY_PDA_SEED, subscription.subscriber.key().as_ref(), mint.key().as_ref()],
		bump = permit_registry.bump
	)]
	pub permit_registry: Account<'info, PermitRegistry>,

	pub mint: Account<'info, anchor_spl::token::Mint>,
}


#[derive(Accounts)]
#[instruction(uuid: [u8; 16], new_allowance: u64)]
pub struct IncreaseAllowance<'info> {
	#[account()]
	pub authority_and_owner: AuthorityAndOwner<'info>,

	// Fee recipient token account (ATA of the authority.owner)
	// Verifies that the token account belongs to authority.owner
	// Creates the ATA if it doesn't exist
    #[account(
		mut,
		associated_token::mint = mint,
		associated_token::authority = authority_and_owner.owner // fee recipient is authority owner
	)]
    pub fee_recipient_token_account: Account<'info, TokenAccount>,

    #[account(
        mut,
        seeds = [SUBSCRIPTION_PDA_SEED, uuid.as_ref()],
        bump = subscription.bump
    )]
    pub subscription: Account<'info, Subscription>,

	#[account(
		mut,
		seeds = [PERMIT_REGISTRY_PDA_SEED, subscription.subscriber.key().as_ref(), mint.key().as_ref()],
		bump = permit_registry.bump
	)]
	pub permit_registry: Account<'info, PermitRegistry>,
    
	// Permit registry, must match the subscriber of the subscription
	// It will raise an error if the signer is not the subscriber (ConstraintAddress)
    #[account(
		mut,
		address = subscription.subscriber @ QBitFlowError::Unauthorized
	)]
    pub subscriber: Signer<'info>,

	// The subscriber's token account from which payments will be made
	// This one needs to be initiated and funded by the subscriber beforehand
	// This ensures the token account belongs to the subscriber
    #[account(
		mut,
		associated_token::mint = mint,
		associated_token::authority = subscriber
	)]
    pub subscriber_token_account: Account<'info, TokenAccount>,

	// The token mint used for this subscription
    pub mint: Account<'info, anchor_spl::token::Mint>,

	pub token_program: Program<'info, Token>,
}


/**
 * Create a regular subscription
 * Emits a SubscriptionCreated event
 */
pub fn create_subscription(
    ctx: Context<CreateSubscription>,
    uuid: [u8; 16],
	amount: u64,
	max_amount: u64,
    frequency: u32,
    allowance: u64,
	compute_refund_params: ComputeRefundData,
	is_payg: bool,
) -> Result<()> {
	// let (next_payment_due, remaining_allowance) = _create_subscription(ctx, uuid, amount, max_amount, frequency, allowance, false, compute_refund_params)?;
	if frequency < MIN_FREQUENCY {
        return err!(QBitFlowError::InvalidFrequency);
    }

	if max_amount <= amount {
		return err!(QBitFlowError::InvalidAmount);
	}

	// Add allowance to the permit registry
	let permit_registry = &mut ctx.accounts.permit_registry;

	if permit_registry.bump == 0 {
		// Newly initialized, set initial values
		permit_registry.bump = ctx.bumps.permit_registry;
	}

	// Add allowance entry in permit registry
	permit_registry.add_allowance(allowance, &ctx.accounts.subscriber, &ctx.accounts.authority_and_owner.authority, &ctx.accounts.token_program, &ctx.accounts.subscriber_token_account)?;


	// Now initialize the subscription account
    let subscription = &mut ctx.accounts.subscription;


    let next_payment_due: i64;
	if is_payg {
		// Next payment due is now + frequency (no trial period for pay-as-you-go)
		// And the billing is done at the end of the period
		next_payment_due = Clock::get()?.unix_timestamp + (frequency as i64);
	} else {
		// For regular subscriptions, the first payment is due immediately
		next_payment_due = Clock::get()?.unix_timestamp;
	}

    
    subscription.subscriber = ctx.accounts.subscriber.key();
    subscription.next_payment_due = next_payment_due;
	subscription.allowance = allowance;
	subscription.used_allowance = 0;
	subscription.stopped = false;
	subscription.max_amount = max_amount;
    subscription.bump = ctx.bumps.subscription;
	subscription.last_payment_amount = amount;


	// Create the hash of the subscription for uniqueness, and to ensure the parameters match during execution
	// We do this, and pass the required parameters to the execute function, instead of storing them directly, to save space (and cost)
	subscription.subscription_hash = create_subscription_hash(
		&ctx.accounts.merchant_token_account.key(),
		&ctx.accounts.subscriber_token_account.key(), 
		frequency, 
		&ctx.accounts.organization_token_account.key()
	);

	// Compute refund in tokens for the authority owner to refund the compute cost paid in SOL
	let refund_result = compute_refund(uuid, max_amount - amount, compute_refund_params, CpiContext::new(
		ctx.accounts.token_program.to_account_info(),
		Transfer {
			from: ctx.accounts.subscriber_token_account.to_account_info(),
			to: ctx.accounts.fee_recipient_token_account.to_account_info(),
			authority: ctx.accounts.subscriber.to_account_info(),
		},
	))?;


	// Update the used allowance with the refunded amount (best-effort, if it fails, the authority owner pays the compute cost in SOL)
	permit_registry.use_allowance(refund_result)?; // And update the permit registry as well (since the total allowance used has increased)
	subscription.used_allowance = refund_result; // Update the used allowance for the subscription


    emit!(SubscriptionCreated {
        uuid,
        next_payment_due,
        initial_allowance: subscription.allowance - subscription.used_allowance,
    });

    Ok(())
}



/**
 * Execute a payment for a regular subscription
 * This can be called by anyone, but requires the authority.owner signature
 * The permit registry will be updated accordingly
 * Emits a SubscriptionPaymentProcessed event
 */
pub fn execute_subscription(
    ctx: Context<ExecuteSubscription>,
    amount: u64,
    fee_bps: u16,
    uuid: [u8; 16],
	frequency: u32,
    organization_fee_bps: u16,
	compute_refund_params: ComputeRefundData,
	is_payg: bool,
) -> Result<()> {
	let subscription = &mut ctx.accounts.subscription;
    
	// Since we're here, the subscription exists (otherwise the PDA derivation would fail)
    if Clock::get()?.unix_timestamp < subscription.next_payment_due {
        return err!(QBitFlowError::PaymentNotDueYet);
    }
    
    if amount == 0 {
        return err!(QBitFlowError::ZeroAmount);
    }
	if amount >= subscription.max_amount {
		return err!(QBitFlowError::MaxAmountExceeded);
	}

	// Create the hash from the arguments provided, and ensure it matches the stored hash
	let computed_hash = create_subscription_hash(
		&ctx.accounts.merchant_token_account.key(),
		&ctx.accounts.subscriber_token_account.key(),
		frequency,
		&ctx.accounts.organization_token_account.key()
	);

	// Ensure the parameters are correct by comparing the hashes
	if subscription.subscription_hash != computed_hash {
		return err!(QBitFlowError::InvalidSubscriptionParameters);
	}
    
	// Ensure the subscription has enough allowance left
    if subscription.used_allowance + amount >= subscription.allowance {
        return err!(QBitFlowError::InsufficientAllowance);
    }

	let permit_registry = &mut ctx.accounts.permit_registry;

	// Ensure the global permit registry has enough allowance left
	if !permit_registry.has_enough_allowance(amount) {
		return err!(QBitFlowError::InsufficientAllowance);
	}

    
	let (fee_amount, org_fee_amount) = calculate_fee(amount, fee_bps, organization_fee_bps)?;
    let remaining_amount = amount
        .checked_sub(fee_amount)
        .and_then(|x| x.checked_sub(org_fee_amount))
        .ok_or(QBitFlowError::Overflow)?;


    // Transfer fee to fee recipient
    token::transfer(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.subscriber_token_account.to_account_info(),
                to: ctx.accounts.fee_recipient_token_account.to_account_info(),
                authority: ctx.accounts.authority_and_owner.authority.to_account_info(), // Program's delegate PDA
            },
            &[&ctx.accounts.authority_and_owner.authority.get_seeds()]
        ),
        fee_amount, 
    )?;

    // Transfer organization fee if applicable
    if org_fee_amount > 0 {
        token::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.subscriber_token_account.to_account_info(),
                    to: ctx.accounts.organization_token_account.to_account_info(),
                    authority: ctx.accounts.authority_and_owner.authority.to_account_info(),
                },
				&[&ctx.accounts.authority_and_owner.authority.get_seeds()]
            ),
            org_fee_amount,
        )?;
    }

    // Transfer remaining amount to merchant
    token::transfer(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.subscriber_token_account.to_account_info(),
                to: ctx.accounts.merchant_token_account.to_account_info(),
                authority: ctx.accounts.authority_and_owner.authority.to_account_info(),
            },
			&[&ctx.accounts.authority_and_owner.authority.get_seeds()]
        ),
        remaining_amount,
    )?;
	
	// Get the allowance from the registry
	let permit_registry = &mut ctx.accounts.permit_registry;

	subscription.last_payment_amount = amount;

	// Compute refund in tokens for the authority owner to refund the compute cost paid in SOL
	// Authority of the transfer is the authority PDA (delegate)
	let refund_result = compute_refund(uuid, subscription.max_amount - amount, compute_refund_params, CpiContext::new_with_signer(
		ctx.accounts.token_program.to_account_info(),
		Transfer {
			from: ctx.accounts.subscriber_token_account.to_account_info(),
			to: ctx.accounts.fee_recipient_token_account.to_account_info(),
			authority: ctx.accounts.authority_and_owner.authority.to_account_info(),
		},
		&[&ctx.accounts.authority_and_owner.authority.get_seeds()]
	))?;


	// Update the total amount to include the refund
	// Update the total used in the permit registry
	permit_registry.use_allowance(amount + refund_result)?; // Increase the global used amount

    // Update subscription
    subscription.used_allowance += amount + refund_result; // Increase the used allowance (including the refund amount)

	let next_payment_due: i64;
	if !is_payg {
		// For regular subscriptions, move the next payment due forward by frequency
		next_payment_due = subscription.next_payment_due + frequency as i64;
	} else {
		// For pay-as-you-go subscriptions, set the next payment due to now + frequency (since the backend might skip some calls if the usage is low to save compute)
		// This ensures the next payment due is always in the future
		// And the billing is done at the end of the period
		// We decrease by one hour to avoid pushing the next billing date a day each time (since the backend executes every 24 hours, therefore if we add 24 hours each time, the next payment due will be pushed by one day each time)
		next_payment_due = Clock::get()?.unix_timestamp + frequency as i64 - 3600;
	}
	subscription.next_payment_due = next_payment_due;

	

	let remaining_allowance: u64;

	if subscription.stopped {
		// If the subscription is stopped, revoke the allowance in the permit registry (must be a pay-as-you-go subscription)
		permit_registry.revoke_allowance(subscription)?;
		remaining_allowance = 0;
		// Close the subscription account by setting the close constraint to the subscriber

		if permit_registry.total_allowance == 0 {
			// If the permit registry has no more allowance, close it as well
		}
	} else {
		remaining_allowance = subscription.allowance - subscription.used_allowance;
	}

    emit!(SubscriptionPaymentProcessed {
        uuid,
        next_payment_due: next_payment_due,
        remaining_allowance: remaining_allowance,
    });

    Ok(())
}


/**
 * Cancel a subscription (regular subscription only)
 * This can be called by the subscriber, and requires their signature
 * The subscription can only be canceled if the nextPaymentDue is in the future (i.e. before the next payment is due)
 * The permit registry will be updated accordingly
 */
pub fn cancel_subscription(
    ctx: Context<CancelSubscription>,
    _uuid: [u8; 16],
	is_payg: bool,
) -> Result<()> {
    let subscription = &mut ctx.accounts.subscription;

	if is_payg {
		// Set the stopped flag to true
		subscription.stopped = true;
		return Ok(());
	}

	// Ensure the nextPaymentDue is in the future
	let current_time = Clock::get()?.unix_timestamp;
	if current_time >= subscription.next_payment_due {
		return err!(QBitFlowError::CannotCancelActiveSubscription);
	}

	// Revoke the allowance from the permit registry
	let permit_registry = &mut ctx.accounts.permit_registry;
	permit_registry.revoke_allowance(subscription)?;
    

	// Close the subscription account 
	ctx.accounts.subscription.close(ctx.accounts.authority_and_owner.owner.to_account_info())?;

	if permit_registry.total_allowance == 0 {
		// If the permit registry has no more allowance, close it as well
		permit_registry.close(ctx.accounts.authority_and_owner.owner.to_account_info())?;
	}

	emit!(SubscriptionCancelled {
		uuid: _uuid,
	});

    // The account will be closed automatically due to the close constraint
    Ok(())
}


/**
 * Force cancel a subscription
 * Can only be called by the authority owner
 * Unlike regular cancel, this doesn't need the signature of the subscriber, and does not perform time checks
 * This is useful for admin purposes, or if the subscriber has lost access to their account
 */
pub fn force_cancel_subscription(
	ctx: Context<ForceCancelSubscription>,
	_uuid: [u8; 16]
) -> Result<()> {
	let subscription = &ctx.accounts.subscription;

	// Revoke the allowance from the permit registry
	let permit_registry = &mut ctx.accounts.permit_registry;
	permit_registry.revoke_allowance(subscription)?;

	// Close the subscription account by setting the close constraint to the subscriber
	ctx.accounts.subscription.close(ctx.accounts.authority_and_owner.owner.to_account_info())?;

	if permit_registry.total_allowance == 0 {
		// If the permit registry has no more allowance, close it as well
		permit_registry.close(ctx.accounts.authority_and_owner.owner.to_account_info())?;
	}

	emit!(SubscriptionCancelled {
		uuid: _uuid,
	});

    // The account will be closed automatically due to the close constraint
    Ok(())
}




/**
 * Increase the allowance of a subscription
 * This can be called by the subscriber, and requires their signature
 * It effectively replaces the current allowance with a new one
 * It also resets the used allowance to 0
 * The permit registry is updated accordingly
 */
pub fn increase_allowance(
    ctx: Context<IncreaseAllowance>,
    uuid: [u8; 16],
    new_allowance: u64,
	compute_refund_params: ComputeRefundData,
) -> Result<()> {
	if new_allowance == 0 {
		return err!(QBitFlowError::ZeroAmount);
	}

    let subscription = &mut ctx.accounts.subscription;

	// Allowance can only be increased
	if new_allowance <= subscription.allowance {
		return err!(QBitFlowError::InvalidAmount);
	}

	// To perform like the ethereum version, increase allowance effectively means setting a new allowance (replacing the old one)

	// First, revoke the current allowance from the permit registry
	let permit_registry = &mut ctx.accounts.permit_registry;
	permit_registry.revoke_allowance(subscription)?;

	// Now, we can set the new allowance
	subscription.allowance = new_allowance;
	subscription.used_allowance = 0; // Reset used allowance

	// And add the new allowance to the permit registry
	permit_registry.add_allowance(new_allowance, &ctx.accounts.subscriber, &ctx.accounts.authority_and_owner.authority, &ctx.accounts.token_program, &ctx.accounts.subscriber_token_account)?;


	// Compute refund in tokens for the authority owner to refund the compute cost paid in SOL
	let refund_result = compute_refund(uuid, subscription.max_amount - subscription.last_payment_amount, compute_refund_params, CpiContext::new(
		ctx.accounts.token_program.to_account_info(),
		Transfer {
			from: ctx.accounts.subscriber_token_account.to_account_info(),
			to: ctx.accounts.fee_recipient_token_account.to_account_info(),
			authority: ctx.accounts.subscriber.to_account_info(),
		},
	));

	// For increase allowance, we do not enforce the refund to succeed
	// If it fails, the authority owner pays the compute cost in SOL
	let refund_result = match refund_result {
		Ok(amount) => amount,
		Err(_) => 0,
	};

	// Update the used allowance with the refunded amount (best-effort, if it fails, the authority owner pays the compute cost in SOL)
	subscription.used_allowance = refund_result; // Update the used allowance for the subscription
	permit_registry.use_allowance(refund_result)?; // And update the permit registry as well

    emit!(AllowanceIncreased {
        new_allowance,
        uuid,
    });

    Ok(())
}


pub fn update_max_amount(ctx: Context<UpdateMaxAmount>, uuid: [u8; 16], new_max_amount: u64, compute_refund_params: ComputeRefundData) -> Result<()> {
	let subscription = &mut ctx.accounts.subscription;

	if new_max_amount == 0 {
		return err!(QBitFlowError::ZeroAmount);
	}

	if new_max_amount <= subscription.last_payment_amount {
		return err!(QBitFlowError::MaxAmountInvalid);
	}

	subscription.max_amount = new_max_amount;


	// Refund the compute cost to the authority owner
	let refund_result = compute_refund(uuid, new_max_amount - subscription.last_payment_amount, compute_refund_params, CpiContext::new(
		ctx.accounts.token_program.to_account_info(),
		Transfer {
			from: ctx.accounts.subscriber_token_account.to_account_info(),
			to: ctx.accounts.fee_recipient_token_account.to_account_info(),
			authority: ctx.accounts.subscriber.to_account_info(),
		},
	))?;

	let permit_registry = &mut ctx.accounts.permit_registry;

	permit_registry.use_allowance(refund_result)?; // And update the permit registry as well
	subscription.used_allowance = refund_result; // Update the used allowance for the subscription

	emit!(MaxAmountUpdated {
		uuid,
		new_max_amount,
	});

	Ok(())
}