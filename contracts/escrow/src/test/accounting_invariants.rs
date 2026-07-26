//! Deterministic accounting invariant tests.
//!
//! These tests exercise the invariant
//!   `funded_amount == released_amount + refunded_amount + available_balance`
//! across concrete deposit/release/cancel sequences, including adversarial
//! cases (over-release, double-release, over-deposit).

#![cfg(test)]

use soroban_sdk::{
    testutils::Address as _, token::{Client as TokenClient, StellarAssetClient},
    vec, Address, Env,
};

use crate::{ContractStatus, Escrow, EscrowClient, ReleaseAuthorization};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_env() -> Env {
    let env = Env::default();
    env.mock_all_auths_allowing_non_root_auth();
    env
}

/// Register escrow, initialize, register and bind a settlement token.
/// Returns `(escrow_client, sac_address, admin)`.
fn make_sac_client(env: &Env) -> (EscrowClient<'_>, Address, Address) {
    let id = env.register(Escrow, ());
    let client = EscrowClient::new(env, &id);
    let admin = Address::generate(env);
    let sac = env.register_stellar_asset_contract(admin.clone());
    client.initialize(&admin);
    client.bind_settlement_token(&admin, &sac);
    (client, sac, admin)
}

fn participants(env: &Env) -> (Address, Address) {
    (Address::generate(env), Address::generate(env))
}

/// Mint `amount` SAC tokens to `holder`.
fn sac_mint(env: &Env, sac: &Address, holder: &Address, amount: i128) {
    StellarAssetClient::new(env, sac).mint(holder, &amount);
}

/// Assert the core accounting invariant on the stored contract data.
fn assert_invariant(client: &EscrowClient, id: u32) {
    let d = client.get_contract(&id);
    let available = d.funded_amount - d.released_amount - d.refunded_amount;
    assert!(
        available >= 0,
        "available_balance < 0 (funded={}, released={}, refunded={})",
        d.funded_amount,
        d.released_amount,
        d.refunded_amount
    );
    assert_eq!(
        d.funded_amount,
        d.released_amount + d.refunded_amount + available,
        "accounting invariant violated"
    );
}

/// Assert the on-chain token balance held by the escrow contract equals the
/// derived accounting balance (`funded - released - refunded + accrued fees`).
fn assert_balance_conservation(client: &EscrowClient, id: u32, sac: &Address) {
    let env = client.env.clone();
    let d = client.get_contract(&id);
    let accrued = client.get_accumulated_protocol_fees();
    let derived = d.funded_amount - d.released_amount - d.refunded_amount + accrued;
    let escrow_addr = client.address.clone();
    let on_chain = TokenClient::new(&env, sac).balance(&escrow_addr);
    assert_eq!(
        on_chain, derived,
        "token balance {} != derived accounting {} (funded={}, released={}, refunded={}, fees={})",
        on_chain, derived, d.funded_amount, d.released_amount, d.refunded_amount, accrued
    );
}

// ---------------------------------------------------------------------------
// Happy-path sequences
// ---------------------------------------------------------------------------

#[test]
fn invariant_holds_after_single_deposit() {
    let env = make_env();
    let (client, sac, _admin) = make_sac_client(&env);
    let (ca, fa) = participants(&env);
    let id = client.create_contract(
        &ca,
        &fa,
        &None,
        &vec![&env, 100_i128, 200_i128],
        &ReleaseAuthorization::ClientOnly,
    );

    sac_mint(&env, &sac, &ca, 100);
    client.deposit_funds(&id, &ca, &100_i128);
    assert_invariant(&client, id);

    let d = client.get_contract(&id);
    assert_eq!(d.total_deposited, 100);
    assert_eq!(d.released_amount, 0);
    assert_eq!(d.refunded_amount, 0);
}

#[test]
fn invariant_holds_after_full_deposit() {
    let env = make_env();
    let (client, sac, _admin) = make_sac_client(&env);
    let (ca, fa) = participants(&env);
    let id = client.create_contract(
        &ca,
        &fa,
        &None,
        &vec![&env, 100_i128, 200_i128],
        &ReleaseAuthorization::ClientOnly,
    );

    sac_mint(&env, &sac, &ca, 300);
    client.deposit_funds(&id, &ca, &300_i128);
    assert_invariant(&client, id);

    let d = client.get_contract(&id);
    assert_eq!(d.status, ContractStatus::Funded);
    assert_eq!(d.total_deposited, 300);
}

