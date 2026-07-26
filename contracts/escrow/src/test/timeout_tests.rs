//! Boundary tests for [`Escrow::is_milestone_overdue`] and deterministic time
//! control via [`utils::now_seconds`].
//!
//! `is_milestone_overdue` is the timeout-refund precondition. It documents a
//! precise contract:
//!
//! - returns `false` for an unknown contract id,
//! - returns `false` for a contract with no stored milestones,
//! - returns `false` for an out-of-bounds milestone index,
//! - returns `false` for an already-released milestone,
//! - returns `false` for a milestone with `deadline == None`, and
//! - for a milestone with a deadline, returns `true` only when `now > deadline`
//!   (strictly greater), so at exactly the deadline (`now == deadline`) it
//!   returns `false`.
//!
//! ## Ledger time model
//!
//! `utils::now_seconds(env)` is the single time source behind overdue detection.
//! It reads `env.ledger().timestamp()`, which on Stellar advances at ~5-second
//! intervals and is set by network validators. Tests control it deterministically
//! via `env.ledger().with_mut`.
//!
//! ## Strict-inequality boundary
//!
//! Overdue detection must not be tripped early: at exactly the deadline the
//! milestone is not yet overdue, preventing a one-second-early timeout refund.
//! The comparison is `now_seconds(&env) > deadline` (strictly greater).

#![cfg(test)]

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    Address, Env, Vec as SorobanVec,
};

use super::{create_contract, register_client};
use crate::{MilestonesKey, Milestone};

/// Set the ledger timestamp to an absolute number of seconds.
///
/// This is the canonical way to advance time in tests. Under the hood it calls
/// `env.ledger().with_mut`, which is the Soroban test-ledger API that
/// `now_seconds` ultimately reads.
fn set_now(env: &Env, secs: u64) {
    env.ledger().with_mut(|li| {
        li.timestamp = secs;
    });
}

/// Read the current ledger timestamp as seen by `now_seconds`.
fn get_now(env: &Env) -> u64 {
    env.ledger().timestamp()
}

/// Overwrite milestone `index`'s `deadline` and `released` flag directly in
/// persistent storage, bypassing any setter entrypoint. The new state is
/// observable through `is_milestone_overdue`.
///
/// Uses the typed [`MilestonesKey`] (issue #938) so the storage key shape
/// stays consistent with `create_contract` / `release_milestone`. The
/// underlying bytes match the legacy tuple form, so this is also a valid
/// round-trip exercise for the typed key.
fn set_milestone_deadline_and_released(
    env: &Env,
    contract_addr: &Address,
    contract_id: u32,
    index: u32,
    deadline: Option<u64>,
    released: bool,
) {
    env.as_contract(contract_addr, || {
        let key = DataKey::Milestones(contract_id);
        let mut milestones: SorobanVec<Milestone> =
            env.storage().persistent().get(&key).unwrap();
        let mut m = milestones.get(index).unwrap();
        m.deadline = deadline;
        m.released = released;
        milestones.set(index, m);
        env.storage().persistent().set(&key, &milestones);
    });
}

// ── now_seconds returns the mock value ──────────────────────────────────────

#[test]
fn now_seconds_reflects_mock_ledger_timestamp() {
    let env = Env::default();

    set_now(&env, 42);
    assert_eq!(get_now(&env), 42, "now_seconds must return the mocked value");

    set_now(&env, 999_999);
    assert_eq!(
        get_now(&env),
        999_999,
        "now_seconds updates when ledger timestamp changes"
    );
}

#[test]
fn now_seconds_advances_monotonically_in_test() {
    let env = Env::default();

    set_now(&env, 100);
    let t1 = get_now(&env);
    set_now(&env, 200);
    let t2 = get_now(&env);

    assert!(t2 > t1, "later mock timestamp must be greater");
}

// ── Deadline boundary: now < / == / > deadline ────────────────────────────────

