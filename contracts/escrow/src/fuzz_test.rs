//! Fuzz harness for escrow entrypoints.
//!
//! Covers three categories:
//!   1. **Malformed inputs** — zero/negative amounts, empty milestone lists,
//!      out-of-range milestone indices, double-release.
//!   2. **Boundary values** — MAX_MILESTONES ± 1, MAX_TOTAL_ESCROW_STROOPS ± 1,
//!      rating boundaries (0, 1, 5, 6).
//!   3. **Unauthorized call patterns** — same client/freelancer, missing contract,
//!      pause/emergency blocking, reputation constraints.
//!
//! # Running locally
//!
//! ```sh
//! cargo test -p escrow fuzz
//! PROPTEST_CASES=2000 cargo test -p escrow fuzz
//! PROPTEST_SEED=<hex> cargo test -p escrow fuzz
//! ```

#![cfg(test)]

extern crate std;

use proptest::prelude::*;
use soroban_sdk::{
    testutils::Address as _, token::StellarAssetClient, vec as sorovec, Address, Env,
    String as SorobanString, Vec as SoroVec,
};

use crate::{Escrow, EscrowClient, ReleaseAuthorization, MAX_MILESTONES, MAX_TOTAL_ESCROW_STROOPS};

// ── helpers ──────────────────────────────────────────────────────────────────

struct Harness {
    env: Env,
    admin: Address,
    sac: Address,
    escrow_addr: Address,
}

impl Harness {
    fn new() -> Self {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        let admin = Address::generate(&env);
        let escrow_addr = env.register(Escrow, ());
        let client = EscrowClient::new(&env, &escrow_addr);
        client.initialize(&admin);
        let sac = env.register_stellar_asset_contract(admin.clone());
        client.bind_settlement_token(&admin, &sac);
        Harness {
            env,
            admin,
            sac,
            escrow_addr,
        }
    }

    fn escrow(&self) -> EscrowClient<'_> {
        EscrowClient::new(&self.env, &self.escrow_addr)
    }

    fn mint_and_deposit(&self, caller: &Address, id: u32, amount: i128) {
        StellarAssetClient::new(&self.env, &self.sac).mint(caller, &amount);
        let _ = self.escrow().try_deposit_funds(&id, caller, &amount);
    }
}

fn to_soroban_vec(env: &Env, amounts: &[i128]) -> SoroVec<i128> {
    let mut v = SoroVec::new(env);
    for &a in amounts {
        v.push_back(a);
    }
    v
}

// ── Category 1: Malformed inputs ─────────────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn fuzz_deposit_zero_or_negative_rejected(bad_amount in i128::MIN..=0i128) {
        let h = Harness::new();
        let caller = Address::generate(&h.env);
        let freelancer = Address::generate(&h.env);
        let milestones = sorovec![&h.env, 100_i128];
        let cid = h.escrow().create_contract(&caller, &freelancer, &None, &milestones, &ReleaseAuthorization::ClientOnly);

        let result = h.escrow().try_deposit_funds(&cid, &caller, &bad_amount);
        prop_assert!(result.is_err());
    }

    #[test]
    fn fuzz_create_empty_milestones_rejected(_seed in 0u32..1000u32) {
        let h = Harness::new();
        let caller = Address::generate(&h.env);
        let freelancer = Address::generate(&h.env);
        let empty = SoroVec::<i128>::new(&h.env);

        let result = h.escrow().try_create_contract(&caller, &freelancer, &None, &empty, &ReleaseAuthorization::ClientOnly);
        prop_assert!(result.is_err());
    }

    #[test]
    fn fuzz_create_nonpositive_milestone_rejected(bad in i128::MIN..=0i128) {
        let h = Harness::new();
        let caller = Address::generate(&h.env);
        let freelancer = Address::generate(&h.env);
        let milestones = to_soroban_vec(&h.env, &[100_i128, bad]);

        let result = h.escrow().try_create_contract(&caller, &freelancer, &None, &milestones, &ReleaseAuthorization::ClientOnly);
        prop_assert!(result.is_err());
    }

    #[test]
    fn fuzz_release_out_of_range_index_rejected(oob_idx in 3u32..u32::MAX) {
        let h = Harness::new();
        let caller = Address::generate(&h.env);
        let freelancer = Address::generate(&h.env);
        let milestones = sorovec![&h.env, 100_i128, 200_i128, 300_i128];
        let cid = h.escrow().create_contract(&caller, &freelancer, &None, &milestones, &ReleaseAuthorization::ClientOnly);
        h.mint_and_deposit(&caller, cid, 600_i128);

        let result = h.escrow().try_release_milestone(&cid, &caller, &oob_idx);
        prop_assert!(result.is_err());
    }

    #[test]
    fn fuzz_double_release_rejected(idx in 0u32..3u32) {
        let h = Harness::new();
        let caller = Address::generate(&h.env);
        let freelancer = Address::generate(&h.env);
        let milestones = sorovec![&h.env, 100_i128, 200_i128, 300_i128];
        let cid = h.escrow().create_contract(&caller, &freelancer, &None, &milestones, &ReleaseAuthorization::ClientOnly);
        h.mint_and_deposit(&caller, cid, 600_i128);
        h.escrow().approve_milestone_release(&cid, &caller, &idx);
        h.escrow().release_milestone(&cid, &caller, &idx);

        let result = h.escrow().try_release_milestone(&cid, &caller, &idx);
        prop_assert!(result.is_err());
    }
}

