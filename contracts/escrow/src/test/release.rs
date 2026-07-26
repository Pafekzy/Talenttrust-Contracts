use super::{assert_contract_error, EscrowFixture, MILESTONE_ONE};
use crate::{ContractStatus, EscrowError};

/// Releases use the funded shortcut and complete after every milestone settles.
#[test]
fn release_funded_milestones_completes_contract() {
    let fixture = EscrowFixture::builder().funded().build();
    let escrow = fixture.escrow();

    for index in 0..3_u32 {
        assert!(escrow.approve_milestone_release(&fixture.escrow_id, &fixture.client, &index));
        assert!(escrow.release_milestone(&fixture.escrow_id, &fixture.client, &index));
    }

    let contract = escrow.get_contract(&fixture.escrow_id);
    assert_eq!(contract.status, ContractStatus::Completed);
    assert_eq!(contract.released_amount, fixture.total_amount());
}

/// A release cannot be repeated after the fixture has settled that milestone.
#[test]
fn release_rejects_an_already_released_milestone() {
    let fixture = EscrowFixture::builder().funded().build();
    let escrow = fixture.escrow();
    assert!(escrow.approve_milestone_release(&fixture.escrow_id, &fixture.client, &0));
    assert!(escrow.release_milestone(&fixture.escrow_id, &fixture.client, &0));

    assert_contract_error(
        escrow.try_approve_milestone_release(&fixture.escrow_id, &fixture.client, &0),
        crate::Error::MilestoneAlreadyReleased,
    );
    assert_contract_error(
        escrow.try_release_milestone(&fixture.escrow_id, &fixture.client, &0),
        crate::Error::MilestoneAlreadyReleased,
    );
    assert_eq!(
        escrow.get_contract(&fixture.escrow_id).released_amount,
        MILESTONE_ONE
    );
}

/// Release state is persisted in the milestone vector's `released` boolean flag.
/// This test verifies that the flag transitions from false → true after release.
#[test]
fn release_sets_milestone_released_flag_in_vector() {
    let fixture = EscrowFixture::builder().funded().build();
    let escrow = fixture.escrow();

    // Before release, milestone should be unreleased
    let milestones_before = escrow.get_milestones(&fixture.escrow_id);
    assert_eq!(milestones_before.len(), 3);
    assert!(!milestones_before.get(0).unwrap().released);
    assert!(!milestones_before.get(1).unwrap().released);
    assert!(!milestones_before.get(2).unwrap().released);

    // Release milestone 0
    assert!(escrow.approve_milestone_release(&fixture.escrow_id, &fixture.client, &0));
    assert!(escrow.release_milestone(&fixture.escrow_id, &fixture.client, &0));

    // After release, only milestone 0 should be marked released
    let milestones_after = escrow.get_milestones(&fixture.escrow_id);
    assert!(milestones_after.get(0).unwrap().released);
    assert!(!milestones_after.get(1).unwrap().released);
    assert!(!milestones_after.get(2).unwrap().released);
}

/// Release state is correctly reported by get_milestone for individual queries.
#[test]
fn release_state_readable_via_get_milestone() {
    let fixture = EscrowFixture::builder().funded().build();
    let escrow = fixture.escrow();

    // Unreleased milestone returns `released: false`
    let ms0_before = escrow.get_milestone(&fixture.escrow_id, &0);
    assert!(ms0_before.is_some());
    assert!(!ms0_before.unwrap().released);

    // Release the milestone
    assert!(escrow.approve_milestone_release(&fixture.escrow_id, &fixture.client, &0));
    assert!(escrow.release_milestone(&fixture.escrow_id, &fixture.client, &0));

    // After release, get_milestone returns `released: true`
    let ms0_after = escrow.get_milestone(&fixture.escrow_id, &0);
    assert!(ms0_after.is_some());
    assert!(ms0_after.unwrap().released);
}

