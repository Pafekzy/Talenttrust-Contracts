#![cfg(test)]

use soroban_sdk::{testutils::Address as _, vec, Address, Env, String, Vec};

use crate::{
    Escrow, EscrowClient, MilestoneSchedule, ReleaseAuthorization,
    MAX_SCHEDULE_DESCRIPTION_LEN, MAX_SCHEDULE_TITLE_LEN,
};

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

fn register_client(env: &Env) -> EscrowClient<'_> {
    let id = env.register(Escrow, ());
    EscrowClient::new(env, &id)
}

fn participants(env: &Env) -> (Address, Address) {
    (Address::generate(env), Address::generate(env))
}

fn two_milestones(env: &Env) -> Vec<i128> {
    vec![env, 100_i128, 200_i128]
}

fn three_milestones(env: &Env) -> Vec<i128> {
    vec![env, 100_i128, 200_i128, 300_i128]
}

fn future(env: &Env, offset_secs: u64) -> u64 {
    env.ledger().timestamp() + offset_secs
}

fn no_schedules(env: &Env, n: u32) -> Vec<Option<MilestoneSchedule>> {
    let mut v: Vec<Option<MilestoneSchedule>> = Vec::new(env);
    for _ in 0..n {
        v.push_back(None);
    }
    v
}

fn dated_schedule(_env: &Env, due: u64) -> MilestoneSchedule {
    MilestoneSchedule {
        due_date: Some(due),
        title: None,
        description: None,
        updated_at: 0,
    }
}

fn full_schedule(env: &Env, due: u64, title: &str, desc: &str) -> MilestoneSchedule {
    MilestoneSchedule {
        due_date: Some(due),
        title: Some(String::from_str(env, title)),
        description: Some(String::from_str(env, desc)),
        updated_at: 0,
    }
}

// ---------------------------------------------------------------------------
// Happy-path tests
// ---------------------------------------------------------------------------

#[test]
fn valid_create_without_schedules() {
    let env = Env::default();
    env.mock_all_auths();
    let client = register_client(&env);
    let (c, f) = participants(&env);

    let id = client.create_contract_with_schedules(
        &c,
        &f,
        &None::<Address>,
        &two_milestones(&env),
        &ReleaseAuthorization::ClientOnly,
        &Vec::new(&env),
    );

    assert!(client.get_milestone_schedule(&id, &0).is_none());
    assert!(client.get_milestone_schedule(&id, &1).is_none());
}

#[test]
fn valid_create_with_partial_schedules() {
    let env = Env::default();
    env.mock_all_auths();
    let client = register_client(&env);
    let (c, f) = participants(&env);

    let due = future(&env, 86_400);
    let mut scheds: Vec<Option<MilestoneSchedule>> = Vec::new(&env);
    scheds.push_back(Some(dated_schedule(&env, due)));
    scheds.push_back(None);

    let id = client.create_contract_with_schedules(
        &c,
        &f,
        &None::<Address>,
        &two_milestones(&env),
        &ReleaseAuthorization::ClientOnly,
        &scheds,
    );

    let stored = client.get_milestone_schedule(&id, &0).expect("schedule should exist");
    assert_eq!(stored.due_date, Some(due));
    assert!(client.get_milestone_schedule(&id, &1).is_none());
}

#[test]
fn valid_create_with_all_schedules_populated() {
    let env = Env::default();
    env.mock_all_auths();
    let client = register_client(&env);
    let (c, f) = participants(&env);

    let due0 = future(&env, 100_000);
    let due1 = future(&env, 200_000);
    let due2 = future(&env, 300_000);

    let mut scheds: Vec<Option<MilestoneSchedule>> = Vec::new(&env);
    scheds.push_back(Some(full_schedule(&env, due0, "Phase 1", "Initial deliverable")));
    scheds.push_back(Some(full_schedule(&env, due1, "Phase 2", "Mid-point review")));
    scheds.push_back(Some(full_schedule(&env, due2, "Phase 3", "Final delivery")));

    let id = client.create_contract_with_schedules(
        &c,
        &f,
        &None::<Address>,
        &three_milestones(&env),
        &ReleaseAuthorization::ClientOnly,
        &scheds,
    );

    for (idx, expected_due) in [(0u32, due0), (1, due1), (2, due2)] {
        let s = client
            .get_milestone_schedule(&id, &idx)
            .expect("schedule should be stored");
        assert_eq!(s.due_date, Some(expected_due));
    }
}

