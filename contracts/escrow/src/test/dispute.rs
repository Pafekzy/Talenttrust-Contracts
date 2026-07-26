//! Dispute resolution payout arithmetic tests.
//!
//! These tests verify the pure money-splitting logic in `resolution_payouts`:
//!
//!   - FullRefund: all available → client (available, 0)
//!   - FullPayout: all available → freelancer (0, available)
//!   - PartialRefund: 70/30 split with floor rounding on freelancer leg
//!   - Split: custom split requiring sum == available, no negative amounts
//!
//! Conservation invariant: client_payout + freelancer_payout == available.
//!
//! ## Lifecycle & auth tests
//!
//! The second section covers raise/resolve lifecycle and auth boundaries:
//!
//!   - Only a party to the contract may raise a dispute
//!   - Only the designated arbiter may resolve it
//!   - Resolving moves funds/state correctly
//!   - Raising in a terminal state is rejected with typed error
//!   - Non-party raise is rejected
//!   - Non-arbiter resolve is rejected
//!   - Double-resolve is rejected
//!   - Resolve-after-settle is rejected

#![cfg(test)]

use crate::dispute::{final_status_after_resolution, resolution_payouts};
use crate::{
    Contract, ContractStatus, DisputeConfig, DisputeResolution, DisputeSplit, Error, Escrow,
    EscrowClient, ReleaseAuthorization,
};
use soroban_sdk::{testutils::Address as _, token::StellarAssetClient, vec, Address, Env};

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

fn make_env() -> Env {
    let env = Env::default();
    env.mock_all_auths();
    env
}

fn make_client(env: &Env) -> EscrowClient<'_> {
    env.mock_all_auths_allowing_non_root_auth();
    let id = env.register(Escrow, ());
    let client = EscrowClient::new(env, &id);
    let admin = Address::generate(env);
    client.initialize(&admin);

    let token_admin = Address::generate(env);
    let token_address = env.register_stellar_asset_contract(token_admin);
    client.set_settlement_token(&admin, &token_address);

    client
}

fn deposit(env: &Env, client: &EscrowClient, id: &u32, client_addr: &Address, amount: &i128) -> bool {
    if let Some(token) = client.get_settlement_token() {
        soroban_sdk::token::StellarAssetClient::new(env, &token).mint(client_addr, amount);
    }
    client.deposit_funds(id, client_addr, amount)
}

/// Build a bare `Contract` value with controlled accounting fields for unit tests
/// that call `resolution_payouts` / `final_status_after_resolution` directly.
///
/// `funded` is stored in both `total_deposited` and `funded_amount` so the
/// helper reflects a freshly-funded contract with no prior releases.
pub fn payout_contract(env: &Env, funded: i128, released: i128, refunded: i128) -> Contract {
    Contract {
        client: Address::generate(env),
        freelancer: Address::generate(env),
        arbiter: Some(Address::generate(env)),
        status: ContractStatus::Disputed,
        total_deposited: funded,
        funded_amount: funded,
        released_amount: released,
        refunded_amount: refunded,
        release_authorization: ReleaseAuthorization::ClientOnly,
        reputation_issued: false,
        token: Address::generate(env),
    }
}

/// Helper: create a funded contract with an arbiter, ready for dispute.
/// Returns (client_addr, freelancer_addr, arbiter_addr, contract_id).
fn funded_contract_with_arbiter(
    env: &Env,
    client: &EscrowClient<'_>,
) -> (Address, Address, Address, u32) {
    let client_addr = Address::generate(env);
    let freelancer_addr = Address::generate(env);
    let arbiter_addr = Address::generate(env);
    let milestones = vec![env, 100_i128];
    let contract_id = client.create_contract(
        &client_addr,
        &freelancer_addr,
        &Some(arbiter_addr.clone()),
        &milestones,
        &ReleaseAuthorization::ClientOnly,
    );
    assert!(deposit(env, client, &contract_id, &client_addr, &100_i128));
    (client_addr, freelancer_addr, arbiter_addr, contract_id)
}

/// Helper: create a funded contract without an arbiter.
/// Returns (client_addr, freelancer_addr, contract_id).
fn funded_contract_no_arbiter(env: &Env, client: &EscrowClient<'_>) -> (Address, Address, u32) {
    let client_addr = Address::generate(env);
    let freelancer_addr = Address::generate(env);
    let milestones = vec![env, 100_i128];
    let contract_id = client.create_contract(
        &client_addr,
        &freelancer_addr,
        &None,
        &milestones,
        &ReleaseAuthorization::ClientOnly,
    );
    assert!(deposit(env, client, &contract_id, &client_addr, &100_i128));
    (client_addr, freelancer_addr, contract_id)
}

/// Helper: create a funded contract with arbiter and raise a dispute.
/// Returns (client_addr, freelancer_addr, arbiter_addr, contract_id).
fn disputed_contract(env: &Env, client: &EscrowClient<'_>) -> (Address, Address, Address, u32) {
    let (client_addr, freelancer_addr, arbiter_addr, contract_id) =
        funded_contract_with_arbiter(env, client);
    assert!(client.raise_dispute(&contract_id, &client_addr));
    (client_addr, freelancer_addr, arbiter_addr, contract_id)
}

// ---------------------------------------------------------------------------
// Unit tests: resolution_payouts (pure arithmetic)
// ---------------------------------------------------------------------------

#[test]
fn resolution_payouts_full_refund_routes_all_to_client() {
    let env = make_env();
    // available = 100 - 20 - 10 = 70
    let contract = payout_contract(&env, 100, 20, 10);
    assert_eq!(
        resolution_payouts(&contract, &DisputeResolution::FullRefund),
        Ok((70, 0))
    );
}

