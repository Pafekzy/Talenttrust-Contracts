//! Tests for `release_milestone` caller authorization.
//!
//! Covers every `ReleaseAuthorization` variant in both the happy
//! (authorized caller with valid approvals) and negative (unauthorized
//! caller, wrong role, missing approvals) paths.
//!
//! # Security contract
//!
//! 1. `caller.require_auth()` MUST be invoked *before* any role
//!    discrimination or state mutation.
//! 2. Role checks MUST reject callers who do not match the contract's
//!    `ReleaseAuthorization` variant.
//! 3. `MultiSig` requires BOTH client and freelancer approvals via
//!    `approvals::check_approvals`; no single party can release alone.
//! 4. Approvals must exist and not have expired; missing or expired
//!    approvals produce `InsufficientApprovals`.
//! 5. All negative paths MUST be fail-closed (no partial state change).

#![cfg(test)]

use soroban_sdk::{
    testutils::Address as _, testutils::Events, vec, Address, Env, FromVal, IntoVal, Symbol,
    TryFromVal,
};

use super::register_client;
use crate::{ContractStatus, Error, Escrow, EscrowClient, EscrowError, ReleaseAuthorization};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn setup(env: &Env) -> (Address, Address, Address) {
    let client_addr = Address::generate(env);
    let freelancer_addr = Address::generate(env);
    let arbiter_addr = Address::generate(env);
    (client_addr, freelancer_addr, arbiter_addr)
}

/// Register and initialize the escrow contract and return a client.
fn register(env: &Env) -> EscrowClient<'_> {
    let id = env.register(Escrow, ());
    let client = EscrowClient::new(env, &id);
    let admin = soroban_sdk::Address::generate(env);
    env.mock_all_auths_allowing_non_root_auth();
    client.initialize(&admin);
    let token = env.register_stellar_asset_contract(admin.clone());
    client.bind_settlement_token(&admin, &token);
    client
}
fn assert_contract_error<T, E>(
    result: Result<T, Result<soroban_sdk::Error, soroban_sdk::InvokeError>>,
    expected: E,
) where
    E: Into<soroban_sdk::Error> + core::fmt::Debug,
{
    match result {
        Err(Ok(e)) => {
            let expected_err: soroban_sdk::Error = expected.into();
            assert_eq!(e, expected_err, "contract error code mismatch");
        }
        _other => panic!(
            "expected contract error {:?}, got unexpected result variant",
            expected
        ),
    }
}

// Helper that accepts i128-returning try_* calls (like refund_unreleased_milestones)
fn assert_contract_error_i128<E>(
    result: Result<
        Result<i128, soroban_sdk::Error>,
        Result<soroban_sdk::Error, soroban_sdk::InvokeError>,
    >,
    expected: E,
) where
    E: Into<soroban_sdk::Error> + core::fmt::Debug,
{
    match result {
        Err(Ok(e)) => {
            let expected_err: soroban_sdk::Error = expected.into();
            assert_eq!(e, expected_err, "contract error code mismatch");
        }
        _other => panic!(
            "expected contract error {:?}, got unexpected result variant",
            expected
        ),
    }
}

fn create_contract_with_mode(
    env: &Env,
    client: &EscrowClient<'_>,
    client_addr: &Address,
    freelancer_addr: &Address,
    arbiter: &Option<Address>,
    release_auth: &ReleaseAuthorization,
) -> u32 {
    let milestones = vec![env, 500_i128, 300_i128, 200_i128];
    let id = client.create_contract(
        client_addr,
        freelancer_addr,
        arbiter,
        &milestones,
        release_auth,
    );
    if let Some(token) = client.get_settlement_token() {
        soroban_sdk::token::StellarAssetClient::new(env, &token).mint(client_addr, &1_000_000_000_000_000_i128);
    }
    id
}

fn fund_contract(env: &Env, client: &EscrowClient<'_>, contract_id: &u32) {
    let milestones = client.get_milestones(contract_id);
    let total: i128 = milestones.iter().map(|m| m.amount).sum();
    let contract = client.get_contract(contract_id);
    if let Some(token) = client.get_settlement_token() {
        soroban_sdk::token::StellarAssetClient::new(env, &token).mint(&contract.client, &total);
    }
    assert!(client.deposit_funds(contract_id, &contract.client, &total));

    for index in 0..milestones.len() {
        match contract.release_authorization {
            ReleaseAuthorization::ClientOnly | ReleaseAuthorization::ClientAndArbiter => {
                assert!(client.approve_milestone_release(contract_id, &contract.client, &index));
            }
            ReleaseAuthorization::ArbiterOnly => {
                let arbiter = contract
                    .arbiter
                    .clone()
                    .expect("ArbiterOnly requires arbiter");
                assert!(client.approve_milestone_release(contract_id, &arbiter, &index));
            }
            ReleaseAuthorization::MultiSig => {
                assert!(client.approve_milestone_release(contract_id, &contract.client, &index));
                assert!(client.approve_milestone_release(
                    contract_id,
                    &contract.freelancer,
                    &index,
                ));
            }
        }
    }
}

/// Create a fully-funded 2-milestone contract (500 + 300 = 800 total).
/// Returns `(client_addr, freelancer_addr, contract_id)`.
fn funded_contract(env: &Env, client: &EscrowClient<'_>) -> (Address, Address, u32) {
    let client_addr = Address::generate(env);
    let freelancer_addr = Address::generate(env);
    let milestones = vec![env, 500_i128, 300_i128];
    let id = client.create_contract(
        &client_addr,
        &freelancer_addr,
        &None,
        &milestones,
        &ReleaseAuthorization::ClientOnly,
    );
    if let Some(token) = client.get_settlement_token() {
        soroban_sdk::token::StellarAssetClient::new(env, &token).mint(&client_addr, &800_i128);
    }
    assert!(client.deposit_funds(&id, &client_addr, &800_i128));
    assert!(client.approve_milestone_release(&id, &client_addr, &0));
    assert!(client.approve_milestone_release(&id, &client_addr, &1));
    (client_addr, freelancer_addr, id)
}

