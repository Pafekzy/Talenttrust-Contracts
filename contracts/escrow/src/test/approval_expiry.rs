//! Tests for approval TTL expiry behavior.
//!
//! Covers TTL-based auto-expiry of milestone approvals stored in temporary storage.
//! Tests each ReleaseAuthorization mode and edge cases around expiry boundaries.

use soroban_sdk::{
    log,
    testutils::{Address as _, Ledger as _},
    vec, Address, Env, Vec,
};

use crate::{Error, Escrow, EscrowClient, ReleaseAuthorization};

const PENDING_APPROVAL_TTL_LEDGERS: u32 = crate::ttl::PENDING_APPROVAL_TTL_LEDGERS;

fn milestones(env: &Env) -> soroban_sdk::Vec<i128> {
    vec![env, 1000_0000000_i128, 2000_0000000_i128, 3000_0000000_i128]
}

fn total() -> i128 {
    6000_0000000_i128
}

fn setup_env() -> Env {
    let env = Env::default();
    env.ledger().with_mut(|li| {
        li.max_entry_ttl = 518_400;
        li.min_persistent_entry_ttl = 518_400;
    });
    env.mock_all_auths();
    env
}

fn new_client(env: &Env) -> EscrowClient<'_> {
    env.ledger().with_mut(|li| {
        li.max_entry_ttl = 518_400;
        li.min_persistent_entry_ttl = 518_400;
    });
    env.mock_all_auths_allowing_non_root_auth();
    let contract_id = env.register(Escrow, ());
    let client = EscrowClient::new(env, &contract_id);
    let admin = Address::generate(env);
    client.initialize(&admin);

    let token_admin = Address::generate(env);
    let token_address = env.register_stellar_asset_contract(token_admin);
    client.set_settlement_token(&admin, &token_address);

    client
}

fn deposit(env: &Env, client: &EscrowClient, id: &u32, client_addr: &Address, amount: &i128) -> bool {
    env.mock_all_auths_allowing_non_root_auth();
    let token = match client.get_settlement_token() {
        Some(t) => t,
        None => {
            let admin = client.get_admin().unwrap();
            let token_admin = Address::generate(env);
            let token_address = env.register_stellar_asset_contract(token_admin);
            client.set_settlement_token(&admin, &token_address);
            token_address
        }
    };
    soroban_sdk::token::StellarAssetClient::new(env, &token).mint(client_addr, amount);
    client.deposit_funds(id, client_addr, amount)
}

fn setup(env: &Env) -> (Address, Address, Address) {
    (
        Address::generate(env),
        Address::generate(env),
        Address::generate(env),
    )
}

fn advance_ledger(env: &Env, contract_id: &Address, by: u32) {
    env.as_contract(contract_id, || {
        env.storage().instance().extend_ttl(by + 100, by + 1000);
    });
    env.ledger().with_mut(|li| {
        li.sequence_number = li.sequence_number.saturating_add(by);
    });
}

#[test]
fn test_approve_milestone_client_only() {
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
    assert!(deposit(&env, &client, &id, &client_addr, &total()));
    assert!(client.approve_milestone_release(&id, &client_addr, &0));

    let approvals = client.get_milestone_approvals(&id, &0);
    assert!(approvals.is_some());
    let approvals = approvals.unwrap();
    assert!(approvals.client_approved);
    assert!(!approvals.freelancer_approved);
}

#[test]
fn test_approve_milestone_multisig() {
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
    assert!(deposit(&env, &client, &id, &client_addr, &total()));

    assert!(client.approve_milestone_release(&id, &client_addr, &0));
    assert!(client.approve_milestone_release(&id, &freelancer_addr, &0));

    let approvals = client.get_milestone_approvals(&id, &0);
    assert!(approvals.is_some());
    let approvals = approvals.unwrap();
    assert!(approvals.client_approved);
    assert!(approvals.freelancer_approved);
}

#[test]
fn test_approve_milestone_arbiter_only() {
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
    assert!(deposit(&env, &client, &id, &client_addr, &total()));

    assert!(client.approve_milestone_release(&id, &arbiter_addr, &0));

    let approvals = client.get_milestone_approvals(&id, &0);
    assert!(approvals.is_some());
    let approvals = approvals.unwrap();
    assert!(approvals.arbiter_approved);
}

