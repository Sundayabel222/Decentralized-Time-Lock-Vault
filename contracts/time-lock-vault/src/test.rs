#![cfg(test)]

extern crate std;

use soroban_sdk::{
    testutils::{Address as _, Events, Ledger, LedgerInfo},
    token::{Client as TokenClient, StellarAssetClient},
    Address, Env, IntoVal, Symbol, symbol_short,
};

use crate::{
    contract::{TimeLockVault, TimeLockVaultClient},
    errors::VaultError,
    types::{VaultEntry, VaultKey, MAX_DEPOSIT_AMOUNT, MAX_LOCK_DURATION_SECS},
};

// ================================================================
//  Test helpers
// ================================================================

/// Returns (env, vault_client, token_address, admin, alice, fee_recipient).
fn setup() -> (Env, TimeLockVaultClient<'static>, Address, Address, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();

    let vault_id = env.register(TimeLockVault, ());
    let vault = TimeLockVaultClient::new(&env, &vault_id);

    let admin: Address = Address::generate(&env);
    let alice: Address = Address::generate(&env);
    let fee_recipient: Address = Address::generate(&env);

    let token_id = env.register_stellar_asset_contract_v2(admin.clone());
    let token_address = token_id.address();

    StellarAssetClient::new(&env, &token_address).mint(&alice, &10_000);

    vault.initialize(&admin, &None, &None);
    vault.set_fee_recipient(&admin, &fee_recipient);

    (env, vault, token_address, admin, alice, fee_recipient)
}

fn advance_time(env: &Env, seconds: u64) {
    env.ledger().set(LedgerInfo {
        timestamp: env.ledger().timestamp() + seconds,
        protocol_version: env.ledger().protocol_version(),
        sequence_number: env.ledger().sequence(),
        network_id: Default::default(),
        base_reserve: 10,
        min_temp_entry_ttl: 16,
        min_persistent_entry_ttl: 4096,
        max_entry_ttl: 33_000_000,
    });
}

// ================================================================
//  Initialization
// ================================================================

#[test]
fn test_initialize_sets_admin() {
    let (_env, vault, _token, admin, _alice, _fee) = setup();
    assert_eq!(vault.get_admin(), Some(admin));
}

#[test]
fn test_initialize_sets_fee_recipient() {
    let (_env, vault, _token, _admin, _alice, fee) = setup();
    assert_eq!(vault.get_fee_recipient(), Some(fee));
}

#[test]
fn test_double_initialize_fails() {
    let (_env, vault, _token, admin, _alice, _fee) = setup();
    assert_eq!(vault.try_initialize(&admin, &None, &None), Err(Ok(VaultError::Unauthorized)));
}

#[test]
fn test_is_initialized() {
    let env = Env::default();
    env.mock_all_auths();
    let vault_id = env.register(TimeLockVault, ());
    let vault = TimeLockVaultClient::new(&env, &vault_id);
    let admin: Address = Address::generate(&env);

    assert!(!vault.is_initialized());
    vault.initialize(&admin, &None, &None);
    assert!(vault.is_initialized());

    vault.renounce_admin(&admin);
    assert!(vault.is_initialized());
}

// ================================================================
//  Deposit — happy path
// ================================================================

#[test]
fn test_deposit_success() {
    let (env, vault, token, _admin, alice, _fee) = setup();
    let unlock_time = env.ledger().timestamp() + 3600;
    vault.deposit(&alice, &token, &1_000, &unlock_time, &0);

    let entry = vault.get_vault(&alice).expect("entry should exist");
    assert_eq!(entry.amount, 1_000);
    assert_eq!(entry.unlock_time, unlock_time);
    assert_eq!(entry.token, token);
    assert_eq!(entry.depositor, alice);
    assert_eq!(entry.penalty_bps, 0);

    let events = env.events().all();
    let last = events.last().unwrap();
    assert_eq!(
        last,
        (
            vault.address.clone(),
            (symbol_short!("deposit"), alice.clone(), token.clone()).into_val(&env),
            (1_000_i128, unlock_time).into_val(&env),
        )
    );
}

#[test]
fn test_deposit_transfers_tokens_to_contract() {
    let (env, vault, token, _admin, alice, _fee) = setup();
    let token_client = TokenClient::new(&env, &token);
    let unlock_time = env.ledger().timestamp() + 3600;
    vault.deposit(&alice, &token, &1_000, &unlock_time, &0);
    assert_eq!(token_client.balance(&alice), 9_000);
}

// ================================================================
//  Deposit — validation errors
// ================================================================

#[test]
fn test_deposit_zero_amount_fails() {
    let (env, vault, token, _admin, alice, _fee) = setup();
    let unlock_time = env.ledger().timestamp() + 3600;
    assert_eq!(vault.try_deposit(&alice, &token, &0, &unlock_time, &0), Err(Ok(VaultError::InvalidAmount)));
}

#[test]
fn test_deposit_negative_amount_fails() {
    let (env, vault, token, _admin, alice, _fee) = setup();
    let unlock_time = env.ledger().timestamp() + 3600;
    assert_eq!(vault.try_deposit(&alice, &token, &-1, &unlock_time, &0), Err(Ok(VaultError::InvalidAmount)));
}

#[test]
fn test_deposit_amount_exceeds_max_fails() {
    let (env, vault, token, _admin, alice, _fee) = setup();
    StellarAssetClient::new(&env, &token).mint(&alice, &MAX_DEPOSIT_AMOUNT);
    let unlock_time = env.ledger().timestamp() + 3600;
    assert_eq!(vault.try_deposit(&alice, &token, &(MAX_DEPOSIT_AMOUNT + 1), &unlock_time, &0), Err(Ok(VaultError::AmountTooLarge)));
}