#[test]
fn invariant_holds_after_each_milestone_release() {
    let env = make_env();
    let (client, sac, _admin) = make_sac_client(&env);
    let (ca, fa) = participants(&env);
    let id = client.create_contract(
        &ca,
        &fa,
        &None,
        &vec![&env, 100_i128, 200_i128, 300_i128],
        &ReleaseAuthorization::ClientOnly,
    );

    sac_mint(&env, &sac, &ca, 600);
    client.deposit_funds(&id, &ca, &600_i128);
    assert_invariant(&client, id);

    client.approve_milestone_release(&id, &ca, &0);
    client.release_milestone(&id, &ca, &0);
    assert_invariant(&client, id);
    assert_eq!(client.get_contract(&id).released_amount, 100);

    client.approve_milestone_release(&id, &ca, &1);
    client.release_milestone(&id, &ca, &1);
    assert_invariant(&client, id);
    assert_eq!(client.get_contract(&id).released_amount, 300);

    client.approve_milestone_release(&id, &ca, &2);
    client.release_milestone(&id, &ca, &2);
    assert_invariant(&client, id);
    let d = client.get_contract(&id);
    assert_eq!(d.released_amount, 600);
    assert_eq!(d.status, ContractStatus::Completed);
}

#[test]
fn invariant_holds_after_incremental_deposits_then_releases() {
    let env = make_env();
    let (client, sac, _admin) = make_sac_client(&env);
    let (ca, fa) = participants(&env);
    let id = client.create_contract(
        &ca,
        &fa,
        &None,
        &vec![&env, 50_i128, 150_i128],
        &ReleaseAuthorization::ClientOnly,
    );

    sac_mint(&env, &sac, &ca, 200);
    client.deposit_funds(&id, &ca, &50_i128);
    assert_invariant(&client, id);
    client.deposit_funds(&id, &ca, &150_i128);
    assert_invariant(&client, id);

    client.approve_milestone_release(&id, &ca, &0);
    client.release_milestone(&id, &ca, &0);
    assert_invariant(&client, id);
    client.approve_milestone_release(&id, &ca, &1);
    client.release_milestone(&id, &ca, &1);
    assert_invariant(&client, id);

    let d = client.get_contract(&id);
    assert_eq!(d.status, ContractStatus::Completed);
    assert_eq!(d.total_deposited, 200);
    assert_eq!(d.released_amount, 200);
    assert_eq!(d.refunded_amount, 0);
}

// ---------------------------------------------------------------------------
// Cancel sequences
// ---------------------------------------------------------------------------

#[test]
fn invariant_holds_after_cancel_with_no_deposit() {
    let env = make_env();
    let (client, _sac, _admin) = make_sac_client(&env);
    let (ca, fa) = participants(&env);
    let id = client.create_contract(
        &ca,
        &fa,
        &None,
        &vec![&env, 100_i128],
        &ReleaseAuthorization::ClientOnly,
    );

    client.cancel_contract(&id, &ca);
    assert_invariant(&client, id);

    let d = client.get_contract(&id);
    assert_eq!(d.status, ContractStatus::Cancelled);
    assert_eq!(d.funded_amount, 0);
}

#[test]
fn invariant_holds_after_cancel_with_partial_deposit() {
    let env = make_env();
    let (client, sac, _admin) = make_sac_client(&env);
    let (ca, fa) = participants(&env);
    let id = client.create_contract(
        &ca,
        &fa,
        &None,
        &vec![&env, 100_i128, 200_i128],
        &ReleaseAuthorization::ClientOnly,
    );

    sac_mint(&env, &sac, &ca, 100);
    client.deposit_funds(&id, &ca, &100_i128);
    assert_invariant(&client, id);

    let result = client.try_cancel_contract(&id, &ca);
    assert!(
        result.is_err(),
        "cancel must be rejected when status is PartiallyFunded"
    );
    assert_invariant(&client, id);
}

#[test]
fn invariant_holds_after_partial_release_then_cancel_rejected() {
    let env = make_env();
    let (client, sac, _admin) = make_sac_client(&env);
    let (ca, fa) = participants(&env);
    let id = client.create_contract(
        &ca,
        &fa,
        &None,
        &vec![&env, 100_i128, 200_i128],
        &ReleaseAuthorization::ClientOnly,
    );

    sac_mint(&env, &sac, &ca, 300);
    client.deposit_funds(&id, &ca, &300_i128);
    client.approve_milestone_release(&id, &ca, &0);
    client.release_milestone(&id, &ca, &0);
    assert_invariant(&client, id);

    let result = client.try_cancel_contract(&id, &ca);
    assert!(
        result.is_err(),
        "cancel must be rejected when funds have already been released"
    );
    assert_invariant(&client, id);

    let d = client.get_contract(&id);
    assert_eq!(d.released_amount, 100);
    assert_ne!(d.status, ContractStatus::Cancelled);
}