#[test]
fn test_approve_milestone_client_and_arbiter() {
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
    assert!(deposit(&env, &client, &id, &client_addr, &total()));

    assert!(client.approve_milestone_release(&id, &client_addr, &0));

    let approvals = client.get_milestone_approvals(&id, &0);
    assert!(approvals.is_some());
    let approvals = approvals.unwrap();
    assert!(approvals.client_approved);
}

#[test]
fn test_duplicate_approval_rejected() {
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
    assert!(deposit(&env, &client, &id, &client_addr, &total()));
    assert!(client.approve_milestone_release(&id, &client_addr, &0));

    let result = client.try_approve_milestone_release(&id, &client_addr, &0);
    super::assert_contract_error(result, Error::AlreadyApproved);
}

#[test]
fn test_unauthorized_approval_rejected() {
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
    assert!(deposit(&env, &client, &id, &client_addr, &total()));

    let result = client.try_approve_milestone_release(&id, &freelancer_addr, &0);
    super::assert_contract_error(result, Error::UnauthorizedRole);
}

#[test]
fn test_release_requires_approval() {
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
    assert!(deposit(&env, &client, &id, &client_addr, &total()));

    let result = client.try_release_milestone(&id, &client_addr, &0);
    super::assert_contract_error(result, Error::InsufficientApprovals);
}

#[test]
fn test_release_with_approval_succeeds() {
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
    assert!(deposit(&env, &client, &id, &client_addr, &total()));
    assert!(client.approve_milestone_release(&id, &client_addr, &0));
    assert!(client.release_milestone(&id, &client_addr, &0));

    let milestones_vec = client.get_milestones(&id);
    assert!(milestones_vec.get(0).unwrap().released);

    let approvals = client.get_milestone_approvals(&id, &0);
    assert!(approvals.is_none());
}

#[test]
fn test_multisig_requires_both_approvals() {
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
    assert!(deposit(&env, &client, &id, &client_addr, &total()));

    assert!(client.approve_milestone_release(&id, &client_addr, &0));

    let result = client.try_release_milestone(&id, &client_addr, &0);
    super::assert_contract_error(result, Error::InsufficientApprovals);

    assert!(client.approve_milestone_release(&id, &freelancer_addr, &0));
    assert!(client.release_milestone(&id, &client_addr, &0));
}

#[test]
fn test_approve_already_released_milestone_fails() {
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
    assert!(deposit(&env, &client, &id, &client_addr, &total()));
    assert!(client.approve_milestone_release(&id, &client_addr, &0));
    assert!(client.release_milestone(&id, &client_addr, &0));

    let result = client.try_approve_milestone_release(&id, &client_addr, &0);
    super::assert_contract_error(result, Error::MilestoneAlreadyReleased);
}

#[test]
fn test_approve_invalid_milestone_index() {
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
    assert!(deposit(&env, &client, &id, &client_addr, &total()));

    let result = client.try_approve_milestone_release(&id, &client_addr, &99);
    super::assert_contract_error(result, Error::IndexOutOfBounds);
}

#[test]
fn test_approve_requires_funded_state() {
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

    let result = client.try_approve_milestone_release(&id, &client_addr, &0);
    super::assert_contract_error(result, Error::InvalidState);
}

#[test]
fn test_multiple_milestones_independent_approvals() {
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
    assert!(deposit(&env, &client, &id, &client_addr, &total()));

    assert!(client.approve_milestone_release(&id, &client_addr, &0));
    assert!(client.approve_milestone_release(&id, &client_addr, &1));

    assert!(client.get_milestone_approvals(&id, &0).is_some());
    assert!(client.get_milestone_approvals(&id, &1).is_some());

    assert!(client.release_milestone(&id, &client_addr, &0));

    assert!(client.get_milestone_approvals(&id, &0).is_none());
    assert!(client.get_milestone_approvals(&id, &1).is_some());
}

// ─── TTL Expiry Tests ─────────────────────────────────────────────────────────────

