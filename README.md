# 🔒 Decentralized Time-Lock Vault

[![Rust](https://img.shields.io/badge/Rust-1.81%2B-orange?logo=rust)](https://www.rust-lang.org/)
[![Soroban SDK](https://img.shields.io/badge/Soroban-SDK%20v22-blue?logo=stellar)](https://github.com/stellar/rs-soroban-sdk)
[![License](https://img.shields.io/badge/License-MIT-green)](./LICENSE)
[![Tests](https://github.com/kenedybok3/Decentralized-Time-Lock-Vault/actions/workflows/ci.yml/badge.svg)](https://github.com/kenedybok3/Decentralized-Time-Lock-Vault/actions)

A production-ready Soroban smart contract on the Stellar blockchain that locks XLM or any Stellar asset until a future timestamp or ledger sequence number is reached.

**Table of Contents**
- [Overview](#overview)
- [How It Works](#how-it-works)
- [Ledger vs Timestamp Deposits](#ledger-vs-timestamp-deposits)
- [Pause Semantics](#pause-semantics)
- [Architecture](#architecture)
- [Contract API](#contract-api)
- [Security Properties](#security-properties)
- [Getting Started](#getting-started)
- [Local Standalone Node Integration Testing](#local-standalone-node-integration-testing)
- [Deployment Checklist](#deployment-checklist)
- [Known Limitations](#known-limitations)

---

## Overview

| Property | Value |
|---|---|
| Network | Stellar (Soroban) |
| Language | Rust |
| SDK | soroban-sdk v22 |
| Storage | Persistent (per-depositor, per-deposit-id) |
| Max deposit | 10^15 stroops (100,000,000 XLM) |
| Max lock duration | 5 years |
| Min lock duration | 60 seconds |
| Max batch size | 20 depositors per batch call |

---

## How It Works

The deposit and withdrawal lifecycle:

1. **Deposit** — A user calls `deposit(depositor, token, amount, unlock_time, penalty_bps)` → tokens transfer from their wallet into the contract. Returns a `deposit_id` (u32) that identifies this specific deposit.
2. **Storage** — The contract stores a `VaultEntry` in **Persistent Storage** keyed by `(depositor, deposit_id)`. Multiple deposits per address are supported.
3. **Verification** — When the user calls `withdraw(depositor, deposit_id)`, the contract checks `env.ledger().timestamp() >= unlock_time` (for timestamp-based deposits) or `env.ledger().sequence() >= unlock_ledger` (for ledger-based deposits).
4. **Unlock** — If the condition is met, tokens are returned. Otherwise the call fails with `FundsStillLocked`.
5. **Early Exit** — A depositor can call `cancel_deposit(depositor, deposit_id)` before the unlock time. A penalty (`penalty_bps` of the amount) goes to the `fee_recipient`; the rest is returned to the depositor.
6. **Admin Recovery** — An admin can perform emergency withdrawals (funds always return to the depositor, never to the admin).
7. **Trustless Mode** — Admin rights can be transferred via a two-step process, or permanently renounced to make the vault fully trustless.

---

## Ledger vs Timestamp Deposits

The contract supports two independent unlock condition modes. Use whichever matches your use case.

### Timestamp-based (`deposit`)

- Unlock condition: `env.ledger().timestamp() >= unlock_time`
- `unlock_time` is a Unix timestamp in seconds.
- Queried via `get_vault`, `time_remaining`.
- **Best for:** human-readable deadlines ("unlock on 2027-01-01"), long-duration locks, UI countdown timers.

```
deposit_id = deposit(depositor, token, amount, unlock_time_unix_secs, penalty_bps)
```

### Ledger-sequence-based (`deposit_by_ledger`)

- Unlock condition: `env.ledger().sequence() >= unlock_ledger`
- `unlock_ledger` is an absolute Stellar ledger sequence number.
- Queried via `get_vault_by_ledger`, `ledgers_remaining`.
- Stellar produces approximately 1 ledger every 5 seconds. Convert from seconds: `unlock_ledger = current_ledger + duration_secs / 5`.
- **Best for:** short, precise locks where ledger-level granularity matters (e.g., "unlock in exactly 720 ledgers / ~1 hour").

```
deposit_id = deposit_by_ledger(depositor, token, amount, unlock_ledger_sequence, penalty_bps)
```

### Key difference

| | Timestamp | Ledger |
|---|---|---|
| Entry type | `VaultEntry` | `LedgerVaultEntry` |
| Unlock field | `unlock_time: u64` (Unix secs) | `unlock_ledger: u32` (sequence number) |
| Query fn | `get_vault`, `time_remaining` | `get_vault_by_ledger`, `ledgers_remaining` |
| Stored under | `VaultKey::Deposit(addr, id)` | `VaultKey::DepositByLedger(addr, id)` |
| `withdraw` | Checks both automatically; tries timestamp-based first, then ledger-based |

`withdraw` and `emergency_withdraw` transparently handle both deposit types — the caller does not need to know which type was used.

---

## Pause Semantics

The admin can pause and unpause the contract at any time.

```
pause(admin)    → ContractPaused flag set to true
unpause(admin)  → ContractPaused flag set to false
is_paused()     → returns current pause state (bool)
```

**What pause affects:**
- `deposit`, `deposit_for`, and `deposit_by_ledger` all fail with `ContractPaused` when the contract is paused. No new funds can be locked.

**What pause does NOT affect:**
- `withdraw`, `withdraw_to`, `cancel_deposit` — existing depositors can always exit, even while paused.
- `emergency_withdraw`, `batch_emergency_withdraw` — admin recovery is always available.
- All read-only queries — `get_vault`, `time_remaining`, `get_admin`, etc.

This design ensures that depositors can never be trapped in the contract by an admin pause.

---

## Architecture

### Deposit / Withdraw Flow

```
Depositor
   │
   ├─► deposit(depositor, token, amount, unlock_time, penalty_bps)
   │       │
   │       ├─ assert not paused
   │       ├─ validate amount, unlock_time, penalty_bps
   │       ├─ deposit_id = next_deposit_id(depositor)   ← per-depositor counter
   │       ├─ token.transfer(depositor → contract)
   │       ├─ storage::set_deposit(VaultKey::Deposit(depositor, id) → VaultEntry)
   │       └─ emit "deposit" event → returns deposit_id
   │
   └─► withdraw(depositor, deposit_id)
           │
           ├─ load VaultEntry (timestamp) OR LedgerVaultEntry (ledger-based)
           ├─ assert now >= unlock_time  (or current_ledger >= unlock_ledger)
           ├─ storage::remove_deposit(depositor, id)    ← state cleared first (CEI)
           ├─ token.transfer(contract → depositor)
           └─ emit "withdraw" event
```

### Storage Layout

```
Persistent Storage
├── VaultKey::Admin                          → Address
│       (set once on initialize; removed on renounce_admin)
│
├── VaultKey::PendingAdmin                   → Address
│       (set by transfer_admin; cleared by accept_admin / cancel_transfer_admin)
│
├── VaultKey::Initialized                    → bool
│       (set once on initialize; never removed)
│
├── VaultKey::FeeRecipient                   → Address
│       (set on initialize; receives penalty_bps on cancel_deposit)
│
├── VaultKey::MaxDeposit                     → i128
│       (set on initialize if overridden; absent → compile-time default 10^15)
│
├── VaultKey::MaxLockSecs                    → u64
│       (set on initialize if overridden; absent → compile-time default 157_788_000)
│
├── VaultKey::Paused                         → bool
│       (toggled by pause/unpause; absent → false)
│
├── VaultKey::DepositCounter(depositor)      → u32
│       (monotonically incremented per depositor; never decremented)
│
├── VaultKey::ActiveDepositIds(depositor)    → Vec<u32>
│       (tracks which deposit_ids are currently active for a depositor)
│
├── VaultKey::Deposit(depositor, deposit_id) → VaultEntry
│       ├── token:       Address   (SEP-41 token contract)
│       ├── amount:      i128      (locked stroops)
│       ├── unlock_time: u64       (Unix seconds)
│       ├── depositor:   Address
│       └── penalty_bps: u32       (0–10000; early-exit penalty rate)
│
├── VaultKey::DepositByLedger(depositor, id) → LedgerVaultEntry
│       ├── token:         Address
│       ├── amount:        i128
│       ├── unlock_ledger: u32     (Stellar ledger sequence number)
│       ├── depositor:     Address
│       └── penalty_bps:   u32
│
├── VaultKey::DepositorMember(depositor)     → bool   (O(1) membership check)
├── VaultKey::DepositorCount                 → u32
└── VaultKey::DepositorAt(slot)              → Address (indexed depositor list)
```

All entries use TTL bump threshold ≈ 30 days (`BUMP_THRESHOLD = 518_400` ledgers) and target ≈ 5.2 years (`BUMP_TARGET` derived from `MAX_LOCK_DURATION_SECS / LEDGER_SECONDS`), ensuring a max-duration deposit cannot expire before its unlock time.

---

## Project Structure

```
.
├── Cargo.toml                          # Workspace manifest
├── Makefile                            # Build / test / lint / deploy helpers
├── rust-toolchain.toml                 # Pins stable Rust + wasm32 target
├── .cargo/
│   └── config.toml                     # Documents --target trade-off
├── .gitignore
├── README.md
├── .github/
│   └── workflows/
│       └── ci.yml                      # CI: lint → test → build WASM
├── scripts/
│   ├── deploy_testnet.sh               # Automated testnet deploy + smoke test
│   └── smoke_test_local.sh             # End-to-end test against local Soroban node
└── contracts/time-lock-vault/
    ├── Cargo.toml
    └── src/
        ├── lib.rs          # Crate root & module declarations
        ├── contract.rs     # All public entry points
        ├── constants.rs    # MAX_DEPOSIT_AMOUNT, MAX_LOCK_DURATION_SECS, etc.
        ├── types.rs        # VaultKey, VaultEntry, LedgerVaultEntry, WithdrawResult
        ├── errors.rs       # VaultError enum (14 typed codes)
        ├── events.rs       # Event emission helpers
        ├── storage.rs      # Persistent storage helpers + TTL bump logic
        └── test.rs         # Full unit test suite (48+ tests)
```

---

## Contract API

### 🔧 Initialization

#### `initialize(admin, fee_recipient, max_deposit, max_lock_secs) → Result<(), VaultError>`

Sets the admin and fee recipient addresses. Optionally overrides the compile-time limits for this deployment. Must be called once after deployment.

| Param | Type | Description |
|---|---|---|
| `admin` | `Address` | Must sign. Becomes the contract admin. |
| `fee_recipient` | `Address` | Receives `penalty_bps` portion on `cancel_deposit`. |
| `max_deposit` | `Option<i128>` | Override max deposit in stroops. `None` → default `1_000_000_000_000_000`. |
| `max_lock_secs` | `Option<u64>` | Override max lock in seconds. `None` → default `157_788_000` (~5 years). |

Fails with `Unauthorized` if already initialized.

---

### 💰 Core Functions

#### `deposit(depositor, token, amount, unlock_time, penalty_bps) → Result<u32, VaultError>`

Locks `amount` of `token` until `unlock_time` (Unix seconds). Returns the `deposit_id`.

| Param | Type | Constraint |
|---|---|---|
| `depositor` | `Address` | Must sign |
| `token` | `Address` | SEP-41 token contract |
| `amount` | `i128` | `0 < amount ≤ max_deposit` |
| `unlock_time` | `u64` | `now + 60s ≤ unlock_time ≤ now + max_lock_secs` |
| `penalty_bps` | `u32` | `0–10000` (basis points; 100 bps = 1%) |

Fails with `ContractPaused` if the contract is paused.

#### `deposit_for(payer, depositor, token, amount, unlock_time, penalty_bps) → Result<u32, VaultError>`

Same as `deposit` but `payer` (not `depositor`) signs and funds the transfer. The vault is owned by `depositor` who is the sole authorised recipient on withdrawal.

#### `deposit_by_ledger(depositor, token, amount, unlock_ledger, penalty_bps) → Result<u32, VaultError>`

Locks `amount` until a specific ledger sequence number is reached. Returns the `deposit_id`. See [Ledger vs Timestamp Deposits](#ledger-vs-timestamp-deposits).

| Param | Type | Constraint |
|---|---|---|
| `unlock_ledger` | `u32` | Must be in the future and within `max_lock_secs / 5` ledgers from now |

Fails with `ContractPaused` if the contract is paused.

#### `cancel_deposit(depositor, deposit_id) → Result<(), VaultError>`

Cancels an active deposit **before** the unlock time. The penalty (`penalty_bps` of the amount) is sent to the `fee_recipient`; the remainder is returned to the depositor.

- Fails with `FundsAlreadyUnlocked` if the vault is already past its unlock time — use `withdraw` instead.
- Fails with `NoDepositFound` if no deposit exists for `(depositor, deposit_id)`.

#### `withdraw(depositor, deposit_id) → Result<(), VaultError>`

Withdraws funds after the unlock time/ledger has passed. Tries timestamp-based deposits first, then ledger-based. Fails with `FundsStillLocked` if the lock has not expired. No penalty.

#### `withdraw_to(depositor, deposit_id, recipient) → Result<(), VaultError>`

Same as `withdraw` but sends funds to `recipient` instead of `depositor`. The `depositor` must sign.

---

### 👨‍⚖️ Admin Functions

#### `pause(admin) → Result<(), VaultError>`

Pauses new deposits. See [Pause Semantics](#pause-semantics). Only the current admin can call this.

#### `unpause(admin) → Result<(), VaultError>`

Re-enables new deposits. Only the current admin can call this.

#### `emergency_withdraw(admin, depositor, deposit_id) → Result<(), VaultError>`

Admin-only. Returns funds to the depositor regardless of lock time. Funds always go to the depositor — never to the admin. Handles both timestamp-based and ledger-based deposits.

#### `batch_emergency_withdraw(admin, depositors) → Result<Vec<WithdrawResult>, VaultError>`

Admin-only. Processes emergency withdrawals for multiple `(depositor, deposit_id)` pairs in a single transaction — useful for contract migrations.

| Param | Type | Description |
|---|---|---|
| `admin` | `Address` | Must be the current admin. Signs **once** for the entire batch. |
| `depositors` | `Vec<(Address, u32)>` | Pairs of `(depositor_address, deposit_id)`. Max `MAX_BATCH_SIZE` (20) entries. |

**Best-effort**: pairs with no active deposit are skipped and recorded as `success: false` — the call never aborts due to a missing deposit, so all valid entries are always processed.

**Returns** `Vec<WithdrawResult>` — one entry per input pair:

| Field | Type | Meaning |
|---|---|---|
| `depositor` | `Address` | The input address |
| `deposit_id` | `u32` | The input deposit ID |
| `success` | `bool` | `true` = funds transferred; `false` = no deposit found, skipped |

**Instruction budget**: Each iteration costs roughly 1–2M instructions (two storage removes, one token transfer, one event). The hard cap of 20 keeps the batch well within Soroban's ~100M instruction limit. For larger sets, page through depositors with `get_depositors(offset, limit)` and call this function multiple times.

#### `transfer_admin(admin, new_admin) → Result<(), VaultError>`

Step 1 of a two-step admin transfer. Nominates `new_admin` as pending admin.

#### `accept_admin(new_admin) → Result<(), VaultError>`

Step 2. The pending admin accepts and becomes the active admin.

#### `cancel_transfer_admin(admin) → Result<(), VaultError>`

Cancels a pending admin transfer. Only the current admin can cancel.

#### `renounce_admin(admin) → Result<(), VaultError>`

Permanently removes admin privileges. After this call, `emergency_withdraw`, `pause`, and all other admin functions are disabled forever. Makes the vault fully trustless.

---

### 📖 Read-only Queries

All read-only functions skip TTL bumps to avoid charging callers extra storage fees.

#### `get_vault(depositor, deposit_id) → Option<VaultEntry>`

Returns the timestamp-based vault entry, or `None` if it doesn't exist.

#### `get_vault_by_ledger(depositor, deposit_id) → Option<LedgerVaultEntry>`

Returns the ledger-sequence-based vault entry, or `None` if it doesn't exist.

#### `get_vault_batch(depositors, deposit_id) → Vec<Option<VaultEntry>>`

Batch query for a single `deposit_id` across multiple depositors. Returns one `Option<VaultEntry>` per input address.

#### `get_deposit_ids(depositor) → Vec<u32>`

Returns all active deposit IDs for a depositor (both timestamp-based and ledger-based).

#### `time_remaining(depositor, deposit_id) → u64`

Returns seconds until the timestamp-based deposit unlocks. Returns `0` if unlocked or no deposit exists.

#### `ledgers_remaining(depositor, deposit_id) → u32`

Returns ledgers until the ledger-based deposit unlocks. Returns `0` if unlocked or no deposit exists.

#### `get_time() → u64`

Returns the current ledger Unix timestamp.

#### `get_admin() → Option<Address>`

Returns the current admin, or `None` if renounced.

#### `get_pending_admin() → Option<Address>`

Returns the pending admin during a transfer, or `None`.

#### `get_fee_recipient() → Option<Address>`

Returns the fee recipient address set at initialization.

#### `get_constants() → (i128, u64)`

Returns the effective `(max_deposit_stroops, max_lock_secs)` for this deployment — runtime-configured values if set at `initialize`, otherwise the compile-time defaults.

#### `get_depositor_count() → u32`

Returns the total number of addresses with at least one active deposit.

#### `get_depositors(offset, limit) → Vec<Address>`

Returns a paginated slice of active depositor addresses. Max page size is 100.

| Param | Type | Description |
|---|---|---|
| `offset` | `u32` | Zero-based start index |
| `limit` | `u32` | Maximum number of addresses to return (capped at 100) |

#### `is_paused() → bool`

Returns `true` if the contract is currently paused.

#### `is_initialized() → bool`

Returns `true` if `initialize` has been called.

---

## 📋 Events

All events are emitted via `env.events().publish(topics, data)`.

| Event | Topics | Data |
|---|---|---|
| `deposit` | `("deposit", depositor, token)` | `(deposit_id, amount, unlock_time)` |
| `withdraw` | `("withdraw", depositor, token)` | `(deposit_id, amount)` |
| `withdraw_to` | `("withdraw_to", depositor, recipient, token)` | `(deposit_id, amount)` |
| `emrg_wdraw` | `("emrg_wdraw", depositor)` | `(deposit_id, admin, token, amount)` |
| `dep_cancel` | `("dep_cancel", depositor, token)` | `(amount, penalty)` |
| `paused` | `("paused", admin)` | `()` |
| `unpaused` | `("unpaused", admin)` | `()` |
| `adm_xfr_init` | `("adm_xfr_init", current_admin)` | `pending_admin` |
| `adm_xfr_done` | `("adm_xfr_done", new_admin)` | `()` |
| `adm_xfr_cancel` | `("adm_xfr_cancel", admin)` | `pending_admin` |
| `adm_renounce` | `("adm_renounce", former_admin)` | `()` |

All `amount` and `penalty` values are `i128` token units (stroops for XLM). `deposit_id` is a `u32` per-depositor monotonic counter starting at 0.

---

## 🗄️ Storage Layout

All entries use **Persistent Storage** with TTL bump threshold ≈ 30 days (`BUMP_THRESHOLD = 518_400` ledgers) and target derived from `MAX_LOCK_DURATION_SECS / LEDGER_SECONDS` (≈ 5.2 years), ensuring a max-duration deposit cannot expire before its unlock time.

| Key | Value Type | Lifetime |
|---|---|---|
| `VaultKey::Admin` | `Address` | Set on `initialize`; removed on `renounce_admin` |
| `VaultKey::PendingAdmin` | `Address` | Set by `transfer_admin`; cleared by `accept_admin` / `cancel_transfer_admin` |
| `VaultKey::Initialized` | `bool` | Set once on `initialize`; never removed |
| `VaultKey::FeeRecipient` | `Address` | Set on `initialize`; never removed |
| `VaultKey::Paused` | `bool` | Toggled by `pause`/`unpause`; absent → false |
| `VaultKey::MaxDeposit` | `i128` | Set on `initialize` if overridden; absent → compile-time default |
| `VaultKey::MaxLockSecs` | `u64` | Set on `initialize` if overridden; absent → compile-time default |
| `VaultKey::DepositCounter(depositor)` | `u32` | Incremented on each deposit; never decremented |
| `VaultKey::ActiveDepositIds(depositor)` | `Vec<u32>` | Updated on deposit and removal; absent → empty |
| `VaultKey::Deposit(depositor, id)` | `VaultEntry` | Created on `deposit`; removed on `withdraw` / `emergency_withdraw` / `cancel_deposit` |
| `VaultKey::DepositByLedger(depositor, id)` | `LedgerVaultEntry` | Created on `deposit_by_ledger`; removed on `withdraw` / `emergency_withdraw` |
| `VaultKey::DepositorMember(depositor)` | `bool` | Set when first deposit is made; removed when all deposits are cleared |
| `VaultKey::DepositorCount` | `u32` | Incremented/decremented as depositors enter/exit |
| `VaultKey::DepositorAt(slot)` | `Address` | Indexed depositor slot; maintained via O(1) swap-remove |

TTL is bumped on every **write**. Read-only query functions skip the TTL bump to avoid charging callers extra fees.

---

## ❌ Error Codes

| Code | Name | Meaning |
|---|---|---|
| 1 | `InvalidAmount` | `amount ≤ 0` |
| 2 | `UnlockTimeNotInFuture` | `unlock_time`/`unlock_ledger` is in the past or present |
| 3 | `NoDepositFound` | No active deposit for this `(depositor, deposit_id)` |
| 4 | `FundsStillLocked` | Lock period not yet expired |
| 5 | `DepositAlreadyExists` | Deposit ID collision (should not occur under normal usage) |
| 6 | `LockDurationTooLong` | Lock period exceeds `max_lock_secs` |
| 7 | `Unauthorized` | Caller is not the admin, or contract already initialized |
| 8 | `AmountTooLarge` | Amount exceeds `max_deposit` |
| 9 | `InvalidPenaltyBps` | `penalty_bps > 10000` |
| 10 | `InvalidAdmin` | Nominated admin is the same as the current admin |
| 11 | `LockDurationTooShort` | Lock period is shorter than the minimum (60 s) |
| 12 | `ContractPaused` | Contract is paused; new deposits are rejected |
| 13 | `FundsAlreadyUnlocked` | `cancel_deposit` called after unlock time; use `withdraw` instead |
| 14 | `BatchTooLarge` | `depositors.len()` exceeds `MAX_BATCH_SIZE` (20) |

---

## 🔐 Security Properties

| Property | Implementation |
|---|---|
| Checks-Effects-Interactions | Storage cleared before token transfer on every withdrawal |
| Auth-first ordering | `require_auth()` is always the first statement in every mutating function |
| No re-entrancy surface | State removed before any external token call |
| Bounded inputs | Amount capped at `max_deposit`; lock duration capped at `max_lock_secs` |
| No admin fund theft | Emergency withdraw always sends to depositor, never to admin |
| Trustless mode | Admin can permanently renounce via `renounce_admin()` |
| Safe admin transfer | Two-step transfer prevents accidental key loss |
| TTL management | Persistent entries bumped to ~5.2 years on every write; view functions skip TTL bump |
| Pause safety | Pause only blocks new deposits; existing depositors can always exit |
| No testutils in production | `features = ["testutils"]` only in `[dev-dependencies]` |
| Initialize front-running | `initialize()` fails if already initialized, but an attacker who observes the deploy transaction can call `initialize` first. **Mitigation:** always call `initialize` in the same transaction as `deploy` (atomic deploy+init). The deploy script does this by default. |

---

## 🔄 Upgradeability

Soroban contracts are **immutable by default** — once deployed, the contract code cannot be changed or patched.

| Implication | Detail |
|---|---|
| No in-place upgrades | There is no `upgrade` or `set_code` function; the deployed WASM is fixed forever |
| Bug fixes require redeployment | A new contract must be deployed and users must migrate their funds to it |
| Migration path | The admin can call `batch_emergency_withdraw` to return funds to depositors in batches of 20, who can then re-deposit into the new contract |
| Trustless trade-off | If `renounce_admin()` has been called, no migration is possible — the contract is fully trustless but also fully immutable with no escape hatch |

Plan deployments carefully. Audit the contract before going to mainnet, because there is no way to patch a live deployment.

---

## 🚀 Getting Started

### 📋 Prerequisites

```bash
# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Add WASM target
rustup target add wasm32-unknown-unknown

# Install Soroban CLI (also installs the stellar CLI)
cargo install --locked soroban-cli

# Install cargo-watch (optional, for make watch)
cargo install cargo-watch
```

### 🔨 Build

```bash
make build
```

> **Why not just `cargo build`?**
> Running `cargo build` without `--target wasm32-unknown-unknown` produces a native binary, not a WASM contract. The Makefile's `build` target always passes the correct flag. A `.cargo/config.toml` is included in the repo that documents this trade-off — the default target is intentionally left commented out because setting it would break `cargo test` (tests must run natively to use Soroban testutils).

### ✅ Test

```bash
make test
```

> Tests run natively (no `--target` flag) so that `soroban-sdk`'s `testutils` feature works. Never run `cargo test --target wasm32-unknown-unknown`.

### 🔍 Full CI check (fmt + lint + test + audit + deny)

```bash
make check
```

### 🛡️ Security audit

```bash
make audit
```

Runs `cargo audit` to check all dependencies against the [RustSec Advisory Database](https://rustsec.org/).

### 📦 License & dependency policy

```bash
make deny
```

Runs `cargo deny check` to enforce license allowlists and ban policies defined in `deny.toml`.

### ⚡ Optimize WASM

```bash
make optimize
```

### 📊 Check WASM size

```bash
make check-wasm-size
```

Fails if the optimized WASM exceeds `MAX_WASM_BYTES` (default **65 536 bytes / 64 KB**).
Override the threshold at the command line:

```bash
make check-wasm-size MAX_WASM_BYTES=81920   # 80 KB
```

The same threshold is enforced in CI via the `Check WASM size` step in `.github/workflows/ci.yml`.
To update the limit, change `MAX_WASM_BYTES` in both places (or only in `ci.yml` if you don't use the Makefile target locally).

### 🌐 Deploy to Testnet

```bash
export SOROBAN_SECRET_KEY=S...
make deploy-testnet
```

See [scripts/deploy_testnet.sh](./scripts/deploy_testnet.sh) for the full list of optional env var overrides (`FEE_RECIPIENT`, `MAX_DEPOSIT`, `MAX_LOCK_SECS`, etc.) and for inline usage examples of every contract entry point printed after a successful deployment.

### 🎯 Release Deployment (CI)

Pushing a version tag triggers the `deploy-testnet` CI job automatically:

```bash
git tag v1.0.0
git push origin v1.0.0
```

The job requires the `SOROBAN_SECRET_KEY` secret to be set in the repository's **testnet** environment (`Settings → Environments → testnet → Secrets`). After the run, the deployed contract ID appears in the job's summary tab.

---

## 🧪 Local Standalone Node Integration Testing

Run a full end-to-end integration test against a local Soroban standalone node — no funded testnet account or internet access required.

### Prerequisites

```bash
# 1. Install the Stellar CLI (includes soroban-cli and the local node runner)
#    Option A — via cargo:
cargo install --locked stellar-cli

#    Option B — download a pre-built binary from GitHub Releases:
#    https://github.com/stellar/stellar-cli/releases
#    Then add it to your PATH.

# 2. Verify the CLI is available:
stellar --version    # should print stellar 22.x.x or later

# 3. Build the contract WASM (required before running the smoke test):
make build
```

> **Docker note:** `stellar network start local` launches a containerised Soroban node. Docker Desktop (or Docker Engine on Linux) must be running before executing the smoke test. The Stellar CLI pulls the `stellar/quickstart` image automatically on the first run; subsequent runs use the cached image.

### Running the smoke test

```bash
# Build + run in one step (recommended):
make smoke-test-local

# Or invoke the script directly:
bash scripts/smoke_test_local.sh
```

### What the smoke test does

The script (`scripts/smoke_test_local.sh`) exercises the full deposit → query → withdraw lifecycle:

| Step | Action | What is verified |
|---|---|---|
| 1 | Check WASM exists | Fails fast if `make build` was not run |
| 2 | `stellar network start local --background` | Local node is up and listening |
| 3 | `stellar keys generate --fund` | A funded test identity is created |
| 4 | `stellar contract deploy` | Contract deploys successfully; a contract ID is returned |
| 5 | `initialize(admin, ...)` | Contract accepts the init call without error |
| 6 | `stellar contract asset deploy --asset native` | Native XLM is wrapped as a SEP-41 token |
| 7 | `deposit(depositor, token, 1000, now+120s, penalty_bps=0)` | Deposit returns a `deposit_id`; token balance decreases |
| 8 | `get_vault(depositor, 0)` | Returned entry contains `amount = 1000` |
| 9 | `time_remaining(depositor, 0)` | Returns > 0 (lock has not expired) |
| 10 | `withdraw(depositor, 0)` | Fails with `FundsStillLocked` (asserts error string) |
| EXIT | `stellar network stop local` | Node is shut down cleanly via `trap` |

### Expected output

```
==> Checking WASM...
  ✓ WASM found: target/wasm32-unknown-unknown/release/time_lock_vault.wasm
==> Starting local Soroban node...
  ✓ Local node started
==> Setting up identity...
  ✓ Identity: GABC...XYZ
==> Deploying contract...
  ✓ Contract deployed: CCCC...AAAA
==> Calling initialize...
  ✓ initialize OK
==> Wrapping native XLM...
  ✓ Token: CDDD...BBBB
==> Calling deposit...
  ✓ deposit OK
==> Calling get_vault...
  ✓ get_vault returns amount 1000
==> Calling time_remaining...
  ✓ time_remaining > 0 (119)
==> Calling withdraw (expect FundsStillLocked)...
  ✓ withdraw fails while locked

All smoke tests passed.
==> Stopping local node...
```

### Extending the smoke test

To add assertions for additional contract functions, edit `scripts/smoke_test_local.sh`. The `assert_contains` helper makes it easy:

```bash
# Example: assert that is_paused returns false after initialize
IS_PAUSED=$(stellar contract invoke \
    --id "$CONTRACT_ID" --source "$IDENTITY" --network "$NETWORK" \
    -- is_paused)
assert_contains "is_paused returns false" "false" "$IS_PAUSED"

# Example: assert that get_deposit_ids returns the deposit we just made
DEP_IDS=$(stellar contract invoke \
    --id "$CONTRACT_ID" --source "$IDENTITY" --network "$NETWORK" \
    -- get_deposit_ids --depositor "$ADMIN_ADDR")
assert_contains "get_deposit_ids includes 0" "0" "$DEP_IDS"

# Example: test deposit_by_ledger
CURRENT_LEDGER=$(stellar ledger --network "$NETWORK" | jq '.sequence')
UNLOCK_LEDGER=$(( CURRENT_LEDGER + 100 ))
stellar contract invoke \
    --id "$CONTRACT_ID" --source "$IDENTITY" --network "$NETWORK" \
    -- deposit_by_ledger \
    --depositor "$ADMIN_ADDR" \
    --token "$TOKEN_ID" \
    --amount 500 \
    --unlock_ledger "$UNLOCK_LEDGER" \
    --penalty_bps 0
assert_contains "deposit_by_ledger OK" "" ""   # just checking it doesn't error
```

### Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| `WASM not found. Run 'make build' first.` | WASM not compiled | `make build` |
| `stellar: command not found` | Stellar CLI not installed or not on PATH | `cargo install --locked stellar-cli` |
| `docker: command not found` or node fails to start | Docker not running | Start Docker Desktop / Docker Engine |
| `Error: account not found` when deploying | Friendbot fund step failed | Check internet connectivity (Friendbot is used only on testnet; local node auto-funds) |
| Port conflict on `stellar network start local` | Another process is using the Soroban RPC port | Stop the conflicting process, or stop a leftover node with `stellar network stop local` |
| Tests pass but `get_vault` returns `null` | Deposit call silently failed (e.g. token allowance missing) | Run the script with `bash -x scripts/smoke_test_local.sh` to trace every command |
| `FundsStillLocked` not in withdraw error | CLI version mismatch — error format changed | Update stellar CLI: `cargo install --locked stellar-cli` |

---

## 📝 Updating the Stellar CLI Version

`STELLAR_CLI_VERSION` is defined as a top-level `env` variable in `.github/workflows/ci.yml`. Dependabot keeps GitHub Actions versions up to date automatically, but it does not track arbitrary binary downloads. When a new `stellar-cli` release is published at https://github.com/stellar/stellar-cli/releases, update the variable manually:

```yaml
# .github/workflows/ci.yml
env:
  STELLAR_CLI_VERSION: "<new-version>"
```

## ✈️ Deployment Checklist

Use this checklist when deploying to production.

- [ ] Deploy and call `initialize` in the same transaction to prevent front-running
- [ ] Pass the correct `fee_recipient` address to `initialize` (receives early-exit penalties)
- [ ] Verify `get_admin` returns the expected admin address
- [ ] Run `get_constants` to confirm `max_deposit` and `max_lock_secs` match your intended parameters
- [ ] Verify `get_fee_recipient` returns the correct fee recipient address
- [ ] Run `is_initialized` to confirm the contract initialized successfully
- [ ] Consider calling `renounce_admin` for fully trustless operation once setup is complete
- [ ] Monitor storage TTL for long-duration vaults — entries are bumped on write but not on read
- [ ] Confirm the optimized WASM size is within the Stellar network limit (`make check-wasm-size`)

---

## 💡 Fee Estimation

Soroban charges fees for persistent storage operations. Here is what each call costs at a high level:

| Operation | Storage effect |
|---|---|
| `deposit` / `deposit_by_ledger` | Creates a new persistent entry + pays for initial TTL bump (~30-day threshold, ~5.2-year target) |
| `withdraw` / `cancel_deposit` / `emergency_withdraw` | Removes the persistent entry (storage freed) |
| `get_vault`, `time_remaining`, `get_time` | Read-only — **no TTL bump**, no extra storage fee |
| `initialize` | Writes admin / fee-recipient / initialized entries once |

Key points:
- The depositor pays the storage-creation fee on `deposit`.
- View functions intentionally skip TTL bumps to avoid charging callers for reads.
- For very long locks (approaching 5 years) the TTL is set well beyond the unlock time, so no manual TTL extension is needed.

For current fee rates see the [Stellar fee documentation](https://developers.stellar.org/docs/learn/fundamentals/fees-resource-limits-metering).

---

## ⚠️ Known Limitations

| Limitation | Detail |
|---|---|
| No partial withdrawals | The full locked amount for a given `deposit_id` is returned in one call; partial releases are not supported. |
| No early withdrawal without penalty or admin | Only `cancel_deposit` (with a configurable penalty) or an admin `emergency_withdraw` can release funds before the unlock time. |
| Single admin address | Admin is one key — no multisig or DAO governance. Use `renounce_admin` to go fully trustless. |
| Storage TTL | Persistent entries are bumped to ~5.2 years on every write. Deposits longer than that would require a TTL extension call (current max lock is 5 years, so this is not an issue in practice). |
| Ledger-based deposits and `cancel_deposit` | `cancel_deposit` currently only cancels timestamp-based deposits. Use `emergency_withdraw` (admin only) to recover a ledger-based deposit early. |

---

## 🧬 Testing

### Run all tests

```bash
make test
```

### Run a specific test

```bash
cargo test test_deposit_success --features testutils -- --nocapture
```

### Run all tests with output

```bash
cargo test --features testutils -- --nocapture
```

> Tests run natively (without `--target wasm32-unknown-unknown`) so that `soroban-sdk`'s `testutils` feature works correctly.

### Test categories

The suite (`contracts/time-lock-vault/src/test.rs`) contains 48+ tests covering:

| Category | What is tested |
|---|---|
| Deposit | Valid deposits, duplicate deposits, amount/time boundary checks |
| Deposit by ledger | Ledger-based unlock condition, boundary checks |
| Withdraw | Successful withdrawal, early withdrawal rejection, missing deposit |
| Cancel deposit | Penalty calculation, fee recipient transfer, post-unlock rejection |
| Pause / Unpause | Deposit blocked while paused; withdraw unaffected |
| Admin | `transfer_admin`, `accept_admin`, `cancel_transfer_admin`, `renounce_admin` |
| Emergency withdraw | Admin-only access, funds always go to depositor |
| Read-only queries | `get_vault`, `time_remaining`, `ledgers_remaining`, `get_constants`, pagination |
| Error codes | Every `VaultError` variant is exercised |

---

## Use Cases

- **Savings accounts** — Lock funds for a fixed period to enforce saving discipline.
- **Token vesting** — Team or investor tokens released on a schedule.
- **HODL challenges** — Commit to not selling until a future date.
- **Escrow** — Time-gated release of payment.

---

## Contributing

See [CONTRIBUTING.md](./CONTRIBUTING.md) for contribution guidelines.

See [CHANGELOG.md](./CHANGELOG.md) for the full version history.

---

## License

MIT