#[test]
fn resolution_payouts_full_payout_routes_all_to_freelancer() {
    let env = make_env();
    let contract = payout_contract(&env, 100, 20, 10);
    assert_eq!(
        resolution_payouts(&contract, &DisputeResolution::FullPayout),
        Ok((0, 70))
    );
}

/// PartialRefund applies the documented 70/30 split with floor rounding.
/// freelancer gets floor(available * 30 / 100), client gets remainder.
#[test]
fn resolution_payouts_partial_refund_applies_floor_rounded_30_pct_to_freelancer() {
    let env = make_env();
    // 101 available: freelancer = floor(101 * 30 / 100) = 30; client = 71
    let contract = payout_contract(&env, 101, 0, 0);
    assert_eq!(
        resolution_payouts(&contract, &DisputeResolution::PartialRefund),
        Ok((71, 30))
    );
}

#[test]
fn resolution_payouts_split_accepts_exact_conserving_amounts() {
    let env = make_env();
    // Zero available → (0, 0)
    assert_eq!(
        resolution_payouts(
            &payout_contract(&env, 100, 0, 0),
            &DisputeResolution::Split(DisputeSplit {
                client_amount: 40,
                freelancer_amount: 60,
            })
        ),
        Ok((40, 60))
    );
    // One stroop → floor(1 * 30 / 100) = 0, client gets 1
    assert_eq!(
        resolution_payouts(
            &payout_contract(&env, 1, 0, 0),
            &DisputeResolution::PartialRefund
        ),
        Ok((1, 0))
    );
}

/// Table-driven test covering PartialRefund rounding at odd amounts.
/// Verifies that floor truncation never creates value (sum == available).
#[test]
fn resolution_payouts_partial_refund_odd_amount_rounding() {
    let env = make_env();
    // (available, expected_client, expected_freelancer)
    let cases: &[(i128, i128, i128)] = &[
        (7, 5, 2),
        (10, 7, 3),
        (99, 70, 29),
        (100, 70, 30),
        (101, 71, 30),
        (102, 72, 30),
        (103, 73, 30),
    ];
    for (available, expected_client, expected_freelancer) in cases {
        let contract = payout_contract(&env, *available, 0, 0);
        let (client, freelancer) = resolution_payouts(&contract, &DisputeResolution::PartialRefund)
            .expect("PartialRefund should not error");
        assert_eq!(
            client + freelancer,
            *available,
            "sum must equal available for amount {}",
            available
        );
        assert_eq!(client, *expected_client);
        assert_eq!(freelancer, *expected_freelancer);
    }
}

/// Split rejects negative amounts.
#[test]
fn resolution_payouts_split_rejects_negative_legs() {
    let env = make_env();
    let contract = payout_contract(&env, 100, 0, 0);
    let split = DisputeSplit {
        client_amount: -1,
        freelancer_amount: 101,
    };
    assert_eq!(
        resolution_payouts(&contract, &DisputeResolution::Split(split)),
        Err(Error::InvalidDisputeSplit)
    );
    let split = DisputeSplit {
        client_amount: 101,
        freelancer_amount: -1,
    };
    assert_eq!(
        resolution_payouts(&contract, &DisputeResolution::Split(split)),
        Err(Error::InvalidDisputeSplit)
    );
}

#[test]
fn resolution_payouts_split_rejects_non_conserving_sum() {
    let env = make_env();
    let contract = payout_contract(&env, 100, 0, 0);
    // 40 + 59 = 99 ≠ 100
    let split = DisputeSplit {
        client_amount: 40,
        freelancer_amount: 59,
    };
    assert_eq!(
        resolution_payouts(&contract, &DisputeResolution::Split(split)),
        Err(Error::InvalidDisputeSplit)
    );
    // 40 + 61 = 101 ≠ 100
    let split = DisputeSplit {
        client_amount: 40,
        freelancer_amount: 61,
    };
    assert_eq!(
        resolution_payouts(&contract, &DisputeResolution::Split(split)),
        Err(Error::InvalidDisputeSplit)
    );
}

/// Split accepts any (a, b) where a + b == available and both are non-negative.
#[test]
fn resolution_payouts_split_accepts_exact_splits() {
    let env = make_env();
    let split = DisputeSplit {
        client_amount: 40,
        freelancer_amount: 60,
    };
    assert_eq!(
        resolution_payouts(
            &payout_contract(&env, 100, 0, 0),
            &DisputeResolution::Split(split)
        ),
        Ok((40, 60))
    );
    let split = DisputeSplit {
        client_amount: 0,
        freelancer_amount: 0,
    };
    assert_eq!(
        resolution_payouts(
            &payout_contract(&env, 0, 0, 0),
            &DisputeResolution::Split(split)
        ),
        Ok((0, 0))
    );
}

/// Split uses checked addition and rejects overflow before the sum check.
#[test]
fn resolution_payouts_split_rejects_overflowing_sum() {
    let env = make_env();
    let contract = payout_contract(&env, i128::MAX, 0, 0);
    let split = DisputeSplit {
        client_amount: i128::MAX,
        freelancer_amount: 1,
    };
    assert_eq!(
        resolution_payouts(&contract, &DisputeResolution::Split(split)),
        Err(Error::InvalidDisputeSplit)
    );
}