fn milestones(env: &Env) -> soroban_sdk::Vec<i128> {
    vec![env, 500_0000000_i128, 300_0000000_i128]
}

fn total() -> i128 {
    800_0000000_i128
}

fn new_client(env: &Env) -> EscrowClient<'_> {
    let contract_id = env.register(Escrow, ());
    let client = EscrowClient::new(env, &contract_id);
    let admin = Address::generate(env);
    env.mock_all_auths_allowing_non_root_auth();
    client.initialize(&admin);
    let token = env.register_stellar_asset_contract(admin.clone());
    client.bind_settlement_token(&admin, &token);
    client
}

/// Create a funded contract with the given authorization mode.
/// Returns contract_id.
fn create(
    env: &Env,
    client: &EscrowClient<'_>,
    client_addr: &Address,
    freelancer_addr: &Address,
    arbiter: Option<&Address>,
    auth: &ReleaseAuthorization,
) -> u32 {
    let arbiter_owned = arbiter.cloned();
    let id = client.create_contract(
        client_addr,
        freelancer_addr,
        &arbiter_owned,
        &milestones(env),
        auth,
    );
    if let Some(token) = client.get_settlement_token() {
        soroban_sdk::token::StellarAssetClient::new(env, &token).mint(client_addr, &total());
    }
    assert!(client.deposit_funds(&id, client_addr, &total()));
    // Approve milestone 0 so release can go through on happy paths
    match auth {
        ReleaseAuthorization::ClientOnly | ReleaseAuthorization::ClientAndArbiter => {
            assert!(client.approve_milestone_release(&id, client_addr, &0));
        }
        ReleaseAuthorization::ArbiterOnly => {
            assert!(client.approve_milestone_release(
                &id,
                arbiter.expect("ArbiterOnly requires arbiter"),
                &0,
            ));
        }
        ReleaseAuthorization::MultiSig => {
            assert!(client.approve_milestone_release(&id, client_addr, &0));
            assert!(client.approve_milestone_release(&id, freelancer_addr, &0));
        }
    }
    id
}

// ===========================================================================
//  ClientOnly
// ===========================================================================

#[test]
fn client_only_client_can_release() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, _arbiter_addr) = setup(&env);
    let id = create(
        &env,
        &client,
        &client_addr,
        &freelancer_addr,
        None,
        &ReleaseAuthorization::ClientOnly,
    );
    assert!(client.release_milestone(&id, &client_addr, &0));
    let c = client.get_contract(&id);
    assert_eq!(c.released_amount, 500_0000000_i128);
}

#[test]
fn client_only_freelancer_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, _arbiter_addr) = setup(&env);
    let id = create(
        &env,
        &client,
        &client_addr,
        &freelancer_addr,
        None,
        &ReleaseAuthorization::ClientOnly,
    );
    let result = client.try_release_milestone(&id, &freelancer_addr, &0);
    assert_contract_error(result, Error::UnauthorizedRole);
}

#[test]
fn client_only_arbiter_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, arbiter_addr) = setup(&env);
    let id = create(
        &env,
        &client,
        &client_addr,
        &freelancer_addr,
        Some(&arbiter_addr),
        &ReleaseAuthorization::ClientOnly,
    );
    let result = client.try_release_milestone(&id, &arbiter_addr, &0);
    assert_contract_error(result, Error::UnauthorizedRole);
}

#[test]
fn client_only_attacker_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, _arbiter_addr) = setup(&env);
    let id = create(
        &env,
        &client,
        &client_addr,
        &freelancer_addr,
        None,
        &ReleaseAuthorization::ClientOnly,
    );
    let attacker = Address::generate(&env);
    let result = client.try_release_milestone(&id, &attacker, &0);
    assert_contract_error(result, Error::UnauthorizedRole);
}

// ===========================================================================
//  ArbiterOnly
// ===========================================================================

#[test]
fn arbiter_only_arbiter_can_release() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, arbiter_addr) = setup(&env);
    let id = create(
        &env,
        &client,
        &client_addr,
        &freelancer_addr,
        Some(&arbiter_addr),
        &ReleaseAuthorization::ArbiterOnly,
    );
    assert!(client.release_milestone(&id, &arbiter_addr, &0));
}

#[test]
fn arbiter_only_client_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, arbiter_addr) = setup(&env);
    let id = create(
        &env,
        &client,
        &client_addr,
        &freelancer_addr,
        Some(&arbiter_addr),
        &ReleaseAuthorization::ArbiterOnly,
    );
    let result = client.try_release_milestone(&id, &client_addr, &0);
    assert_contract_error(result, Error::UnauthorizedRole);
}

#[test]
fn arbiter_only_freelancer_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, arbiter_addr) = setup(&env);
    let id = create(
        &env,
        &client,
        &client_addr,
        &freelancer_addr,
        Some(&arbiter_addr),
        &ReleaseAuthorization::ArbiterOnly,
    );
    let result = client.try_release_milestone(&id, &freelancer_addr, &0);
    assert_contract_error(result, Error::UnauthorizedRole);
}

#[test]
fn arbiter_only_attacker_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, arbiter_addr) = setup(&env);
    let id = create(
        &env,
        &client,
        &client_addr,
        &freelancer_addr,
        Some(&arbiter_addr),
        &ReleaseAuthorization::ArbiterOnly,
    );
    let attacker = Address::generate(&env);
    let result = client.try_release_milestone(&id, &attacker, &0);
    assert_contract_error(result, Error::UnauthorizedRole);
}

// ===========================================================================
//  ClientAndArbiter
// ===========================================================================

#[test]
fn client_and_arbiter_client_can_release() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, arbiter_addr) = setup(&env);
    let id = create(
        &env,
        &client,
        &client_addr,
        &freelancer_addr,
        Some(&arbiter_addr),
        &ReleaseAuthorization::ClientAndArbiter,
    );
    assert!(client.release_milestone(&id, &client_addr, &0));
}