#[test]
fn test_deposit_at_max_amount_succeeds() {
    let (env, vault, token, _admin, alice, _fee) = setup();
    StellarAssetClient::new(&env, &token).mint(&alice, &MAX_DEPOSIT_AMOUNT);
    let unlock_time = env.ledger().timestamp() + 3600;
    vault.deposit(&alice, &token, &MAX_DEPOSIT_AMOUNT, &unlock_time, &0);
    assert_eq!(vault.get_vault(&alice).unwrap().amount, MAX_DEPOSIT_AMOUNT);
}

#[test]
fn test_deposit_past_unlock_time_fails() {
    let (env, vault, token, _admin, alice, _fee) = setup();
    let unlock_time = env.ledger().timestamp();
    assert_eq!(vault.try_deposit(&alice, &token, &1_000, &unlock_time, &0), Err(Ok(VaultError::UnlockTimeNotInFuture)));
}

#[test]
fn test_deposit_unlock_time_in_past_fails() {
    let (env, vault, token, _admin, alice, _fee) = setup();
    let unlock_time = env.ledger().timestamp().saturating_sub(1);
    assert_eq!(vault.try_deposit(&alice, &token, &1_000, &unlock_time, &0), Err(Ok(VaultError::UnlockTimeNotInFuture)));
}

#[test]
fn test_deposit_lock_duration_too_long_fails() {
    let (env, vault, token, _admin, alice, _fee) = setup();
    let unlock_time = env.ledger().timestamp() + MAX_LOCK_DURATION_SECS + 1;
    assert_eq!(vault.try_deposit(&alice, &token, &1_000, &unlock_time, &0), Err(Ok(VaultError::LockDurationTooLong)));
}

#[test]
fn test_deposit_at_max_duration_succeeds() {
    let (env, vault, token, _admin, alice, _fee) = setup();
    let unlock_time = env.ledger().timestamp() + MAX_LOCK_DURATION_SECS;
    vault.deposit(&alice, &token, &1_000, &unlock_time, &0);
    assert!(vault.get_vault(&alice).is_some());
}

#[test]
fn test_deposit_duplicate_fails() {
    let (env, vault, token, _admin, alice, _fee) = setup();
    let unlock_time = env.ledger().timestamp() + 3600;
    vault.deposit(&alice, &token, &500, &unlock_time, &0);
    assert_eq!(vault.try_deposit(&alice, &token, &500, &unlock_time, &0), Err(Ok(VaultError::DepositAlreadyExists)));
}

#[test]
fn test_deposit_invalid_penalty_bps_fails() {
    let (env, vault, token, _admin, alice, _fee) = setup();
    let unlock_time = env.ledger().timestamp() + 3600;
    assert_eq!(vault.try_deposit(&alice, &token, &1_000, &unlock_time, &10_001), Err(Ok(VaultError::InvalidPenaltyBps)));
}

// ================================================================
//  Withdraw — happy path
// ================================================================

#[test]
fn test_withdraw_after_unlock_succeeds() {
    let (env, vault, token, _admin, alice, _fee) = setup();
    let token_client = TokenClient::new(&env, &token);
    let unlock_time = env.ledger().timestamp() + 3600;
    vault.deposit(&alice, &token, &1_000, &unlock_time, &0);
    advance_time(&env, 3601);
    vault.withdraw(&alice);

    assert!(vault.get_vault(&alice).is_none());
    assert_eq!(token_client.balance(&alice), 10_000);

    let events = env.events().all();
    let last = events.last().unwrap();
    assert_eq!(
        last,
        (
            vault.address.clone(),
            (symbol_short!("withdraw"), alice.clone(), token.clone()).into_val(&env),
            1_000_i128.into_val(&env),
        )
    );
}

#[test]
fn test_withdraw_exactly_at_unlock_time_succeeds() {
    let (env, vault, token, _admin, alice, _fee) = setup();
    let unlock_time = env.ledger().timestamp() + 3600;
    vault.deposit(&alice, &token, &1_000, &unlock_time, &0);
    advance_time(&env, 3600);
    vault.withdraw(&alice);
    assert!(vault.get_vault(&alice).is_none());
}

// ================================================================
//  Withdraw — error paths
// ================================================================

#[test]
fn test_withdraw_before_unlock_fails() {
    let (env, vault, token, _admin, alice, _fee) = setup();
    let unlock_time = env.ledger().timestamp() + 3600;
    vault.deposit(&alice, &token, &1_000, &unlock_time, &0);
    advance_time(&env, 1800);
    assert_eq!(vault.try_withdraw(&alice), Err(Ok(VaultError::FundsStillLocked)));
}

#[test]
fn test_withdraw_no_deposit_fails() {
    let (_env, vault, _token, _admin, alice, _fee) = setup();
    assert_eq!(vault.try_withdraw(&alice), Err(Ok(VaultError::NoDepositFound)));
}

// ================================================================
//  cancel_deposit
// ================================================================

#[test]
fn test_cancel_deposit_zero_penalty_returns_full_amount() {
    let (env, vault, token, _admin, alice, _fee) = setup();
    let token_client = TokenClient::new(&env, &token);
    let unlock_time = env.ledger().timestamp() + 3600;
    vault.deposit(&alice, &token, &1_000, &unlock_time, &0);
    vault.cancel_deposit(&alice);
    assert!(vault.get_vault(&alice).is_none());
    assert_eq!(token_client.balance(&alice), 10_000);
}

#[test]
fn test_cancel_deposit_partial_penalty_splits_correctly() {
    let (env, vault, token, _admin, alice, fee) = setup();
    let token_client = TokenClient::new(&env, &token);
    let unlock_time = env.ledger().timestamp() + 3600;
    vault.deposit(&alice, &token, &1_000, &unlock_time, &1_000); // 10%
    vault.cancel_deposit(&alice);
    assert!(vault.get_vault(&alice).is_none());
    assert_eq!(token_client.balance(&alice), 9_900);
    assert_eq!(token_client.balance(&fee), 100);
}