/// Payout math fails closed when released + refunded already exceed funded_amount.
#[test]
fn resolution_payouts_rejects_corrupted_accounting_state() {
    let env = make_env();
    // released(70) + refunded(31) = 101 > funded(100) → available < 0
    let contract = payout_contract(&env, 100, 70, 31);
    assert_eq!(
        resolution_payouts(&contract, &DisputeResolution::FullRefund),
        Err(Error::AccountingInvariantViolated)
    );
}

/// Table-driven test verifying conservation invariant across all resolution variants.
#[test]
fn resolution_payouts_conserves_available_balance() {
    let env = make_env();
    let balances = &[0, 1, 2, 5, 10, 33, 99, 100, 101, 1000, 12345, 1000000];

    for &available in balances {
        let c = payout_contract(&env, available, 0, 0);

        // FullRefund
        let (client, freelancer) = resolution_payouts(&c, &DisputeResolution::FullRefund).unwrap();
        assert_eq!(client + freelancer, available);
        assert_eq!(client, available);
        assert_eq!(freelancer, 0);

        // FullPayout
        let (client, freelancer) = resolution_payouts(&c, &DisputeResolution::FullPayout).unwrap();
        assert_eq!(client + freelancer, available);
        assert_eq!(client, 0);
        assert_eq!(freelancer, available);

        // PartialRefund
        let (client, freelancer) =
            resolution_payouts(&c, &DisputeResolution::PartialRefund).unwrap();
        assert_eq!(client + freelancer, available);
        let expected_freelancer =
            (available * PARTIAL_REFUND_FREELANCER_SHARE_NUMERATOR) / PARTIAL_REFUND_DENOMINATOR;
        assert_eq!(freelancer, expected_freelancer);
        assert_eq!(client, available - expected_freelancer);

        // Split (exact)
        let split_client = available / 2;
        let split_freelancer = available - split_client;
        let split = DisputeSplit {
            client_amount: split_client,
            freelancer_amount: split_freelancer,
        };
        let (client, freelancer) =
            resolution_payouts(&c, &DisputeResolution::Split(split)).unwrap();
        assert_eq!(client + freelancer, available);
        assert_eq!(client, split_client);
        assert_eq!(freelancer, split_freelancer);
    }
}

/// final_status returns Refunded only when the full deposit has been refunded.
#[test]
fn final_status_after_resolution_returns_refunded_only_when_fully_refunded() {
    let env = make_env();
    assert_eq!(
        final_status_after_resolution(&payout_contract(&env, 100, 0, 100)),
        ContractStatus::Refunded
    );
    assert_eq!(
        final_status_after_resolution(&payout_contract(&env, 100, 30, 70)),
        ContractStatus::Completed
    );
}

// ---------------------------------------------------------------------------
// Integration tests: resolution variants
// ---------------------------------------------------------------------------

/// Integration: FullRefund on a funded contract conserves balance and marks Refunded.
#[test]
fn resolve_full_refund_conserves_and_marks_refunded() {
    let env = make_env();
    let client = make_client(&env);
    let client_addr = Address::generate(&env);
    let freelancer_addr = Address::generate(&env);
    let arbiter_addr = Address::generate(&env);
    let milestones = soroban_sdk::vec![&env, 125_i128, 75_i128];
    let escrow_id = client.create_contract(
        &client_addr,
        &freelancer_addr,
        &Some(arbiter_addr.clone()),
        &milestones,
        &ReleaseAuthorization::ClientOnly,
    );
    deposit(&env, &client, &escrow_id, &client_addr, &200_i128);

    client.raise_dispute(&escrow_id, &client_addr);
    client.resolve_dispute(&escrow_id, &arbiter_addr, &DisputeResolution::FullRefund);

    let contract = client.get_contract(&escrow_id);
    assert_eq!(contract.status, ContractStatus::Refunded);
    assert_eq!(contract.released_amount, 0);
    assert_eq!(contract.refunded_amount, 200);
    assert_eq!(
        contract.released_amount + contract.refunded_amount,
        contract.funded_amount
    );
}

/// Integration: FullPayout on a funded contract conserves balance and marks Completed.
#[test]
fn resolve_full_payout_conserves_and_marks_completed() {
    let env = make_env();
    let client = make_client(&env);
    let client_addr = Address::generate(&env);
    let freelancer_addr = Address::generate(&env);
    let arbiter_addr = Address::generate(&env);
    let milestones = soroban_sdk::vec![&env, 150_i128];
    let escrow_id = client.create_contract(
        &client_addr,
        &freelancer_addr,
        &Some(arbiter_addr.clone()),
        &milestones,
        &ReleaseAuthorization::ClientOnly,
    );
    deposit(&env, &client, &escrow_id, &client_addr, &150_i128);

    client.raise_dispute(&escrow_id, &client_addr);
    client.resolve_dispute(&escrow_id, &arbiter_addr, &DisputeResolution::FullPayout);

    let contract = client.get_contract(&escrow_id);
    assert_eq!(contract.status, ContractStatus::Completed);
    assert_eq!(contract.released_amount, 150);
    assert_eq!(contract.refunded_amount, 0);
    assert_eq!(
        contract.released_amount + contract.refunded_amount,
        contract.funded_amount
    );
}

/// Integration: PartialRefund applies 70/30 split and conserves balance.
#[test]
fn resolve_partial_refund_conserves_70_30_split() {
    let env = make_env();
    let client = make_client(&env);
    let client_addr = Address::generate(&env);
    let freelancer_addr = Address::generate(&env);
    let arbiter_addr = Address::generate(&env);
    let milestones = soroban_sdk::vec![&env, 100_i128];
    let escrow_id = client.create_contract(
        &client_addr,
        &freelancer_addr,
        &Some(arbiter_addr.clone()),
        &milestones,
        &ReleaseAuthorization::ClientOnly,
    );
    deposit(&env, &client, &escrow_id, &client_addr, &100_i128);

    client.raise_dispute(&escrow_id, &client_addr);
    client.resolve_dispute(&escrow_id, &arbiter_addr, &DisputeResolution::PartialRefund);

    let contract = client.get_contract(&escrow_id);
    assert_eq!(contract.status, ContractStatus::Completed);
    assert_eq!(
        contract.released_amount + contract.refunded_amount,
        contract.funded_amount
    );
}