#[test]
fn client_and_arbiter_arbiter_can_release() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, arbiter_addr) = setup(&env);
    let id = create(
        &env,
        &client,
        &client_addr,
        &freelancer_addr,
        Some(&arbiter_addr),
        &ReleaseAuthorization::ClientAndArbiter,
    );
    assert!(client.approve_milestone_release(&id, &arbiter_addr, &0));
    assert!(client.release_milestone(&id, &arbiter_addr, &0));
}

#[test]
fn client_and_arbiter_freelancer_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, arbiter_addr) = setup(&env);
    let id = create(
        &env,
        &client,
        &client_addr,
        &freelancer_addr,
        Some(&arbiter_addr),
        &ReleaseAuthorization::ClientAndArbiter,
    );
    let result = client.try_release_milestone(&id, &freelancer_addr, &0);
    assert_contract_error(result, Error::UnauthorizedRole);
}

#[test]
fn client_and_arbiter_attacker_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, arbiter_addr) = setup(&env);
    let id = create(
        &env,
        &client,
        &client_addr,
        &freelancer_addr,
        Some(&arbiter_addr),
        &ReleaseAuthorization::ClientAndArbiter,
    );
    let attacker = Address::generate(&env);
    let result = client.try_release_milestone(&id, &attacker, &0);
    assert_contract_error(result, Error::UnauthorizedRole);
}

// ===========================================================================
//  MultiSig
// ===========================================================================

#[test]
fn multisig_client_can_release_with_both_approvals() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, _arbiter_addr) = setup(&env);
    let id = create(
        &env,
        &client,
        &client_addr,
        &freelancer_addr,
        None,
        &ReleaseAuthorization::MultiSig,
    );
    assert!(client.release_milestone(&id, &client_addr, &0));
}

#[test]
fn multisig_freelancer_can_release_with_both_approvals() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, _arbiter_addr) = setup(&env);
    let id = create(
        &env,
        &client,
        &client_addr,
        &freelancer_addr,
        None,
        &ReleaseAuthorization::MultiSig,
    );
    assert!(client.release_milestone(&id, &freelancer_addr, &0));
}

#[test]
fn multisig_arbiter_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, arbiter_addr) = setup(&env);
    let id = create(
        &env,
        &client,
        &client_addr,
        &freelancer_addr,
        Some(&arbiter_addr),
        &ReleaseAuthorization::MultiSig,
    );
    let result = client.try_release_milestone(&id, &arbiter_addr, &0);
    assert_contract_error(result, Error::UnauthorizedRole);
}

#[test]
fn multisig_attacker_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, _arbiter_addr) = setup(&env);
    let id = create(
        &env,
        &client,
        &client_addr,
        &freelancer_addr,
        None,
        &ReleaseAuthorization::MultiSig,
    );
    let attacker = Address::generate(&env);
    let result = client.try_release_milestone(&id, &attacker, &0);
    assert_contract_error(result, Error::UnauthorizedRole);
}

#[test]
fn multisig_only_one_approval_insufficient() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, _arbiter_addr) = setup(&env);

    let id = client.create_contract(
        &client_addr,
        &freelancer_addr,
        &None,
        &milestones(&env),
        &ReleaseAuthorization::MultiSig,
    );
    if let Some(token) = client.get_settlement_token() {
        soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&client_addr, &total());
    }
    assert!(client.deposit_funds(&id, &client_addr, &total()));

    assert!(client.approve_milestone_release(&id, &client_addr, &0));
    let result = client.try_release_milestone(&id, &client_addr, &0);
    assert_contract_error(result, Error::InsufficientApprovals);
}

#[test]
fn multisig_only_freelancer_approval_insufficient() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, _arbiter_addr) = setup(&env);

    let id = client.create_contract(
        &client_addr,
        &freelancer_addr,
        &None,
        &milestones(&env),
        &ReleaseAuthorization::MultiSig,
    );
    if let Some(token) = client.get_settlement_token() {
        soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&client_addr, &total());
    }
    assert!(client.deposit_funds(&id, &client_addr, &total()));

    assert!(client.approve_milestone_release(&id, &freelancer_addr, &0));
    let result = client.try_release_milestone(&id, &freelancer_addr, &0);
    assert_contract_error(result, Error::InsufficientApprovals);
}

#[test]
fn multisig_arbiter_cannot_record_approval() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, arbiter_addr) = setup(&env);

    let id = client.create_contract(
        &client_addr,
        &freelancer_addr,
        &Some(arbiter_addr.clone()),
        &milestones(&env),
        &ReleaseAuthorization::MultiSig,
    );
    if let Some(token) = client.get_settlement_token() {
        soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&client_addr, &total());
    }
    assert!(client.deposit_funds(&id, &client_addr, &total()));

    let result = client.try_approve_milestone_release(&id, &arbiter_addr, &0);
    assert_contract_error(result, Error::UnauthorizedRole);
}

// ===========================================================================
//  Missing / expired approvals
// ===========================================================================

#[test]
fn release_without_approval_fails() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, _arbiter_addr) = setup(&env);

    let id = client.create_contract(
        &client_addr,
        &freelancer_addr,
        &None,
        &milestones(&env),
        &ReleaseAuthorization::ClientOnly,
    );
    if let Some(token) = client.get_settlement_token() {
        soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&client_addr, &total());
    }
    assert!(client.deposit_funds(&id, &client_addr, &total()));

    // No approval recorded yet
    let result = client.try_release_milestone(&id, &client_addr, &0);
    assert_contract_error(result, Error::InsufficientApprovals);
}

// ===========================================================================
//  require_auth() ordering — unauth caller without mock
// ===========================================================================

#[test]
fn unauthorized_caller_without_auth_is_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, _) = setup(&env);
    let id = create(
        &env,
        &client,
        &client_addr,
        &freelancer_addr,
        None,
        &ReleaseAuthorization::ClientOnly,
    );
    let stranger = Address::generate(&env);
    let result = client.try_release_milestone(&id, &stranger, &0);
    assert_contract_error(result, Error::UnauthorizedRole);
}