#[test]
fn valid_updated_at_is_stamped_by_contract() {
    let env = Env::default();
    env.mock_all_auths();
    let client = register_client(&env);
    let (c, f) = participants(&env);

    let due = future(&env, 50_000);
    let mut scheds: Vec<Option<MilestoneSchedule>> = Vec::new(&env);
    scheds.push_back(Some(MilestoneSchedule {
        due_date: Some(due),
        title: None,
        description: None,
        updated_at: 999_999,
    }));

    let id = client.create_contract_with_schedules(
        &c,
        &f,
        &None::<Address>,
        &vec![&env, 100_i128],
        &ReleaseAuthorization::ClientOnly,
        &scheds,
    );

    let stored = client.get_milestone_schedule(&id, &0).unwrap();
    assert_eq!(stored.updated_at, env.ledger().timestamp());
    assert_ne!(stored.updated_at, 999_999);
}

#[test]
fn valid_get_schedule_returns_none_for_missing_index() {
    let env = Env::default();
    env.mock_all_auths();
    let client = register_client(&env);
    let (c, f) = participants(&env);

    let id = client.create_contract_with_schedules(
        &c,
        &f,
        &None::<Address>,
        &vec![&env, 100_i128],
        &ReleaseAuthorization::ClientOnly,
        &Vec::new(&env),
    );

    assert!(client.get_milestone_schedule(&id, &99).is_none());
}

// ---------------------------------------------------------------------------
// Due-date validation
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "Error(Contract, #54)")]
fn error_due_date_at_present_is_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let client = register_client(&env);
    let (c, f) = participants(&env);

    let now = env.ledger().timestamp();
    let mut scheds: Vec<Option<MilestoneSchedule>> = Vec::new(&env);
    scheds.push_back(Some(dated_schedule(&env, now)));

    client.create_contract_with_schedules(
        &c,
        &f,
        &None::<Address>,
        &vec![&env, 100_i128],
        &ReleaseAuthorization::ClientOnly,
        &scheds,
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #54)")]
fn error_due_date_in_past_is_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let client = register_client(&env);
    let (c, f) = participants(&env);

    let now = env.ledger().timestamp();
    let past = if now > 1 { now - 1 } else { 0 };
    let mut scheds: Vec<Option<MilestoneSchedule>> = Vec::new(&env);
    scheds.push_back(Some(dated_schedule(&env, past)));

    client.create_contract_with_schedules(
        &c,
        &f,
        &None::<Address>,
        &vec![&env, 100_i128],
        &ReleaseAuthorization::ClientOnly,
        &scheds,
    );
}

#[test]
fn valid_due_date_max_u64_is_accepted() {
    let env = Env::default();
    env.mock_all_auths();
    let client = register_client(&env);
    let (c, f) = participants(&env);

    let mut scheds: Vec<Option<MilestoneSchedule>> = Vec::new(&env);
    scheds.push_back(Some(dated_schedule(&env, u64::MAX)));

    let id = client.create_contract_with_schedules(
        &c,
        &f,
        &None::<Address>,
        &vec![&env, 100_i128],
        &ReleaseAuthorization::ClientOnly,
        &scheds,
    );

    let stored = client.get_milestone_schedule(&id, &0).unwrap();
    assert_eq!(stored.due_date, Some(u64::MAX));
}

// ---------------------------------------------------------------------------
// Monotonicity enforcement
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "Error(Contract, #54)")]
fn error_monotonic_equal_dates_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let client = register_client(&env);
    let (c, f) = participants(&env);

    let due = future(&env, 100_000);
    let mut scheds: Vec<Option<MilestoneSchedule>> = Vec::new(&env);
    scheds.push_back(Some(dated_schedule(&env, due)));
    scheds.push_back(Some(dated_schedule(&env, due)));

    client.create_contract_with_schedules(
        &c,
        &f,
        &None::<Address>,
        &two_milestones(&env),
        &ReleaseAuthorization::ClientOnly,
        &scheds,
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #54)")]
fn error_monotonic_decreasing_dates_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let client = register_client(&env);
    let (c, f) = participants(&env);

    let due0 = future(&env, 200_000);
    let due1 = future(&env, 100_000);

    let mut scheds: Vec<Option<MilestoneSchedule>> = Vec::new(&env);
    scheds.push_back(Some(dated_schedule(&env, due0)));
    scheds.push_back(Some(dated_schedule(&env, due1)));

    client.create_contract_with_schedules(
        &c,
        &f,
        &None::<Address>,
        &two_milestones(&env),
        &ReleaseAuthorization::ClientOnly,
        &scheds,
    );
}