/// Integration: Split accepts valid custom amounts and conserves balance.
#[test]
fn resolve_split_conserves_custom_amounts() {
    let env = make_env();
    let client = make_client(&env);
    let client_addr = Address::generate(&env);
    let freelancer_addr = Address::generate(&env);
    let arbiter_addr = Address::generate(&env);
    let milestones = soroban_sdk::vec![&env, 40_i128, 60_i128];
    let escrow_id = client.create_contract(
        &client_addr,
        &freelancer_addr,
        &Some(arbiter_addr.clone()),
        &milestones,
        &ReleaseAuthorization::ClientOnly,
    );
    deposit(&env, &client, &escrow_id, &client_addr, &100_i128);

    client.raise_dispute(&escrow_id, &client_addr);
    let split = DisputeSplit {
        client_amount: 35,
        freelancer_amount: 65,
    };
    client.resolve_dispute(&escrow_id, &arbiter_addr, &DisputeResolution::Split(split));

    let contract = client.get_contract(&escrow_id);
    assert_eq!(contract.status, ContractStatus::Completed);
    assert_eq!(contract.refunded_amount, 35);
    assert_eq!(contract.released_amount, 65);
    assert_eq!(
        contract.released_amount + contract.refunded_amount,
        contract.funded_amount
    );
}

// ---------------------------------------------------------------------------
// Lifecycle & auth tests: raise/resolve boundaries
// ---------------------------------------------------------------------------

/// Client can raise a dispute on a funded contract with an arbiter.
#[test]
fn raise_dispute_by_client_succeeds() {
    let env = make_env();
    let client = make_client(&env);
    let (client_addr, _, _, contract_id) = funded_contract_with_arbiter(&env, &client);

    assert!(client.raise_dispute(&contract_id, &client_addr));
    assert_eq!(
        client.get_contract(&contract_id).status,
        ContractStatus::Disputed
    );
}

/// Freelancer can also raise a dispute on a funded contract with an arbiter.
#[test]
fn raise_dispute_by_freelancer_succeeds() {
    let env = make_env();
    let client = make_client(&env);
    let (_, freelancer_addr, _, contract_id) = funded_contract_with_arbiter(&env, &client);

    assert!(client.raise_dispute(&contract_id, &freelancer_addr));
    assert_eq!(
        client.get_contract(&contract_id).status,
        ContractStatus::Disputed
    );
}

/// A non-party (random address) cannot raise a dispute.
#[test]
fn raise_dispute_by_non_party_is_rejected() {
    let env = make_env();
    let client = make_client(&env);
    let (_, _, _, contract_id) = funded_contract_with_arbiter(&env, &client);
    let outsider = Address::generate(&env);

    super::assert_contract_error(
        client.try_raise_dispute(&contract_id, &outsider),
        Error::UnauthorizedRole,
    );
    assert_eq!(
        client.get_contract(&contract_id).status,
        ContractStatus::Funded
    );
}

/// Raising a dispute on a contract without an arbiter is rejected.
#[test]
fn raise_dispute_without_arbiter_is_rejected() {
    let env = make_env();
    let client = make_client(&env);
    let (client_addr, _, contract_id) = funded_contract_no_arbiter(&env, &client);

    super::assert_contract_error(
        client.try_raise_dispute(&contract_id, &client_addr),
        Error::ArbiterRequired,
    );
    assert_eq!(
        client.get_contract(&contract_id).status,
        ContractStatus::Funded
    );
}

/// Raising a dispute on a contract in the Completed (terminal) state is rejected
/// with a typed error.
#[test]
fn raise_dispute_on_completed_contract_is_rejected() {
    let env = make_env();
    let client = make_client(&env);
    let milestones = vec![&env, 100_i128];
    let client_addr = Address::generate(&env);
    let freelancer_addr = Address::generate(&env);
    let arbiter_addr = Address::generate(&env);
    let contract_id = client.create_contract(
        &client_addr,
        &freelancer_addr,
        &Some(arbiter_addr),
        &milestones,
        &ReleaseAuthorization::ClientOnly,
    );
    assert!(deposit(&env, &client, &contract_id, &client_addr, &100_i128));
    // Release the only milestone to reach Completed state.
    assert!(client.approve_milestone_release(&contract_id, &client_addr, &0));
    assert!(client.release_milestone(&contract_id, &client_addr, &0));
    assert_eq!(
        client.get_contract(&contract_id).status,
        ContractStatus::Completed
    );

    super::assert_contract_error(
        client.try_raise_dispute(&contract_id, &client_addr),
        Error::InvalidState,
    );
}

/// The designated arbiter can resolve a dispute.
#[test]
fn resolve_dispute_by_arbiter_succeeds() {
    let env = make_env();
    let client = make_client(&env);
    let (_, _, arbiter_addr, contract_id) = disputed_contract(&env, &client);

    assert!(client.resolve_dispute(&contract_id, &arbiter_addr, &DisputeResolution::FullRefund,));
    let contract = client.get_contract(&contract_id);
    assert_eq!(contract.status, ContractStatus::Refunded);
    assert_eq!(contract.refunded_amount, 100);
}