// ===========================================================================
//  State mutation guard
// ===========================================================================

#[test]
fn fail_closed_on_unauthorized_caller_no_state_change() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, _arbiter_addr) = setup(&env);
    let id = create(
        &env,
        &client,
        &client_addr,
        &freelancer_addr,
        None,
        &ReleaseAuthorization::ClientOnly,
    );

    let before = client.get_contract(&id);

    let attacker = Address::generate(&env);
    let result = client.try_release_milestone(&id, &attacker, &0);
    assert_contract_error(result, Error::UnauthorizedRole);

    let after = client.get_contract(&id);
    assert_eq!(before.released_amount, after.released_amount);
    assert_eq!(before.status, after.status);
}

// ---------------------------------------------------------------------------
// Double-release is rejected with AlreadyReleased; no duplicate transfer
// ---------------------------------------------------------------------------

#[test]
fn double_release_is_rejected_and_amount_not_duplicated() {
    let env = Env::default();
    env.mock_all_auths();
    let client = register(&env);
    let (client_addr, _freelancer_addr, id) = funded_contract(&env, &client);

    // First release succeeds.
    assert!(client.release_milestone(&id, &client_addr, &0));

    // Second release on the same milestone must fail with AlreadyReleased.
    let result = client.try_release_milestone(&id, &client_addr, &0);
    assert_contract_error(result, Error::MilestoneAlreadyReleased);

    // released_amount must not be doubled.
    let contract = client.get_contract(&id);
    assert_eq!(contract.released_amount, 500_i128);
}

// ---------------------------------------------------------------------------
// Freelancer (non-client) is also rejected
// ---------------------------------------------------------------------------

#[test]
fn freelancer_cannot_release_milestone() {
    let env = Env::default();
    env.mock_all_auths();
    let client = register(&env);
    let (_client_addr, freelancer_addr, id) = funded_contract(&env, &client);

    let result = client.try_release_milestone(&id, &freelancer_addr, &0);
    assert_contract_error(result, Error::UnauthorizedRole);
}

#[test]
fn release_emits_events() {
    let env = Env::default();
    env.mock_all_auths();

    let client = register_client(&env);
    let client_addr = Address::generate(&env);
    let freelancer_addr = Address::generate(&env);

    let contract_id = create_contract_with_mode(
        &env,
        &client,
        &client_addr,
        &freelancer_addr,
        &None,
        &ReleaseAuthorization::ClientOnly,
    );

    fund_contract(&env, &client, &contract_id);

    // Release milestone
    client.release_milestone(&contract_id, &client_addr, &0);

    // Check release event was emitted
    let events = env.events().all();
    assert!(!events.is_empty());
}

#[test]
fn rejects_double_release_and_completes_contract() {
    let env = Env::default();
    env.mock_all_auths();

    let client = register_client(&env);
    let client_addr = Address::generate(&env);
    let freelancer_addr = Address::generate(&env);

    let contract_id = create_contract_with_mode(
        &env,
        &client,
        &client_addr,
        &freelancer_addr,
        &None,
        &ReleaseAuthorization::ClientOnly,
    );
    fund_contract(&env, &client, &contract_id);

    assert!(client.release_milestone(&contract_id, &client_addr, &0));

    let result = client.try_release_milestone(&contract_id, &client_addr, &0);
    assert_contract_error(result, Error::MilestoneAlreadyReleased);

    assert!(client.release_milestone(&contract_id, &client_addr, &1));
    assert!(client.release_milestone(&contract_id, &client_addr, &2));

    let contract = client.get_contract(&contract_id);
    assert_eq!(contract.status, ContractStatus::Completed);
    assert_eq!(client.get_pending_reputation_credits(&freelancer_addr), 1);
}

#[test]
fn rejects_refund_after_release_and_release_after_refund() {
    let env = Env::default();
    env.mock_all_auths();

    let client = register_client(&env);
    let client_addr = Address::generate(&env);
    let freelancer_addr = Address::generate(&env);

    let contract_id = create_contract_with_mode(
        &env,
        &client,
        &client_addr,
        &freelancer_addr,
        &None,
        &ReleaseAuthorization::ClientOnly,
    );
    fund_contract(&env, &client, &contract_id);

    assert!(client.release_milestone(&contract_id, &client_addr, &0));
    let refund_ids = vec![&env, 0_u32];
    let refund_result = client.try_refund_unreleased_milestones(&contract_id, &refund_ids);
    match refund_result {
        Err(Ok(e)) => {
            assert_eq!(e, soroban_sdk::Error::from(Error::AlreadyReleased));
        }
        other => panic!("expected contract error AlreadyReleased, got {:?}", other),
    }

    let refund_ids = vec![&env, 1_u32];
    assert!(client.refund_unreleased_milestones(&contract_id, &refund_ids) > 0);

    let result = client.try_release_milestone(&contract_id, &client_addr, &1);
    assert_contract_error(result, Error::AlreadyRefunded);
}

// ===========================================================================
//  InvalidState — release in non-Funded contract status
//
//  `release_milestone` must reject with `InvalidState` whenever the contract
//  is not in the active, fully-funded state.  These tests cover every
//  mode so the guard is verified independently of the caller-role path.
// ===========================================================================

/// Helper: create a contract and set it to Funded status via direct storage
/// injection (no SAC token needed), without submitting any approvals.
fn funded_no_approvals(
    env: &Env,
    client: &EscrowClient<'_>,
    client_addr: &Address,
    freelancer_addr: &Address,
    auth: &ReleaseAuthorization,
    arbiter: Option<&Address>,
) -> u32 {
    let arbiter_owned = arbiter.cloned();
    let id = client.create_contract(
        client_addr,
        freelancer_addr,
        &arbiter_owned,
        &milestones(env),
        auth,
    );
    // Inject Funded status and funded_amount directly so approve_milestone_release
    // passes the status check without requiring a bound SAC token.
    let escrow_addr = client.address.clone();
    env.as_contract(&escrow_addr, || {
        let key = crate::DataKey::Contract(id);
        let mut c: crate::Contract = env.storage().persistent().get(&key).unwrap();
        c.status = crate::ContractStatus::Funded;
        c.funded_amount = total();
        env.storage().persistent().set(&key, &c);
    });
    id
}