#[test]
fn test_cancel_deposit_100_percent_penalty() {
    let (env, vault, token, _admin, alice, fee) = setup();
    let token_client = TokenClient::new(&env, &token);
    let unlock_time = env.ledger().timestamp() + 3600;
    vault.deposit(&alice, &token, &1_000, &unlock_time, &10_000);
    vault.cancel_deposit(&alice);
    assert!(vault.get_vault(&alice).is_none());
    assert_eq!(token_client.balance(&alice), 9_000);
    assert_eq!(token_client.balance(&fee), 1_000);
}

#[test]
fn test_cancel_deposit_no_deposit_fails() {
    let (_env, vault, _token, _admin, alice, _fee) = setup();
    assert_eq!(vault.try_cancel_deposit(&alice), Err(Ok(VaultError::NoDepositFound)));
}

#[test]
fn test_cancel_deposit_after_unlock_fails() {
    let (env, vault, token, _admin, alice, _fee) = setup();
    let unlock_time = env.ledger().timestamp() + 3600;
    vault.deposit(&alice, &token, &1_000, &unlock_time, &500);
    advance_time(&env, 3601);
    assert_eq!(vault.try_cancel_deposit(&alice), Err(Ok(VaultError::FundsStillLocked)));
}

#[test]
fn test_cancel_deposit_penalty_stored_in_vault_entry() {
    let (env, vault, token, _admin, alice, _fee) = setup();
    let unlock_time = env.ledger().timestamp() + 3600;
    vault.deposit(&alice, &token, &1_000, &unlock_time, &500);
    assert_eq!(vault.get_vault(&alice).unwrap().penalty_bps, 500);
}

// ================================================================
//  Time helpers
// ================================================================

#[test]
fn test_time_remaining_before_unlock() {
    let (env, vault, token, _admin, alice, _fee) = setup();
    let unlock_time = env.ledger().timestamp() + 3600;
    vault.deposit(&alice, &token, &1_000, &unlock_time, &0);
    advance_time(&env, 1800);
    assert_eq!(vault.time_remaining(&alice), 1800);
}

#[test]
fn test_time_remaining_after_unlock_is_zero() {
    let (env, vault, token, _admin, alice, _fee) = setup();
    let unlock_time = env.ledger().timestamp() + 3600;
    vault.deposit(&alice, &token, &1_000, &unlock_time, &0);
    advance_time(&env, 7200);
    assert_eq!(vault.time_remaining(&alice), 0);
}

#[test]
fn test_time_remaining_no_deposit_is_zero() {
    let (_env, vault, _token, _admin, alice, _fee) = setup();
    assert_eq!(vault.time_remaining(&alice), 0);
}

#[test]
fn test_get_time_returns_ledger_timestamp() {
    let (env, vault, _token, _admin, _alice, _fee) = setup();
    assert_eq!(vault.get_time(), env.ledger().timestamp());
}

#[test]
fn test_get_constants_returns_correct_values() {
    let (_env, vault, _token, _admin, _alice, _fee) = setup();
    let (max_amount, max_duration) = vault.get_constants();
    assert_eq!(max_amount, MAX_DEPOSIT_AMOUNT);
    assert_eq!(max_duration, MAX_LOCK_DURATION_SECS);
}

// ================================================================
//  Emergency Withdrawal
// ================================================================

#[test]
fn test_emergency_withdraw_by_admin_before_unlock_succeeds() {
    let (env, vault, token, admin, alice, _fee) = setup();
    let token_client = TokenClient::new(&env, &token);
    let unlock_time = env.ledger().timestamp() + 86400;
    vault.deposit(&alice, &token, &2_000, &unlock_time, &0);
    vault.emergency_withdraw(&admin, &alice);
    assert!(vault.get_vault(&alice).is_none());
    assert_eq!(token_client.balance(&alice), 10_000);

    let events = env.events().all();
    let last = events.last().unwrap();
    assert_eq!(
        last,
        (
            vault.address.clone(),
            (Symbol::new(&env, "emrg_wdraw"), admin.clone(), alice.clone()).into_val(&env),
            (token.clone(), 2_000_i128).into_val(&env),
        )
    );
}

#[test]
fn test_emergency_withdraw_by_non_admin_fails() {
    let (env, vault, token, _admin, alice, _fee) = setup();
    let bob: Address = Address::generate(&env);
    let unlock_time = env.ledger().timestamp() + 86400;
    vault.deposit(&alice, &token, &2_000, &unlock_time, &0);
    assert_eq!(vault.try_emergency_withdraw(&bob, &alice), Err(Ok(VaultError::Unauthorized)));
}

#[test]
fn test_emergency_withdraw_no_deposit_fails() {
    let (_env, vault, _token, admin, alice, _fee) = setup();
    assert_eq!(vault.try_emergency_withdraw(&admin, &alice), Err(Ok(VaultError::NoDepositFound)));
}

// ================================================================
//  Admin Transfer — two-step
// ================================================================

#[test]
fn test_transfer_admin_two_step_succeeds() {
    let (env, vault, _token, admin, _alice, _fee) = setup();
    let new_admin: Address = Address::generate(&env);

    vault.transfer_admin(&admin, &new_admin);
    assert_eq!(vault.get_pending_admin(), Some(new_admin.clone()));
    assert_eq!(vault.get_admin(), Some(admin.clone()));

    {
        let events = env.events().all();
        let last = events.last().unwrap();
        assert_eq!(
            last,
            (
                vault.address.clone(),
                (Symbol::new(&env, "adm_xfr_init"), admin.clone()).into_val(&env),
                new_admin.clone().into_val(&env),
            )
        );
    }

    vault.accept_admin(&new_admin);
    assert_eq!(vault.get_admin(), Some(new_admin.clone()));
    assert_eq!(vault.get_pending_admin(), None);

    {
        let events = env.events().all();
        let last = events.last().unwrap();
        assert_eq!(
            last,
            (
                vault.address.clone(),
                (Symbol::new(&env, "adm_xfr_done"), new_admin.clone()).into_val(&env),
                ().into_val(&env),
            )
        );
    }
}