// ---------------------------------------------------------------------------
// Adversarial sequences
// ---------------------------------------------------------------------------

#[test]
fn double_release_rejected_invariant_preserved() {
    let env = make_env();
    let (client, sac, _admin) = make_sac_client(&env);
    let (ca, fa) = participants(&env);
    let id = client.create_contract(
        &ca,
        &fa,
        &None,
        &vec![&env, 100_i128, 200_i128],
        &ReleaseAuthorization::ClientOnly,
    );

    sac_mint(&env, &sac, &ca, 300);
    client.deposit_funds(&id, &ca, &300_i128);
    client.approve_milestone_release(&id, &ca, &0);
    client.release_milestone(&id, &ca, &0);
    assert_invariant(&client, id);

    let before = client.get_contract(&id);
    let result = client.try_release_milestone(&id, &ca, &0);
    assert!(result.is_err(), "double release must be rejected");
    assert_invariant(&client, id);

    let after = client.get_contract(&id);
    assert_eq!(before.released_amount, after.released_amount);
    assert_eq!(before.total_deposited, after.total_deposited);
}

#[test]
fn release_without_funds_rejected_invariant_preserved() {
    let env = make_env();
    let (client, _sac, _admin) = make_sac_client(&env);
    let (ca, fa) = participants(&env);
    let id = client.create_contract(
        &ca,
        &fa,
        &None,
        &vec![&env, 100_i128],
        &ReleaseAuthorization::ClientOnly,
    );

    let result = client.try_release_milestone(&id, &ca, &0);
    assert!(result.is_err(), "release without funds must be rejected");
    assert_invariant(&client, id);
}

#[test]
fn overfund_rejected_invariant_preserved() {
    let env = make_env();
    let (client, sac, _admin) = make_sac_client(&env);
    let (ca, fa) = participants(&env);
    let id = client.create_contract(
        &ca,
        &fa,
        &None,
        &vec![&env, 100_i128],
        &ReleaseAuthorization::ClientOnly,
    );

    sac_mint(&env, &sac, &ca, 101);
    client.deposit_funds(&id, &ca, &100_i128);
    assert_invariant(&client, id);

    let result = client.try_deposit_funds(&id, &ca, &1_i128);
    assert!(result.is_err(), "over-deposit must be rejected");
    assert_invariant(&client, id);

    assert_eq!(client.get_contract(&id).total_deposited, 100);
}

#[test]
fn out_of_range_release_rejected_invariant_preserved() {
    let env = make_env();
    let (client, sac, _admin) = make_sac_client(&env);
    let (ca, fa) = participants(&env);
    let id = client.create_contract(
        &ca,
        &fa,
        &None,
        &vec![&env, 100_i128],
        &ReleaseAuthorization::ClientOnly,
    );

    sac_mint(&env, &sac, &ca, 100);
    client.deposit_funds(&id, &ca, &100_i128);
    assert_invariant(&client, id);

    let result = client.try_release_milestone(&id, &ca, &99);
    assert!(result.is_err(), "out-of-range milestone must be rejected");
    assert_invariant(&client, id);
}

#[test]
fn zero_deposit_rejected_invariant_preserved() {
    let env = make_env();
    let (client, _sac, _admin) = make_sac_client(&env);
    let (ca, fa) = participants(&env);
    let id = client.create_contract(
        &ca,
        &fa,
        &None,
        &vec![&env, 100_i128],
        &ReleaseAuthorization::ClientOnly,
    );

    let result = client.try_deposit_funds(&id, &ca, &0_i128);
    assert!(result.is_err(), "zero deposit must be rejected");
    assert_invariant(&client, id);
}

#[test]
fn negative_deposit_rejected_invariant_preserved() {
    let env = make_env();
    let (client, _sac, _admin) = make_sac_client(&env);
    let (ca, fa) = participants(&env);
    let id = client.create_contract(
        &ca,
        &fa,
        &None,
        &vec![&env, 100_i128],
        &ReleaseAuthorization::ClientOnly,
    );

    let result = client.try_deposit_funds(&id, &ca, &-1_i128);
    assert!(result.is_err(), "negative deposit must be rejected");
    assert_invariant(&client, id);
}