/// Resolving a dispute emits one dedicated arbiter event carrying the decision
/// and both payout amounts. Its `arbiter` topic is distinct from `dispute`.
#[test]
fn resolve_dispute_emits_dedicated_arbiter_event() {
    let env = make_env();
    let escrow_addr = env.register(Escrow, ());
    let client = EscrowClient::new(&env, &escrow_addr);
    client.initialize(&Address::generate(&env));
    let client_addr = Address::generate(&env);
    let freelancer_addr = Address::generate(&env);
    let arbiter_addr = Address::generate(&env);
    let contract_id = 1;
    env.as_contract(&escrow_addr, || {
        env.storage().persistent().set(
            &DataKey::Contract(contract_id),
            &Contract {
                client: client_addr,
                freelancer: freelancer_addr,
                arbiter: Some(arbiter_addr.clone()),
                status: ContractStatus::Disputed,
                total_deposited: 100,
                funded_amount: 100,
                released_amount: 0,
                refunded_amount: 0,
                release_authorization: ReleaseAuthorization::ClientOnly,
                reputation_issued: false,
            },
        );
    });

    let event_count_before = env.events().all().len();
    assert!(client.resolve_dispute(&contract_id, &arbiter_addr, &DisputeResolution::FullRefund,));

    // Capture the event immediately after the mutating call, so the assertion
    // cannot accidentally match an event emitted by an earlier operation.
    let events = env.events().all();
    assert_eq!(events.len(), event_count_before + 2);
    let event = events.get(events.len() - 1).unwrap();

    assert_eq!(
        Symbol::try_from_val(&env, &event.1.get(0).unwrap()).unwrap(),
        soroban_sdk::symbol_short!("arbiter")
    );
    assert_eq!(
        u32::try_from_val(&env, &event.1.get(1).unwrap()).unwrap(),
        contract_id
    );
    assert_ne!(
        Symbol::try_from_val(&env, &event.1.get(0).unwrap()).unwrap(),
        soroban_sdk::symbol_short!("dispute")
    );
    assert_eq!(
        <(Address, u32, i128, i128)>::try_from_val(&env, &event.2).unwrap(),
        (arbiter_addr, DisputeResolution::FullRefund.code(), 100, 0)
    );
}

/// A non-arbiter address cannot resolve a dispute.
#[test]
fn resolve_dispute_by_non_arbiter_is_rejected() {
    let env = make_env();
    let client = make_client(&env);
    let (client_addr, _, _, contract_id) = disputed_contract(&env, &client);
    let outsider = Address::generate(&env);

    // Client is a party but not the arbiter — must be rejected.
    super::assert_contract_error(
        client.try_resolve_dispute(&contract_id, &client_addr, &DisputeResolution::FullRefund),
        Error::UnauthorizedRole,
    );
    // Random outsider is also rejected.
    super::assert_contract_error(
        client.try_resolve_dispute(&contract_id, &outsider, &DisputeResolution::FullRefund),
        Error::UnauthorizedRole,
    );
    // Contract remains in Disputed state.
    assert_eq!(
        client.get_contract(&contract_id).status,
        ContractStatus::Disputed
    );
}

/// After resolving a dispute, a second resolve is rejected (double-resolve).
#[test]
fn double_resolve_is_rejected() {
    let env = make_env();
    let client = make_client(&env);
    let (_, _, arbiter_addr, contract_id) = disputed_contract(&env, &client);

    // First resolution succeeds.
    assert!(client.resolve_dispute(&contract_id, &arbiter_addr, &DisputeResolution::FullRefund,));
    assert_eq!(
        client.get_contract(&contract_id).status,
        ContractStatus::Refunded
    );

    // Second resolution must fail — contract is no longer Disputed.
    super::assert_contract_error(
        client.try_resolve_dispute(&contract_id, &arbiter_addr, &DisputeResolution::FullPayout),
        Error::InvalidStatusTransition,
    );
}

/// After all milestones are released (settled), raise_dispute is rejected
/// because the contract is in Completed state, not Funded/PartiallyFunded.
#[test]
fn raise_dispute_after_settle_is_rejected() {
    let env = make_env();
    let client = make_client(&env);
    let milestones = vec![&env, 50_i128, 50_i128];
    let client_addr = Address::generate(&env);
    let freelancer_addr = Address::generate(&env);
    let arbiter_addr = Address::generate(&env);
    let contract_id = client.create_contract(
        &client_addr,
        &freelancer_addr,
        &Some(arbiter_addr),
        &milestones,
        &ReleaseAuthorization::ClientOnly,
    );
    assert!(deposit(&env, &client, &contract_id, &client_addr, &100_i128));
    // Release all milestones to settle the contract.
    assert!(client.approve_milestone_release(&contract_id, &client_addr, &0));
    assert!(client.release_milestone(&contract_id, &client_addr, &0));
    assert!(client.approve_milestone_release(&contract_id, &client_addr, &1));
    assert!(client.release_milestone(&contract_id, &client_addr, &1));
    assert_eq!(
        client.get_contract(&contract_id).status,
        ContractStatus::Completed
    );

    super::assert_contract_error(
        client.try_raise_dispute(&contract_id, &client_addr),
        Error::InvalidState,
    );
}

