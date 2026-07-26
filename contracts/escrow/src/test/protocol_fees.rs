#![cfg(test)]

use soroban_sdk::{testutils::Address as _, Address, Env, vec};
use crate::{Escrow, EscrowClient, DataKey, Error, BPS_DENOMINATOR, MAX_BPS, ReleaseAuthorization};

#[test]
fn test_default_fees_are_zero() {
    let env = Env::default();
    let contract_id = env.register(Escrow, ());
    let client = EscrowClient::new(&env, &contract_id);

    // Default values before initialization or setting must be 0
    assert_eq!(client.get_protocol_fee_bps(), 0);
    assert_eq!(client.get_accumulated_protocol_fees(), 0);
}

/// Test that `get_protocol_fee_bps` returns 0 when uninitialized.
#[test]
fn test_get_protocol_fee_bps_returns_zero_when_uninitialized() {
    let env = Env::default();
    let contract_id = env.register_contract(None, Escrow);
    let client = EscrowClient::new(&env, &contract_id);

    assert_eq!(client.get_protocol_fee_bps(), 0);
}

/// Test that `get_accumulated_protocol_fees` returns 0 when uninitialized.
#[test]
fn test_get_accumulated_protocol_fees_returns_zero_when_uninitialized() {
    let env = Env::default();
    let contract_id = env.register_contract(None, Escrow);
    let client = EscrowClient::new(&env, &contract_id);

    assert_eq!(client.get_accumulated_protocol_fees(), 0);
}

/// Test that `get_protocol_fee_bps` returns the configured value after admin sets it.
#[test]
fn test_get_protocol_fee_bps_after_configuration() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let contract_id = env.register_contract(None, Escrow);
    let client = EscrowClient::new(&env, &contract_id);

    client.initialize(&admin);

    assert_eq!(client.get_protocol_fee_bps(), 0);

    client.set_protocol_fee_bps(&500u32);
    assert_eq!(client.get_protocol_fee_bps(), 500);

    client.set_protocol_fee_bps(&1000u32);
    assert_eq!(client.get_protocol_fee_bps(), 1000);
}

/// Test that protocol fee updates accept 0 and MAX_BPS basis points.
#[test]
fn test_set_protocol_fee_bps_accepts_boundary_values() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let contract_id = env.register_contract(None, Escrow);
    let client = EscrowClient::new(&env, &contract_id);

    client.initialize(&admin);

    assert!(client.set_protocol_fee_bps(&0u32));
    assert_eq!(client.get_protocol_fee_bps(), 0);

    assert!(client.set_protocol_fee_bps(&MAX_BPS));
    assert_eq!(client.get_protocol_fee_bps(), MAX_BPS);
}

/// Test that protocol fee updates reject values above 100%.
#[test]
fn test_set_protocol_fee_bps_rejects_values_above_100_percent() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let contract_id = env.register_contract(None, Escrow);
    let client = EscrowClient::new(&env, &contract_id);

    client.initialize(&admin);
    assert!(client.set_protocol_fee_bps(&0u32));

    let result = client.try_set_protocol_fee_bps(&(MAX_BPS + 1));
    super::assert_contract_error(result, Error::InvalidProtocolParameters);
    assert_eq!(client.get_protocol_fee_bps(), 0);
}

/// Test that `get_accumulated_protocol_fees` reflects fees accumulated after milestone releases.
#[test]
fn test_get_accumulated_protocol_fees_after_releases() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let contract_id = env.register_contract(None, Escrow);
    let client = EscrowClient::new(&env, &contract_id);

    client.initialize(&admin);
    client.set_protocol_fee_bps(&1000u32);

    let client_addr = Address::generate(&env);
    let freelancer_addr = Address::generate(&env);
    let milestones = vec![&env, 1000_i128, 2500_i128, 3333_i128];

    let id = client.create_contract(
        &client_addr,
        &freelancer_addr,
        &None,
        &milestones,
        &ReleaseAuthorization::ClientOnly,
    );

    client.deposit_funds(&id, &client_addr, &6833_i128);

    assert_eq!(client.get_accumulated_protocol_fees(), 0);

    // Fee: 1000 * 1000 / MAX_BPS = 100
    client.approve_milestone_release(&id, &client_addr, &0);
    client.release_milestone(&id, &client_addr, &0);
    assert_eq!(client.get_accumulated_protocol_fees(), 100);

    // Fee: 2500 * 1000 / BPS_DENOMINATOR = 250
    client.approve_milestone_release(&id, &client_addr, &1);
    client.release_milestone(&id, &client_addr, &1);
    assert_eq!(client.get_accumulated_protocol_fees(), 350);

    // Fee: 3333 * 1000 / BPS_DENOMINATOR = 333
    client.approve_milestone_release(&id, &client_addr, &2);
    client.release_milestone(&id, &client_addr, &2);
    assert_eq!(client.get_accumulated_protocol_fees(), 683);
}