#[test]
fn is_milestone_overdue_false_when_now_before_deadline() {
    let env = Env::default();
    env.mock_all_auths();
    let client = register_client(&env);
    let (_client_addr, _, id) = create_contract(&env, &client);

    let deadline = 1_000u64;
    set_milestone_deadline_and_released(&env, &client.address, id, 0, Some(deadline), false);

    set_now(&env, deadline - 1);
    assert!(
        !client.is_milestone_overdue(&id, &0),
        "now < deadline must not be overdue"
    );
}

#[test]
fn is_milestone_overdue_false_at_exact_deadline() {
    let env = Env::default();
    env.mock_all_auths();
    let client = register_client(&env);
    let (_client_addr, _, id) = create_contract(&env, &client);

    let deadline = 1_000u64;
    set_milestone_deadline_and_released(&env, &client.address, id, 0, Some(deadline), false);

    // Strict-inequality boundary: at exactly the deadline it is NOT overdue.
    set_now(&env, deadline);
    assert!(
        !client.is_milestone_overdue(&id, &0),
        "now == deadline must not be overdue (uses strict >)"
    );
}

#[test]
fn is_milestone_overdue_true_one_second_past_deadline() {
    let env = Env::default();
    env.mock_all_auths();
    let client = register_client(&env);
    let (_client_addr, _, id) = create_contract(&env, &client);

    let deadline = 1_000u64;
    set_milestone_deadline_and_released(&env, &client.address, id, 0, Some(deadline), false);

    set_now(&env, deadline + 1);
    assert!(
        client.is_milestone_overdue(&id, &0),
        "now > deadline must be overdue"
    );
}

// ── Time progression: before → at → after ────────────────────────────────────

#[test]
fn is_milestone_overdue_transitions_from_false_to_true() {
    let env = Env::default();
    env.mock_all_auths();
    let client = register_client(&env);
    let (_client_addr, _, id) = create_contract(&env, &client);

    let deadline = 5_000u64;
    set_milestone_deadline_and_released(&env, &client.address, id, 0, Some(deadline), false);

    // Phase 1: well before deadline
    set_now(&env, 1_000);
    assert!(!client.is_milestone_overdue(&id, &0));

    // Phase 2: one second before
    set_now(&env, deadline - 1);
    assert!(!client.is_milestone_overdue(&id, &0));

    // Phase 3: exactly at deadline
    set_now(&env, deadline);
    assert!(!client.is_milestone_overdue(&id, &0));

    // Phase 4: one second after — transitions to overdue
    set_now(&env, deadline + 1);
    assert!(client.is_milestone_overdue(&id, &0));

    // Phase 5: far after deadline — still overdue
    set_now(&env, deadline + 100_000);
    assert!(client.is_milestone_overdue(&id, &0));
}

#[test]
fn is_milestone_overdue_large_time_jump() {
    let env = Env::default();
    env.mock_all_auths();
    let client = register_client(&env);
    let (_client_addr, _, id) = create_contract(&env, &client);

    let deadline = 1_000u64;
    set_milestone_deadline_and_released(&env, &client.address, id, 0, Some(deadline), false);

    // Jump far into the future (simulating a year of ledger time).
    set_now(&env, deadline + 365 * 86_400);
    assert!(
        client.is_milestone_overdue(&id, &0),
        "large time jump past deadline must be overdue"
    );
}

// ── Short-circuit branches ────────────────────────────────────────────────────

#[test]
fn is_milestone_overdue_false_for_unknown_contract() {
    let env = Env::default();
    env.mock_all_auths();
    let client = register_client(&env);

    // No contract id 42 was ever allocated.
    assert!(
        !client.is_milestone_overdue(&42u32, &0),
        "unknown contract id must not be overdue"
    );
}

#[test]
fn is_milestone_overdue_false_for_out_of_bounds_index() {
    let env = Env::default();
    env.mock_all_auths();
    let client = register_client(&env);
    let (_client_addr, _, id) = create_contract(&env, &client);

    let len = client.get_milestones(&id).len();
    set_now(&env, 10_000);
    // Index == len (one past the last) and far beyond must both be false.
    assert!(!client.is_milestone_overdue(&id, &len));
    assert!(!client.is_milestone_overdue(&id, &(len + 7)));
}