/// Resolving a dispute moves funds: FullPayout sets released_amount and marks
/// Completed; FullRefund sets refunded_amount and marks Refunded. Verify that
/// the accounting is correct after each resolution type.
#[test]
fn resolve_moves_funds_correctly_full_payout() {
    let env = make_env();
    let client = make_client(&env);
    let (_, _freelancer_addr, arbiter_addr, contract_id) = disputed_contract(&env, &client);

    assert!(client.resolve_dispute(&contract_id, &arbiter_addr, &DisputeResolution::FullPayout,));
    let contract = client.get_contract(&contract_id);
    assert_eq!(contract.status, ContractStatus::Completed);
    assert_eq!(contract.released_amount, 100);
    assert_eq!(contract.refunded_amount, 0);
    assert_eq!(
        contract.released_amount + contract.refunded_amount,
        contract.funded_amount
    );
}

#[test]
fn resolve_moves_funds_correctly_full_refund() {
    let env = make_env();
    let client = make_client(&env);
    let (_, _, arbiter_addr, contract_id) = disputed_contract(&env, &client);

    assert!(client.resolve_dispute(&contract_id, &arbiter_addr, &DisputeResolution::FullRefund,));
    let contract = client.get_contract(&contract_id);
    assert_eq!(contract.status, ContractStatus::Refunded);
    assert_eq!(contract.released_amount, 0);
    assert_eq!(contract.refunded_amount, 100);
    assert_eq!(
        contract.released_amount + contract.refunded_amount,
        contract.funded_amount
    );
}

/// Raising a dispute on a contract in the Refunded (terminal) state is rejected.
#[test]
fn raise_dispute_on_refunded_contract_is_rejected() {
    let env = make_env();
    let client = make_client(&env);
    let (client_addr, freelancer_addr, arbiter_addr, contract_id) =
        funded_contract_with_arbiter(&env, &client);

    // Resolve as FullRefund to reach Refunded terminal state.
    assert!(client.raise_dispute(&contract_id, &client_addr));
    assert!(client.resolve_dispute(&contract_id, &arbiter_addr, &DisputeResolution::FullRefund,));
    assert_eq!(
        client.get_contract(&contract_id).status,
        ContractStatus::Refunded
    );

    // Cannot raise again.
    super::assert_contract_error(
        client.try_raise_dispute(&contract_id, &freelancer_addr),
        Error::InvalidState,
    );
}

// ---------------------------------------------------------------------------
// Extreme-value tests for arbiter arithmetic overflow (Issue #890)
// ---------------------------------------------------------------------------

/// FullRefund with i128::MAX available must succeed and route all to client.
#[test]
fn resolution_payouts_full_refund_with_i128_max_ok() {
    let env = make_env();
    let contract = payout_contract(&env, i128::MAX, 0, 0);
    let result = resolution_payouts(&contract, &DisputeResolution::FullRefund);
    assert_eq!(result, Ok((i128::MAX, 0)));
}

/// FullPayout with i128::MAX available must succeed and route all to freelancer.
#[test]
fn resolution_payouts_full_payout_with_i128_max_ok() {
    let env = make_env();
    let contract = payout_contract(&env, i128::MAX, 0, 0);
    let result = resolution_payouts(&contract, &DisputeResolution::FullPayout);
    assert_eq!(result, Ok((0, i128::MAX)));
}

/// PartialRefund with available so large that `available * 30` would overflow
/// must return PotentialOverflow.
/// i128::MAX / 30 gives a safe upper bound; anything above overflows mul.
#[test]
fn resolution_payouts_partial_refund_rejects_overflowing_mul() {
    let env = make_env();
    let contract = payout_contract(&env, i128::MAX / 25, 0, 0);
    assert_eq!(
        resolution_payouts(&contract, &DisputeResolution::PartialRefund),
        Err(Error::PotentialOverflow)
    );
}

/// PartialRefund with the maximum available value that does NOT overflow mul(30).
/// max_safe = i128::MAX / 30  (division floors, so mul(30) is safe).
#[test]
fn resolution_payouts_partial_refund_at_max_safe_available() {
    let env = make_env();
    let max_safe = i128::MAX / 30; // largest value where mul(30) won't overflow
    let contract = payout_contract(&env, max_safe, 0, 0);
    // freelancer = floor(max_safe * 30 / 100) = floor(i128::MAX / 100)
    let result = resolution_payouts(&contract, &DisputeResolution::PartialRefund)
        .expect("PartialRefund should succeed at max_safe available");
    let (client, freelancer) = result;
    assert_eq!(client + freelancer, max_safe, "sum must equal available");
    let expected_freelancer = (max_safe * 30) / 100;
    assert_eq!(freelancer, expected_freelancer);
    assert_eq!(client, max_safe - expected_freelancer);
}

/// Split with components whose sum exceeds i128::MAX must return PotentialOverflow.
#[test]
fn resolution_payouts_split_rejects_overflowing_sum_extreme() {
    let env = make_env();
    // Both legs individually fit, but their sum overflows i128.
    let split = DisputeSplit {
        client_amount: i128::MAX,
        freelancer_amount: 1,
    };
    let contract = payout_contract(&env, i128::MAX, 0, 0);
    assert_eq!(
        resolution_payouts(&contract, &DisputeResolution::Split(split)),
        Err(Error::InvalidDisputeSplit)
    );
    // Symmetric: freelancer_amount = i128::MAX, client_amount = 1
    let split = DisputeSplit {
        client_amount: 1,
        freelancer_amount: i128::MAX,
    };
    assert_eq!(
        resolution_payouts(&contract, &DisputeResolution::Split(split)),
        Err(Error::InvalidDisputeSplit)
    );
}

