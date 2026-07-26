use soroban_sdk::{testutils::Address as _, testutils::Events, vec, Address, FromVal, Symbol};

use super::{assert_contract_error, EscrowFixture};
use crate::{ContractStatus, Error, EscrowError};

fn setup_funded_fixture() -> EscrowFixture {
    EscrowFixture::builder().funded().build()
}

fn release_one_milestone(fixture: &EscrowFixture) {
    let escrow = fixture.escrow();
    assert!(escrow.approve_milestone_release(&fixture.escrow_id, &fixture.client, &0));
    assert!(escrow.release_milestone(&fixture.escrow_id, &fixture.client, &0));
}

fn refund_one_milestone(fixture: &EscrowFixture) {
    let escrow = fixture.escrow();
    let ids = vec![&fixture.env, 1_u32];
    assert!(escrow.refund_unreleased_milestones(&fixture.escrow_id, &ids) > 0);
}

fn complete_contract(fixture: &EscrowFixture) {
    let escrow = fixture.escrow();
    for index in 0..3_u32 {
        assert!(escrow.approve_milestone_release(&fixture.escrow_id, &fixture.client, &index));
        assert!(escrow.release_milestone(&fixture.escrow_id, &fixture.client, &index));
    }
}

#[test]
fn rollback_released_milestone_succeeds() {
    let fixture = setup_funded_fixture();
    let escrow = fixture.escrow();

    release_one_milestone(&fixture);

    let before = escrow.get_contract(&fixture.escrow_id);
    assert_eq!(before.status, ContractStatus::Funded);
    assert!(before.released_amount > 0);

    assert!(escrow.rollback_milestone(&fixture.escrow_id, &fixture.admin, &0));

    let after = escrow.get_contract(&fixture.escrow_id);
    assert_eq!(after.released_amount, 0);
    assert_eq!(after.status, ContractStatus::Funded);

    let milestone = escrow.get_milestone(&fixture.escrow_id, &0).unwrap();
    assert!(!milestone.released);
    assert_eq!(milestone.funded_amount, 0);
    assert_eq!(milestone.protocol_fee, 0);
}

#[test]
fn rollback_refunded_milestone_succeeds() {
    let fixture = setup_funded_fixture();
    let escrow = fixture.escrow();

    refund_one_milestone(&fixture);

    let before = escrow.get_contract(&fixture.escrow_id);
    assert!(before.refunded_amount > 0);

    assert!(escrow.rollback_milestone(&fixture.escrow_id, &fixture.admin, &1));

    let after = escrow.get_contract(&fixture.escrow_id);
    assert_eq!(after.refunded_amount, 0);
    assert_eq!(after.status, ContractStatus::Funded);

    let milestone = escrow.get_milestone(&fixture.escrow_id, &1).unwrap();
    assert!(!milestone.refunded);
    assert_eq!(milestone.refunded_amount, 0);
}

#[test]
fn rollback_released_milestone_with_protocol_fees() {
    let fixture = setup_funded_fixture();
    let escrow = fixture.escrow();

    escrow.set_protocol_fee_bps(&1000u32);

    release_one_milestone(&fixture);

    let before = escrow.get_contract(&fixture.escrow_id);
    let accumulated_before = escrow.get_accumulated_protocol_fees();
    assert!(before.released_amount > 0);
    assert!(accumulated_before > 0);

    assert!(escrow.rollback_milestone(&fixture.escrow_id, &fixture.admin, &0));

    let after = escrow.get_contract(&fixture.escrow_id);
    assert_eq!(after.released_amount, 0);
    assert_eq!(escrow.get_accumulated_protocol_fees(), 0);

    let milestone = escrow.get_milestone(&fixture.escrow_id, &0).unwrap();
    assert!(!milestone.released);
    assert_eq!(milestone.protocol_fee, 0);
}

#[test]
fn rollback_multiple_milestones_independently() {
    let fixture = setup_funded_fixture();
    let escrow = fixture.escrow();

    assert!(escrow.approve_milestone_release(&fixture.escrow_id, &fixture.client, &0));
    assert!(escrow.release_milestone(&fixture.escrow_id, &fixture.client, &0));

    let ids = vec![&fixture.env, 2_u32];
    assert!(escrow.refund_unreleased_milestones(&fixture.escrow_id, &ids) > 0);

    let contract = escrow.get_contract(&fixture.escrow_id);
    assert_eq!(contract.status, ContractStatus::Funded);

    assert!(escrow.rollback_milestone(&fixture.escrow_id, &fixture.admin, &0));
    assert!(escrow.rollback_milestone(&fixture.escrow_id, &fixture.admin, &2));

    let contract = escrow.get_contract(&fixture.escrow_id);
    assert_eq!(contract.released_amount, 0);
    assert_eq!(contract.refunded_amount, 0);
}

#[test]
fn rollback_rejects_non_admin() {
    let fixture = setup_funded_fixture();
    let escrow = fixture.escrow();

    release_one_milestone(&fixture);

    let stranger = Address::generate(&fixture.env);
    assert_contract_error(
        escrow.try_rollback_milestone(&fixture.escrow_id, &stranger, &0),
        EscrowError::UnauthorizedRole,
    );
}

#[test]
fn rollback_rejects_in_created_state() {
    let fixture = EscrowFixture::builder().build();
    let escrow = fixture.escrow();

    assert_contract_error(
        escrow.try_rollback_milestone(&fixture.escrow_id, &fixture.admin, &0),
        EscrowError::RollbackNotAllowed,
    );
}