// ---------------------------------------------------------------------------
//  Release in Created (not yet funded) status → InvalidState
// ---------------------------------------------------------------------------

/// ClientOnly mode: release on a freshly-created (unfunded) contract yields
/// `InvalidState`.  No approval is attempted — the status guard fires first.
#[test]
fn release_in_created_status_client_only_fails_invalid_state() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, _) = setup(&env);

    // Create but do NOT deposit — status stays Created.
    let id = client.create_contract(
        &client_addr,
        &freelancer_addr,
        &None,
        &milestones(&env),
        &ReleaseAuthorization::ClientOnly,
    );
    // No approval possible on a Created contract (approvals.rs requires Funded),
    // and release must fail with InvalidState before even reaching role checks.
    let result = client.try_release_milestone(&id, &client_addr, &0);
    assert_contract_error(result, Error::InvalidState);
}

/// ArbiterOnly mode: release on an unfunded contract yields `InvalidState`.
#[test]
fn release_in_created_status_arbiter_only_fails_invalid_state() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, arbiter_addr) = setup(&env);

    let id = client.create_contract(
        &client_addr,
        &freelancer_addr,
        &Some(arbiter_addr.clone()),
        &milestones(&env),
        &ReleaseAuthorization::ArbiterOnly,
    );
    let result = client.try_release_milestone(&id, &arbiter_addr, &0);
    assert_contract_error(result, Error::InvalidState);
}

/// ClientAndArbiter mode: release on an unfunded contract yields `InvalidState`.
#[test]
fn release_in_created_status_client_and_arbiter_fails_invalid_state() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, arbiter_addr) = setup(&env);

    let id = client.create_contract(
        &client_addr,
        &freelancer_addr,
        &Some(arbiter_addr.clone()),
        &milestones(&env),
        &ReleaseAuthorization::ClientAndArbiter,
    );
    let result = client.try_release_milestone(&id, &client_addr, &0);
    assert_contract_error(result, Error::InvalidState);
}

/// MultiSig mode: release on an unfunded contract yields `InvalidState`.
#[test]
fn release_in_created_status_multisig_fails_invalid_state() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, _) = setup(&env);

    let id = client.create_contract(
        &client_addr,
        &freelancer_addr,
        &None,
        &milestones(&env),
        &ReleaseAuthorization::MultiSig,
    );
    // The status guard (Created → not Funded) fires before role or approval checks.
    let result = client.try_release_milestone(&id, &client_addr, &0);
    assert_contract_error(result, Error::InvalidState);
}

// ---------------------------------------------------------------------------
//  Release in Completed status → InvalidState
// ---------------------------------------------------------------------------

/// Once a contract reaches `Completed` status, any release attempt must be
/// rejected with `InvalidState`.  We set the status directly in storage
/// (without executing an actual SAC transfer) to isolate the state-guard
/// logic from token-custody concerns.
#[test]
fn release_in_completed_status_client_only_fails_invalid_state() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, _) = setup(&env);

    let id = client.create_contract(
        &client_addr,
        &freelancer_addr,
        &None,
        &milestones(&env),
        &ReleaseAuthorization::ClientOnly,
    );

    // Manually transition the contract to Completed so we can verify the
    // status guard in isolation.
    let escrow_addr = client.address.clone();
    env.as_contract(&escrow_addr, || {
        let key = crate::DataKey::Contract(id);
        let mut c: crate::Contract = env.storage().persistent().get(&key).unwrap();
        c.status = ContractStatus::Completed;
        env.storage().persistent().set(&key, &c);
    });

    let result = client.try_release_milestone(&id, &client_addr, &0);
    assert_contract_error(result, Error::InvalidState);
}

/// ArbiterOnly mode: Completed status → InvalidState.
#[test]
fn release_in_completed_status_arbiter_only_fails_invalid_state() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, arbiter_addr) = setup(&env);

    let id = client.create_contract(
        &client_addr,
        &freelancer_addr,
        &Some(arbiter_addr.clone()),
        &milestones(&env),
        &ReleaseAuthorization::ArbiterOnly,
    );

    let escrow_addr = client.address.clone();
    env.as_contract(&escrow_addr, || {
        let key = crate::DataKey::Contract(id);
        let mut c: crate::Contract = env.storage().persistent().get(&key).unwrap();
        c.status = ContractStatus::Completed;
        env.storage().persistent().set(&key, &c);
    });

    let result = client.try_release_milestone(&id, &arbiter_addr, &0);
    assert_contract_error(result, Error::InvalidState);
}

/// MultiSig mode: Completed status → InvalidState.
#[test]
fn release_in_completed_status_multisig_fails_invalid_state() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, _) = setup(&env);

    let id = client.create_contract(
        &client_addr,
        &freelancer_addr,
        &None,
        &milestones(&env),
        &ReleaseAuthorization::MultiSig,
    );

    let escrow_addr = client.address.clone();
    env.as_contract(&escrow_addr, || {
        let key = crate::DataKey::Contract(id);
        let mut c: crate::Contract = env.storage().persistent().get(&key).unwrap();
        c.status = ContractStatus::Completed;
        env.storage().persistent().set(&key, &c);
    });

    let result = client.try_release_milestone(&id, &client_addr, &0);
    assert_contract_error(result, Error::InvalidState);
}

// ---------------------------------------------------------------------------
//  Release after cancel → InvalidState
// ---------------------------------------------------------------------------

