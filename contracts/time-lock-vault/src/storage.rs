use soroban_sdk::{Address, Env, Vec};

use crate::types::{VaultEntry, VaultKey};

// ----------------------------------------------------------------
//  Persistent storage TTL constants
// ----------------------------------------------------------------
// Soroban persistent storage requires explicit TTL (time-to-live)
// bump calls to keep entries alive beyond the default ledger window.

/// Minimum ledger TTL threshold before we bump (≈ 30 days at 5s/ledger).
pub const BUMP_THRESHOLD: u32 = 518_400;

/// Target TTL after a bump (≈ 1 year at 5s/ledger).
pub const BUMP_TARGET: u32 = 6_307_200;

// ----------------------------------------------------------------
//  Deposit helpers
// ----------------------------------------------------------------

/// Persist a new vault entry for `depositor`.
pub fn set_deposit(env: &Env, depositor: &Address, entry: &VaultEntry) {
    let key = VaultKey::Deposit(depositor.clone());
    env.storage().persistent().set(&key, entry);
    env.storage()
        .persistent()
        .extend_ttl(&key, BUMP_THRESHOLD, BUMP_TARGET);
}

/// Retrieve the vault entry for `depositor` — bumps TTL (use for writes/mutations).
pub fn get_deposit(env: &Env, depositor: &Address) -> Option<VaultEntry> {
    let key = VaultKey::Deposit(depositor.clone());
    let entry: Option<VaultEntry> = env.storage().persistent().get(&key);
    if entry.is_some() {
        // Refresh TTL so active vaults never expire during state-changing calls.
        env.storage()
            .persistent()
            .extend_ttl(&key, BUMP_THRESHOLD, BUMP_TARGET);
    }
    entry
}

/// Retrieve the vault entry for `depositor` — does NOT bump TTL.
/// Use this in read-only / view functions to avoid charging callers
/// for unnecessary storage write operations.
pub fn get_deposit_readonly(env: &Env, depositor: &Address) -> Option<VaultEntry> {
    let key = VaultKey::Deposit(depositor.clone());
    env.storage().persistent().get(&key)
}

/// Remove the vault entry for `depositor` after a successful withdrawal.
pub fn remove_deposit(env: &Env, depositor: &Address) {
    let key = VaultKey::Deposit(depositor.clone());
    env.storage().persistent().remove(&key);
}

/// Returns `true` if a deposit record exists for `depositor`.
pub fn has_deposit(env: &Env, depositor: &Address) -> bool {
    let key = VaultKey::Deposit(depositor.clone());
    env.storage().persistent().has(&key)
}

// ----------------------------------------------------------------
//  Depositor index helpers (for paginated queries)
// ----------------------------------------------------------------

/// Load the full depositor index. Returns an empty Vec if not yet set.
pub fn get_depositor_index(env: &Env) -> Vec<Address> {
    env.storage()
        .persistent()
        .get(&VaultKey::DepositorIndex)
        .unwrap_or_else(|| Vec::new(env))
}

/// Append `depositor` to the index (called on first deposit).
pub fn add_to_depositor_index(env: &Env, depositor: &Address) {
    let mut list = get_depositor_index(env);
    list.push_back(depositor.clone());
    env.storage()
        .persistent()
        .set(&VaultKey::DepositorIndex, &list);
    env.storage()
        .persistent()
        .extend_ttl(&VaultKey::DepositorIndex, BUMP_THRESHOLD, BUMP_TARGET);
}

/// Remove `depositor` from the index (called on withdrawal/emergency_withdraw).
pub fn remove_from_depositor_index(env: &Env, depositor: &Address) {
    let list = get_depositor_index(env);
    let mut new_list: Vec<Address> = Vec::new(env);
    for addr in list.iter() {
        if &addr != depositor {
            new_list.push_back(addr);
        }
    }
    env.storage()
        .persistent()
        .set(&VaultKey::DepositorIndex, &new_list);
    env.storage()
        .persistent()
        .extend_ttl(&VaultKey::DepositorIndex, BUMP_THRESHOLD, BUMP_TARGET);
}

// ----------------------------------------------------------------
//  Admin helpers
// ----------------------------------------------------------------

/// Store the admin address (called once during initialization).
pub fn set_admin(env: &Env, admin: &Address) {
    env.storage().persistent().set(&VaultKey::Admin, admin);
    env.storage()
        .persistent()
        .extend_ttl(&VaultKey::Admin, BUMP_THRESHOLD, BUMP_TARGET);
}

/// Retrieve the admin address.
pub fn get_admin(env: &Env) -> Option<Address> {
    env.storage().persistent().get(&VaultKey::Admin)
}

/// Store a pending admin address for two-step transfer.
pub fn set_pending_admin(env: &Env, pending: &Address) {
    env.storage()
        .persistent()
        .set(&VaultKey::PendingAdmin, pending);
    env.storage()
        .persistent()
        .extend_ttl(&VaultKey::PendingAdmin, BUMP_THRESHOLD, BUMP_TARGET);
}

/// Retrieve the pending admin address.
pub fn get_pending_admin(env: &Env) -> Option<Address> {
    env.storage().persistent().get(&VaultKey::PendingAdmin)
}

/// Remove the pending admin entry (after acceptance or cancellation).
pub fn remove_pending_admin(env: &Env) {
    env.storage()
        .persistent()
        .remove(&VaultKey::PendingAdmin);
}