#[test]
fn test_transfer_admin_non_admin_cannot_initiate() {
    let (env, vault, _token, _admin, _alice, _fee) = setup();
    let bob: Address = Address::generate(&env);
    let carol: Address = Address::generate(&env);
    assert_eq!(vault.try_transfer_admin(&bob, &carol), Err(Ok(VaultError::Unauthorized)));
}

#[test]
fn test_accept_admin_wrong_address_fails() {
    let (env, vault, _token, admin, _alice, _fee) = setup();
    let new_admin: Address = Address::generate(&env);
    let impostor: Address = Address::generate(&env);
    vault.transfer_admin(&admin, &new_admin);
    assert_eq!(vault.try_accept_admin(&impostor), Err(Ok(VaultError::Unauthorized)));
    assert_eq!(vault.get_admin(), Some(admin));
}

#[test]
fn test_accept_admin_with_no_pending_fails() {
    let (env, vault, _token, _admin, _alice, _fee) = setup();
    let bob: Address = Address::generate(&env);
    assert_eq!(vault.try_accept_admin(&bob), Err(Ok(VaultError::Unauthorized)));
}

#[test]
fn test_cancel_transfer_admin_clears_pending() {
    let (env, vault, _token, admin, _alice, _fee) = setup();
    let new_admin: Address = Address::generate(&env);
    vault.transfer_admin(&admin, &new_admin);
    vault.cancel_transfer_admin(&admin);
    assert_eq!(vault.get_pending_admin(), None);
    assert_eq!(vault.get_admin(), Some(admin));
}

#[test]
fn test_cancel_transfer_admin_by_non_admin_fails() {
    let (env, vault, _token, admin, _alice, _fee) = setup();
    let new_admin: Address = Address::generate(&env);
    let bob: Address = Address::generate(&env);
    vault.transfer_admin(&admin, &new_admin);
    assert_eq!(vault.try_cancel_transfer_admin(&bob), Err(Ok(VaultError::Unauthorized)));
}

#[test]
fn test_accept_admin_after_cancel_fails() {
    let (env, vault, _token, admin, _alice, _fee) = setup();
    let new_admin: Address = Address::generate(&env);
    vault.transfer_admin(&admin, &new_admin);
    vault.cancel_transfer_admin(&admin);
    assert_eq!(vault.try_accept_admin(&new_admin), Err(Ok(VaultError::Unauthorized)));
    assert_eq!(vault.get_pending_admin(), None);
}

#[test]
fn test_new_admin_can_emergency_withdraw_after_transfer() {
    let (env, vault, token, admin, alice, _fee) = setup();
    let new_admin: Address = Address::generate(&env);
    let token_client = TokenClient::new(&env, &token);
    let unlock_time = env.ledger().timestamp() + 86400;
    vault.deposit(&alice, &token, &1_000, &unlock_time, &0);
    vault.transfer_admin(&admin, &new_admin);
    vault.accept_admin(&new_admin);
    assert_eq!(vault.try_emergency_withdraw(&admin, &alice), Err(Ok(VaultError::Unauthorized)));
    vault.emergency_withdraw(&new_admin, &alice);
    assert_eq!(token_client.balance(&alice), 10_000);
}

// ================================================================
//  Admin Renounce
// ================================================================

#[test]
fn test_renounce_admin_removes_admin() {
    let (env, vault, _token, admin, _alice, _fee) = setup();
    vault.renounce_admin(&admin);
    assert_eq!(vault.get_admin(), None);

    let events = env.events().all();
    let last = events.last().unwrap();
    assert_eq!(
        last,
        (
            vault.address.clone(),
            (Symbol::new(&env, "adm_renounce"), admin.clone()).into_val(&env),
            ().into_val(&env),
        )
    );
}

#[test]
fn test_renounce_admin_disables_emergency_withdraw() {
    let (env, vault, token, admin, alice, _fee) = setup();
    let unlock_time = env.ledger().timestamp() + 86400;
    vault.deposit(&alice, &token, &1_000, &unlock_time, &0);
    vault.renounce_admin(&admin);
    assert_eq!(vault.try_emergency_withdraw(&admin, &alice), Err(Ok(VaultError::Unauthorized)));
}

#[test]
fn test_renounce_admin_by_non_admin_fails() {
    let (env, vault, _token, _admin, _alice, _fee) = setup();
    let bob: Address = Address::generate(&env);
    assert_eq!(vault.try_renounce_admin(&bob), Err(Ok(VaultError::Unauthorized)));
}

#[test]
fn test_renounce_admin_clears_pending_transfer() {
    let (env, vault, _token, admin, _alice, _fee) = setup();
    let new_admin: Address = Address::generate(&env);
    vault.transfer_admin(&admin, &new_admin);
    vault.renounce_admin(&admin);
    assert_eq!(vault.get_admin(), None);
    assert_eq!(vault.get_pending_admin(), None);
}

// ================================================================
//  Re-deposit after withdrawal
// ================================================================

#[test]
fn test_redeposit_after_withdraw_succeeds() {
    let (env, vault, token, _admin, alice, _fee) = setup();
    let unlock_time = env.ledger().timestamp() + 3600;
    vault.deposit(&alice, &token, &1_000, &unlock_time, &0);
    advance_time(&env, 3601);
    vault.withdraw(&alice);

    let new_unlock = env.ledger().timestamp() + 7200;
    vault.deposit(&alice, &token, &500, &new_unlock, &0);
    assert_eq!(vault.get_vault(&alice).unwrap().amount, 500);
}

// ================================================================
//  TTL / storage constants
// ================================================================