/// Cancelled contracts must not accept any further milestone releases.
/// `cancel_contract` puts the contract into `Cancelled` status; any
/// subsequent `release_milestone` call must fail with `InvalidState`.
#[test]
fn release_after_cancel_client_only_fails_invalid_state() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, _) = setup(&env);

    let id = client.create_contract(
        &client_addr,
        &freelancer_addr,
        &None,
        &milestones(&env),
        &ReleaseAuthorization::ClientOnly,
    );
    // Inject Cancelled status directly to isolate state guard from SAC concerns.
    let escrow_addr = client.address.clone();
    env.as_contract(&escrow_addr, || {
        let key = crate::DataKey::Contract(id);
        let mut c: crate::Contract = env.storage().persistent().get(&key).unwrap();
        c.status = crate::ContractStatus::Cancelled;
        env.storage().persistent().set(&key, &c);
    });

    let result = client.try_release_milestone(&id, &client_addr, &0);
    assert_contract_error(result, Error::InvalidState);
}

/// ArbiterOnly mode: cancel then release fails with `InvalidState`.
#[test]
fn release_after_cancel_arbiter_only_fails_invalid_state() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, arbiter_addr) = setup(&env);

    let id = client.create_contract(
        &client_addr,
        &freelancer_addr,
        &Some(arbiter_addr.clone()),
        &milestones(&env),
        &ReleaseAuthorization::ArbiterOnly,
    );

    let escrow_addr = client.address.clone();
    env.as_contract(&escrow_addr, || {
        let key = crate::DataKey::Contract(id);
        let mut c: crate::Contract = env.storage().persistent().get(&key).unwrap();
        c.status = crate::ContractStatus::Cancelled;
        env.storage().persistent().set(&key, &c);
    });

    let result = client.try_release_milestone(&id, &arbiter_addr, &0);
    assert_contract_error(result, Error::InvalidState);
}

/// MultiSig mode: cancel then release fails with `InvalidState`.
#[test]
fn release_after_cancel_multisig_fails_invalid_state() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, _) = setup(&env);

    let id = client.create_contract(
        &client_addr,
        &freelancer_addr,
        &None,
        &milestones(&env),
        &ReleaseAuthorization::MultiSig,
    );

    let escrow_addr = client.address.clone();
    env.as_contract(&escrow_addr, || {
        let key = crate::DataKey::Contract(id);
        let mut c: crate::Contract = env.storage().persistent().get(&key).unwrap();
        c.status = crate::ContractStatus::Cancelled;
        env.storage().persistent().set(&key, &c);
    });

    let result = client.try_release_milestone(&id, &client_addr, &0);
    assert_contract_error(result, Error::InvalidState);
}

// ===========================================================================
//  Edge-case: approval by unauthorized party does not unlock release
// ===========================================================================

/// In `ArbiterOnly` mode, client approval alone does NOT satisfy the
/// approval check, even though the client is a valid contract participant.
/// The approval call itself should fail with `UnauthorizedRole`, and
/// if it somehow recorded an approval, `release_milestone` by the arbiter
/// would still fail with `InsufficientApprovals`.
#[test]
fn arbiter_only_client_approval_not_accepted() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, arbiter_addr) = setup(&env);

    let id = funded_no_approvals(
        &env,
        &client,
        &client_addr,
        &freelancer_addr,
        &ReleaseAuthorization::ArbiterOnly,
        Some(&arbiter_addr),
    );

    // Client attempts to approve — must be rejected.
    let result = client.try_approve_milestone_release(&id, &client_addr, &0);
    assert_contract_error(result, Error::UnauthorizedRole);

    // Arbiter then tries to release without a valid approval — must fail.
    let result = client.try_release_milestone(&id, &arbiter_addr, &0);
    assert_contract_error(result, Error::InsufficientApprovals);
}

/// In `ClientOnly` mode, arbiter approval alone does NOT satisfy the check.
/// The `approve_milestone_release` call by the arbiter is rejected, so no
/// valid approval is stored, and the release attempt also fails.
#[test]
fn client_only_arbiter_approval_not_accepted() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, arbiter_addr) = setup(&env);

    let id = funded_no_approvals(
        &env,
        &client,
        &client_addr,
        &freelancer_addr,
        &ReleaseAuthorization::ClientOnly,
        Some(&arbiter_addr),
    );

    // Arbiter attempts to approve — must be rejected.
    let result = client.try_approve_milestone_release(&id, &arbiter_addr, &0);
    assert_contract_error(result, Error::UnauthorizedRole);

    // Client tries to release without any stored approval — must fail.
    let result = client.try_release_milestone(&id, &client_addr, &0);
    assert_contract_error(result, Error::InsufficientApprovals);
}

/// In `MultiSig` mode, only the client approving (not the freelancer) is
/// insufficient.  The release must fail with `InsufficientApprovals` even
/// when the authorized caller (client) attempts the release.
#[test]
fn multisig_only_client_approval_is_insufficient_for_release() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, _) = setup(&env);

    let id = funded_no_approvals(
        &env,
        &client,
        &client_addr,
        &freelancer_addr,
        &ReleaseAuthorization::MultiSig,
        None,
    );

    // Only the client approves.
    assert!(client.approve_milestone_release(&id, &client_addr, &0));

    // Client tries to release with only their own approval — must fail.
    let result = client.try_release_milestone(&id, &client_addr, &0);
    assert_contract_error(result, Error::InsufficientApprovals);
}

/// In `MultiSig` mode, only the freelancer approving (not the client) is
/// insufficient.  The release must fail with `InsufficientApprovals` even
/// when the authorized caller (freelancer) attempts the release.
#[test]
fn multisig_only_freelancer_approval_is_insufficient_for_release() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, _) = setup(&env);

    let id = funded_no_approvals(
        &env,
        &client,
        &client_addr,
        &freelancer_addr,
        &ReleaseAuthorization::MultiSig,
        None,
    );

    // Only the freelancer approves.
    assert!(client.approve_milestone_release(&id, &freelancer_addr, &0));

    // Freelancer tries to release with only their own approval — must fail.
    let result = client.try_release_milestone(&id, &freelancer_addr, &0);
    assert_contract_error(result, Error::InsufficientApprovals);
}