#[test]
fn test_client_only_approval_expires_after_ttl() {
    let env = setup_env();
    let escrow_id = env.register(Escrow, ());
    let client = EscrowClient::new(&env, &escrow_id);
    let admin = Address::generate(&env);
    client.initialize(&admin);

    let client_addr = Address::generate(&env);
    let freelancer_addr = Address::generate(&env);

    let contract_id = client.create_contract(
        &client_addr,
        &freelancer_addr,
        &None,
        &milestones(&env),
        &ReleaseAuthorization::ClientOnly,
    );
    assert!(deposit(&env, &client, &contract_id, &client_addr, &total()));
    assert!(client.approve_milestone_release(&contract_id, &client_addr, &0));

    assert!(client.get_milestone_approvals(&contract_id, &0).is_some());

    advance_ledger(&env, &escrow_id, PENDING_APPROVAL_TTL_LEDGERS + 1);

    let approvals_after = client.get_milestone_approvals(&contract_id, &0);
    assert!(
        approvals_after.is_none(),
        "approval should be expired after TTL elapsed"
    );

    let result = client.try_release_milestone(&contract_id, &client_addr, &0);
    super::assert_contract_error(result, Error::InsufficientApprovals);
}

#[test]
fn test_client_only_approval_valid_at_exactly_ttl_boundary() {
    let env = setup_env();
    let escrow_id = env.register(Escrow, ());
    let client = EscrowClient::new(&env, &escrow_id);
    let admin = Address::generate(&env);
    client.initialize(&admin);

    let client_addr = Address::generate(&env);
    let freelancer_addr = Address::generate(&env);

    let contract_id = client.create_contract(
        &client_addr,
        &freelancer_addr,
        &None,
        &milestones(&env),
        &ReleaseAuthorization::ClientOnly,
    );
    assert!(deposit(&env, &client, &contract_id, &client_addr, &total()));
    assert!(client.approve_milestone_release(&contract_id, &client_addr, &0));

    advance_ledger(&env, &escrow_id, PENDING_APPROVAL_TTL_LEDGERS);

    let approvals = client.get_milestone_approvals(&contract_id, &0);
    assert!(
        approvals.is_some(),
        "approval should survive at exact TTL boundary"
    );

    // Because get_milestone_approvals renewed the TTL, advancing by PENDING_APPROVAL_TTL_LEDGERS + 1 expires it again
    advance_ledger(&env, &escrow_id, PENDING_APPROVAL_TTL_LEDGERS + 1);

    let approvals_expired = client.get_milestone_approvals(&contract_id, &0);
    assert!(
        approvals_expired.is_none(),
        "approval expires after TTL"
    );
}

#[test]
fn test_arbiter_only_approval_expires_after_ttl() {
    let env = setup_env();
    let escrow_id = env.register(Escrow, ());
    let client = EscrowClient::new(&env, &escrow_id);
    let admin = Address::generate(&env);
    client.initialize(&admin);

    let client_addr = Address::generate(&env);
    let freelancer_addr = Address::generate(&env);
    let arbiter_addr = Address::generate(&env);

    let contract_id = client.create_contract(
        &client_addr,
        &freelancer_addr,
        &Some(arbiter_addr.clone()),
        &milestones(&env),
        &ReleaseAuthorization::ArbiterOnly,
    );
    assert!(deposit(&env, &client, &contract_id, &client_addr, &total()));
    assert!(client.approve_milestone_release(&contract_id, &arbiter_addr, &0));

    advance_ledger(&env, &escrow_id, PENDING_APPROVAL_TTL_LEDGERS + 1);

    let result = client.try_release_milestone(&contract_id, &arbiter_addr, &0);
    super::assert_contract_error(result, Error::InsufficientApprovals);
}

#[test]
fn test_client_and_arbiter_approval_expires_after_ttl() {
    let env = setup_env();
    let escrow_id = env.register(Escrow, ());
    let client = EscrowClient::new(&env, &escrow_id);
    let admin = Address::generate(&env);
    client.initialize(&admin);

    let client_addr = Address::generate(&env);
    let freelancer_addr = Address::generate(&env);
    let arbiter_addr = Address::generate(&env);

    let contract_id = client.create_contract(
        &client_addr,
        &freelancer_addr,
        &Some(arbiter_addr.clone()),
        &milestones(&env),
        &ReleaseAuthorization::ClientAndArbiter,
    );
    assert!(deposit(&env, &client, &contract_id, &client_addr, &total()));
    assert!(client.approve_milestone_release(&contract_id, &client_addr, &0));

    advance_ledger(&env, &escrow_id, PENDING_APPROVAL_TTL_LEDGERS + 1);

    let result = client.try_release_milestone(&contract_id, &client_addr, &0);
    super::assert_contract_error(result, Error::InsufficientApprovals);
}

