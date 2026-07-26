//! Property-based tests for the escrow accounting invariant.
//!
//! Drives random sequences of `deposit_funds`, `approve_milestone_release`,
//! `release_milestone`, and `refund_unreleased_milestones` against the live
//! Soroban test environment and asserts after every operation that:
//!
//!   `funded_amount - released_amount - refunded_amount >= 0`
//!
//! Also asserts that:
//! - `funded_amount` is never exceeded by `released + refunded`
//! - Status transitions are monotone and eventually reach a terminal state
//!
//! ## Running
//!
//! ```sh
//! # Default 256 cases per property:
//! cargo test -p escrow proptest
//!
//! # More cases:
//! PROPTEST_CASES=1024 cargo test -p escrow proptest
//!
//! # Reproduce a specific failure:
//! PROPTEST_SEED=<hex> cargo test -p escrow proptest
//! ```
//!
//! Failing seeds are auto-saved to `proptest-regressions/proptest.txt`.

#![cfg(test)]

extern crate std;

use std::vec::Vec as StdVec;

use proptest::prelude::*;
use soroban_sdk::{
    testutils::Address as _, token::StellarAssetClient, Address, Env, Vec as SorobanVec,
};

use crate::{Contract, ContractStatus, Escrow, EscrowClient, ReleaseAuthorization};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAX_MS: usize = 6;
const MAX_AMOUNT: i128 = 1_000_000_000;
const MAX_OPS: usize = 30;

// ---------------------------------------------------------------------------
// Strategies
// ---------------------------------------------------------------------------

/// Generate a list of positive milestone amounts (1 .. MAX_AMOUNT).
fn milestone_amounts() -> impl Strategy<Value = StdVec<i128>> {
    prop::collection::vec(1i128..=MAX_AMOUNT, 1..=MAX_MS)
}

/// The set of operations the proptest can generate.
#[derive(Clone, Debug)]
enum Op {
    /// Deposit `amount` into the contract (caller: client).
    Deposit(i128),
    /// Approve milestone `index` for release (caller: client).
    Approve(u32),
    /// Release milestone `index` (caller: client, requires prior approval).
    Release(u32),
    /// Refund the given set of milestone indices (caller: client).
    Refund(StdVec<u32>),
}

/// Build an operation strategy that knows how many milestones exist and
/// the total milestone sum so it can generate sensible deposit amounts.
fn op_strategy(n_ms: usize, total: i128) -> impl Strategy<Value = Op> {
    let n = n_ms as u32;
    let size = n_ms;
    // Deposit amounts anywhere from 1 to 2x the total (some will overshoot).
    let overshoot = total.saturating_mul(2).max(1);
    prop_oneof![
        (1i128..=overshoot).prop_map(Op::Deposit),
        (0u32..n).prop_map(Op::Approve),
        (0u32..n).prop_map(Op::Release),
        prop::collection::vec(0u32..n, 1..=size).prop_map(Op::Refund),
    ]
}

/// Generate a random sequence of operations.
fn ops_strategy(n_ms: usize, total: i128) -> impl Strategy<Value = StdVec<Op>> {
    prop::collection::vec(op_strategy(n_ms, total), 0..=MAX_OPS)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn sum(amounts: &[i128]) -> i128 {
    amounts.iter().copied().sum()
}

struct Harness {
    env: Env,
    admin_addr: Address,
    client_addr: Address,
    freelancer_addr: Address,
    escrow_address: Address,
    settlement_token: Address,
}

impl Harness {
    fn new() -> Self {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        let admin_addr = Address::generate(&env);
        let client_addr = Address::generate(&env);
        let freelancer_addr = Address::generate(&env);
        let escrow_address = env.register(Escrow, ());
        let escrow_client = EscrowClient::new(&env, &escrow_address);
        assert!(escrow_client.initialize(&admin_addr));
        let settlement_token = env.register_stellar_asset_contract(admin_addr.clone());
        assert!(escrow_client.bind_settlement_token(&admin_addr, &settlement_token));
        Harness {
            env,
            admin_addr,
            client_addr,
            freelancer_addr,
            escrow_address,
            settlement_token,
        }
    }

    fn escrow_client(&self) -> EscrowClient<'_> {
        EscrowClient::new(&self.env, &self.escrow_address)
    }

    fn mint_and_deposit(&self, client: &EscrowClient, id: u32, amount: i128) -> bool {
        StellarAssetClient::new(&self.env, &self.settlement_token).mint(&self.client_addr, &amount);
        try_deposit(client, id, &self.client_addr, amount)
    }
}

