# QBitFlow Payment System - Solana Program

[![License: MPL 2.0](https://img.shields.io/badge/License-MPL%202.0-brightgreen.svg)](https://opensource.org/licenses/MPL-2.0)
[![Anchor](https://img.shields.io/badge/Anchor-1.0.2-blue.svg)](https://www.anchor-lang.com/)
[![Solana](https://img.shields.io/badge/Solana-2.x-purple.svg)](https://solana.com/)
[![Version](https://img.shields.io/badge/Program-v2.0.0-orange.svg)](#)

> Non-custodial payment and subscription infrastructure for Solana

🌐 **Website:** [qbitflow.app](https://qbitflow.app)

**Program ID:** [`861HhZZMrLC1twfHciW3YuRviNAKDrEd9uVmqrBHc2f8`](https://solscan.io/address/861HhZZMrLC1twfHciW3YuRviNAKDrEd9uVmqrBHc2f8)

**Testnet Program ID:** [`DmCGjNxLMc1xzxJQdi6PguTY6JCjBAEmJVMn9JuvPpF`](https://solscan.io/address/DmCGjNxLMc1xzxJQdi6PguTY6JCjBAEmJVMn9JuvPpF?cluster=devnet)

> It ships as an **in-place upgrade** of the v1 deployment (same Program ID above — the program is *not* redeployed to a new address). Once v2 is live, the upgrade authority will be **revoked**, making the program **immutable**. v1 was intentionally left upgradeable to allow this update.

## 📋 Overview

QBitFlow is a decentralized payment system for **non-custodial** one-time payments and recurring subscriptions on Solana. Users pay in SPL tokens **without holding SOL** for transaction fees, via a token-denominated compute-refund mechanism. The Solana program is built to **feature/security/flow parity** with QBitFlow's EVM contract (see [EVM Parity & Design Decisions](#-evm-parity--design-decisions)).

### Key Features

- ✅ **Non-Custodial** — funds never leave the user's wallet except via SPL delegate transfers they pre-authorized.
- 💰 **One-Time Payments** — instant payments in native SOL or any SPL / Token-2022 token.
- 🔄 **Recurring Subscriptions** (`Regular`) — fixed-cadence billing with a per-cycle cap.
- ⚡ **Pay-As-You-Go** (`X402` / `MerchantX402`) — off-chain micropayment accrual settled on-chain in **nonce-sequenced batches** committed by a merkle root.
- 💸 **Gasless for Users** — the operator (relayer) pays SOL fees and is reimbursed in tokens, hard-capped by `MAX_COMPUTE_COST_LAMPORTS`.
- 🏢 **Layered Fees** — platform fee + optional **referral** cut (a slice of the platform fee) + optional **organization** fee, with strict recipient-distinctness validation.
- 🔐 **PDA-Based Delegation** — the program's Authority PDA is the SPL delegate; it can pull only what the subscriber pre-approved.
- 📊 **Permit Registry** — per `(subscriber, mint)` allowance accounting shared across all of a user's subscriptions.
- 🔑 **Session-Signer Rotation** — off-chain receipt-signing keys can be rotated, optionally settling a final batch atomically under the old key.
- 🧾 **Subscription Digest** — a domain-tagged hash commits every consent-bearing parameter, so a compromised relayer cannot vary fees, frequency, mode, recipients, or signing keys.

## 🧭 Settlement Modes

The settlement mode is chosen at `create_subscription` and is **immutable** for the life of the subscription (it is committed into the subscription digest).

| Mode | Discriminant | Settled via | First bill | Receipt signer |
|------|--------------|-------------|------------|----------------|
| **Regular** | `0` | `execute_subscription` | **Immediately** at creation (cycle 1 billed atomically) | n/a (operator triggers when due) |
| **X402** | `1` | `settle_x402_batch` | Deferred by one `frequency` (usage accrues off-chain) | Subscriber (or delegated session key) |
| **MerchantX402** | `2` | `settle_merchant_x402_batch` | Deferred by one `frequency` | **Merchant** (merkle leaves are merchant-signed usage reports) |

`session_signer == Pubkey::default()` means "the subscriber signs receipts directly." X402 / MerchantX402 normally delegate to a hot key (the merchant's key for `MerchantX402`).

## 🏗️ Architecture

Anchor program, organized into focused instruction modules under `programs/qbitflow_payment_system/src/`:

### `state.rs` — Accounts, events, fee math
- `Authority` PDA (owner + co-signer), `Subscription` PDA, the `Fee` / `Amounts` argument structs, the `SettleMode` enum, all events, and the shared `calculate_fee` / `validate_fee_recipients` / `create_subscription_hash` helpers.

### `instructions/initialize.rs` — Authority system
- `initialize`, `update_owner`, `update_co_signer`, `set_delegate`. Owner = protocol fee recipient + relayer key. Co-signer = cold-wallet key required (alongside owner) for ownership/co-signer rotation.

### `instructions/payments.rs` — One-time payments
- `process_payment` (native SOL) and `process_token_payment` (SPL). Four-leg fee split; the payer is a **live signer** and the operator co-signs.

### `instructions/subscriptions.rs` — Subscription lifecycle
- `create_subscription`, `execute_subscription` (Regular), `cancel_subscription`, `cancel_pay_as_you_go_subscription`, `force_cancel_subscription`, `extend_allowance`, `update_max_amount_per_cycle`.

### `instructions/settle.rs` — PAYG batch settlement
- `settle_x402_batch` and `settle_merchant_x402_batch`. Both share one internal path differing only in the emitted event; nonce-sequenced, per-cycle-capped, period-gated.

### `instructions/rotate_signer.rs` — Session-key rotation
- `rotate_session_signer` (subscriber-authorized) and `rotate_merchant_signer` (merchant-authorized). Optional atomic final settle under the old key, gated by an Ed25519 attestation.

### `instructions/permit.rs` — Permit registry
- `PermitRegistry` per `(subscriber, mint)`: tracks `total_allowance` / `total_used` and (re-)approves the program PDA as SPL delegate.

### `instructions/compute_refund.rs` — Token-denominated gas refund
- Converts a backend-claimed SOL cost into tokens (ceiling division, hard-capped) and pulls it to the fee recipient. Hard-revert and best-effort variants.

### `instructions/ed25519.rs` — Ed25519 precompile verification
- Reads the Instructions sysvar to confirm a runtime-verified `Ed25519Program` precompile carries the expected signer + message (used for rotation atomic-settle attestations).

### `instructions/mint_check.rs` — Token-2022 extension policy
- Rejects mints with `TransferFeeConfig` or `ConfidentialTransferMint` extensions (incompatible with exact-amount accounting and the delegate model).

### `instructions/shared.rs` — Settlement core
- `verify_subscription_digest`, `check_subscription_allowance`, `distribute_payment_pda` (the 4-leg + refund payout), `close_subscription`, `batch_attestation_message`.

## 📊 Program Flow Diagrams

### One-Time Token Payment
```
Payer + Operator(owner) sign → process_token_payment
        ↓ reject unsupported Token-2022 extensions
        ↓ calculate_fee (platform / referral / org / merchant)
        ↓ validate_fee_recipients (distinctness)
        ↓ transfer platform fee  → owner ATA
        ↓ transfer referral fee  → referrer ATA   (if referral active & in deadline)
        ↓ transfer organization  → org ATA        (if org fee > 0)
        ↓ transfer remainder     → merchant ATA
        ↓ gas refund (tokens)    → owner ATA       (capped by MAX_COMPUTE_COST_LAMPORTS)
        ↓ emit PaymentProcessed
```

### Subscription Creation (`Regular`)
```
Subscriber + Operator sign → create_subscription(mode = Regular)
        ↓ validate frequency / amount / max_amount / allowance / mode
        ↓ reject unsupported Token-2022 extensions
        ↓ validate referral terms + fee-recipient distinctness
        ↓ permit_registry.add_allowance → approve PDA as SPL delegate
        ↓ compute subscription digest (domain-tagged hash of consent params)
        ↓ BILL CYCLE 1 ATOMICALLY (4-leg split + gas refund via PDA delegate)
        ↓ next_payment_due = now + frequency
        ↓ emit SubscriptionCreated
```
*PAYG modes (`X402` / `MerchantX402`) skip the atomic first bill — usage accrues off-chain and is billed at period end. Only the creation-tx gas refund is pulled.*

### Regular Billing
```
Operator → execute_subscription (mode = Regular)
        ↓ require now >= next_payment_due
        ↓ require amount < max_amount_per_cycle
        ↓ verify_subscription_digest (no drift in any consent param)
        ↓ check allowance (per-sub + registry)
        ↓ distribute_payment_pda (4-leg + gas refund via PDA delegate)
        ↓ used_allowance += spent ; current_cycle_used = spent
        ↓ next_payment_due += frequency
        ↓ emit SubscriptionPaymentProcessed
```

### PAYG Batch Settlement (`X402` / `MerchantX402`)
```
Operator → settle_x402_batch / settle_merchant_x402_batch
        ↓ mode binding (Wrong mode → WrongSettleMode)
        ↓ require now >= next_payment_due          (one settle per period; anti dust-drain)
        ↓ require amount < max_amount_per_cycle
        ↓ verify_subscription_digest
        ↓ require first_nonce >= next_nonce && last_nonce >= first_nonce
        ↓ check allowance
        ↓ distribute_payment_pda (4-leg + gas refund)
        ↓ current_cycle_used = spent ; next_nonce = last_nonce + 1
        ↓ next_payment_due = now + frequency       (anchored to now, not prior due)
        ↓ emit X402BatchSettled / MerchantX402BatchSettled
        ↓ if subscription was `stopped`: final bill done → emit SubscriptionCancelled → close
```

### Session-Signer Rotation (optional atomic settle)
```
Subscriber (X402) / Merchant (MerchantX402) signs → rotate_*_signer
        ↓ verify_subscription_digest with OLD session_signer  (proves knowledge of current state)
        ↓ check authority_signer == subscriber|merchant for the mode
        ↓ if settle.is_some():
            ↓ require all settle accounts present
            ↓ CUMULATIVE cap: current_cycle_used + amount < max_amount_per_cycle
            ↓ nonce range check
            ↓ verify Ed25519 attestation over (uuid||first||last||amount||root) by authority
            ↓ distribute_payment_pda (cap = max - current_cycle_used - amount)
            ↓ current_cycle_used += spent ; next_nonce = last+1 ; next_payment_due = now + freq
            ↓ emit *BatchSettled (+ close if stopped)
        ↓ rewrite digest with NEW session_signer
        ↓ emit SessionSignerRotated
```

## 🔧 Installation

### Prerequisites
- Rust >= 1.75
- Solana CLI (Agave) 2.x
- Anchor CLI 1.0.2
- Node.js >= 18

### Setup

```bash
npm install

# Build the program + generate the IDL
anchor build

# Run the integration suite.
# NOTE: tests/test.ts uses a 5-second billing cycle, so MIN_FREQUENCY in
# constants.rs must be temporarily lowered to 5 before testing, then restored
# to the production value (3600 = 1 hour). See the TODO at the top of test.ts.
anchor test

# Local validator deploy
anchor localnet
node scripts/deploy-local.js .env.development <co-signer-public-key>
```

## 📝 Program API

Shared argument structs:

```rust
// Fee schedule (numeric/temporal fields; recipient ADDRESSES travel as accounts/ATAs).
pub struct Fee {
    pub platform_fee_bps: u16,
    pub organization_fee_bps: u16,
    pub referral_fee_bps: u16,   // a slice OF the platform fee
    pub referral_deadline: i64,  // unix ts; referral leg ignored once passed (0 when no referrer)
}

// Backend-supplied gas reimbursement inputs.
pub struct ComputeRefundData {
    pub token_price_in_lamports: u64,  // lamports per SMALLEST token unit (pre-scaled by decimals)
    pub compute_cost_in_lamports: u64, // SOL cost to reimburse; hard-capped by MAX_COMPUTE_COST_LAMPORTS
}

// Optional atomic-settle payload for rotation.
pub struct RotateSettleArgs {
    pub amount: u64,
    pub first_nonce: u64,
    pub last_nonce: u64,
    pub merkle_root: [u8; 32],
    pub attestation_ix_index: u16,        // index of the Ed25519 precompile ix in this tx
    pub compute_refund_params: ComputeRefundData,
}
```

### Authority

| Instruction | Signers | Purpose |
|-------------|---------|---------|
| `initialize(co_signer)` | signer (→ owner) | Create the Authority PDA. `co_signer` must be non-default and ≠ owner. |
| `update_owner(new_owner)` | owner **+** co_signer | Rotate the relayer/fee-recipient key (multi-sig). |
| `update_co_signer(new_co_signer)` | owner **+** co_signer | Rotate the cold-wallet co-signer (multi-sig). |
| `set_delegate()` | owner + subscriber | Restore the program PDA as SPL delegate using the registry's effective allowance (no privilege escalation). |

### One-Time Payments

```rust
pub fn process_payment(ctx, amount: u64, uuid: [u8; 16], fee: Fee) -> Result<()>
pub fn process_token_payment(ctx, amount: u64, uuid: [u8; 16], fee: Fee, compute_refund_params: ComputeRefundData) -> Result<()>
```
- Both the **payer** and the **operator (owner)** sign. Native SOL takes **no** gas refund (`refund_amount = 0`); the SPL path takes a token gas refund to the owner ATA.
- A referrer account/ATA is **required** when `fee.referral_fee_bps > 0`; an organization account/ATA is **required** when `fee.organization_fee_bps > 0`.

### Subscriptions

```rust
pub fn create_subscription(
    ctx, uuid: [u8; 16], amount: u64, max_amount_per_cycle: u64, frequency: u32,
    fee: Fee, allowance: u64, compute_refund_params: ComputeRefundData,
    mode: u8, session_signer: Pubkey,
) -> Result<()>
```
- `max_amount_per_cycle > amount`; `allowance >= max_amount_per_cycle` and `allowance > 0`; `frequency >= MIN_FREQUENCY`.
- `mode` must be a valid `SettleMode` (0/1/2). `Regular` bills cycle 1 atomically; PAYG defers.
- Subscription PDA seeds: `["subscription", subscriber, uuid]` — UUIDs cannot collide across subscribers or be front-run.

```rust
pub fn execute_subscription(ctx, amount, uuid, frequency, fee, compute_refund_params, mode, session_signer) -> Result<()>
```
- **Regular only** (PAYG rejected with `WrongSettleMode`). Requires `now >= next_payment_due` and `amount < max_amount_per_cycle`. Re-derives the digest and rejects any drift.

```rust
pub fn settle_x402_batch(ctx, uuid, amount, fee, frequency, session_signer,
                         first_nonce, last_nonce, merkle_root, compute_refund_params) -> Result<()>
pub fn settle_merchant_x402_batch(ctx, /* identical signature */) -> Result<()>
```
- PAYG settlement. One settle per period (`now >= next_payment_due`), nonce-sequenced (`first_nonce >= next_nonce`), per-cycle-capped. The `merkle_root` is committed in the event log; per-receipt detail stays off-chain.

```rust
pub fn rotate_session_signer(ctx, uuid, frequency, fee, mode,
                             old_session_signer, new_session_signer, settle: Option<RotateSettleArgs>) -> Result<()>
pub fn rotate_merchant_signer(ctx, /* identical signature */) -> Result<()>
```
- Rebinds the digest to `new_session_signer`. With `settle = Some(..)`, it settles a final batch under the **old** signer in the same tx — bypassing the period guard (authorized by the Ed25519 attestation) but using a **cumulative** per-cycle cap and rolling the period, so billing stays bounded by `max_amount_per_cycle` per window even across a boundary.

```rust
pub fn cancel_subscription(ctx, uuid) -> Result<()>                  // Regular only; subscriber-signed; only before next_payment_due
pub fn cancel_pay_as_you_go_subscription(ctx, uuid) -> Result<()>    // PAYG only; marks `stopped`, finalizes on next settle
pub fn force_cancel_subscription(ctx, uuid) -> Result<()>            // owner-only escape hatch; bypasses the due-date guard
pub fn extend_allowance(ctx, uuid, new_allowance, compute_refund_params) -> Result<()>          // subscriber-signed; replaces allowance
pub fn update_max_amount_per_cycle(ctx, uuid, new_max_amount, compute_refund_params) -> Result<()> // subscriber-signed; ↑ ≤3×, ↓ above current-cycle spend
```

## 🧠 Key Design Decisions

These are the deliberate engineering choices behind the program — worth understanding before reading or extending it.

### 1. Live transaction signers instead of off-chain signature schemes
On EVM, consent is carried by EIP-712 signatures the operator submits. On Solana, the **subscriber/payer signs the actual transaction**. This is simpler and trust-minimizing: the wallet shows the real accounts (merchant, amounts, fees) before signing. A direct consequence: `max_amount_per_cycle` is **deliberately excluded** from the subscription digest — the user signs every create/update/cancel tx themselves, so there is no need to commit the cap into a hash (and doing so would force a digest rewrite on every cap change).

### 2. The subscription digest is the single source of consent
`create_subscription_hash` produces a domain-tagged (`qbf:sub:v2`) hash over: merchant ATA, subscriber ATA, frequency, organization ATA, referrer ATA, referral bps, referral deadline, **session signer**, and **mode**. Every settle/rotate re-derives this hash from caller-supplied params and rejects any mismatch. **Even a fully compromised relayer cannot** change the merchant, fee schedule, frequency, mode, referral terms, or signing key. The mint is bound implicitly because ATAs derive from `(owner, mint)`.

### 3. Session signer is stored only in the digest, not as a field
Rather than persist `session_signer` (and a version counter) in account state, it lives **only** inside the digest. Rotation rewrites the digest; off-chain readers reconstruct the current signer from the `SubscriptionCreated` + `SessionSignerRotated` event log. This shrinks the account and removes a class of "field vs. hash" drift bugs.

### 4. PAYG batches: commit a merkle root, keep detail off-chain
A batch's on-chain footprint is a single `merkle_root` + aggregate amount + nonce range (in the event log). Per-receipt detail (user-signed receipts for X402, merchant-signed usage reports for MerchantX402) stays off-chain. On-chain authorization is the digest + per-cycle cap + monotonic nonce range. Nonces are **monotonic-greater** (`first_nonce >= next_nonce`), so the relayer may skip burned/lost receipts but can never replay.

### 5. Period anchoring to prevent cadence drift and dust-drain
PAYG settles advance `next_payment_due = now + frequency` (anchored to *now*), so a relayer that lets billing slip cannot drift the cadence forward, and cannot stack multiple settles in one window to drain the per-cycle cap. Regular billing advances from the previously-scheduled due time (fixed cadence). The rotation atomic-settle uses a **cumulative** (non-reset) `current_cycle_used` and also rolls the period — closing a ~2× cap leak across period boundaries.

### 6. Token-denominated gas refund, hard-capped in the program
Solana has no on-chain gas price, so the backend supplies `ComputeRefundData`. The user's sole protection against a compromised backend is `MAX_COMPUTE_COST_LAMPORTS` (0.1 SOL) — a hard ceiling with **no owner knob**; tightening it requires a program upgrade. Conversion uses **ceiling division** in `u128` (favoring the operator by ≤1 base unit, overflow-safe). On settle paths the refund is **hard-revert**; on user-facing edit paths (`extend_allowance`, `update_max_amount_per_cycle`) it is **best-effort** so a user can never be locked out of editing their own subscription.

### 7. Permit registry for multi-subscription allowance sharing
A single SPL token account has one delegate. The `PermitRegistry` per `(subscriber, mint)` tracks the **sum** of allowances and cumulative spend across all of a subscriber's subscriptions on that mint, and re-approves the PDA for the *effective* (allowance − used) amount — so multiple subscriptions coexist without clobbering each other's delegate approval.

### 8. Token-2022 extension allow-list-by-rejection
Mints with `TransferFeeConfig` (receiver gets less than `amount` — breaks every leg's accounting) or `ConfidentialTransferMint` (incompatible with the delegate model) are rejected **at the program boundary**, not left to fail mid-CPI, so users get a clean error. All other extensions (interest-bearing, metadata-pointer, transfer-hook, …) and all legacy SPL mints are accepted.

## 🔐 Security Features

### Program Derived Addresses
- **Authority PDA** — seeds `["authority"]`. The program's signing authority and SPL delegate; cannot be controlled by any external wallet.
- **Subscription PDA** — seeds `["subscription", subscriber, uuid]`. Scoped to the subscriber so UUIDs can't collide or be front-run.
- **Permit Registry PDA** — seeds `["permit_registry", subscriber, mint]`. Per-user-token allowance accounting.

### Access Control
- **Multi-sig (owner + co-signer):** `update_owner`, `update_co_signer`. The co-signer is a cold wallet reserved for these rotations only.
- **Owner-only:** `force_cancel_subscription` (merchant/user escape hatch routed through the backend — owner signature only, **no** co-signer, so it isn't a cold-wallet operation).
- **Subscriber-only:** `cancel_subscription`, `extend_allowance`, `update_max_amount_per_cycle` (address-constrained to `subscription.subscriber`).
- **Mode-appropriate authority:** rotation requires the subscriber (X402) or merchant (MerchantX402) signature, checked dynamically against the subscription's mode.
- **Account binding:** subscription/registry PDAs are re-derived from seeds; `UncheckedAccount`s used only for PDA derivation can't be spoofed (mismatched keys fail derivation).

### Consent Integrity (Subscription Digest)
The domain-tagged digest commits merchant, subscriber, frequency, organization, referrer + referral terms, session signer, and mode. Any drift at settle/rotate time reverts with `InvalidSubscriptionParameters`. "No referrer" is committed as `Pubkey::default()` / zero terms, so a referrer cannot be injected later.

### Fee Safety
- **Distinctness** (`validate_fee_recipients`): merchant ≠ payer; and platform / organization / referrer recipients must all be distinct — preventing an operator from disguising a higher composite fee as separate legs routed to one wallet.
- **Caps:** platform ≤ `MAX_FEE_PLATFORM_BPS` (15%); organization ≤ 50% of the post-platform remainder; referral ≤ 50% **of the platform fee** (it does not dilute the merchant or org). A `MIN_FEE_FOR_CONTRACT_BPS` (0.50%) floor is applied to the platform leg.
- **Required recipients:** an org/referral fee with no recipient reverts (`MissingOrganizationAccount` / `MissingReferrerAccount`) — it never silently shortchanges the merchant.
- **Deadline-gated referral:** the referral leg pays only while `referral_deadline >= now`.

### Allowance & Arithmetic
- **Dual-layer tracking:** per-subscription `used_allowance` and registry-wide `total_used`, both checked before every pull.
- **Per-cycle cap:** `max_amount_per_cycle` bounds each period's spend (including the refund leg, which needs strict headroom below the cap).
- **`update_max_amount_per_cycle` guards:** increases bounded at **3×** the current cap (phishing guard); decreases floored strictly above the current cycle's spend.
- **Overflow-checked everywhere:** `checked_*` arithmetic with `u128` intermediates for fee/refund math.
- **Delegate-based, non-custodial transfers:** funds move via the SPL delegate the subscriber approved; cancellation revokes the registry slice and re-derives the effective delegate.

### Cryptographic Attestations
- The rotation **atomic-settle** requires an `Ed25519Program` precompile (runtime-verified) over `hash(qbf:x402-att:v1 || uuid || first_nonce || last_nonce || amount || merkle_root)`, signed by the mode's authority. `ed25519.rs` enforces `count == 1` and that the pubkey/sig/message all live in the precompile's own data (`instruction_index == 0xFFFF`), preventing an attacker from pointing the precompile at attacker-controlled instruction data.

## 📊 Constants

```rust
pub const FEE_DENOMINATOR: u16 = 10000;

// Production minimum frequency: 1 hour. NOTE: temporarily lowered to 5 in the repo for the
// integration suite (tests/test.ts uses 5-second cycles) — restore to 3600 before deploy.
pub const MIN_FREQUENCY: u32 = 3600; // 1 hour

pub const MIN_FEE_FOR_CONTRACT_BPS: u16 = 50;    // 0.50% floor on the platform leg
pub const MAX_FEE_PLATFORM_BPS: u16 = 1500;      // 15%
pub const MAX_REFERRAL_FEE_BPS: u16 = 5000;      // referrer ≤ 50% OF the platform fee
pub const MAX_FEE_ORGANIZATION_BPS: u16 = 5000;  // org ≤ 50% of the post-platform remainder

pub const MAX_COMPUTE_COST_LAMPORTS: u64 = 100_000_000; // 0.1 SOL — hard gas-refund ceiling
pub const MAX_INCREASE_FACTOR_BPS: u16 = 30_000;        // 3× per-call cap-raise limit

pub const SUBSCRIPTION_HASH_DOMAIN: &[u8] = b"qbf:sub:v2";
pub const BATCH_ATTESTATION_HASH_DOMAIN: &[u8] = b"qbf:x402-att:v1";

pub const AUTHORITY_PDA_SEED: &[u8] = b"authority";
pub const SUBSCRIPTION_PDA_SEED: &[u8] = b"subscription";
pub const PERMIT_REGISTRY_PDA_SEED: &[u8] = b"permit_registry";
```

## 📡 Events

| Event | Emitted by |
|-------|------------|
| `PaymentProcessed { uuid, from, to, mint: Option<Pubkey>, amounts, refund_amount }` | one-time payments (`mint = None` for native SOL) |
| `SubscriptionCreated { uuid, subscriber, merchant_token_account, mint, next_payment_due, initial_allowance, max_amount_per_cycle, frequency, fee_bps, organization_fee_bps, referral_fee_bps, referral_deadline, mode, session_signer }` | `create_subscription` |
| `SubscriptionPaymentProcessed { uuid, subscriber, mint, next_payment_due, remaining_allowance, amounts, refund_amount }` | `execute_subscription` |
| `X402BatchSettled { uuid, subscriber, mint, merkle_root, first_nonce, last_nonce, amounts, refund_amount, next_payment_due, remaining_allowance }` | `settle_x402_batch` / X402 atomic-settle |
| `MerchantX402BatchSettled { … same shape … }` | `settle_merchant_x402_batch` / MerchantX402 atomic-settle |
| `SessionSignerRotated { uuid, old_session_signer, new_session_signer, settled_in_same_tx }` | rotation |
| `SubscriptionCancelled { uuid }` | user cancel / PAYG final bill |
| `SubscriptionStopped { uuid }` | `cancel_pay_as_you_go_subscription` |
| `SubscriptionForceCancelled { uuid }` | `force_cancel_subscription` |
| `AllowanceExtended { uuid, new_amount }` | `extend_allowance` |
| `MaxAmountPerCycleUpdated { uuid, new_max_amount_per_cycle }` | `update_max_amount_per_cycle` |
| `GasRefundProcessed { uuid, amount }` | every successful token gas refund |
| `OwnerUpdated { new_owner }` / `CosignerUpdated { new_cosigner }` | authority rotations |

`Amounts { platform_fee_amount, org_fee_amount, referral_fee_amount, merchant_amount }` — the four legs sum to the gross billed amount.

## ⚠️ Error Codes

```
ZeroAmount                       Amount must be greater than zero
InvalidFeePercentage             Platform fee exceeds maximum
PaymentNotDueYet                 Regular billing before next_payment_due
InvalidFrequency                 Frequency below MIN_FREQUENCY
InsufficientAllowance            Per-sub or registry headroom exceeded
Unauthorized                     Signer not authorized
Overflow                         Arithmetic overflow
InvalidSubscriptionParameters    Subscription digest mismatch
CannotCancelActiveSubscription   Regular cancel after due date
MaxAmountExceeded                Amount/refund exceeds per-cycle cap
InvalidAmount                    Amount/allowance/max validation failed
MaxAmountInvalid                 New max below current-cycle spend
RefundExceedsCap                 Claimed compute cost > MAX_COMPUTE_COST_LAMPORTS
InvalidArgument                  Zero / default pubkey / equal-to-existing
SubscriptionStopped              Stopped sub pending final bill / settled
UnsupportedTokenExtension        Mint uses transfer-fee or confidential-transfer
MaxIncreaseExceeded              Cap raise beyond the 3× per-call factor
InvalidOrganizationFeePercentage Organization fee exceeds maximum
InvalidReferralFeePercentage     Referral fee exceeds maximum
InvalidReferralDeadline          Referral deadline in the past / nonzero with no referrer
DuplicateFeeRecipient            Owner / org / referrer not distinct
MissingReferrerAccount           Referrer required when referral_fee_bps > 0
MissingOrganizationAccount       Organization required when organization_fee_bps > 0
WrongSettleMode                  Settle/cancel called on the wrong mode
SettleTooEarly                   PAYG settle before next_payment_due
InvalidNonceRange                Nonce range below next_nonce / inverted
PayloadExpired                   Receipt or report expiry passed
UserAttestationInvalid           x402 batch attestation signature invalid
MerchantSignatureInvalid         Merchant report signature invalid
AmountMismatch                   Asserted amount ≠ signed payload
MissingEd25519Instruction        Required Ed25519 precompile not found
InvalidEd25519Instruction        Precompile contents don't match expected signer/message
InvalidComputeRefundParams       token_price or compute_cost is zero
MissingSettleAccount             Atomic-settle payload missing required accounts
NoChange                         New value equals current (no-op)
```

## 💡 Compute Refund Mechanism

QBitFlow lets users pay entirely in SPL tokens — the operator fronts SOL for fees and is reimbursed in tokens within the same transaction.

**Conversion** (ceiling division, `u128`, overflow-checked):
```
compute_cost_in_tokens = ceil(compute_cost_in_lamports × token_price_in_lamports / 1e9)
```
- `token_price_in_lamports` is **lamports per smallest token unit** — the backend pre-scales by the mint's decimals; the program does not read `mint.decimals` for this conversion.

**Safety:**
- **Hard ceiling:** `compute_cost_in_lamports > MAX_COMPUTE_COST_LAMPORTS` (0.1 SOL) reverts with `RefundExceedsCap`. No owner knob.
- **Headroom cap:** for subscriptions, the refund can't exceed `max_amount_per_cycle − amount` (cumulative on the rotation atomic-settle path).
- **Non-zero params:** zero `token_price` or `compute_cost` reverts with `InvalidComputeRefundParams`.
- **Hard-revert vs best-effort:** settle paths revert on refund failure (the operator must be made whole); user-facing edit paths (`extend_allowance`, `update_max_amount_per_cycle`) swallow refund failures so the user is never locked out of editing.

## 🔁 EVM Parity & Design Decisions

QBitFlow ships parallel **EVM** (`contracts/eth`, Solidity) and **Solana** (`contracts/sol`, this program) implementations kept at feature / security / flow parity. **The EVM contract is the reference; the Solana program follows.** Some differences are intentional consequences of each chain's mechanics and should **not** be "fixed" toward the other chain:

| Concern | EVM | Solana |
|---------|-----|--------|
| Consent | EIP-712 signatures submitted by the operator | **Live tx signers** — subscriber/payer signs the real transaction |
| Allowance isolation | per-sub CREATE2 proxies + Permit2 | `PermitRegistry` per `(subscriber, mint)` + SPL delegate |
| Gas price | reads `tx.gasprice` | no on-chain gas price → backend supplies `ComputeRefundData` capped by `MAX_COMPUTE_COST_LAMPORTS` |
| `max_amount` in digest | committed (subscriber signs only a param subset) | **omitted** (subscriber signs the whole tx; cap is a trusted on-chain field) |
| One-time token payment | two paths: `processX402PaymentExact` (EIP-3009, x402-SDK-compatible, **trusted-merchant**) and `processTokenPayment` (Permit2 witness, trustless) | **one path**: `process_token_payment` with the payer as a live `Signer` (trustless by construction) |

**Why no x402 one-shot on Solana?** The canonical x402 SVM "exact" scheme forbids arbitrary program invocations in the client-signed transaction (it must be a bare `TransferChecked` sequence), so it cannot carry QBitFlow's on-chain fee split + token gas reimbursement. It isn't needed: a Solana client always signs the real QBitFlow transaction, so `process_token_payment` is already trustless. The old `process_x402_payment_exact` was removed.

**Parity behaviors mirrored on both chains:**
- `cancel_subscription` rejects PAYG; `cancel_pay_as_you_go_subscription` rejects Regular (mode guards).
- Batch settles carry **no** attestation (parity with EVM); the attestation lives only on the rotation atomic-settle path.
- Rotation atomic-settle advances `next_payment_due = now + frequency` and uses a cumulative cap (closes a ~2× cap leak across a boundary).
- `Regular` subscriptions bill cycle 1 **atomically** at creation, so `next_payment_due = now + frequency` right after `create_subscription`.

## 📚 Additional Resources

- **Anchor:** https://www.anchor-lang.com/
- **Solana:** https://docs.solana.com/
- **SPL Token / Token-2022:** https://spl.solana.com/token
- **QBitFlow:** [qbitflow.app](https://qbitflow.app) · [docs](https://qbitflow.app/docs)

## 📄 License

Licensed under the Mozilla Public License 2.0 — see [LICENSE](LICENSE).

## 🔒 Security

For security concerns, email _security@qbitflow.app_. Do not open public issues for vulnerabilities.

## 📞 Support

- **Website:** [qbitflow.app](https://qbitflow.app)
- **Documentation:** [qbitflow.app/docs](https://qbitflow.app/docs)
- **Email:** support@qbitflow.app