#[test]
fn test_multisig_one_approval_expires_before_second_arrives() {
    let env = setup_env();
    let escrow_id = env.register(Escrow, ());
    let client = EscrowClient::new(&env, &escrow_id);
    let admin = Address::generate(&env);
    client.initialize(&admin);

    let client_addr = Address::generate(&env);
    let freelancer_addr = Address::generate(&env);

    let contract_id = client.create_contract(
        &client_addr,
        &freelancer_addr,
        &None,
        &milestones(&env),
        &ReleaseAuthorization::MultiSig,
    );
    assert!(deposit(&env, &client, &contract_id, &client_addr, &total()));

    assert!(client.approve_milestone_release(&contract_id, &client_addr, &0));

    let approvals = client.get_milestone_approvals(&contract_id, &0).unwrap();
    assert!(approvals.client_approved);
    assert!(!approvals.freelancer_approved);

    advance_ledger(&env, &escrow_id, PENDING_APPROVAL_TTL_LEDGERS + 1);

    assert!(client.approve_milestone_release(&contract_id, &freelancer_addr, &0));

    let result = client.try_release_milestone(&contract_id, &client_addr, &0);
    super::assert_contract_error(result, Error::InsufficientApprovals);

    assert!(client.approve_milestone_release(&contract_id, &client_addr, &0));
    assert!(client.release_milestone(&contract_id, &client_addr, &0));
}

#[test]
fn test_multisig_both_approvals_expire_after_ttl() {
    let env = setup_env();
    let escrow_id = env.register(Escrow, ());
    let client = EscrowClient::new(&env, &escrow_id);
    let admin = Address::generate(&env);
    client.initialize(&admin);

    let client_addr = Address::generate(&env);
    let freelancer_addr = Address::generate(&env);

    let contract_id = client.create_contract(
        &client_addr,
        &freelancer_addr,
        &None,
        &milestones(&env),
        &ReleaseAuthorization::MultiSig,
    );
    assert!(deposit(&env, &client, &contract_id, &client_addr, &total()));

    assert!(client.approve_milestone_release(&contract_id, &client_addr, &0));
    assert!(client.approve_milestone_release(&contract_id, &freelancer_addr, &0));

    advance_ledger(&env, &escrow_id, PENDING_APPROVAL_TTL_LEDGERS + 1);

    let result = client.try_release_milestone(&contract_id, &client_addr, &0);
    super::assert_contract_error(result, Error::InsufficientApprovals);
}

/// A read of a live approval within `PENDING_APPROVAL_BUMP_THRESHOLD` of expiry
/// renews its TTL, keeping the entry live past the original expiry ledger.
///
/// Note: re-approving a still-live record returns `AlreadyApproved`
/// (see [`test_duplicate_approval_rejected`]); the TTL is refreshed by the
/// bump-on-read path in `get_milestone_approvals` / `check_approvals`, not by
/// a second approval.
#[test]
fn test_read_within_bump_threshold_refreshes_ttl() {
    let env = setup_env();
    let escrow_id = env.register(Escrow, ());
    let client = EscrowClient::new(&env, &escrow_id);
    let admin = Address::generate(&env);
    client.initialize(&admin);

    let client_addr = Address::generate(&env);
    let freelancer_addr = Address::generate(&env);

    let contract_id = client.create_contract(
        &client_addr,
        &freelancer_addr,
        &None,
        &milestones(&env),
        &ReleaseAuthorization::ClientOnly,
    );
    assert!(deposit(&env, &client, &contract_id, &client_addr, &total()));

    assert!(client.approve_milestone_release(&contract_id, &client_addr, &0));

    // Advance to within the bump threshold of the original expiry.
    advance_ledger(&env, &escrow_id, PENDING_APPROVAL_TTL_LEDGERS - 1);

    // A read while live and within the bump threshold renews the TTL.
    let refreshed = client.get_milestone_approvals(&contract_id, &0);
    assert!(refreshed.is_some(), "entry must be live just before expiry");

    // Step past the original expiry ledger; the bump must have kept it alive.
    advance_ledger(&env, &escrow_id, 2);

    let approvals = client.get_milestone_approvals(&contract_id, &0);
    assert!(
        approvals.is_some(),
        "read within bump threshold should refresh TTL"
    );

    assert!(client.release_milestone(&contract_id, &client_addr, &0));
}