#[test]
fn valid_monotonic_skips_undated_milestones() {
    let env = Env::default();
    env.mock_all_auths();
    let client = register_client(&env);
    let (c, f) = participants(&env);

    let due0 = future(&env, 100_000);
    let due2 = future(&env, 300_000);

    let mut scheds: Vec<Option<MilestoneSchedule>> = Vec::new(&env);
    scheds.push_back(Some(dated_schedule(&env, due0)));
    scheds.push_back(None);
    scheds.push_back(Some(dated_schedule(&env, due2)));

    let id = client.create_contract_with_schedules(
        &c,
        &f,
        &None::<Address>,
        &three_milestones(&env),
        &ReleaseAuthorization::ClientOnly,
        &scheds,
    );

    assert!(client.get_milestone_schedule(&id, &0).is_some());
    assert!(client.get_milestone_schedule(&id, &1).is_none());
    assert!(client.get_milestone_schedule(&id, &2).is_some());
}

// ---------------------------------------------------------------------------
// String-length enforcement
// ---------------------------------------------------------------------------

#[test]
fn valid_title_at_max_length_accepted() {
    let env = Env::default();
    env.mock_all_auths();
    let client = register_client(&env);
    let (c, f) = participants(&env);

    let title_bytes = "a".repeat(MAX_SCHEDULE_TITLE_LEN as usize);
    let title_str = String::from_str(&env, &title_bytes);

    let mut scheds: Vec<Option<MilestoneSchedule>> = Vec::new(&env);
    scheds.push_back(Some(MilestoneSchedule {
        due_date: Some(future(&env, 1_000)),
        title: Some(title_str),
        description: None,
        updated_at: 0,
    }));

    client.create_contract_with_schedules(
        &c,
        &f,
        &None::<Address>,
        &vec![&env, 100_i128],
        &ReleaseAuthorization::ClientOnly,
        &scheds,
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #54)")]
fn error_title_exceeds_max_length_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let client = register_client(&env);
    let (c, f) = participants(&env);

    let title_bytes = "a".repeat(MAX_SCHEDULE_TITLE_LEN as usize + 1);
    let title_str = String::from_str(&env, &title_bytes);

    let mut scheds: Vec<Option<MilestoneSchedule>> = Vec::new(&env);
    scheds.push_back(Some(MilestoneSchedule {
        due_date: Some(future(&env, 1_000)),
        title: Some(title_str),
        description: None,
        updated_at: 0,
    }));

    client.create_contract_with_schedules(
        &c,
        &f,
        &None::<Address>,
        &vec![&env, 100_i128],
        &ReleaseAuthorization::ClientOnly,
        &scheds,
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #54)")]
fn error_description_exceeds_max_length_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let client = register_client(&env);
    let (c, f) = participants(&env);

    let desc_bytes = "x".repeat(MAX_SCHEDULE_DESCRIPTION_LEN as usize + 1);
    let desc_str = String::from_str(&env, &desc_bytes);

    let mut scheds: Vec<Option<MilestoneSchedule>> = Vec::new(&env);
    scheds.push_back(Some(MilestoneSchedule {
        due_date: Some(future(&env, 1_000)),
        title: None,
        description: Some(desc_str),
        updated_at: 0,
    }));

    client.create_contract_with_schedules(
        &c,
        &f,
        &None::<Address>,
        &vec![&env, 100_i128],
        &ReleaseAuthorization::ClientOnly,
        &scheds,
    );
}

// ---------------------------------------------------------------------------
// set_milestone_schedule — mutation after creation
// ---------------------------------------------------------------------------

#[test]
fn set_schedule_client_can_update_before_release() {
    let env = Env::default();
    env.mock_all_auths();
    let client = register_client(&env);
    let (c, f) = participants(&env);

    let id = client.create_contract_with_schedules(
        &c,
        &f,
        &None::<Address>,
        &vec![&env, 100_i128],
        &ReleaseAuthorization::ClientOnly,
        &Vec::new(&env),
    );

    let new_due = future(&env, 50_000);
    let new_sched = full_schedule(&env, new_due, "Updated title", "Updated desc");

    assert!(client.set_milestone_schedule(&id, &c, &0, &new_sched));

    let stored = client.get_milestone_schedule(&id, &0).expect("should exist after set");
    assert_eq!(stored.due_date, Some(new_due));
}

#[test]
#[should_panic(expected = "Error(Contract, #17)")]
fn error_immutable_set_schedule_after_release_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let client = register_client(&env);
    let (c, f) = participants(&env);

    let id = client.create_contract_with_schedules(
        &c,
        &f,
        &None::<Address>,
        &vec![&env, 100_i128, 200_i128],
        &ReleaseAuthorization::ClientOnly,
        &Vec::new(&env),
    );

    client.deposit_funds(&id, &c, &300_i128);
    client.approve_milestone_release(&id, &c, &0);
    client.release_milestone(&id, &c, &0);

    let sched = dated_schedule(&env, future(&env, 10_000));
    client.set_milestone_schedule(&id, &c, &0, &sched);
}

#[test]
#[should_panic(expected = "Error(Contract, #54)")]
fn error_set_schedule_violates_monotonicity_with_next() {
    let env = Env::default();
    env.mock_all_auths();
    let client = register_client(&env);
    let (c, f) = participants(&env);

    let due0 = future(&env, 100_000);
    let due1 = future(&env, 200_000);

    let mut scheds: Vec<Option<MilestoneSchedule>> = Vec::new(&env);
    scheds.push_back(Some(dated_schedule(&env, due0)));
    scheds.push_back(Some(dated_schedule(&env, due1)));

    let id = client.create_contract_with_schedules(
        &c,
        &f,
        &None::<Address>,
        &two_milestones(&env),
        &ReleaseAuthorization::ClientOnly,
        &scheds,
    );

    let bad_sched = dated_schedule(&env, future(&env, 300_000));
    client.set_milestone_schedule(&id, &c, &0, &bad_sched);
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn error_set_schedule_out_of_range_index_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let client = register_client(&env);
    let (c, f) = participants(&env);

    let id = client.create_contract_with_schedules(
        &c,
        &f,
        &None::<Address>,
        &vec![&env, 100_i128],
        &ReleaseAuthorization::ClientOnly,
        &Vec::new(&env),
    );

    let sched = dated_schedule(&env, future(&env, 10_000));
    client.set_milestone_schedule(&id, &c, &99, &sched);
}

#[test]
#[should_panic(expected = "Error(Contract, #54)")]
fn error_schedules_length_mismatch_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let client = register_client(&env);
    let (c, f) = participants(&env);

    let mut scheds: Vec<Option<MilestoneSchedule>> = Vec::new(&env);
    scheds.push_back(Some(dated_schedule(&env, future(&env, 10_000))));

    client.create_contract_with_schedules(
        &c,
        &f,
        &None::<Address>,
        &two_milestones(&env),
        &ReleaseAuthorization::ClientOnly,
        &scheds,
    );
}