// ---------------------------------------------------------------------------
// Multi-contract isolation
// ---------------------------------------------------------------------------

#[test]
fn invariant_holds_across_multiple_independent_contracts() {
    let env = make_env();
    let (client, sac, _admin) = make_sac_client(&env);
    let (ca1, fa1) = participants(&env);
    let (ca2, fa2) = participants(&env);

    let id1 = client.create_contract(
        &ca1,
        &fa1,
        &None,
        &vec![&env, 100_i128],
        &ReleaseAuthorization::ClientOnly,
    );
    let id2 = client.create_contract(
        &ca2,
        &fa2,
        &None,
        &vec![&env, 200_i128, 300_i128],
        &ReleaseAuthorization::ClientOnly,
    );

    sac_mint(&env, &sac, &ca1, 100);
    sac_mint(&env, &sac, &ca2, 500);
    client.deposit_funds(&id1, &ca1, &100_i128);
    client.deposit_funds(&id2, &ca2, &500_i128);

    client.approve_milestone_release(&id1, &ca1, &0);
    client.release_milestone(&id1, &ca1, &0);
    client.approve_milestone_release(&id2, &ca2, &0);
    client.release_milestone(&id2, &ca2, &0);

    assert_invariant(&client, id1);
    assert_invariant(&client, id2);

    assert_eq!(client.get_contract(&id1).released_amount, 100);
    assert_eq!(client.get_contract(&id2).released_amount, 200);
}

// ---------------------------------------------------------------------------
// On-chain token balance conservation
// ---------------------------------------------------------------------------

#[test]
fn balance_conserved_through_deposit() {
    let env = make_env();
    let (client, sac, _admin) = make_sac_client(&env);
    let (ca, fa) = participants(&env);

    let id = client.create_contract(
        &ca,
        &fa,
        &None,
        &vec![&env, 100_i128, 200_i128],
        &ReleaseAuthorization::ClientOnly,
    );

    assert_balance_conservation(&client, id, &sac);

    let total = 300_i128;
    sac_mint(&env, &sac, &ca, total);
    assert!(client.deposit_funds(&id, &ca, &total));

    assert_eq!(client.get_contract(&id).status, ContractStatus::Funded);
    assert_eq!(client.get_contract(&id).funded_amount, total);
    assert_eq!(TokenClient::new(&env, &sac).balance(&client.address), total);
    assert_balance_conservation(&client, id, &sac);
}

#[test]
fn balance_conserved_when_cancel_returns_full_remaining_balance() {
    let env = make_env();
    let (client, sac, _admin) = make_sac_client(&env);
    let (ca, fa) = participants(&env);

    let id = client.create_contract(
        &ca,
        &fa,
        &None,
        &vec![&env, 100_i128, 200_i128],
        &ReleaseAuthorization::ClientOnly,
    );

    let total = 300_i128;
    sac_mint(&env, &sac, &ca, total);
    assert!(client.deposit_funds(&id, &ca, &total));
    assert_balance_conservation(&client, id, &sac);

    assert!(client.cancel_contract(&id, &ca));

    let d = client.get_contract(&id);
    assert_eq!(d.status, ContractStatus::Cancelled);
    assert_eq!(d.refunded_amount, total, "cancel refunds the full balance");

    assert_eq!(TokenClient::new(&env, &sac).balance(&ca), total);
    assert_balance_conservation(&client, id, &sac);
}

#[test]
fn cancel_without_deposit_moves_no_tokens() {
    let env = make_env();
    let (client, sac, _admin) = make_sac_client(&env);
    let (ca, fa) = participants(&env);

    let id = client.create_contract(
        &ca,
        &fa,
        &None,
        &vec![&env, 100_i128],
        &ReleaseAuthorization::ClientOnly,
    );
    assert_balance_conservation(&client, id, &sac);

    assert!(client.cancel_contract(&id, &ca));
    let d = client.get_contract(&id);
    assert_eq!(d.status, ContractStatus::Cancelled);
    assert_eq!(d.funded_amount, 0);
    assert_eq!(d.refunded_amount, 0);
    assert_eq!(TokenClient::new(&env, &sac).balance(&client.address), 0);
    assert_balance_conservation(&client, id, &sac);
}