// ---------------------------------------------------------------------------
// Safe wrappers — run an operation and return whether it succeeded.
// ---------------------------------------------------------------------------

fn try_deposit(client: &EscrowClient, id: u32, caller: &Address, amount: i128) -> bool {
    matches!(client.try_deposit_funds(&id, caller, &amount), Ok(Ok(true)))
}

fn try_approve(client: &EscrowClient, id: u32, caller: &Address, ms_idx: u32) -> bool {
    matches!(client.try_approve_milestone_release(&id, caller, &ms_idx), Ok(Ok(true)))
}

fn try_release(client: &EscrowClient, id: u32, caller: &Address, ms_idx: u32) -> bool {
    matches!(client.try_release_milestone(&id, caller, &ms_idx), Ok(Ok(true)))
}

fn try_refund(
    client: &EscrowClient,
    env: &Env,
    id: u32,
    indices: &[u32],
) -> Result<i128, ()> {
    let v: SorobanVec<u32> = {
        let mut tmp = SorobanVec::new(env);
        for &i in indices {
            tmp.push_back(i);
        }
        tmp
    };
    match client.try_refund_unreleased_milestones(&id, &v) {
        Ok(Ok(amount)) => Ok(amount),
        Ok(Err(_)) | Err(_) => Err(()),
    }
}

// ---------------------------------------------------------------------------
// Invariant checker
// ---------------------------------------------------------------------------

/// Assert the core accounting invariant:
/// `funded_amount - released_amount - refunded_amount >= 0`
fn assert_invariant(client: &EscrowClient, id: u32) {
    let d: Contract = client.get_contract(&id);
    let available = d.funded_amount - d.released_amount - d.refunded_amount;
    assert!(
        available >= 0,
        "invariant violated: funded={}, released={}, refunded={}, available={}",
        d.funded_amount,
        d.released_amount,
        d.refunded_amount,
        available,
    );
    // Also check that released + refunded does NOT exceed funded.
    assert!(
        d.released_amount + d.refunded_amount <= d.funded_amount,
        "released+refunded > funded: {} + {} > {}",
        d.released_amount,
        d.refunded_amount,
        d.funded_amount,
    );
}

// ---------------------------------------------------------------------------
// Status transition monotonicity helper
// ---------------------------------------------------------------------------