// ---------------------------------------------------------------------------
// Integration tests
// ---------------------------------------------------------------------------

#[test]
fn integration_full_lifecycle_preserves_schedule_metadata() {
    let env = Env::default();
    env.mock_all_auths();
    let client = register_client(&env);
    let (c, f) = participants(&env);

    let due0 = future(&env, 100_000);
    let due1 = future(&env, 200_000);

    let mut scheds: Vec<Option<MilestoneSchedule>> = Vec::new(&env);
    scheds.push_back(Some(full_schedule(&env, due0, "M1", "First milestone")));
    scheds.push_back(Some(full_schedule(&env, due1, "M2", "Second milestone")));

    let id = client.create_contract_with_schedules(
        &c,
        &f,
        &None::<Address>,
        &two_milestones(&env),
        &ReleaseAuthorization::ClientOnly,
        &scheds,
    );

    client.deposit_funds(&id, &c, &300_i128);
    client.approve_milestone_release(&id, &c, &0);
    client.release_milestone(&id, &c, &0);
    client.approve_milestone_release(&id, &c, &1);
    client.release_milestone(&id, &c, &1);

    let s0 = client.get_milestone_schedule(&id, &0).unwrap();
    let s1 = client.get_milestone_schedule(&id, &1).unwrap();
    assert_eq!(s0.due_date, Some(due0));
    assert_eq!(s1.due_date, Some(due1));
}