#[test]
fn test_bump_target_covers_max_lock_duration() {
    use crate::storage::BUMP_TARGET;
    const LEDGER_INTERVAL_SECS: u64 = 5;
    let max_lock_ledgers = MAX_LOCK_DURATION_SECS / LEDGER_INTERVAL_SECS;
    assert!(
        BUMP_TARGET as u64 >= max_lock_ledgers,
        "BUMP_TARGET ({}) must be >= max lock duration in ledgers ({})",
        BUMP_TARGET,
        max_lock_ledgers,
    );
}

// ================================================================
//  View functions do not mutate state
// ================================================================

#[test]
fn test_get_vault_is_readonly() {
    let (_env, vault, _token, _admin, alice, _fee) = setup();
    assert!(vault.get_vault(&alice).is_none());
    assert!(vault.get_vault(&alice).is_none());
}

#[test]
fn test_time_remaining_is_readonly() {
    let (_env, vault, _token, _admin, alice, _fee) = setup();
    assert_eq!(vault.time_remaining(&alice), 0);
    assert_eq!(vault.time_remaining(&alice), 0);
}

// ================================================================
//  Depositor List / Pagination
// ================================================================

#[test]
fn test_depositor_count_empty() {
    let (_env, vault, _token, _admin, _alice, _fee) = setup();
    assert_eq!(vault.get_depositor_count(), 0);
}

#[test]
fn test_depositors_empty_returns_empty_vec() {
    let (_env, vault, _token, _admin, _alice, _fee) = setup();
    assert_eq!(vault.get_depositors(&0, &10).len(), 0);
}

#[test]
fn test_depositor_count_single_entry() {
    let (env, vault, token, _admin, alice, _fee) = setup();
    let unlock_time = env.ledger().timestamp() + 3600;
    vault.deposit(&alice, &token, &1_000, &unlock_time, &0);
    assert_eq!(vault.get_depositor_count(), 1);
}

#[test]
fn test_depositor_removed_on_withdraw() {
    let (env, vault, token, _admin, alice, _fee) = setup();
    let unlock_time = env.ledger().timestamp() + 3600;
    vault.deposit(&alice, &token, &1_000, &unlock_time, &0);
    advance_time(&env, 3601);
    vault.withdraw(&alice);
    assert_eq!(vault.get_depositor_count(), 0);
    assert_eq!(vault.get_depositors(&0, &10).len(), 0);
}

#[test]
fn test_depositor_removed_on_emergency_withdraw() {
    let (env, vault, token, admin, alice, _fee) = setup();
    let unlock_time = env.ledger().timestamp() + 86400;
    vault.deposit(&alice, &token, &1_000, &unlock_time, &0);
    vault.emergency_withdraw(&admin, &alice);
    assert_eq!(vault.get_depositor_count(), 0);
}

#[test]
fn test_depositor_list_consistent_after_partial_removal() {
    let (env, vault, token, _admin, alice, _fee) = setup();
    let bob: Address = Address::generate(&env);
    StellarAssetClient::new(&env, &token).mint(&bob, &5_000);
    let unlock_time = env.ledger().timestamp() + 3600;
    vault.deposit(&alice, &token, &1_000, &unlock_time, &0);
    vault.deposit(&bob, &token, &2_000, &unlock_time, &0);
    assert_eq!(vault.get_depositor_count(), 2);
    advance_time(&env, 3601);
    vault.withdraw(&alice);
    assert_eq!(vault.get_depositor_count(), 1);
    let page = vault.get_depositors(&0, &10);
    assert_eq!(page.len(), 1);
    assert_eq!(page.get(0).unwrap(), bob);
}

#[test]
fn test_pagination_offset_and_limit() {
    let (env, vault, token, _admin, alice, _fee) = setup();
    let bob: Address = Address::generate(&env);
    let carol: Address = Address::generate(&env);
    StellarAssetClient::new(&env, &token).mint(&bob, &5_000);
    StellarAssetClient::new(&env, &token).mint(&carol, &5_000);
    let unlock_time = env.ledger().timestamp() + 3600;
    vault.deposit(&alice, &token, &1_000, &unlock_time, &0);
    vault.deposit(&bob, &token, &2_000, &unlock_time, &0);
    vault.deposit(&carol, &token, &3_000, &unlock_time, &0);
    assert_eq!(vault.get_depositors(&0, &2).len(), 2);
    assert_eq!(vault.get_depositors(&2, &2).len(), 1);
}

#[test]
fn test_pagination_offset_beyond_end_returns_empty() {
    let (env, vault, token, _admin, alice, _fee) = setup();
    let unlock_time = env.ledger().timestamp() + 3600;
    vault.deposit(&alice, &token, &1_000, &unlock_time, &0);
    assert_eq!(vault.get_depositors(&10, &5).len(), 0);
}

#[test]
fn test_pagination_limit_zero_returns_empty() {
    let (env, vault, token, _admin, alice, _fee) = setup();
    let unlock_time = env.ledger().timestamp() + 3600;
    vault.deposit(&alice, &token, &1_000, &unlock_time, &0);
    assert_eq!(vault.get_depositors(&0, &0).len(), 0);
}

#[test]
fn test_redeposit_after_withdraw_adds_back_to_list() {
    let (env, vault, token, _admin, alice, _fee) = setup();
    let unlock_time = env.ledger().timestamp() + 3600;
    vault.deposit(&alice, &token, &1_000, &unlock_time, &0);
    assert_eq!(vault.get_depositor_count(), 1);
    advance_time(&env, 3601);
    vault.withdraw(&alice);
    assert_eq!(vault.get_depositor_count(), 0);
    let new_unlock = env.ledger().timestamp() + 7200;
    vault.deposit(&alice, &token, &500, &new_unlock, &0);
    assert_eq!(vault.get_depositor_count(), 1);
    assert_eq!(vault.get_depositors(&0, &10).get(0).unwrap(), alice);
}

// ================================================================
//  Configurable limits
// ================================================================