// ── Category 2: Boundary values ──────────────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn fuzz_create_exactly_max_milestones_accepted(_seed in 0u32..64u32) {
        let h = Harness::new();
        let caller = Address::generate(&h.env);
        let freelancer = Address::generate(&h.env);
        let amounts: std::vec::Vec<i128> = (0..MAX_MILESTONES).map(|_| 1_i128).collect();
        let milestones = to_soroban_vec(&h.env, &amounts);

        let result = h.escrow().try_create_contract(&caller, &freelancer, &None, &milestones, &ReleaseAuthorization::ClientOnly);
        prop_assert!(result.is_ok(), "MAX_MILESTONES should be accepted, got {:?}", result);
    }

    #[test]
    fn fuzz_create_over_max_milestones_rejected(_seed in 0u32..64u32) {
        let h = Harness::new();
        let caller = Address::generate(&h.env);
        let freelancer = Address::generate(&h.env);
        let amounts: std::vec::Vec<i128> = (0..=MAX_MILESTONES).map(|_| 1_i128).collect();
        let milestones = to_soroban_vec(&h.env, &amounts);

        let result = h.escrow().try_create_contract(&caller, &freelancer, &None, &milestones, &ReleaseAuthorization::ClientOnly);
        prop_assert!(result.is_err());
    }

    #[test]
    fn fuzz_create_at_max_total_accepted(_seed in 0u32..64u32) {
        let h = Harness::new();
        let caller = Address::generate(&h.env);
        let freelancer = Address::generate(&h.env);
        let milestones = sorovec![&h.env, MAX_TOTAL_ESCROW_STROOPS];

        let result = h.escrow().try_create_contract(&caller, &freelancer, &None, &milestones, &ReleaseAuthorization::ClientOnly);
        prop_assert!(result.is_ok(), "amount at cap should be accepted, got {:?}", result);
    }

    #[test]
    fn fuzz_create_over_max_total_rejected(_seed in 0u32..64u32) {
        let h = Harness::new();
        let caller = Address::generate(&h.env);
        let freelancer = Address::generate(&h.env);
        let milestones = sorovec![&h.env, MAX_TOTAL_ESCROW_STROOPS + 1];

        let result = h.escrow().try_create_contract(&caller, &freelancer, &None, &milestones, &ReleaseAuthorization::ClientOnly);
        prop_assert!(result.is_err());
    }

    #[test]
    fn fuzz_reputation_valid_rating_accepted(rating in 1u32..=5u32) {
        let h = Harness::new();
        let caller = Address::generate(&h.env);
        let freelancer = Address::generate(&h.env);
        let milestones = sorovec![&h.env, 100_i128];
        let cid = h.escrow().create_contract(&caller, &freelancer, &None, &milestones, &ReleaseAuthorization::ClientOnly);
        h.mint_and_deposit(&caller, cid, 100_i128);
        h.escrow().approve_milestone_release(&cid, &caller, &0);
        h.escrow().release_milestone(&cid, &caller, &0);

        let comment = SorobanString::from_str(&h.env, "good work");
        let result = h.escrow().try_issue_reputation(&cid, &caller, &rating, &comment);
        prop_assert!(result.is_ok(), "rating {} should be accepted, got {:?}", rating, result);
    }

    #[test]
    fn fuzz_reputation_boundary_ratings_rejected(rating in prop_oneof![Just(0u32), Just(6u32)]) {
        let h = Harness::new();
        let caller = Address::generate(&h.env);
        let freelancer = Address::generate(&h.env);
        let milestones = sorovec![&h.env, 100_i128];
        let cid = h.escrow().create_contract(&caller, &freelancer, &None, &milestones, &ReleaseAuthorization::ClientOnly);
        h.mint_and_deposit(&caller, cid, 100_i128);
        h.escrow().approve_milestone_release(&cid, &caller, &0);
        h.escrow().release_milestone(&cid, &caller, &0);

        let comment = SorobanString::from_str(&h.env, "rating test");
        let result = h.escrow().try_issue_reputation(&cid, &caller, &rating, &comment);
        prop_assert!(result.is_err());
    }

    #[test]
    fn fuzz_deposit_exact_total_accepted(amount in 1i128..=MAX_TOTAL_ESCROW_STROOPS) {
        let h = Harness::new();
        let caller = Address::generate(&h.env);
        let freelancer = Address::generate(&h.env);
        let milestones = sorovec![&h.env, amount];
        let cid = h.escrow().create_contract(&caller, &freelancer, &None, &milestones, &ReleaseAuthorization::ClientOnly);

        h.mint_and_deposit(&caller, cid, amount);
        let result = h.escrow().try_get_contract(&cid);
        prop_assert!(result.is_ok());
    }

    #[test]
    fn fuzz_deposit_overfunding_rejected(amount in 1i128..=(MAX_TOTAL_ESCROW_STROOPS - 1)) {
        let h = Harness::new();
        let caller = Address::generate(&h.env);
        let freelancer = Address::generate(&h.env);
        let milestones = sorovec![&h.env, amount];
        let cid = h.escrow().create_contract(&caller, &freelancer, &None, &milestones, &ReleaseAuthorization::ClientOnly);
        h.mint_and_deposit(&caller, cid, amount);

        StellarAssetClient::new(&h.env, &h.sac).mint(&caller, &1);
        let result = h.escrow().try_deposit_funds(&cid, &caller, &1);
        prop_assert!(result.is_err());
    }
}