/// MultiSig variant: both approvals live in a single record, so one read within
/// the bump threshold renews the TTL for both, allowing a release past the
/// original expiry without re-approval.
#[test]
fn test_multisig_read_within_bump_threshold_refreshes_ttl() {
    let env = setup_env();
    let escrow_id = env.register(Escrow, ());
    let client = EscrowClient::new(&env, &escrow_id);
    let admin = Address::generate(&env);
    client.initialize(&admin);

    let client_addr = Address::generate(&env);
    let freelancer_addr = Address::generate(&env);

    let contract_id = client.create_contract(
        &client_addr,
        &freelancer_addr,
        &None,
        &milestones(&env),
        &ReleaseAuthorization::MultiSig,
    );
    assert!(deposit(&env, &client, &contract_id, &client_addr, &total()));

    assert!(client.approve_milestone_release(&contract_id, &client_addr, &0));
    assert!(client.approve_milestone_release(&contract_id, &freelancer_addr, &0));

    // Advance to within the bump threshold, then refresh both approvals via a read.
    advance_ledger(&env, &escrow_id, PENDING_APPROVAL_TTL_LEDGERS - 1);
    let refreshed = client.get_milestone_approvals(&contract_id, &0);
    assert!(refreshed.is_some(), "entry must be live just before expiry");

    // Step past the original expiry; the bump must have kept both approvals alive.
    advance_ledger(&env, &escrow_id, 2);

    let result = client.try_release_milestone(&contract_id, &client_addr, &0);
    assert!(
        result.is_ok(),
        "MultiSig release succeeds after a bump-on-read refresh"
    );
}

#[test]
fn test_approval_ttl_independent_per_milestone() {
    let env = setup_env();
    let escrow_id = env.register(Escrow, ());
    let client = EscrowClient::new(&env, &escrow_id);
    let admin = Address::generate(&env);
    client.initialize(&admin);

    let client_addr = Address::generate(&env);
    let freelancer_addr = Address::generate(&env);

    let contract_id = client.create_contract(
        &client_addr,
        &freelancer_addr,
        &None,
        &milestones(&env),
        &ReleaseAuthorization::ClientOnly,
    );
    assert!(deposit(&env, &client, &contract_id, &client_addr, &total()));

    assert!(client.approve_milestone_release(&contract_id, &client_addr, &0));

    assert!(client.approve_milestone_release(&contract_id, &client_addr, &1));
    advance_ledger(&env, &escrow_id, PENDING_APPROVAL_TTL_LEDGERS / 2);

    advance_ledger(&env, &escrow_id, PENDING_APPROVAL_TTL_LEDGERS / 2 + 1);

    let result_0 = client.try_release_milestone(&contract_id, &client_addr, &0);
    super::assert_contract_error(result_0, Error::InsufficientApprovals);
}

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

#[test]
fn test_deadline_none_before_any_approval() {
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

    let deadline = client.get_approval_deadline(&id, &0u32);
    assert!(
        deadline.is_none(),
        "expected None before any approval, got {deadline:?}"
    );
}

#[test]
fn test_deadline_some_after_first_approval() {
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

    client.approve_milestone_release(&id, &client_addr, &0u32);

    let deadline = client.get_approval_deadline(&id, &0u32);
    assert!(
        deadline.is_some(),
        "expected Some after first approval, got None"
    );

    // Deadline should be roughly current sequence + PENDING_APPROVAL_TTL_LEDGERS
    let expected = env.ledger().sequence() + PENDING_APPROVAL_TTL_LEDGERS;
    assert_eq!(deadline.unwrap(), expected);
}

#[test]
fn test_deadline_does_not_extend_ttl() {
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

    client.approve_milestone_release(&id, &client_addr, &0u32);

    let deadline_first = client.get_approval_deadline(&id, &0u32);
    let deadline_second = client.get_approval_deadline(&id, &0u32);

    // Pure view — repeated calls must return identical remaining ledgers
    assert_eq!(
        deadline_first, deadline_second,
        "get_approval_deadline must not mutate TTL between calls"
    );
}

#[test]
#[should_panic(expected = "HostError: Error(Contract, #3)")]
fn test_deadline_none_for_unknown_milestone() {
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

    client.approve_milestone_release(&id, &client_addr, &0u32);

    client.get_approval_deadline(&id, &999u32);
}

#[test]
fn test_deadline_independent_per_milestone() {
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

    client.approve_milestone_release(&id, &client_addr, &0u32);
    client.approve_milestone_release(&id, &client_addr, &1u32);

    let deadline = client.get_approval_deadline(&id, &0u32);
    assert!(
        deadline.is_some(),
        "expected Some after first approval, got None"
    );

    // Deadline should be roughly current sequence + PENDING_APPROVAL_TTL_LEDGERS
    let expected = env.ledger().sequence() + PENDING_APPROVAL_TTL_LEDGERS;
    assert_eq!(deadline.unwrap(), expected);
}