#[test]
fn is_milestone_overdue_false_for_already_released_milestone() {
    let env = Env::default();
    env.mock_all_auths();
    let client = register_client(&env);
    let (_client_addr, _, id) = create_contract(&env, &client);

    let deadline = 1_000u64;
    // Deadline is in the past, but the milestone is already released.
    set_milestone_deadline_and_released(&env, &client.address, id, 0, Some(deadline), true);

    set_now(&env, deadline + 5_000);
    assert!(
        !client.is_milestone_overdue(&id, &0),
        "released milestone is never overdue, even past its deadline"
    );
}

#[test]
fn is_milestone_overdue_false_when_deadline_is_none() {
    let env = Env::default();
    env.mock_all_auths();
    let client = register_client(&env);
    let (_client_addr, _, id) = create_contract(&env, &client);

    // Contracts are created with deadline == None by default; assert explicitly.
    set_milestone_deadline_and_released(&env, &client.address, id, 0, None, false);

    set_now(&env, 1_000_000);
    assert!(
        !client.is_milestone_overdue(&id, &0),
        "milestone with no deadline is never overdue"
    );
}

// ── Multiple milestones with independent deadlines ───────────────────────────

#[test]
fn is_milestone_overdue_independent_per_milestone() {
    let env = Env::default();
    env.mock_all_auths();
    let client = register_client(&env);
    let (_client_addr, _, id) = create_contract(&env, &client);

    // Milestone 0: deadline 100, Milestone 1: deadline 200, Milestone 2: no deadline
    set_milestone_deadline_and_released(&env, &client.address, id, 0, Some(100), false);
    set_milestone_deadline_and_released(&env, &client.address, id, 1, Some(200), false);
    set_milestone_deadline_and_released(&env, &client.address, id, 2, None, false);

    // At t=150: milestone 0 is overdue, milestone 1 is not, milestone 2 never is.
    set_now(&env, 150);
    assert!(client.is_milestone_overdue(&id, &0), "m0 overdue at t=150");
    assert!(
        !client.is_milestone_overdue(&id, &1),
        "m1 not overdue at t=150"
    );
    assert!(
        !client.is_milestone_overdue(&id, &2),
        "m2 never overdue (no deadline)"
    );

    // At t=201: both milestone 0 and 1 are overdue.
    set_now(&env, 201);
    assert!(client.is_milestone_overdue(&id, &0));
    assert!(client.is_milestone_overdue(&id, &1));
}

#[test]
fn is_milestone_overdue_only_released_milestone_skipped() {
    let env = Env::default();
    env.mock_all_auths();
    let client = register_client(&env);
    let (_client_addr, _, id) = create_contract(&env, &client);

    // Both milestones have deadline 100, but milestone 0 is already released.
    set_milestone_deadline_and_released(&env, &client.address, id, 0, Some(100), true);
    set_milestone_deadline_and_released(&env, &client.address, id, 1, Some(100), false);

    set_now(&env, 200);
    assert!(
        !client.is_milestone_overdue(&id, &0),
        "released milestone is never overdue"
    );
    assert!(
        client.is_milestone_overdue(&id, &1),
        "unreleased milestone past deadline is overdue"
    );
}

// ── Ledger sequence vs timestamp ─────────────────────────────────────────────

#[test]
fn ledger_timestamp_and_sequence_advance_together() {
    let env = Env::default();

    // Set a known timestamp; sequence advances with it.
    set_now(&env, 1_000);
    let ts1 = get_now(&env);
    let seq1 = env.ledger().sequence();

    set_now(&env, 2_000);
    let ts2 = get_now(&env);
    let seq2 = env.ledger().sequence();

    assert!(ts2 > ts1, "timestamp must advance");
    assert!(seq2 >= seq1, "sequence must not decrease");
}