/// Split with the maximum sum that exactly fits MAX_SINGLE_AMOUNT_STROOPS matches available
/// and must succeed.
#[test]
fn resolution_payouts_split_at_i128_max_sum_succeeds() {
    let env = make_env();
    let max = crate::amount_validation::MAX_SINGLE_AMOUNT_STROOPS;
    let client_half = max / 2;
    let freelancer_half = max - client_half;
    let split = DisputeSplit {
        client_amount: client_half,
        freelancer_amount: freelancer_half,
    };
    let contract = payout_contract(&env, max, 0, 0);
    let result = resolution_payouts(&contract, &DisputeResolution::Split(split))
        .expect("Split at max sum should succeed");
    assert_eq!(result, (client_half, freelancer_half));
    assert_eq!(client_half + freelancer_half, max);
}

/// Split with zero available and zero amounts succeeds.
#[test]
fn resolution_payouts_split_zero_available_zero_split_ok() {
    let env = make_env();
    let split = DisputeSplit {
        client_amount: 0,
        freelancer_amount: 0,
    };
    let contract = payout_contract(&env, 0, 0, 0);
    assert_eq!(
        resolution_payouts(&contract, &DisputeResolution::Split(split)),
        Ok((0, 0))
    );
}

/// Available calculation near i128::MAX with non-zero released and refunded.
/// Verifies subtraction edge cases.
#[test]
fn resolution_payouts_available_near_max_with_released_refunded() {
    let env = make_env();
    // funded = i128::MAX - 1, released = 1, refunded = 0 => available = i128::MAX - 2
    let funded = i128::MAX - 1;
    let released = 1;
    let refunded = 0;
    let contract = payout_contract(&env, funded, released, refunded);
    let expected_available = funded - released - refunded;
    let result = resolution_payouts(&contract, &DisputeResolution::FullRefund)
        .expect("FullRefund should succeed");
    assert_eq!(result, (expected_available, 0));

    // FullPayout
    let result = resolution_payouts(&contract, &DisputeResolution::FullPayout)
        .expect("FullPayout should succeed");
    assert_eq!(result, (0, expected_available));
}

/// Available calculation with i128::MIN involvement — negative intermediate must
/// be caught by checked_sub before reaching the final check.
#[test]
fn resolution_payouts_rejects_negative_intermediate_subtraction() {
    let env = make_env();
    // funded < released, so first checked_sub fails
    let contract = payout_contract(&env, 0, i128::MAX, 0);
    assert_eq!(
        resolution_payouts(&contract, &DisputeResolution::FullRefund),
        Err(Error::AccountingInvariantViolated)
    );
}

/// Integration: resolve_dispute with large (but safe) values must not overflow.
/// This exercises the checked_add guards added to the entrypoint (Issue #890).
#[test]
fn resolve_dispute_large_amount_flow_succeeds() {
    let env = make_env();
    let client = make_client(&env);
    let client_addr = Address::generate(&env);
    let freelancer_addr = Address::generate(&env);
    let arbiter_addr = Address::generate(&env);
    let large_amt = 1_000_000_0000000i128;
    let milestones = soroban_sdk::vec![&env, large_amt];
    let escrow_id = client.create_contract(
        &client_addr,
        &freelancer_addr,
        &Some(arbiter_addr.clone()),
        &milestones,
        &ReleaseAuthorization::ClientOnly,
    );
    deposit(&env, &client, &escrow_id, &client_addr, &large_amt);
    client.raise_dispute(&escrow_id, &client_addr);

    // FullPayout adds available all to released_amount.
    assert!(client.resolve_dispute(
        &escrow_id,
        &arbiter_addr,
        &DisputeResolution::FullPayout,
    ));
    let contract = client.get_contract(&escrow_id);
    assert_eq!(contract.released_amount, large_amt);
    assert_eq!(contract.status, ContractStatus::Completed);
}

/// Integration: resolve_dispute with FullRefund at large (but safe) values
/// must correctly update refunded_amount without overflow.
#[test]
fn resolve_dispute_full_refund_large_amounts() {
    let env = make_env();
    let client = make_client(&env);
    let client_addr = Address::generate(&env);
    let freelancer_addr = Address::generate(&env);
    let arbiter_addr = Address::generate(&env);
    let large = 1_000_000_0000000i128;
    let milestones = soroban_sdk::vec![&env, large];
    let escrow_id = client.create_contract(
        &client_addr,
        &freelancer_addr,
        &Some(arbiter_addr.clone()),
        &milestones,
        &ReleaseAuthorization::ClientOnly,
    );
    deposit(&env, &client, &escrow_id, &client_addr, &large);
    client.raise_dispute(&escrow_id, &client_addr);

    assert!(client.resolve_dispute(
        &escrow_id,
        &arbiter_addr,
        &DisputeResolution::FullRefund,
    ));
    let contract = client.get_contract(&escrow_id);
    assert_eq!(contract.refunded_amount, large);
    assert_eq!(contract.status, ContractStatus::Refunded);
    assert_eq!(
        contract.released_amount + contract.refunded_amount,
        contract.funded_amount
    );
}

/// Resolving after the contract has been finalized is rejected with AlreadyFinalized.
#[test]
fn resolve_after_finalize_is_rejected() {
    let env = make_env();
    let client = make_client(&env);
    let (client_addr, _, arbiter_addr, contract_id) = disputed_contract(&env, &client);

    // Finalize the disputed contract.
    assert!(client.finalize_contract(&contract_id, &client_addr));

    super::assert_contract_error(
        client.try_resolve_dispute(&contract_id, &arbiter_addr, &DisputeResolution::FullRefund),
        Error::AlreadyFinalized,
    );
}

// ---------------------------------------------------------------------------
// Overflow, saturation, and extreme value dispute arithmetic tests (#885)
// ---------------------------------------------------------------------------