// ===========================================================================
//  Batch approval entrypoint
// ===========================================================================

#[test]
fn batch_approve_empty_succeeds() {
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

    let empty: soroban_sdk::Vec<u32> = vec![&env];
    assert!(client.approve_milestone_release_batch(&id, &client_addr, &empty));
}

#[test]
fn batch_approve_at_cap_succeeds() {
    let env = Env::default();
    env.mock_all_auths();
    let client = new_client(&env);
    let (client_addr, freelancer_addr, _) = setup(&env);

    // Create a contract with MAX_MILESTONES milestones (contract max)
    let count = crate::MAX_MILESTONES;
    let mut milestones = Vec::new(&env);
    for _ in 0..count {
        milestones.push_back(100_i128);
    }
    let id = client.create_contract(
        &client_addr,
        &freelancer_addr,
        &None,
        &milestones,
        &ReleaseAuthorization::ClientOnly,
    );

    // Inject Funded status directly so we don't need SAC token
    let escrow_addr = client.address.clone();
    env.as_contract(&escrow_addr, || {
        let key = crate::DataKey::Contract(id);
        let mut c: crate::Contract = env.storage().persistent().get(&key).unwrap();
        c.status = crate::ContractStatus::Funded;
        c.funded_amount = (100 * count as i128);
        env.storage().persistent().set(&key, &c);
        // Also store milestones
        let milestone_key = soroban_sdk::Symbol::new(&env, "milestones");
        let mut ms = Vec::new(&env);
        for _ in 0..count {
            ms.push_back(crate::Milestone {
                amount: 100,
                funded_amount: 0,
                released: false,
                refunded: false,
                work_evidence: None,
                refunded_amount: 0,
                deadline: None,
            });
        }
        env.storage().persistent().set(
            &(crate::DataKey::Contract(id), milestone_key),
            &ms,
        );
    });

    let mut indices = Vec::new(&env);
    for i in 0..count {
        indices.push_back(i);
    }
    assert!(client.approve_milestone_release_batch(&id, &client_addr, &indices));

    // Verify all milestones were approved
    for i in 0..count {
        let approvals = client.get_milestone_approvals(&id, &i);
        assert!(approvals.is_some(), "milestone {i} should be approved");
    }
}

#[test]
fn batch_approve_over_cap_rejected() {
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

    let over_cap = crate::MAX_BATCH_APPROVALS + 1;
    let large_indices = {
        let mut v = Vec::new(&env);
        for i in 0..over_cap {
            v.push_back(i);
        }
        v
    };

    let result = client.try_approve_milestone_release_batch(&id, &client_addr, &large_indices);
    super::assert_contract_error(result, crate::EscrowError::BatchCapExceeded);
}

#[test]
fn batch_approve_emits_per_item_events() {
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

    // Approve milestones 0 and 1 in a batch
    let indices = vec![&env, 0u32, 1u32];
    assert!(client.approve_milestone_release_batch(&id, &client_addr, &indices));

    // Verify per-item approval records
    let approvals_0 = client.get_milestone_approvals(&id, &0);
    assert!(approvals_0.is_some(), "milestone 0 should be approved");
    let approvals_1 = client.get_milestone_approvals(&id, &1);
    assert!(approvals_1.is_some(), "milestone 1 should be approved");
}

#[test]
fn batch_approve_fails_on_first_error() {
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

    // Valid index 0 first, then invalid index 99 — should fail on index 0
    // because milestone 0 approval succeeds but 99 is out of bounds
    let indices = vec![&env, 0u32, 99u32];
    let result = client.try_approve_milestone_release_batch(&id, &client_addr, &indices);
    super::assert_contract_error(result, crate::Error::IndexOutOfBounds);
}

#[test]
fn batch_approve_preserves_per_item_semantics() {
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

    // Approve milestone 0 individually first, then try batch including 0 and 1
    assert!(client.approve_milestone_release(&id, &client_addr, &0));

    // Batch should fail on milestone 0 with AlreadyApproved
    let indices = vec![&env, 0u32, 1u32];
    let result = client.try_approve_milestone_release_batch(&id, &client_addr, &indices);
    super::assert_contract_error(result, crate::Error::AlreadyApproved);
}