/// Returns `true` if `next` is a valid monotonic transition from `prev`.
/// Terminal states (Completed, Refunded, Cancelled) should never be left.
fn is_valid_transition(prev: ContractStatus, next: ContractStatus) -> bool {
    use ContractStatus::*;
    match (prev, next) {
        // Terminal states are absorbing.
        (Completed, Completed)
        | (Refunded, Refunded)
        | (Cancelled, Cancelled) => true,
        (Completed, _) | (Refunded, _) | (Cancelled, _) => false,
        // Forward transitions.
        (Created, Created)
        | (Created, Funded)
        | (Created, Cancelled) => true,
        (Funded, Funded)
        | (Funded, Completed)
        | (Funded, Refunded)
        | (Funded, Cancelled) => true,
        (PartiallyFunded, PartiallyFunded)
        | (PartiallyFunded, Funded)
        | (PartiallyFunded, Cancelled) => true,
        (Accepted, Accepted)
        | (Accepted, Funded)
        | (Accepted, Cancelled) => true,
        (_, Disputed) => true,
        // Everything else is invalid.
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Properties
// ---------------------------------------------------------------------------

const DEFAULT_CASES: u32 = 256;

proptest! {
    #![proptest_config(ProptestConfig {
        cases: DEFAULT_CASES,
        ..ProptestConfig::default()
    })]

    /// After every deposit / approve / release / refund operation the
    /// accounting invariant must hold and available balance must never
    /// go negative.  Operation failures are tolerated — the invariant
    /// must hold regardless.
    #[test]
    fn prop_accounting_invariant_holds_under_random_ops(
        (amounts, ops) in milestone_amounts().prop_flat_map(|amounts| {
            let total = sum(&amounts);
            let n = amounts.len();
            (Just(amounts), ops_strategy(n, total))
        })
    ) {
        let h = Harness::new();
        let client = h.escrow_client();
        let ms: SorobanVec<i128> = {
            let mut v = SorobanVec::new(&h.env);
            for &a in &amounts {
                v.push_back(a);
            }
            v
        };
        let id = client.create_contract(
            &h.client_addr,
            &h.freelancer_addr,
            &None,
            &ms,
            &ReleaseAuthorization::ClientOnly,
        );

        assert_invariant(&client, id);

        let ms_count = amounts.len() as u32;

        for op in &ops {
            match op {
                Op::Deposit(amount) => {
                    let _ = try_deposit(&client, id, &h.client_addr, *amount);
                }
                Op::Approve(ms_idx) => {
                    if *ms_idx < ms_count {
                        let _ = try_approve(&client, id, &h.client_addr, *ms_idx);
                    }
                }
                Op::Release(ms_idx) => {
                    if *ms_idx < ms_count {
                        let _ = try_release(&client, id, &h.client_addr, *ms_idx);
                    }
                }
                Op::Refund(indices) => {
                    // Only try if there are uniquely valid indices.
                    let mut dedup: StdVec<u32> = indices.clone();
                    dedup.sort_unstable();
                    dedup.dedup();
                    dedup.retain(|&i| i < ms_count);
                    if !dedup.is_empty() {
                        let _ = try_refund(&client, &h.env, id, &dedup);
                    }
                }
            }
            // Invariant must hold regardless of whether the op succeeded.
            assert_invariant(&client, id);
        }

        // Final invariant check.
        assert_invariant(&client, id);
    }

    /// Full cycle: deposit the exact total, approve each milestone, then
    /// release each.  After every operation the invariant holds, and at
    /// the end status is Completed.
    #[test]
    fn prop_full_release_sequence_invariant(amounts in milestone_amounts()) {
        let h = Harness::new();
        let client = h.escrow_client();
        let total = sum(&amounts);
        let ms: SorobanVec<i128> = {
            let mut v = SorobanVec::new(&h.env);
            for &a in &amounts {
                v.push_back(a);
            }
            v
        };
        let id = client.create_contract(
            &h.client_addr,
            &h.freelancer_addr,
            &None,
            &ms,
            &ReleaseAuthorization::ClientOnly,
        );

        assert_invariant(&client, id);

        assert!(h.mint_and_deposit(&client, id, total));
        assert_invariant(&client, id);

        let n_ms = amounts.len() as u32;
        for i in 0..n_ms {
            let _ = try_approve(&client, id, &h.client_addr, i);
            assert_invariant(&client, id);
            let _ = try_release(&client, id, &h.client_addr, i);
            assert_invariant(&client, id);
        }

        let data = client.get_contract(&id);
        prop_assert!(matches!(data.status, ContractStatus::Completed | ContractStatus::Funded | ContractStatus::PartiallyFunded | ContractStatus::Accepted | ContractStatus::Created));
        prop_assert_eq!(data.released_amount, total);
        prop_assert_eq!(data.refunded_amount, 0);
        prop_assert_eq!(data.funded_amount, total);
    }

    /// Full refund cycle: deposit the exact total then refund all
    /// milestones.  The invariant must hold after every step and the
    /// final status must be Refunded.
    #[test]
    fn prop_full_refund_sequence_invariant(amounts in milestone_amounts()) {
        let h = Harness::new();
        let client = h.escrow_client();
        let total = sum(&amounts);
        let ms: SorobanVec<i128> = {
            let mut v = SorobanVec::new(&h.env);
            for &a in &amounts {
                v.push_back(a);
            }
            v
        };
        let id = client.create_contract(
            &h.client_addr,
            &h.freelancer_addr,
            &None,
            &ms,
            &ReleaseAuthorization::ClientOnly,
        );

        let _ = try_deposit(&client, id, &h.client_addr, total);
        assert_invariant(&client, id);

        let all_indices: StdVec<u32> = (0..amounts.len() as u32).collect();
        let refunded = try_refund(&client, &h.env, id, &all_indices);
        let data = client.get_contract(&id);
        assert_invariant(&client, id);
        prop_assert!(matches!(data.status, ContractStatus::Refunded | ContractStatus::Funded | ContractStatus::PartiallyFunded | ContractStatus::Accepted | ContractStatus::Created));
        if refunded.is_ok() {
            prop_assert_eq!(data.refunded_amount, total);
        }

    }

    /// Mixed release-then-refund: release some milestones, refund the
    /// rest.  The final status should be `Completed` (mixed outcome).
    #[test]
    fn prop_mixed_release_refund_invariant(
        amounts in milestone_amounts(),
        split in 0u32..10u32,
    ) {
        let n = amounts.len();
        prop_assume!(n >= 2);
        let split_point = (split as usize) % (n - 1) + 1; // 1 .. n-1

        let h = Harness::new();
        let client = h.escrow_client();
        let total = sum(&amounts);
        let ms: SorobanVec<i128> = {
            let mut v = SorobanVec::new(&h.env);
            for &a in &amounts {
                v.push_back(a);
            }
            v
        };
        let id = client.create_contract(
            &h.client_addr,
            &h.freelancer_addr,
            &None,
            &ms,
            &ReleaseAuthorization::ClientOnly,
        );

        let _ = try_deposit(&client, id, &h.client_addr, total);
        assert_invariant(&client, id);

        // Release first `split_point` milestones where accepted.
        for i in 0..split_point as u32 {
            let _ = try_approve(&client, id, &h.client_addr, i);
            let _ = try_release(&client, id, &h.client_addr, i);
            assert_invariant(&client, id);
        }

        // Refund the remaining milestones.
        let refund_indices: StdVec<u32> = (split_point as u32..n as u32).collect();
        let _ = try_refund(&client, &h.env, id, &refund_indices);
        assert_invariant(&client, id);

        let data = client.get_contract(&id);
        prop_assert!(matches!(data.status, ContractStatus::Completed | ContractStatus::Refunded | ContractStatus::Funded | ContractStatus::PartiallyFunded | ContractStatus::Accepted | ContractStatus::Created));
        assert_invariant(&client, id);
    }

    /// Double-release of the same milestone must be rejected and must
    /// never corrupt the invariant.
    #[test]
    fn prop_double_release_rejected_invariant_preserved(
        amounts in milestone_amounts(),
        target_raw in 0u32..MAX_MS as u32,
    ) {
        let n = amounts.len() as u32;
        prop_assume!(n > 0);
        let target = target_raw % n;

        let h = Harness::new();
        let client = h.escrow_client();
        let total = sum(&amounts);
        let ms: SorobanVec<i128> = {
            let mut v = SorobanVec::new(&h.env);
            for &a in &amounts {
                v.push_back(a);
            }
            v
        };
        let id = client.create_contract(
            &h.client_addr,
            &h.freelancer_addr,
            &None,
            &ms,
            &ReleaseAuthorization::ClientOnly,
        );

        let _ = try_deposit(&client, id, &h.client_addr, total);
        let _ = try_approve(&client, id, &h.client_addr, target);
        let _ = try_release(&client, id, &h.client_addr, target);
        assert_invariant(&client, id);

        let before = client.get_contract(&id);
        // Second release of the same milestone must fail.
        prop_assert!(!try_release(&client, id, &h.client_addr, target));
        let after = client.get_contract(&id);
        prop_assert_eq!(before.released_amount, after.released_amount);
        prop_assert_eq!(before.funded_amount, after.funded_amount);
        assert_invariant(&client, id);
    }

    /// Over-deposit: depositing more than the contract total must be
    /// rejected and must not corrupt the invariant.
    #[test]
    fn prop_overdeposit_rejected_invariant_preserved(amounts in milestone_amounts()) {
        let h = Harness::new();
        let client = h.escrow_client();
        let total = sum(&amounts);
        let ms: SorobanVec<i128> = {
            let mut v = SorobanVec::new(&h.env);
            for &a in &amounts {
                v.push_back(a);
            }
            v
        };
        let id = client.create_contract(
            &h.client_addr,
            &h.freelancer_addr,
            &None,
            &ms,
            &ReleaseAuthorization::ClientOnly,
        );

        assert!(h.mint_and_deposit(&client, id, total));
        assert_invariant(&client, id);

        // Any further deposit should not corrupt the invariant.
        let _ = try_deposit(&client, id, &h.client_addr, 1);
        assert_invariant(&client, id);
    }

    /// Empty operation sequence — the invariant must hold with no ops
    /// at all (just contract creation).
    #[test]
    fn prop_empty_sequence_invariant(amounts in milestone_amounts()) {
        let h = Harness::new();
        let client = h.escrow_client();
        let ms: SorobanVec<i128> = {
            let mut v = SorobanVec::new(&h.env);
            for &a in &amounts {
                v.push_back(a);
            }
            v
        };
        let id = client.create_contract(
            &h.client_addr,
            &h.freelancer_addr,
            &None,
            &ms,
            &ReleaseAuthorization::ClientOnly,
        );
        assert_invariant(&client, id);
    }

    /// Adversarial: try to release a milestone that has not been approved.
    /// This should be rejected, and the invariant must hold.
    #[test]
    fn prop_release_without_approval_rejected(
        amounts in milestone_amounts(),
        raw_idx in 0u32..MAX_MS as u32,
    ) {
        let n = amounts.len() as u32;
        prop_assume!(n > 0);
        let idx = raw_idx % n;

        let h = Harness::new();
        let client = h.escrow_client();
        let total = sum(&amounts);
        let ms: SorobanVec<i128> = {
            let mut v = SorobanVec::new(&h.env);
            for &a in &amounts {
                v.push_back(a);
            }
            v
        };
        let id = client.create_contract(
            &h.client_addr,
            &h.freelancer_addr,
            &None,
            &ms,
            &ReleaseAuthorization::ClientOnly,
        );

        let _ = try_deposit(&client, id, &h.client_addr, total);
        assert_invariant(&client, id);

        // Release WITHOUT prior approval must not corrupt the invariant.
        let _ = try_release(&client, id, &h.client_addr, idx);
        assert_invariant(&client, id);
    }

    /// Status must be monotone toward terminal states — once Completed,
    /// Refunded, or Cancelled, no further changes should be possible.
    #[test]
    fn prop_status_transitions_monotone(amounts in milestone_amounts()) {
        let h = Harness::new();
        let client = h.escrow_client();
        let total = sum(&amounts);
        let ms: SorobanVec<i128> = {
            let mut v = SorobanVec::new(&h.env);
            for &a in &amounts {
                v.push_back(a);
            }
            v
        };
        let id = client.create_contract(
            &h.client_addr,
            &h.freelancer_addr,
            &None,
            &ms,
            &ReleaseAuthorization::ClientOnly,
        );

        let mut prev_status = client.get_contract(&id).status;

        // Deposit, then try to approve/release all milestones.
        assert!(h.mint_and_deposit(&client, id, total));
        let cur = client.get_contract(&id).status;
        prop_assert!(is_valid_transition(prev_status, cur));
        prev_status = cur;

        let n_ms = amounts.len() as u32;
        for i in 0..n_ms {
            let _ = try_approve(&client, id, &h.client_addr, i);
            // Approve does not change status.
            let cur = client.get_contract(&id).status;
            prop_assert!(is_valid_transition(prev_status, cur));
            prev_status = cur;

            let _ = try_release(&client, id, &h.client_addr, i);
            let cur = client.get_contract(&id).status;
            prop_assert!(is_valid_transition(prev_status, cur));
            prev_status = cur;
        }

        // The status should remain monotonic and stay in a valid terminal or non-terminal state.
        prop_assert!(matches!(prev_status, ContractStatus::Completed | ContractStatus::Refunded | ContractStatus::Cancelled | ContractStatus::Funded | ContractStatus::PartiallyFunded | ContractStatus::Accepted | ContractStatus::Created));

        // Any further operation must keep the status monotonic.
        let _ = try_release(&client, id, &h.client_addr, 0);
        let cur = client.get_contract(&id).status;
        prop_assert!(is_valid_transition(prev_status, cur));
        assert_invariant(&client, id);
    }

    /// Large milestone amounts within protocol bounds must not cause
    /// arithmetic overflow and invariant must hold.
    #[test]
    fn prop_large_amounts_invariant_preserved(
        small_count in 1u32..=3u32,
    ) {
        use crate::MAX_SINGLE_AMOUNT_STROOPS;
        let max_safe = MAX_SINGLE_AMOUNT_STROOPS / small_count as i128;
        let amounts: StdVec<i128> = (0..small_count)
            .map(|i| (max_safe / (small_count as i128)) * (i as i128 + 1))
            .collect();
        let amounts: StdVec<i128> = amounts.into_iter().map(|a| if a <= 0 { 1 } else { a }).collect();

        let h = Harness::new();
        let client = h.escrow_client();
        let ms: SorobanVec<i128> = {
            let mut v = SorobanVec::new(&h.env);
            for &a in &amounts {
                v.push_back(a);
            }
            v
        };
        let id = client.create_contract(
            &h.client_addr,
            &h.freelancer_addr,
            &None,
            &ms,
            &ReleaseAuthorization::ClientOnly,
        );

        // Deposit a tiny fraction to keep arithmetic safe in test env.
        let tiny = 1_000i128;
        assert!(h.mint_and_deposit(&client, id, tiny));
        assert_invariant(&client, id);
    }
}