fn setup_with_limits(
    max_deposit: Option<i128>,
    max_lock_secs: Option<u64>,
) -> (Env, TimeLockVaultClient<'static>, Address, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let vault_id = env.register(TimeLockVault, ());
    let vault = TimeLockVaultClient::new(&env, &vault_id);
    let admin: Address = Address::generate(&env);
    let alice: Address = Address::generate(&env);
    let token_id = env.register_stellar_asset_contract_v2(admin.clone());
    let token_address = token_id.address();
    StellarAssetClient::new(&env, &token_address).mint(&alice, &1_000_000);
    vault.initialize(&admin, &max_deposit, &max_lock_secs);
    (env, vault, token_address, admin, alice)
}

#[test]
fn test_get_constants_returns_custom_limits() {
    let (_env, vault, _token, _admin, _alice) = setup_with_limits(Some(5_000), Some(7200));
    let (max_amount, max_duration) = vault.get_constants();
    assert_eq!(max_amount, 5_000);
    assert_eq!(max_duration, 7200);
}

#[test]
fn test_custom_max_deposit_enforced() {
    let (env, vault, token, _admin, alice) = setup_with_limits(Some(500), None);
    let unlock_time = env.ledger().timestamp() + 3600;
    vault.deposit(&alice, &token, &500, &unlock_time, &0);
    advance_time(&env, 3601);
    vault.withdraw(&alice);
    assert_eq!(vault.try_deposit(&alice, &token, &501, &(env.ledger().timestamp() + 3600), &0), Err(Ok(VaultError::AmountTooLarge)));
}

#[test]
fn test_custom_max_lock_secs_enforced() {
    let (env, vault, token, _admin, alice) = setup_with_limits(None, Some(3600));
    let unlock_time = env.ledger().timestamp() + 3600;
    vault.deposit(&alice, &token, &100, &unlock_time, &0);
    advance_time(&env, 3601);
    vault.withdraw(&alice);
    assert_eq!(
        vault.try_deposit(&alice, &token, &100, &(env.ledger().timestamp() + 3601), &0),
        Err(Ok(VaultError::LockDurationTooLong))
    );
}

#[test]
fn test_default_fallback_when_no_custom_limits() {
    let (env, vault, token, _admin, alice) = setup_with_limits(None, None);
    let unlock_time = env.ledger().timestamp() + 3600;
    assert_eq!(vault.try_deposit(&alice, &token, &(MAX_DEPOSIT_AMOUNT + 1), &unlock_time, &0), Err(Ok(VaultError::AmountTooLarge)));
    assert_eq!(
        vault.try_deposit(&alice, &token, &100, &(env.ledger().timestamp() + MAX_LOCK_DURATION_SECS + 1), &0),
        Err(Ok(VaultError::LockDurationTooLong))
    );
}

#[test]
fn test_initialize_invalid_max_deposit_fails() {
    let env = Env::default();
    env.mock_all_auths();
    let vault_id = env.register(TimeLockVault, ());
    let vault = TimeLockVaultClient::new(&env, &vault_id);
    let admin: Address = Address::generate(&env);
    assert_eq!(vault.try_initialize(&admin, &Some(0_i128), &None), Err(Ok(VaultError::InvalidAmount)));
}

#[test]
fn test_initialize_invalid_max_lock_secs_fails() {
    let env = Env::default();
    env.mock_all_auths();
    let vault_id = env.register(TimeLockVault, ());
    let vault = TimeLockVaultClient::new(&env, &vault_id);
    let admin: Address = Address::generate(&env);
    assert_eq!(vault.try_initialize(&admin, &None, &Some(0_u64)), Err(Ok(VaultError::LockDurationTooLong)));
}

// ================================================================
//  XDR serialization snapshot tests
// ================================================================

#[test]
fn test_vault_entry_xdr_snapshot() {
    use soroban_sdk::xdr::{FromXdr, ToXdr};
    let env = Env::default();
    let token: Address = Address::generate(&env);
    let depositor: Address = Address::generate(&env);
    let entry = VaultEntry {
        token: token.clone(),
        amount: 1_000_i128,
        unlock_time: 9_999_u64,
        depositor: depositor.clone(),
        penalty_bps: 0,
    };
    let xdr_bytes = entry.clone().to_xdr(&env);
    let entry2 = VaultEntry::from_xdr(&env, &xdr_bytes).expect("round-trip must succeed");
    assert_eq!(entry2.amount, entry.amount);
    assert_eq!(entry2.unlock_time, entry.unlock_time);
    assert_eq!(entry2.token, entry.token);
    assert_eq!(entry2.depositor, entry.depositor);
    assert_eq!(entry2.penalty_bps, entry.penalty_bps);
}

#[test]
fn test_vault_key_deposit_xdr_snapshot() {
    use soroban_sdk::xdr::{FromXdr, ToXdr};
    let env = Env::default();
    let depositor: Address = Address::generate(&env);
    let key = VaultKey::Deposit(depositor.clone());
    let xdr_bytes = key.to_xdr(&env);
    let key2 = VaultKey::from_xdr(&env, &xdr_bytes).expect("round-trip must succeed");
    assert_eq!(key2, VaultKey::Deposit(depositor));
}

#[test]
fn test_vault_key_admin_xdr_snapshot() {
    use soroban_sdk::xdr::{FromXdr, ToXdr};
    let env = Env::default();
    let xdr_bytes = VaultKey::Admin.to_xdr(&env);
    let key2 = VaultKey::from_xdr(&env, &xdr_bytes).expect("round-trip must succeed");
    assert_eq!(key2, VaultKey::Admin);
}

#[test]
fn test_vault_key_pending_admin_xdr_snapshot() {
    use soroban_sdk::xdr::{FromXdr, ToXdr};
    let env = Env::default();
    let xdr_bytes = VaultKey::PendingAdmin.to_xdr(&env);
    let key2 = VaultKey::from_xdr(&env, &xdr_bytes).expect("round-trip must succeed");
    assert_eq!(key2, VaultKey::PendingAdmin);
}