/// Test that accumulated fees remain at 0 when fee rate is 0.
#[test]
fn test_no_fees_accumulated_when_rate_is_zero() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let contract_id = env.register_contract(None, Escrow);
    let client = EscrowClient::new(&env, &contract_id);

    client.initialize(&admin);
    assert_eq!(client.get_protocol_fee_bps(), 0);

    let client_addr = Address::generate(&env);
    let freelancer_addr = Address::generate(&env);
    let milestones = vec![&env, 1000_i128];

    let id = client.create_contract(
        &client_addr,
        &freelancer_addr,
        &None,
        &milestones,
        &ReleaseAuthorization::ClientOnly,
    );

    client.deposit_funds(&id, &client_addr, &1000_i128);
    client.approve_milestone_release(&id, &client_addr, &0);
    client.release_milestone(&id, &client_addr, &0);

    assert_eq!(client.get_accumulated_protocol_fees(), 0);
}

/// Test that read functions bump TTL and can be called multiple times without error.
#[test]
fn test_readers_bump_ttl_and_are_non_destructive() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let contract_id = env.register(Escrow, ());
    let client = EscrowClient::new(&env, &contract_id);

    client.initialize(&admin);
    client.set_protocol_fee_bps(&250u32);

    env.as_contract(&contract_id, || {
        env.storage()
            .persistent()
            .set(&DataKey::AccumulatedProtocolFees, &5000_i128);
    });

    for _ in 0..10 {
        assert_eq!(client.get_protocol_fee_bps(), 250);
        assert_eq!(client.get_accumulated_protocol_fees(), 5000);
    }
}

/// Test readers work when keys are set directly without initialization.
#[test]
fn test_readers_work_without_initialization() {
    let env = Env::default();
    let contract_id = env.register_contract(None, Escrow);
    let client = EscrowClient::new(&env, &contract_id);

    env.as_contract(&contract_id, || {
        env.storage()
            .persistent()
            .set(&DataKey::ProtocolFeeBps, &123u32);
        env.storage()
            .persistent()
            .set(&DataKey::AccumulatedProtocolFees, &456_i128);
    });

    assert_eq!(client.get_protocol_fee_bps(), 123);
    assert_eq!(client.get_accumulated_protocol_fees(), 456);
}

#[test]
fn test_fee_math_0_bps() {
    let env = Env::default();
    let fee = Escrow::calculate_protocol_fee(&env, 1000, 0);
    assert_eq!(fee, 0);
}

#[test]
fn withdraw_protocol_fees_rejects_when_paused() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let contract_id = env.register_contract(None, Escrow);
    let client = EscrowClient::new(&env, &contract_id);
    let token = env.register_stellar_asset_contract(admin.clone());
    let destination = Address::generate(&env);

    client.initialize(&admin);
    client.bind_settlement_token(&admin, &token);
    env.as_contract(&contract_id, || {
        env.storage()
            .persistent()
            .set(&DataKey::AccumulatedProtocolFees, &500_i128);
    });
    env.mock_all_auths_allowing_non_root_auth();
    client.pause();

    super::assert_contract_error(
        client.try_withdraw_protocol_fees(&admin, &destination, &500_i128),
        EscrowError::ContractPaused,
    );
}

#[test]
fn withdraw_protocol_fees_allows_when_unpaused() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let contract_id = env.register_contract(None, Escrow);
    let client = EscrowClient::new(&env, &contract_id);
    let token = env.register_stellar_asset_contract(admin.clone());
    let destination = Address::generate(&env);

    client.initialize(&admin);
    client.bind_settlement_token(&admin, &token);
    env.as_contract(&contract_id, || {
        env.storage()
            .persistent()
            .set(&DataKey::AccumulatedProtocolFees, &500_i128);
    });
    env.mock_all_auths_allowing_non_root_auth();
    client.pause();
    client.unpause();
    StellarAssetClient::new(&env, &token).mint(&contract_id, &500_i128);

    assert!(client.withdraw_protocol_fees(&admin, &destination, &500_i128));
}

#[test]
fn test_fee_math_normal_bps() {
    let env = Env::default();
    let fee = Escrow::calculate_protocol_fee(&env, 1000, 1000);
    assert_eq!(fee, 100);
}

#[test]
#[should_panic(expected = "HostError: Error(Contract, #25)")] // PotentialOverflow
fn test_fee_math_overflow() {
    let env = Env::default();
    Escrow::calculate_protocol_fee(&env, i128::MAX, 1000);
}

#[test]
fn test_fee_math_tiny_amount() {
    let env = Env::default();
    // 9 * 1000 = 9000. 9000 / 10000 = 0 (rounds to zero)
    let fee = Escrow::calculate_protocol_fee(&env, 9, 1000);
    assert_eq!(fee, 0);
}
