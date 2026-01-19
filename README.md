# QBitFlow Payment System - Solana Program

[![License: MPL 2.0](https://img.shields.io/badge/License-MPL%202.0-brightgreen.svg)](https://opensource.org/licenses/MPL-2.0)
[![Anchor](https://img.shields.io/badge/Anchor-0.31.1-blue.svg)](https://www.anchor-lang.com/)
[![Solana](https://img.shields.io/badge/Solana-1.18+-purple.svg)](https://solana.com/)

> Non-custodial payment and subscription infrastructure for Solana

🌐 **Website:** [qbitflow.app](https://qbitflow.app)

**Deployed Program ID:** [`861HhZZMrLC1twfHciW3YuRviNAKDrEd9uVmqrBHc2f8`](https://solscan.io/address/861HhZZMrLC1twfHciW3YuRviNAKDrEd9uVmqrBHc2f8)

## 📋 Overview

QBitFlow is a decentralized payment system that enables **non-custodial** one-time payments and recurring subscriptions on Solana. Built with security and user experience in mind, QBitFlow allows users to pay in SPL tokens without holding SOL for transaction fees, through an innovative compute refund mechanism.

### Key Features

- ✅ **Non-Custodial**: Users maintain full control of their funds at all times
- 💰 **One-Time Payments**: Process instant payments in SOL or any SPL token
- 🔄 **Recurring Subscriptions**: Automated subscription payments with configurable frequencies
- ⚡ **Pay-As-You-Go**: Flexible subscription model with variable payment amounts
- 💸 **Compute Refunds**: Users can pay in tokens without holding SOL for fees
- 🏢 **Organization Fees**: Support for revenue sharing with platform partners
- 🔐 **PDA-Based Security**: Secure delegation using Program Derived Addresses
- 📊 **Permit Registry**: Efficient allowance management across multiple subscriptions
- 🎯 **Anchor Framework**: Type-safe and developer-friendly program interface

## 🏗️ Architecture

The system is built using the Anchor framework and consists of several core modules:

### 1. **Authority System** (`state.rs`, `instructions/initialize.rs`)
Manages program ownership and administrative functions.

**Capabilities:**
- Initialize program with owner and co-signer
- Update program ownership (requires both owner and co-signer)
- Manage fee recipient account
- PDA-based authority for secure delegation

### 2. **Payment Processing** (`instructions/payments.rs`)
Handles one-time payments in SOL and SPL tokens.

**Features:**
- Process SOL payments with automatic fee distribution
- Process SPL token payments with permit-based delegation
- Calculate and distribute fees to owner and organizations
- Compute refund mechanism for gasless transactions
- Event emission for payment tracking

### 3. **Subscription Management** (`instructions/subscriptions.rs`)
Manages recurring subscriptions and pay-as-you-go models.

**Functions:**
- Create regular subscriptions with fixed frequencies
- Create pay-as-you-go subscriptions with flexible amounts
- Execute subscription payments automatically
- Cancel subscriptions (user or admin)
- Increase subscription allowances
- Update maximum payment amounts
- Force cancel (admin only)

### 4. **Permit Registry** (`instructions/permit.rs`)
Tracks token allowances across multiple subscriptions per user.

**Purpose:**
- Manage total allowances per user-token pair
- Track cumulative token usage across subscriptions
- Delegate tokens using SPL Token Program
- Add/revoke allowances when subscriptions change
- Prevent allowance conflicts and overspending

### 5. **Compute Refund System** (`instructions/compute_refund.rs`)
Enables gasless transactions by refunding compute costs in tokens.

**Mechanism:**
- Owner pays SOL for compute fees upfront
- User refunds owner in SPL tokens
- Best-effort basis (doesn't fail transaction if refund fails)
- Price-based conversion from SOL to tokens

## 📊 Program Flow Diagrams

### One-Time Payment Flow
```
User → Backend → process_sol_payment/process_token_payment
                   ↓
            Validates amount and fees
                   ↓
         Transfers fee to authority owner
                   ↓
         Transfers organization fee (if applicable)
                   ↓
         Transfers remaining amount to merchant
                   ↓
         Refunds compute cost in tokens (SPL only)
                   ↓
         Emits PaymentProcessed event
```

### Subscription Creation Flow
```
User → Backend → create_subscription
                   ↓
            Validates frequency and amounts
                   ↓
         Creates/Updates Permit Registry
                   ↓
         Adds allowance to permit registry
                   ↓
         Delegates tokens to program PDA
                   ↓
         Creates Subscription PDA
                   ↓
         Computes subscription hash
                   ↓
         Refunds compute cost in tokens
                   ↓
         Emits SubscriptionCreated event
```

### Subscription Execution Flow
```
Backend → execute_subscription
            ↓
      Verifies payment is due
            ↓
      Validates subscription hash
            ↓
      Checks allowance availability
            ↓
      Transfers fee to authority owner (via PDA delegation)
            ↓
      Transfers organization fee (if applicable)
            ↓
      Transfers remaining amount to merchant
            ↓
      Updates subscription state
            ↓
      Updates permit registry usage
            ↓
      Updates next payment date
            ↓
      Refunds compute cost in tokens
            ↓
      Emits SubscriptionPaymentProcessed event
```

### Cancel Subscription Flow
```
User/Admin → cancel_subscription
               ↓
         Validates subscription exists
               ↓
         Checks authorization (user or admin)
               ↓
         Revokes allowance from permit registry
               ↓
         Closes subscription PDA
               ↓
         Emits SubscriptionCancelled event
```

## 🔧 Installation

### Prerequisites
- Rust >= 1.75.0
- Solana CLI >= 1.18.0
- Anchor CLI >= 0.31.1
- Node.js >= 16.x
- Yarn or npm

### Setup

```bash
# Clone the repository
git clone https://github.com/QBitFlow/qbitflow-sol-programs.git
cd qbitflow-sol-programs

# Install dependencies
npm install

# Build the program
anchor build

# Run tests
anchor test

# Local deployment

# Start local validator and deploy program
anchor localnet
node scripts/deploy-local.js .env.development  <co-signer-public-key>
```

## 📝 Program API

### Initialization

#### `initialize`
Initialize the payment system and set the authority.

```rust
pub fn initialize(
    ctx: Context<Initialize>,
    co_signer: Pubkey
) -> Result<()>
```

**Parameters:**
- `co_signer`: Required co-signer for administrative updates

**Accounts:**
- `authority`: PDA account to initialize
- `signer`: Transaction signer (becomes owner)
- `system_program`: Solana System Program

#### `update_owner`
Update the program owner (requires both owner and co-signer).

```rust
pub fn update_owner(
    ctx: Context<UpdateOwner>,
    new_owner: Pubkey
) -> Result<()>
```

#### `set_delegate`
Reset delegate on user's token account to current effective allowance.

```rust
pub fn set_delegate(
    ctx: Context<SetDelegate>
) -> Result<()>
```

### One-Time Payments

#### `process_sol_payment`
Process a payment in native SOL.

```rust
pub fn process_sol_payment(
    ctx: Context<ProcessSolPayment>,
    amount: u64,
    fee_bps: u16,
    uuid: [u8; 16],
    organization_fee_bps: u16
) -> Result<()>
```

**Parameters:**
- `amount`: Payment amount in lamports
- `fee_bps`: Fee percentage in basis points (1 bps = 0.01%)
- `uuid`: Unique identifier for the payment
- `organization_fee_bps`: Organization fee percentage in basis points

**Accounts:**
- `authority_and_owner`: Authority PDA and owner accounts
- `payer`: User making the payment
- `fee_recipient`: Receives the platform fee (authority owner)
- `merchant`: Receives the payment
- `organization_fee_recipient`: Optional organization fee recipient
- `system_program`: Solana System Program

#### `process_token_payment`
Process a payment in SPL tokens with compute refund.

```rust
pub fn process_token_payment(
    ctx: Context<ProcessTokenPayment>,
    amount: u64,
    fee_bps: u16,
    uuid: [u8; 16],
    organization_fee_bps: u16,
    compute_refund_params: ComputeRefundData
) -> Result<()>
```

**Additional Parameters:**
- `compute_refund_params`: Contains token price and compute cost for refund calculation

**Additional Accounts:**
- `payer_token_account`: Payer's SPL token account
- `merchant_token_account`: Merchant's SPL token account
- `fee_recipient_token_account`: Fee recipient's token account
- `organization_token_account`: Organization's token account
- `mint`: SPL token mint
- `token_program`: SPL Token Program
- `associated_token_program`: Associated Token Program

### Subscriptions

#### `create_subscription`
Create a new recurring subscription.

```rust
pub fn create_subscription(
    ctx: Context<CreateSubscription>,
    uuid: [u8; 16],
    amount: u64,
    max_amount: u64,
    frequency: u32,
    allowance: u64,
    compute_refund_params: ComputeRefundData,
    is_payg: bool
) -> Result<()>
```

**Parameters:**
- `uuid`: Unique identifier for the subscription
- `amount`: Initial/expected payment amount per period
- `max_amount`: Maximum allowed payment per period (must be > amount)
- `frequency`: Payment frequency in seconds (minimum 7 days = 604800 seconds)
- `allowance`: Total tokens reserved for this subscription
- `compute_refund_params`: Compute refund calculation data
- `is_payg`: Whether this is a pay-as-you-go subscription

**Accounts:**
- `authority_and_owner`: Authority PDA and owner accounts
- `permit_registry`: PDA tracking allowances for subscriber-mint pair
- `subscription`: PDA for this specific subscription
- `subscriber`: User creating the subscription
- `subscriber_token_account`: Subscriber's token account
- `merchant`: Merchant receiving payments
- `merchant_token_account`: Merchant's token account
- `fee_recipient_token_account`: Fee recipient's token account
- `mint`: SPL token mint
- `organization`: Optional organization account
- `organization_token_account`: Organization's token account
- `system_program`, `token_program`, `associated_token_program`

**Notes:**
- Regular subscriptions: First payment due immediately
- Pay-as-you-go: First payment due after one frequency period

#### `execute_subscription`
Execute a subscription payment (called by backend when payment is due).

```rust
pub fn execute_subscription(
    ctx: Context<ExecuteSubscription>,
    amount: u64,
    fee_bps: u16,
    uuid: [u8; 16],
    frequency: u32,
    organization_fee_bps: u16,
    compute_refund_params: ComputeRefundData,
    is_payg: bool
) -> Result<()>
```

**Parameters:**
- `amount`: Payment amount for this period (must be < max_amount)
- `fee_bps`: Fee percentage in basis points
- `uuid`: Subscription identifier
- `frequency`: Must match subscription's original frequency
- `organization_fee_bps`: Organization fee percentage
- `compute_refund_params`: Compute refund data
- `is_payg`: Whether this is a pay-as-you-go subscription

**Validation:**
- Payment must be due (`current_time >= next_payment_due`)
- Amount must not exceed `max_amount`
- Subscription hash must match (validates merchant, subscriber, frequency, organization)
- Sufficient allowance must remain

**Accounts:**
- `authority_and_owner`: Authority PDA and owner accounts
- `subscription`: Subscription PDA
- `permit_registry`: Permit registry PDA
- `subscriber`: Subscription owner (must match)
- `subscriber_token_account`: Subscriber's token account
- `merchant`: Merchant receiving payment
- `merchant_token_account`: Merchant's token account
- `fee_recipient_token_account`: Fee recipient's token account
- `mint`: SPL token mint
- `organization`: Organization account
- `organization_token_account`: Organization's token account
- `system_program`, `token_program`, `associated_token_program`

#### `cancel_subscription`
Cancel an active subscription.

```rust
pub fn cancel_subscription(
    ctx: Context<CancelSubscription>,
    uuid: [u8; 16],
    is_payg: bool
) -> Result<()>
```

**Parameters:**
- `uuid`: Subscription identifier
- `is_payg`: Whether this is a pay-as-you-go subscription

**Authorization:**
- Can be called by subscriber at any time
- For regular subscriptions: Can only cancel if payment is not due yet
- For pay-as-you-go: Can cancel anytime after stopping

**Accounts:**
- `authority_and_owner`: Authority PDA and owner accounts
- `subscription`: Subscription PDA (closed after cancellation)
- `permit_registry`: Permit registry PDA (allowance revoked)
- `subscriber`: Must sign and match subscription owner
- `system_program`

#### `force_cancel_subscription`
Force cancel a subscription (admin only).

```rust
pub fn force_cancel_subscription(
    ctx: Context<ForceCancelSubscription>,
    uuid: [u8; 16]
) -> Result<()>
```

**Authorization:** Only program owner can call this function.

#### `increase_allowance`
Increase the allowance for an existing subscription.

```rust
pub fn increase_allowance(
    ctx: Context<IncreaseAllowance>,
    uuid: [u8; 16],
    new_allowance: u64,
    compute_refund_params: ComputeRefundData
) -> Result<()>
```

**Parameters:**
- `uuid`: Subscription identifier
- `new_allowance`: New total allowance (must be > current allowance)
- `compute_refund_params`: Compute refund data

**Effect:**
- Increases subscription's total allowance
- Updates permit registry's total allowance
- Approves additional tokens for program delegation

#### `update_max_amount`
Update the maximum payment amount per period for a subscription.

```rust
pub fn update_max_amount(
    ctx: Context<UpdateMaxAmount>,
    uuid: [u8; 16],
    new_max_amount: u64,
    compute_refund_params: ComputeRefundData
) -> Result<()>
```

**Parameters:**
- `uuid`: Subscription identifier
- `new_max_amount`: New maximum amount per period
- `compute_refund_params`: Compute refund data

**Validation:**
- `new_max_amount` must be greater than last payment amount
- Only subscriber can update this

## 🔐 Security Features

### Program Derived Addresses (PDAs)
- **Authority PDA**: Seeds: `["authority"]`
  - Serves as program's signing authority
  - Cannot be controlled by external wallets
  
- **Subscription PDA**: Seeds: `["subscription", uuid]`
  - Unique per subscription
  - Prevents duplicate subscriptions
  
- **Permit Registry PDA**: Seeds: `["permit_registry", subscriber, mint]`
  - Tracks allowances per user-token pair
  - Prevents allowance conflicts across subscriptions

### Access Control
- **Owner-only functions**: `update_owner`, `force_cancel_subscription`
- **Co-signer requirement**: `update_owner` requires both owner and co-signer
- **Subscriber-only functions**: `cancel_subscription`, `increase_allowance`, `update_max_amount`
- **Address validation**: All accounts validated against PDAs and expected addresses

### Subscription Hash Validation
Subscriptions use a hash to ensure parameters cannot be modified:
```rust
hash(merchant_token_account, subscriber_token_account, frequency, organization_token_account)
```
This prevents:
- Changing the merchant during execution
- Modifying the frequency after creation
- Redirecting organization fees

### Allowance Management
- **Dual-layer tracking**: Both subscription-level and global permit registry
- **Overflow protection**: All arithmetic operations checked for overflow
- **Delegate-based transfers**: Uses SPL Token Program's delegate mechanism
- **Automatic revocation**: Allowances revoked on subscription cancellation

### Fee Protection
- **Minimum fee**: `MIN_FEE_FOR_CONTRACT_BPS = 75` (0.75%)
- **Maximum fee**: `MAX_FEE_BPS = 1000` (10%)
- **Fee validation**: Both owner and organization fees validated
- **Overflow checks**: All fee calculations checked for arithmetic safety

## 📊 Constants

```rust
// Fee denominator for basis points calculation
pub const FEE_DENOMINATOR: u16 = 10000;

// Minimum frequency: 7 days in seconds
pub const MIN_FREQUENCY: u32 = 7 * 86400; // 604800 seconds

// Minimum fee for contract: 0.75%
pub const MIN_FEE_FOR_CONTRACT_BPS: u16 = 75;

// Maximum fee: 10%
pub const MAX_FEE_BPS: u16 = 1000;

// PDA seeds
pub const AUTHORITY_PDA_SEED: &[u8] = b"authority";
pub const SUBSCRIPTION_PDA_SEED: &[u8] = b"subscription";
pub const PERMIT_REGISTRY_PDA_SEED: &[u8] = b"permit_registry";
```

## 📡 Events

The program emits the following events for tracking:

### `PaymentProcessed`
```rust
pub struct PaymentProcessed {
    pub uuid: [u8; 16],
}
```

### `SubscriptionCreated`
```rust
pub struct SubscriptionCreated {
    pub uuid: [u8; 16],
    pub next_payment_due: i64,
    pub initial_allowance: u64,
}
```

### `SubscriptionPaymentProcessed`
```rust
pub struct SubscriptionPaymentProcessed {
    pub uuid: [u8; 16],
    pub next_payment_due: i64,
    pub remaining_allowance: u64,
}
```

### `SubscriptionCancelled`
```rust
pub struct SubscriptionCancelled {
    pub uuid: [u8; 16],
}
```

### `AllowanceIncreased`
```rust
pub struct AllowanceIncreased {
    pub new_allowance: u64,
    pub uuid: [u8; 16],
}
```

### `MaxAmountUpdated`
```rust
pub struct MaxAmountUpdated {
    pub uuid: [u8; 16],
    pub new_max_amount: u64,
}
```

### `ComputeRefundFailed`
```rust
pub struct ComputeRefundFailed {
    pub uuid: [u8; 16],
}
```
*Emitted when compute refund fails (non-critical, transaction continues)*

## ⚠️ Error Codes

```rust
pub enum QBitFlowError {
    ZeroAmount,                      // Payment amount cannot be zero
    InvalidFeePercentage,            // Fee exceeds maximum allowed
    PaymentNotDueYet,                // Subscription payment not yet due
    InvalidFrequency,                // Frequency below minimum (7 days)
    InsufficientAllowance,           // Not enough tokens reserved
    Unauthorized,                    // Caller not authorized
    Overflow,                        // Arithmetic overflow detected
    InvalidSubscriptionParameters,   // Subscription hash mismatch
    CannotCancelActiveSubscription,  // Cannot cancel when payment due
    MaxAmountExceeded,               // Payment exceeds maximum allowed
    InvalidAmount,                   // Amount validation failed
    MaxAmountInvalid,                // Max amount lower than last payment
}
```

## 💡 Compute Refund Mechanism

One of the unique features of QBitFlow on Solana is the compute refund system, which enables gasless transactions for users paying in SPL tokens.

### How It Works

1. **Authority pays upfront**: The program owner pays SOL for compute units
2. **User refunds in tokens**: User transfers equivalent value in SPL tokens
3. **Price-based conversion**: Uses provided price data to calculate token amount
4. **Best-effort basis**: If refund fails, transaction continues (owner absorbs cost)

### Parameters

```rust
pub struct ComputeRefundData {
    pub token_price_in_lamports: u64,  // Token price in lamports
    pub compute_cost_in_lamports: u64, // Estimated compute cost
}
```

### Calculation

```rust
compute_cost_in_tokens = (compute_cost_in_lamports * token_price_in_lamports) / 1_000_000_000
```

### Safety Measures

- **Max refund limit**: For subscriptions, refund cannot exceed `max_amount - payment_amount`
- **Non-failing**: Refund failure emits event but doesn't revert transaction
- **Signed data**: User signs refund parameters, preventing manipulation


## 📚 Additional Resources

- **Anchor Documentation**: https://www.anchor-lang.com/
- **Solana Documentation**: https://docs.solana.com/
- **SPL Token Program**: https://spl.solana.com/token
- **QBitFlow Website**: https://qbitflow.app
- **QBitFlow Documentation**: https://qbitflow.app/docs


## 📄 License

This project is licensed under the Mozilla Public License 2.0 - see the [LICENSE](LICENSE) file for details.

## 🔒 Security

For security concerns, please email _security@qbitflow.app_. Do not open public issues for security vulnerabilities.

## 📞 Support

- **Website**: [qbitflow.app](https://qbitflow.app)
- **Documentation**: [qbitflow.app/docs](https://qbitflow.app/docs)
- **Issues**: [GitHub Issues](https://github.com/QBitFlow/qbitflow-sol-programs/issues)
- **Email**: support@qbitflow.app