// ================================================================
//  Auth assertion tests
// ================================================================

#[test]
fn test_auth_deposit_requires_depositor() {
    let (env, vault, token, _admin, alice, _fee) = setup();
    let unlock_time = env.ledger().timestamp() + 3600;
    vault.deposit(&alice, &token, &1_000, &unlock_time, &0);
    assert_eq!(env.auths()[0].0, alice);
}

#[test]
fn test_auth_withdraw_requires_depositor() {
    let (env, vault, token, _admin, alice, _fee) = setup();
    let unlock_time = env.ledger().timestamp() + 3600;
    vault.deposit(&alice, &token, &1_000, &unlock_time, &0);
    advance_time(&env, 3601);
    vault.withdraw(&alice);
    assert_eq!(env.auths()[0].0, alice);
}

#[test]
fn test_auth_emergency_withdraw_requires_admin() {
    let (env, vault, token, admin, alice, _fee) = setup();
    let unlock_time = env.ledger().timestamp() + 86400;
    vault.deposit(&alice, &token, &1_000, &unlock_time, &0);
    vault.emergency_withdraw(&admin, &alice);
    assert_eq!(env.auths()[0].0, admin);
}

#[test]
fn test_auth_transfer_admin_requires_admin() {
    let (env, vault, _token, admin, _alice, _fee) = setup();
    let new_admin: Address = Address::generate(&env);
    vault.transfer_admin(&admin, &new_admin);
    assert_eq!(env.auths()[0].0, admin);
}

#[test]
fn test_auth_accept_admin_requires_new_admin() {
    let (env, vault, _token, admin, _alice, _fee) = setup();
    let new_admin: Address = Address::generate(&env);
    vault.transfer_admin(&admin, &new_admin);
    vault.accept_admin(&new_admin);
    assert_eq!(env.auths()[0].0, new_admin);
}

#[test]
fn test_auth_renounce_admin_requires_admin() {
    let (env, vault, _token, admin, _alice, _fee) = setup();
    vault.renounce_admin(&admin);
    assert_eq!(env.auths()[0].0, admin);
}

// ================================================================
//  Property-based tests — deposit input validation
//
//  Uses a simple LCG (linear congruential generator) to produce
//  deterministic pseudo-random inputs without any new dependencies.
//  Each generated (amount, unlock_time, penalty_bps) triple is
//  classified and the contract must return the expected error (or
//  succeed) for every case.
// ================================================================

/// Minimal deterministic PRNG (Knuth MMIX LCG).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0
    }

    /// Returns a value in `[lo, hi]` (inclusive).
    fn gen_range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next() % (hi - lo + 1)
    }
}

/// Classify what error (if any) `deposit` should return for the given inputs.
fn expected_deposit_result(
    amount: i128,
    unlock_time: u64,
    penalty_bps: u32,
    now: u64,
    max_deposit: i128,
    max_lock: u64,
) -> Option<VaultError> {
    // Validation order must match the contract implementation.
    if amount <= 0 {
        return Some(VaultError::InvalidAmount);
    }
    if amount > max_deposit {
        return Some(VaultError::AmountTooLarge);
    }
    if penalty_bps > 10_000 {
        return Some(VaultError::InvalidPenaltyBps);
    }
    if unlock_time <= now {
        return Some(VaultError::UnlockTimeNotInFuture);
    }
    if unlock_time.saturating_sub(now) > max_lock {
        return Some(VaultError::LockDurationTooLong);
    }
    None
}

#[test]
fn prop_deposit_amount_validation() {
    let mut rng = Lcg(0xDEAD_BEEF_CAFE_1234);

    // Amounts to test: negatives, zero, small positives, near-max, over-max.
    let amount_cases: &[i128] = &[
        i128::MIN, -1_000_000, -1, 0,
        1, 100, 1_000,
        MAX_DEPOSIT_AMOUNT - 1,
        MAX_DEPOSIT_AMOUNT,
        MAX_DEPOSIT_AMOUNT + 1,
        MAX_DEPOSIT_AMOUNT + 1_000,
        i128::MAX,
    ];

    let env = Env::default();
    env.mock_all_auths();
    let vault_id = env.register(TimeLockVault, ());
    let vault = TimeLockVaultClient::new(&env, &vault_id);
    let admin: Address = Address::generate(&env);
    let alice: Address = Address::generate(&env);
    let token_id = env.register_stellar_asset_contract_v2(admin.clone());
    let token = token_id.address();
    // Mint enough for valid deposits.
    StellarAssetClient::new(&env, &token).mint(&alice, &MAX_DEPOSIT_AMOUNT);
    vault.initialize(&admin, &None, &None);

    let penalty_bps: u32 = 0;

    for &amount in amount_cases {
        // Recapture now each iteration since time may have advanced.
        let now = env.ledger().timestamp();
        let unlock_time = now + 3600;
        let expected = expected_deposit_result(
            amount, unlock_time, penalty_bps, now, MAX_DEPOSIT_AMOUNT, MAX_LOCK_DURATION_SECS,
        );
        let result = vault.try_deposit(&alice, &token, &amount, &unlock_time, &penalty_bps);
        match expected {
            Some(err) => assert_eq!(result, Err(Ok(err)), "amount={amount}"),
            None => {
                assert!(result.is_ok(), "amount={amount} should succeed, got {result:?}");
                // Clean up so the next iteration doesn't hit DepositAlreadyExists.
                advance_time(&env, 3601);
                vault.withdraw(&alice);
            }
        }
    }}