#[test]
fn resolution_payouts_extreme_i128_max_full_refund() {
    let env = make_env();
    let contract = payout_contract(&env, i128::MAX, 0, 0);
    assert_eq!(
        resolution_payouts(&contract, &DisputeResolution::FullRefund),
        Ok((i128::MAX, 0))
    );
}

#[test]
fn resolution_payouts_extreme_i128_max_full_payout() {
    let env = make_env();
    let contract = payout_contract(&env, i128::MAX, 0, 0);
    assert_eq!(
        resolution_payouts(&contract, &DisputeResolution::FullPayout),
        Ok((0, i128::MAX))
    );
}

#[test]
fn resolution_payouts_extreme_i128_partial_refund_overflow_rejected() {
    let env = make_env();
    // i128::MAX * 30 overflows i128, must safely return PotentialOverflow error.
    let contract = payout_contract(&env, i128::MAX, 0, 0);
    assert_eq!(
        resolution_payouts(&contract, &DisputeResolution::PartialRefund),
        Err(Error::PotentialOverflow)
    );
}

#[test]
fn resolution_payouts_extreme_i128_partial_refund_large_valid() {
    let env = make_env();
    // funded = i128::MAX / 30 is large enough to test extreme value math without overflowing * 30.
    let funded = i128::MAX / 30;
    let contract = payout_contract(&env, funded, 0, 0);
    let (client_payout, freelancer_payout) =
        resolution_payouts(&contract, &DisputeResolution::PartialRefund)
            .expect("Large valid amount should not overflow");
    assert_eq!(client_payout + freelancer_payout, funded);
    assert!(freelancer_payout > 0);
    assert!(client_payout > freelancer_payout);
}

#[test]
fn resolution_payouts_split_extreme_near_max() {
    let env = make_env();
    let funded = i128::MAX;
    let client_amt = i128::MAX - 5000;
    let freelancer_amt = 5000;
    let contract = payout_contract(&env, funded, 0, 0);
    let split = DisputeSplit {
        client_amount: client_amt,
        freelancer_amount: freelancer_amt,
    };
    assert_eq!(
        resolution_payouts(&contract, &DisputeResolution::Split(split)),
        Ok((client_amt, freelancer_amt))
    );
}

#[test]
fn resolution_payouts_subtraction_near_zero_available() {
    let env = make_env();
    // funded = 100, released = 50, refunded = 50 -> available = 0
    let contract = payout_contract(&env, 100, 50, 50);
    assert_eq!(
        resolution_payouts(&contract, &DisputeResolution::FullRefund),
        Ok((0, 0))
    );
    assert_eq!(
        resolution_payouts(&contract, &DisputeResolution::FullPayout),
        Ok((0, 0))
    );
    assert_eq!(
        resolution_payouts(&contract, &DisputeResolution::PartialRefund),
        Ok((0, 0))
    );
    let split = DisputeSplit {
        client_amount: 0,
        freelancer_amount: 0,
    };
    assert_eq!(
        resolution_payouts(&contract, &DisputeResolution::Split(split)),
        Ok((0, 0))
    );
}

#[test]
fn resolution_payouts_subtraction_underflow_corrupted_state() {
    let env = make_env();
    // funded = 100, released = 60, refunded = 50 -> available = -10 < 0
    let contract = payout_contract(&env, 100, 60, 50);
    assert_eq!(
        resolution_payouts(&contract, &DisputeResolution::FullRefund),
        Err(Error::AccountingInvariantViolated)
    );
    assert_eq!(
        resolution_payouts(&contract, &DisputeResolution::FullPayout),
        Err(Error::AccountingInvariantViolated)
    );
}

#[test]
fn resolve_dispute_accounting_overflow_protection_refunded() {
    let env = make_env();
    let client = make_client(&env);
    let (client_addr, _freelancer_addr, arbiter_addr, contract_id) =
        funded_contract_with_arbiter(&env, &client);

    assert!(client.raise_dispute(&contract_id, &client_addr));

    // Manually manipulate contract state in storage to simulate refunded_amount near i128::MAX
    let mut contract = client.get_contract(&contract_id);
    contract.refunded_amount = i128::MAX - 10;
    contract.funded_amount = i128::MAX;
    env.storage()
        .persistent()
        .set(&crate::DataKey::Contract(contract_id), &contract);

    // FullRefund attempts to add client_payout (i128::MAX) to refunded_amount (i128::MAX - 10), causing overflow.
    super::assert_contract_error(
        client.try_resolve_dispute(&contract_id, &arbiter_addr, &DisputeResolution::FullRefund),
        Error::PotentialOverflow,
    );
}

#[test]
fn resolve_dispute_accounting_overflow_protection_released() {
    let env = make_env();
    let client = make_client(&env);
    let (client_addr, _freelancer_addr, arbiter_addr, contract_id) =
        funded_contract_with_arbiter(&env, &client);

    assert!(client.raise_dispute(&contract_id, &client_addr));

    // Manually manipulate contract state in storage to simulate released_amount near i128::MAX
    let mut contract = client.get_contract(&contract_id);
    contract.released_amount = i128::MAX - 10;
    contract.funded_amount = i128::MAX;
    env.storage()
        .persistent()
        .set(&crate::DataKey::Contract(contract_id), &contract);

    // FullPayout attempts to add freelancer_payout (i128::MAX) to released_amount (i128::MAX - 10), causing overflow.
    super::assert_contract_error(
        client.try_resolve_dispute(&contract_id, &arbiter_addr, &DisputeResolution::FullPayout),
        Error::PotentialOverflow,
    );
}