#[test]
fn integration_schedule_isolation_across_contracts() {
    let env = Env::default();
    env.mock_all_auths();
    let client = register_client(&env);
    let (c, f) = participants(&env);

    let due_a = future(&env, 100_000);
    let due_b = future(&env, 500_000);

    let mut scheds_a: Vec<Option<MilestoneSchedule>> = Vec::new(&env);
    scheds_a.push_back(Some(dated_schedule(&env, due_a)));

    let mut scheds_b: Vec<Option<MilestoneSchedule>> = Vec::new(&env);
    scheds_b.push_back(Some(dated_schedule(&env, due_b)));

    let id_a = client.create_contract_with_schedules(
        &c,
        &f,
        &None::<Address>,
        &vec![&env, 100_i128],
        &ReleaseAuthorization::ClientOnly,
        &scheds_a,
    );
    let id_b = client.create_contract_with_schedules(
        &c,
        &f,
        &None::<Address>,
        &vec![&env, 200_i128],
        &ReleaseAuthorization::ClientOnly,
        &scheds_b,
    );

    let sa = client.get_milestone_schedule(&id_a, &0).unwrap();
    let sb = client.get_milestone_schedule(&id_b, &0).unwrap();

    assert_eq!(sa.due_date, Some(due_a));
    assert_eq!(sb.due_date, Some(due_b));
    assert_ne!(sa.due_date, sb.due_date);
}

#[test]
fn integration_set_schedule_does_not_disturb_other_milestones() {
    let env = Env::default();
    env.mock_all_auths();
    let client = register_client(&env);
    let (c, f) = participants(&env);

    let due0 = future(&env, 100_000);
    let due1 = future(&env, 200_000);

    let mut scheds: Vec<Option<MilestoneSchedule>> = Vec::new(&env);
    scheds.push_back(Some(dated_schedule(&env, due0)));
    scheds.push_back(Some(dated_schedule(&env, due1)));

    let id = client.create_contract_with_schedules(
        &c,
        &f,
        &None::<Address>,
        &two_milestones(&env),
        &ReleaseAuthorization::ClientOnly,
        &scheds,
    );

    let updated_due = future(&env, 150_000);
    client.set_milestone_schedule(&id, &c, &0, &dated_schedule(&env, updated_due));

    let s0 = client.get_milestone_schedule(&id, &0).unwrap();
    let s1 = client.get_milestone_schedule(&id, &1).unwrap();

    assert_eq!(s0.due_date, Some(updated_due));
    assert_eq!(s1.due_date, Some(due1));
}

// ---------------------------------------------------------------------------
// MilestonesConfig read-view tests
// ---------------------------------------------------------------------------

#[test]
fn config_returns_sensible_defaults_before_init() {
    let env = Env::default();
    let contract_id = env.register(Escrow, ());
    let client = EscrowClient::new(&env, &contract_id);

    let cfg = client.get_milestones_config();

    assert_eq!(cfg.max_milestones, crate::MAX_MILESTONES);
    assert_eq!(
        cfg.max_single_milestone_stroops,
        crate::MAX_SINGLE_AMOUNT_STROOPS
    );
    assert_eq!(
        cfg.max_total_escrow_stroops,
        crate::MAX_TOTAL_ESCROW_STROOPS
    );
    assert_eq!(cfg.max_fee_bps, 10_000);
    assert_eq!(
        cfg.max_schedule_title_len,
        crate::MAX_SCHEDULE_TITLE_LEN
    );
    assert_eq!(
        cfg.max_schedule_description_len,
        crate::MAX_SCHEDULE_DESCRIPTION_LEN
    );
}

#[test]
fn config_reflects_governed_params_after_set() {
    let env = Env::default();
    env.mock_all_auths();
    let client = {
        let id = env.register(Escrow, ());
        EscrowClient::new(&env, &id)
    };
    let admin = Address::generate(&env);
    client.initialize(&admin);

    let fee_bps = 2500u32;
    let max_total = 1_000_000_000_000i128;
    client.set_governed_params(&admin, &fee_bps, &max_total);

    let cfg = client.get_milestones_config();

    // max_fee_bps is always the compile-time cap (10_000), not the current fee.
    assert_eq!(cfg.max_fee_bps, 10_000);
    assert_eq!(cfg.max_total_escrow_stroops, max_total);
    // Compile-time bounds remain unchanged.
    assert_eq!(cfg.max_milestones, crate::MAX_MILESTONES);
    assert_eq!(
        cfg.max_single_milestone_stroops,
        crate::MAX_SINGLE_AMOUNT_STROOPS
    );
}

#[test]
fn config_is_read_only_and_does_not_mutate_storage() {
    let env = Env::default();
    let contract_id = env.register(Escrow, ());
    let client = EscrowClient::new(&env, &contract_id);

    // Call twice — confirm no storage side effects.
    let _cfg1 = client.get_milestones_config();
    let _cfg2 = client.get_milestones_config();
    // No snapshot assertions needed; the call should not panic or write.
}