#[test]
fn prop_deposit_unlock_time_validation() {
    let env = Env::default();
    env.mock_all_auths();
    let vault_id = env.register(TimeLockVault, ());
    let vault = TimeLockVaultClient::new(&env, &vault_id);
    let admin: Address = Address::generate(&env);
    let alice: Address = Address::generate(&env);
    let token_id = env.register_stellar_asset_contract_v2(admin.clone());
    let token = token_id.address();
    StellarAssetClient::new(&env, &token).mint(&alice, &MAX_DEPOSIT_AMOUNT);
    vault.initialize(&admin, &None, &None);

    let amount: i128 = 1_000;
    let penalty_bps: u32 = 0;

    // Each entry is a signed offset from `now` (negative = past, 0 = now, positive = future).
    // We use i128 to safely represent large offsets without overflow.
    let offsets: &[i128] = &[
        i128::MIN,
        -3600,
        -1,
        0,
        1,
        3600,
        MAX_LOCK_DURATION_SECS as i128,
        MAX_LOCK_DURATION_SECS as i128 + 1,
        i128::MAX,
    ];

    for &offset in offsets {
        let now = env.ledger().timestamp();
        // Compute unlock_time, clamping to u64 range.
        let unlock_time: u64 = if offset < 0 {
            now.saturating_sub((-offset).min(u64::MAX as i128) as u64)
        } else {
            now.saturating_add(offset.min(u64::MAX as i128) as u64)
        };
        let expected = expected_deposit_result(
            amount, unlock_time, penalty_bps, now, MAX_DEPOSIT_AMOUNT, MAX_LOCK_DURATION_SECS,
        );
        let result = vault.try_deposit(&alice, &token, &amount, &unlock_time, &penalty_bps);
        match expected {
            Some(err) => assert_eq!(result, Err(Ok(err)), "offset={offset}, unlock_time={unlock_time}"),
            None => {
                assert!(result.is_ok(), "offset={offset} should succeed");
                advance_time(&env, unlock_time - now + 1);
                vault.withdraw(&alice);
            }
        }
    }
}

#[test]
fn prop_deposit_penalty_bps_validation() {
    let env = Env::default();
    env.mock_all_auths();
    let vault_id = env.register(TimeLockVault, ());
    let vault = TimeLockVaultClient::new(&env, &vault_id);
    let admin: Address = Address::generate(&env);
    let alice: Address = Address::generate(&env);
    let token_id = env.register_stellar_asset_contract_v2(admin.clone());
    let token = token_id.address();
    StellarAssetClient::new(&env, &token).mint(&alice, &MAX_DEPOSIT_AMOUNT);
    vault.initialize(&admin, &None, &None);

    let amount: i128 = 1_000;
    let bps_cases: &[u32] = &[0, 1, 5_000, 9_999, 10_000, 10_001, u32::MAX];

    for &penalty_bps in bps_cases {
        let now = env.ledger().timestamp();
        let unlock_time = now + 3600;
        let expected = expected_deposit_result(
            amount, unlock_time, penalty_bps, now, MAX_DEPOSIT_AMOUNT, MAX_LOCK_DURATION_SECS,
        );
        let result = vault.try_deposit(&alice, &token, &amount, &unlock_time, &penalty_bps);
        match expected {
            Some(err) => assert_eq!(result, Err(Ok(err)), "penalty_bps={penalty_bps}"),
            None => {
                assert!(result.is_ok(), "penalty_bps={penalty_bps} should succeed");
                advance_time(&env, 3601);
                vault.withdraw(&alice);
            }
        }
    }
}

#[test]
fn prop_deposit_random_inputs() {
    /// Number of random triples to test.
    const ITERATIONS: u32 = 200;

    let mut rng = Lcg(0x1234_5678_9ABC_DEF0);

    let env = Env::default();
    env.mock_all_auths();
    let vault_id = env.register(TimeLockVault, ());
    let vault = TimeLockVaultClient::new(&env, &vault_id);
    let admin: Address = Address::generate(&env);
    let alice: Address = Address::generate(&env);
    let token_id = env.register_stellar_asset_contract_v2(admin.clone());
    let token = token_id.address();
    StellarAssetClient::new(&env, &token).mint(&alice, &i64::MAX.into());
    vault.initialize(&admin, &None, &None);

    for _ in 0..ITERATIONS {
        let now = env.ledger().timestamp();

        // Generate amount in a range that covers invalid, valid, and over-max.
        let raw_amount = rng.gen_range(0, (MAX_DEPOSIT_AMOUNT as u64).saturating_add(1_000));
        // Occasionally inject negative / zero amounts.
        let amount: i128 = if rng.next() % 8 == 0 {
            -((rng.next() % 1_000_000) as i128)
        } else {
            raw_amount as i128
        };

        // Generate unlock_time spanning past, present, valid future, and over-max.
        let unlock_time: u64 = match rng.next() % 4 {
            0 => now.saturating_sub(rng.gen_range(0, 3600)),          // past / now
            1 => now + rng.gen_range(1, MAX_LOCK_DURATION_SECS),      // valid future
            2 => now + MAX_LOCK_DURATION_SECS + rng.gen_range(1, 1_000_000), // too far
            _ => now + rng.gen_range(1, 3600),                        // short valid
        };

        let penalty_bps: u32 = rng.gen_range(0, 10_002) as u32;

        let expected = expected_deposit_result(
            amount, unlock_time, penalty_bps, now, MAX_DEPOSIT_AMOUNT, MAX_LOCK_DURATION_SECS,
        );
        let result = vault.try_deposit(&alice, &token, &amount, &unlock_time, &penalty_bps);

        match expected {
            Some(err) => assert_eq!(
                result,
                Err(Ok(err)),
                "iter: amount={amount}, unlock_time={unlock_time}, penalty_bps={penalty_bps}, now={now}"
            ),
            None => {
                assert!(
                    result.is_ok(),
                    "should succeed: amount={amount}, unlock_time={unlock_time}, penalty_bps={penalty_bps}"
                );
                // Advance past unlock so we can withdraw and re-deposit next iteration.
                let wait = unlock_time.saturating_sub(now) + 1;
                advance_time(&env, wait);
                vault.withdraw(&alice);
            }
        }
    }
}