/// Release state is preserved across partial and full release scenarios.
#[test]
fn release_state_consistent_in_partial_release() {
    let fixture = EscrowFixture::builder().funded().build();
    let escrow = fixture.escrow();

    // Release only milestone 1
    assert!(escrow.approve_milestone_release(&fixture.escrow_id, &fixture.client, &1));
    assert!(escrow.release_milestone(&fixture.escrow_id, &fixture.client, &1));

    // Verify state after selective release
    let milestones = escrow.get_milestones(&fixture.escrow_id);
    assert!(!milestones.get(0).unwrap().released, "Milestone 0 should remain unreleased");
    assert!(milestones.get(1).unwrap().released, "Milestone 1 should be released");
    assert!(!milestones.get(2).unwrap().released, "Milestone 2 should remain unreleased");

    // Release milestone 0 and 2
    for index in [0, 2].iter() {
        assert!(escrow.approve_milestone_release(&fixture.escrow_id, &fixture.client, index));
        assert!(escrow.release_milestone(&fixture.escrow_id, &fixture.client, index));
    }

    // All milestones should now be released
    let milestones_final = escrow.get_milestones(&fixture.escrow_id);
    assert!(milestones_final.get(0).unwrap().released);
    assert!(milestones_final.get(1).unwrap().released);
    assert!(milestones_final.get(2).unwrap().released);
    assert_eq!(escrow.get_contract(&fixture.escrow_id).status, ContractStatus::Completed);
}

/// Attempting to release an already-released milestone fails with AlreadyReleased error.
#[test]
fn release_double_release_attempt_rejected() {
    let fixture = EscrowFixture::builder().funded().build();
    let escrow = fixture.escrow();

    // First release succeeds
    assert!(escrow.approve_milestone_release(&fixture.escrow_id, &fixture.client, &0));
    assert!(escrow.release_milestone(&fixture.escrow_id, &fixture.client, &0));

    // Second release attempt is rejected
    assert!(escrow.approve_milestone_release(&fixture.escrow_id, &fixture.client, &0));
    let result = escrow.try_release_milestone(&fixture.escrow_id, &fixture.client, &0);
    assert_contract_error(result, EscrowError::AlreadyReleased);

    // Verify state is unchanged (released_amount unchanged)
    let contract = escrow.get_contract(&fixture.escrow_id);
    assert_eq!(contract.released_amount, MILESTONE_ONE);
}

/// Out-of-bounds milestone indices are properly rejected at release time.
#[test]
fn release_invalid_index_rejected() {
    let fixture = EscrowFixture::builder().funded().build();
    let escrow = fixture.escrow();

    // Contract has 3 milestones (indices 0, 1, 2)
    // Attempt to release index 3 (out of bounds)
    assert!(escrow.approve_milestone_release(&fixture.escrow_id, &fixture.client, &3));
    let result = escrow.try_release_milestone(&fixture.escrow_id, &fixture.client, &3);
    assert_contract_error(result, EscrowError::IndexOutOfBounds);

    // Verify state is unchanged
    let milestones = escrow.get_milestones(&fixture.escrow_id);
    for i in 0..milestones.len() {
        assert!(!milestones.get(i as u32).unwrap().released);
    }
}

/// Contract's released_amount is correctly incremented with each milestone release.
#[test]
fn release_incremental_released_amount_tracking() {
    let fixture = EscrowFixture::builder().funded().build();
    let escrow = fixture.escrow();

    let contract_before = escrow.get_contract(&fixture.escrow_id);
    assert_eq!(contract_before.released_amount, 0);

    // Release milestone 0
    assert!(escrow.approve_milestone_release(&fixture.escrow_id, &fixture.client, &0));
    assert!(escrow.release_milestone(&fixture.escrow_id, &fixture.client, &0));

    let contract_after_0 = escrow.get_contract(&fixture.escrow_id);
    assert_eq!(contract_after_0.released_amount, MILESTONE_ONE);

    // Release milestone 1
    assert!(escrow.approve_milestone_release(&fixture.escrow_id, &fixture.client, &1));
    assert!(escrow.release_milestone(&fixture.escrow_id, &fixture.client, &1));

    let contract_after_1 = escrow.get_contract(&fixture.escrow_id);
    assert_eq!(contract_after_1.released_amount, MILESTONE_ONE + MILESTONE_ONE);

    // Release milestone 2
    assert!(escrow.approve_milestone_release(&fixture.escrow_id, &fixture.client, &2));
    assert!(escrow.release_milestone(&fixture.escrow_id, &fixture.client, &2));

    let contract_final = escrow.get_contract(&fixture.escrow_id);
    assert_eq!(contract_final.released_amount, fixture.total_amount());
}