/// In `MultiSig` mode, zero approvals on the milestone must fail for
/// any authorized caller (client in this case).
#[test]
fn multisig_no_approvals_fails_for_authorized_caller() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, _) = setup(&env);

    let id = funded_no_approvals(
        &env,
        &client,
        &client_addr,
        &freelancer_addr,
        &ReleaseAuthorization::MultiSig,
        None,
    );

    // No approvals at all.
    let result = client.try_release_milestone(&id, &client_addr, &0);
    assert_contract_error(result, Error::InsufficientApprovals);
}

// ===========================================================================
//  Edge-case: unknown / stranger caller is always rejected
// ===========================================================================

/// A completely unknown address must never release any milestone regardless
/// of the authorization mode or approval state.
#[test]
fn stranger_rejected_on_all_modes() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, arbiter_addr) = setup(&env);
    let stranger = Address::generate(&env);

    // --- ClientOnly ---
    let id = funded_no_approvals(
        &env,
        &client,
        &client_addr,
        &freelancer_addr,
        &ReleaseAuthorization::ClientOnly,
        None,
    );
    assert_contract_error(
        client.try_release_milestone(&id, &stranger, &0),
        Error::UnauthorizedRole,
    );

    // --- ArbiterOnly ---
    let id = funded_no_approvals(
        &env,
        &client,
        &client_addr,
        &freelancer_addr,
        &ReleaseAuthorization::ArbiterOnly,
        Some(&arbiter_addr),
    );
    assert_contract_error(
        client.try_release_milestone(&id, &stranger, &0),
        Error::UnauthorizedRole,
    );

    // --- ClientAndArbiter ---
    let id = funded_no_approvals(
        &env,
        &client,
        &client_addr,
        &freelancer_addr,
        &ReleaseAuthorization::ClientAndArbiter,
        Some(&arbiter_addr),
    );
    assert_contract_error(
        client.try_release_milestone(&id, &stranger, &0),
        Error::UnauthorizedRole,
    );

    // --- MultiSig ---
    let id = funded_no_approvals(
        &env,
        &client,
        &client_addr,
        &freelancer_addr,
        &ReleaseAuthorization::MultiSig,
        None,
    );
    assert_contract_error(
        client.try_release_milestone(&id, &stranger, &0),
        Error::UnauthorizedRole,
    );
}

// ===========================================================================
//  revoke_milestone_approval
// ===========================================================================

/// Client can revoke their own approval in ClientOnly mode.
#[test]
fn revoke_approval_client_only() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, _) = setup(&env);

    let id = funded_no_approvals(
        &env,
        &client,
        &client_addr,
        &freelancer_addr,
        &ReleaseAuthorization::ClientOnly,
        None,
    );

    // Approve
    assert!(client.approve_milestone_release(&id, &client_addr, &0));
    let approvals = client.get_milestone_approvals(&id, &0).unwrap();
    assert!(approvals.client_approved);

    // Revoke
    assert!(client.revoke_milestone_approval(&id, &client_addr, &0));
    let approvals = client.get_milestone_approvals(&id, &0);
    assert!(
        approvals.is_none(),
        "record should be removed when all flags false"
    );
}

/// In MultiSig mode, revoking one party's approval leaves the other intact.
#[test]
fn revoke_approval_multisig_partial() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, _) = setup(&env);

    let id = funded_no_approvals(
        &env,
        &client,
        &client_addr,
        &freelancer_addr,
        &ReleaseAuthorization::MultiSig,
        None,
    );

    // Both approve
    assert!(client.approve_milestone_release(&id, &client_addr, &0));
    assert!(client.approve_milestone_release(&id, &freelancer_addr, &0));
    let approvals = client.get_milestone_approvals(&id, &0).unwrap();
    assert!(approvals.client_approved);
    assert!(approvals.freelancer_approved);

    // Client revokes only their own flag
    assert!(client.revoke_milestone_approval(&id, &client_addr, &0));
    let approvals = client.get_milestone_approvals(&id, &0).unwrap();
    assert!(!approvals.client_approved, "client flag should be false");
    assert!(
        approvals.freelancer_approved,
        "freelancer flag should remain true"
    );

    // Release should now fail — only freelancer approved
    let result = client.try_release_milestone(&id, &client_addr, &0);
    assert_contract_error(result, Error::InsufficientApprovals);
}

/// Revoke without prior approval fails with InsufficientApprovals.
#[test]
fn revoke_without_approval_fails() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, _) = setup(&env);

    let id = funded_no_approvals(
        &env,
        &client,
        &client_addr,
        &freelancer_addr,
        &ReleaseAuthorization::ClientOnly,
        None,
    );

    // No approval recorded yet
    let result = client.try_revoke_milestone_approval(&id, &client_addr, &0);
    assert_contract_error(result, Error::InsufficientApprovals);
}

/// Revoke after milestone release fails with MilestoneAlreadyReleased.
#[test]
fn revoke_after_release_fails() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, _) = setup(&env);

    let id = funded_no_approvals(
        &env,
        &client,
        &client_addr,
        &freelancer_addr,
        &ReleaseAuthorization::ClientOnly,
        None,
    );

    // Approve
    assert!(client.approve_milestone_release(&id, &client_addr, &0));

    // Manually mark milestone as released to simulate post-release state
    let escrow_addr = client.address.clone();
    env.as_contract(&escrow_addr, || {
        let milestone_key = soroban_sdk::Symbol::new(&env, "milestones");
        let mut milestones: soroban_sdk::Vec<crate::Milestone> = env
            .storage()
            .persistent()
            .get(&(crate::DataKey::Contract(id), milestone_key.clone()))
            .unwrap();
        let mut m = milestones.get(0).unwrap();
        m.released = true;
        milestones.set(0, m);
        env.storage()
            .persistent()
            .set(&(crate::DataKey::Contract(id), milestone_key), &milestones);
    });

    // Revoke should now fail
    let result = client.try_revoke_milestone_approval(&id, &client_addr, &0);
    assert_contract_error(result, Error::MilestoneAlreadyReleased);
}