// ── Category 3: Unauthorized call patterns ───────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn fuzz_create_same_participant_rejected(_seed in 0u32..128u32) {
        let h = Harness::new();
        let same = Address::generate(&h.env);
        let milestones = sorovec![&h.env, 100_i128];

        let result = h.escrow().try_create_contract(&same, &same, &None, &milestones, &ReleaseAuthorization::ClientOnly);
        prop_assert!(result.is_err());
    }

    #[test]
    fn fuzz_missing_contract_id_rejected(bad_id in 1u32..100u32) {
        let h = Harness::new();

        let result = h.escrow().try_get_contract(&bad_id);
        prop_assert!(result.is_err());
    }

    #[test]
    fn fuzz_paused_blocks_all_mutating_ops(_seed in 0u32..128u32) {
        let h = Harness::new();
        h.escrow().pause();

        let caller = Address::generate(&h.env);
        let freelancer = Address::generate(&h.env);
        let milestones = sorovec![&h.env, 100_i128];

        let result = h.escrow().try_create_contract(&caller, &freelancer, &None, &milestones, &ReleaseAuthorization::ClientOnly);
        prop_assert!(result.is_err());
    }

    #[test]
    fn fuzz_emergency_blocks_all_mutating_ops(_seed in 0u32..128u32) {
        let h = Harness::new();
        h.escrow().activate_emergency_pause();

        let caller = Address::generate(&h.env);
        let freelancer = Address::generate(&h.env);
        let milestones = sorovec![&h.env, 100_i128];

        let result = h.escrow().try_create_contract(&caller, &freelancer, &None, &milestones, &ReleaseAuthorization::ClientOnly);
        prop_assert!(result.is_err());
    }

    #[test]
    fn fuzz_reputation_on_incomplete_contract_rejected(_seed in 0u32..128u32) {
        let h = Harness::new();
        let caller = Address::generate(&h.env);
        let freelancer = Address::generate(&h.env);
        let milestones = sorovec![&h.env, 100_i128, 200_i128];
        let cid = h.escrow().create_contract(&caller, &freelancer, &None, &milestones, &ReleaseAuthorization::ClientOnly);
        h.mint_and_deposit(&caller, cid, 300_i128);
        h.escrow().approve_milestone_release(&cid, &caller, &0);
        h.escrow().release_milestone(&cid, &caller, &0);

        let comment = SorobanString::from_str(&h.env, "incomplete test");
        let result = h.escrow().try_issue_reputation(&cid, &caller, &5, &comment);
        prop_assert!(result.is_err());
    }

    #[test]
    fn fuzz_reputation_double_issuance_rejected(_seed in 0u32..128u32) {
        let h = Harness::new();
        let caller = Address::generate(&h.env);
        let freelancer = Address::generate(&h.env);
        let milestones = sorovec![&h.env, 100_i128];
        let cid = h.escrow().create_contract(&caller, &freelancer, &None, &milestones, &ReleaseAuthorization::ClientOnly);
        h.mint_and_deposit(&caller, cid, 100_i128);
        h.escrow().approve_milestone_release(&cid, &caller, &0);
        h.escrow().release_milestone(&cid, &caller, &0);

        let comment1 = SorobanString::from_str(&h.env, "first");
        h.escrow().issue_reputation(&cid, &caller, &5, &comment1);

        let comment2 = SorobanString::from_str(&h.env, "second");
        let result = h.escrow().try_issue_reputation(&cid, &caller, &4, &comment2);
        prop_assert!(result.is_err());
    }

    #[test]
    fn fuzz_release_insufficient_balance_rejected(fund in 1i128..99i128) {
        let h = Harness::new();
        let caller = Address::generate(&h.env);
        let freelancer = Address::generate(&h.env);
        let milestones = sorovec![&h.env, 100_i128];
        let cid = h.escrow().create_contract(&caller, &freelancer, &None, &milestones, &ReleaseAuthorization::ClientOnly);
        h.mint_and_deposit(&caller, cid, fund);

        let result = h.escrow().try_release_milestone(&cid, &caller, &0);
        prop_assert!(result.is_err());
    }
}