#[test]
fn rollback_rejects_in_completed_state() {
    let fixture = setup_funded_fixture();
    let escrow = fixture.escrow();

    complete_contract(&fixture);

    assert_contract_error(
        escrow.try_rollback_milestone(&fixture.escrow_id, &fixture.admin, &0),
        EscrowError::RollbackNotAllowed,
    );
}

#[test]
fn rollback_rejects_in_cancelled_state() {
    let fixture = EscrowFixture::builder().with_settlement_token().build();
    let escrow = fixture.escrow();

    assert!(escrow.cancel_contract(&fixture.escrow_id, &fixture.client));

    assert_contract_error(
        escrow.try_rollback_milestone(&fixture.escrow_id, &fixture.admin, &0),
        EscrowError::RollbackNotAllowed,
    );
}

#[test]
fn rollback_rejects_in_refunded_state() {
    let fixture = setup_funded_fixture();
    let escrow = fixture.escrow();

    let ids = vec![&fixture.env, 0_u32, 1_u32, 2_u32];
    assert!(escrow.refund_unreleased_milestones(&fixture.escrow_id, &ids) > 0);

    assert_contract_error(
        escrow.try_rollback_milestone(&fixture.escrow_id, &fixture.admin, &0),
        EscrowError::RollbackNotAllowed,
    );
}

#[test]
fn rollback_rejects_in_disputed_state() {
    let builder = EscrowFixture::builder();
    let client = Address::generate(builder.env());
    let freelancer = Address::generate(builder.env());
    let arbiter = Address::generate(builder.env());
    let fixture = builder
        .with_participants(client, freelancer, Some(arbiter))
        .funded()
        .build();
    let escrow = fixture.escrow();

    assert!(escrow.raise_dispute(&fixture.escrow_id, &fixture.client));

    assert_contract_error(
        escrow.try_rollback_milestone(&fixture.escrow_id, &fixture.admin, &0),
        EscrowError::RollbackNotAllowed,
    );
}

#[test]
fn rollback_rejects_milestone_not_released_or_refunded() {
    let fixture = setup_funded_fixture();
    let escrow = fixture.escrow();

    assert_contract_error(
        escrow.try_rollback_milestone(&fixture.escrow_id, &fixture.admin, &0),
        EscrowError::RollbackNotAllowed,
    );
}

#[test]
fn rollback_rejects_index_out_of_bounds() {
    let fixture = setup_funded_fixture();
    let escrow = fixture.escrow();

    assert_contract_error(
        escrow.try_rollback_milestone(&fixture.escrow_id, &fixture.admin, &99),
        Error::IndexOutOfBounds,
    );
}

#[test]
fn rollback_rejects_contract_not_found() {
    let fixture = setup_funded_fixture();
    let escrow = fixture.escrow();

    assert_contract_error(
        escrow.try_rollback_milestone(&9999, &fixture.admin, &0),
        EscrowError::ContractNotFound,
    );
}

#[test]
fn rollback_rejects_after_finalization() {
    let fixture = setup_funded_fixture();
    let escrow = fixture.escrow();

    complete_contract(&fixture);

    assert!(escrow.finalize_contract(&fixture.escrow_id, &fixture.client));

    assert_contract_error(
        escrow.try_rollback_milestone(&fixture.escrow_id, &fixture.admin, &0),
        Error::AlreadyFinalized,
    );
}

#[test]
fn rollback_clears_approvals() {
    let fixture = setup_funded_fixture();
    let escrow = fixture.escrow();

    assert!(escrow.approve_milestone_release(&fixture.escrow_id, &fixture.client, &0));

    let approvals_before_release = escrow.get_milestone_approvals(&fixture.escrow_id, &0);
    assert!(approvals_before_release.is_some());

    assert!(escrow.release_milestone(&fixture.escrow_id, &fixture.client, &0));

    let approvals_after_release = escrow.get_milestone_approvals(&fixture.escrow_id, &0);
    assert!(approvals_after_release.is_none());

    assert!(escrow.rollback_milestone(&fixture.escrow_id, &fixture.admin, &0));

    let approvals_after_rollback = escrow.get_milestone_approvals(&fixture.escrow_id, &0);
    assert!(approvals_after_rollback.is_none());
}

#[test]
fn rollback_emits_event() {
    let fixture = setup_funded_fixture();
    let escrow = fixture.escrow();

    release_one_milestone(&fixture);

    assert!(escrow.rollback_milestone(&fixture.escrow_id, &fixture.admin, &0));

    let events = fixture.env.events().all();
    let rollback_topic = Symbol::new(&fixture.env, "rollback");
    let found = events.iter().any(|event| {
        event.1.len() > 0
            && Symbol::from_val(&fixture.env, &event.1.get(0).unwrap()) == rollback_topic
    });
    assert!(found, "rollback event must be emitted");
}

#[test]
fn rollback_preserves_accounting_invariant() {
    let fixture = setup_funded_fixture();
    let escrow = fixture.escrow();

    escrow.set_protocol_fee_bps(&500u32);
    release_one_milestone(&fixture);

    let ids = vec![&fixture.env, 2_u32];
    escrow.refund_unreleased_milestones(&fixture.escrow_id, &ids);

    escrow.rollback_milestone(&fixture.escrow_id, &fixture.admin, &0);

    let contract = escrow.get_contract(&fixture.escrow_id);
    let accumulated = escrow.get_accumulated_protocol_fees();
    let invariant_sum = contract.released_amount + contract.refunded_amount + accumulated;
    assert!(
        invariant_sum <= contract.funded_amount,
        "accounting invariant violated: {} > {}",
        invariant_sum,
        contract.funded_amount
    );
}