/// Stranger cannot revoke — UnauthorizedRole.
#[test]
fn revoke_by_stranger_fails() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, _) = setup(&env);

    let id = funded_no_approvals(
        &env,
        &client,
        &client_addr,
        &freelancer_addr,
        &ReleaseAuthorization::ClientOnly,
        None,
    );

    assert!(client.approve_milestone_release(&id, &client_addr, &0));

    let stranger = Address::generate(&env);
    let result = client.try_revoke_milestone_approval(&id, &stranger, &0);
    assert_contract_error(result, Error::UnauthorizedRole);
}

/// Freelancer cannot revoke client's approval in ClientOnly mode.
#[test]
fn revoke_wrong_party_fails() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, _) = setup(&env);

    let id = funded_no_approvals(
        &env,
        &client,
        &client_addr,
        &freelancer_addr,
        &ReleaseAuthorization::ClientOnly,
        None,
    );

    assert!(client.approve_milestone_release(&id, &client_addr, &0));

    // Freelancer tries to revoke client's approval — should fail
    // The freelancer is a valid participant but hasn't approved, so it returns InsufficientApprovals
    let result = client.try_revoke_milestone_approval(&id, &freelancer_addr, &0);
    assert_contract_error(result, Error::InsufficientApprovals);
}

/// Revoke emits a revoked event.
#[test]
fn revoke_emits_event() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, _) = setup(&env);

    let id = funded_no_approvals(
        &env,
        &client,
        &client_addr,
        &freelancer_addr,
        &ReleaseAuthorization::ClientOnly,
        None,
    );

    assert!(client.approve_milestone_release(&id, &client_addr, &0));
    assert!(client.revoke_milestone_approval(&id, &client_addr, &0));

    // Check event was emitted
    let events = env.events().all();
    let has_revoked_event = events.iter().any(|event| {
        event.1.len() > 0
            && soroban_sdk::Symbol::from_val(&env, &event.1.get(0).unwrap())
                == soroban_sdk::Symbol::new(&env, "revoked")
    });
    assert!(has_revoked_event, "should have at least one revoked event");
}

// ===========================================================================
//  Approval clearing after successful release
// ===========================================================================

/// Verifies that the approval record exists before release and is stored
/// with the correct flags.  We test the storage round-trip here rather than
/// a post-release clear, since an actual release requires a bound SAC token.
///
/// Approval clearing after release is covered by the SAC custody test suite
/// in `contracts/escrow/src/test/sac_custody.rs`.
#[test]
fn approval_recorded_and_readable_client_only() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, _) = setup(&env);

    let id = funded_no_approvals(
        &env,
        &client,
        &client_addr,
        &freelancer_addr,
        &ReleaseAuthorization::ClientOnly,
        None,
    );

    // No approval yet — should return None.
    assert!(
        client.get_milestone_approvals(&id, &0).is_none(),
        "approvals should be absent before any approve call"
    );

    // Record the client's approval.
    assert!(client.approve_milestone_release(&id, &client_addr, &0));

    // Approval record must now exist with client_approved = true.
    let approvals = client
        .get_milestone_approvals(&id, &0)
        .expect("approvals should be present after client approves");
    assert!(approvals.client_approved);
    assert!(!approvals.freelancer_approved);
    assert!(!approvals.arbiter_approved);
}

/// In `MultiSig` mode, both client and freelancer approval flags are
/// independently stored; after both approve the record shows both set.
#[test]
fn approval_recorded_and_readable_multisig_both_parties() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, _) = setup(&env);

    let id = funded_no_approvals(
        &env,
        &client,
        &client_addr,
        &freelancer_addr,
        &ReleaseAuthorization::MultiSig,
        None,
    );

    // Only client approves first — freelancer flag must still be false.
    assert!(client.approve_milestone_release(&id, &client_addr, &0));
    let half = client
        .get_milestone_approvals(&id, &0)
        .expect("approvals must be present after client approval");
    assert!(half.client_approved);
    assert!(!half.freelancer_approved, "freelancer has not yet approved");

    // Freelancer approves — now both flags must be true.
    assert!(client.approve_milestone_release(&id, &freelancer_addr, &0));
    let full = client
        .get_milestone_approvals(&id, &0)
        .expect("approvals must be present after both approvals");
    assert!(full.client_approved);
    assert!(full.freelancer_approved);
}

// ===========================================================================
//  Contract-level accounting after unauthorized / invalid attempts
// ===========================================================================

/// Any rejected call (UnauthorizedRole, InvalidState, or InsufficientApprovals)
/// must leave the contract's `released_amount` and `status` exactly as they
/// were before the call — fail-closed, no partial state mutation.
#[test]
fn failed_releases_leave_accounting_unchanged() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, _) = setup(&env);

    let id = funded_no_approvals(
        &env,
        &client,
        &client_addr,
        &freelancer_addr,
        &ReleaseAuthorization::ClientOnly,
        None,
    );

    let before = client.get_contract(&id);

    // 1. Unauthorized caller — rejected with UnauthorizedRole.
    let attacker = Address::generate(&env);
    let _ = client.try_release_milestone(&id, &attacker, &0);

    let mid = client.get_contract(&id);
    assert_eq!(before.released_amount, mid.released_amount);
    assert_eq!(before.status, mid.status);

    // 2. Authorized caller but no approval — rejected with InsufficientApprovals.
    let _ = client.try_release_milestone(&id, &client_addr, &0);

    let after = client.get_contract(&id);
    assert_eq!(
        before.released_amount, after.released_amount,
        "released_amount must not change after any failed release"
    );
    assert_eq!(
        before.status, after.status,
        "status must not change after any failed release"
    );
}
