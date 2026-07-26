//! Role-by-action authorization matrix validation tests.
//!
//! This module provides exhaustive testing of authorization rules across all 5 roles
//! (`Admin`, `Client`, `Freelancer`, `Arbiter`, `Stranger`) and all state-mutating contract
//! entrypoints across all `ReleaseAuthorization` modes.
//!
//! Documented rules are verified against implementation in `contracts/escrow/src/lib.rs`,
//! `contracts/escrow/src/approvals.rs`, `contracts/escrow/src/release.rs`,
//! `contracts/escrow/src/deposit.rs`, `contracts/escrow/src/finalize.rs`,
//! `contracts/escrow/src/migration.rs`, and `contracts/escrow/src/governance.rs`.

#![cfg(test)]

use crate::{Error, Escrow, EscrowClient, EscrowError, ReleaseAuthorization};
use soroban_sdk::{
    testutils::Address as _, token::StellarAssetClient, vec, Address, Env, String,
};

use super::assert_contract_error;

/// Full test environment setup returning client, contract ID, and all role addresses.
struct TestEnv<'a> {
    env: Env,
    client: EscrowClient<'a>,
    admin: Address,
    client_addr: Address,
    freelancer_addr: Address,
    arbiter_addr: Address,
    stranger_addr: Address,
    token_addr: Address,
}

fn setup_full() -> TestEnv<'static> {
    let env = Env::default();
    env.mock_all_auths_allowing_non_root_auth();

    let contract_id = env.register(Escrow, ());
    let client = EscrowClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    client.initialize(&admin);

    let token_addr = env.register_stellar_asset_contract(admin.clone());
    client.bind_settlement_token(&admin, &token_addr);

    let client_addr = Address::generate(&env);
    let freelancer_addr = Address::generate(&env);
    let arbiter_addr = Address::generate(&env);
    let stranger_addr = Address::generate(&env);

    TestEnv {
        env,
        client,
        admin,
        client_addr,
        freelancer_addr,
        arbiter_addr,
        stranger_addr,
        token_addr,
    }
}

fn create_funded_contract(
    test_env: &TestEnv,
    auth: &ReleaseAuthorization,
) -> u32 {
    let milestones = vec![&test_env.env, 500_0000000_i128, 300_0000000_i128];
    let arbiter = match auth {
        ReleaseAuthorization::ArbiterOnly | ReleaseAuthorization::ClientAndArbiter => {
            Some(test_env.arbiter_addr.clone())
        }
        _ => None,
    };
    let id = test_env.client.create_contract(
        &test_env.client_addr,
        &test_env.freelancer_addr,
        &arbiter,
        &milestones,
        auth,
    );
    let total = 800_0000000_i128;
    StellarAssetClient::new(&test_env.env, &test_env.token_addr).mint(&test_env.client_addr, &total);
    test_env.client.deposit_funds(&id, &test_env.client_addr, &total);
    id
}

// ===========================================================================
// 1. Release Authorization Approvals Matrix (5 Roles x 4 Modes)
// ===========================================================================

#[test]
fn matrix_approve_client_only_all_roles() {
    let t = setup_full();
    let id = create_funded_contract(&t, &ReleaseAuthorization::ClientOnly);

    // Client: ALLOW
    assert!(t.client.approve_milestone_release(&id, &t.client_addr, &0));

    // Reset contract for testing other roles
    let id2 = create_funded_contract(&t, &ReleaseAuthorization::ClientOnly);

    // Admin: DENY
    assert_contract_error(
        t.client.try_approve_milestone_release(&id2, &t.admin, &0),
        Error::UnauthorizedRole,
    );

    // Freelancer: DENY
    assert_contract_error(
        t.client.try_approve_milestone_release(&id2, &t.freelancer_addr, &0),
        Error::UnauthorizedRole,
    );

    // Arbiter: DENY
    assert_contract_error(
        t.client.try_approve_milestone_release(&id2, &t.arbiter_addr, &0),
        Error::UnauthorizedRole,
    );

    // Stranger: DENY
    assert_contract_error(
        t.client.try_approve_milestone_release(&id2, &t.stranger_addr, &0),
        Error::UnauthorizedRole,
    );
}

#[test]
fn matrix_approve_arbiter_only_all_roles() {
    let t = setup_full();
    let id = create_funded_contract(&t, &ReleaseAuthorization::ArbiterOnly);

    // Arbiter: ALLOW
    assert!(t.client.approve_milestone_release(&id, &t.arbiter_addr, &0));

    let id2 = create_funded_contract(&t, &ReleaseAuthorization::ArbiterOnly);

    // Client: DENY
    assert_contract_error(
        t.client.try_approve_milestone_release(&id2, &t.client_addr, &0),
        Error::UnauthorizedRole,
    );

    // Admin: DENY
    assert_contract_error(
        t.client.try_approve_milestone_release(&id2, &t.admin, &0),
        Error::UnauthorizedRole,
    );

    // Freelancer: DENY
    assert_contract_error(
        t.client.try_approve_milestone_release(&id2, &t.freelancer_addr, &0),
        Error::UnauthorizedRole,
    );

    // Stranger: DENY
    assert_contract_error(
        t.client.try_approve_milestone_release(&id2, &t.stranger_addr, &0),
        Error::UnauthorizedRole,
    );
}

#[test]
fn matrix_approve_client_and_arbiter_all_roles() {
    let t = setup_full();
    let id = create_funded_contract(&t, &ReleaseAuthorization::ClientAndArbiter);

    // Client: ALLOW
    assert!(t.client.approve_milestone_release(&id, &t.client_addr, &0));

    let id2 = create_funded_contract(&t, &ReleaseAuthorization::ClientAndArbiter);

    // Arbiter: ALLOW
    assert!(t.client.approve_milestone_release(&id2, &t.arbiter_addr, &0));

    let id3 = create_funded_contract(&t, &ReleaseAuthorization::ClientAndArbiter);

    // Admin: DENY
    assert_contract_error(
        t.client.try_approve_milestone_release(&id3, &t.admin, &0),
        Error::UnauthorizedRole,
    );

    // Freelancer: DENY
    assert_contract_error(
        t.client.try_approve_milestone_release(&id3, &t.freelancer_addr, &0),
        Error::UnauthorizedRole,
    );

    // Stranger: DENY
    assert_contract_error(
        t.client.try_approve_milestone_release(&id3, &t.stranger_addr, &0),
        Error::UnauthorizedRole,
    );
}

#[test]
fn matrix_approve_multisig_all_roles() {
    let t = setup_full();
    let id = create_funded_contract(&t, &ReleaseAuthorization::MultiSig);

    // Client: ALLOW
    assert!(t.client.approve_milestone_release(&id, &t.client_addr, &0));

    // Freelancer: ALLOW
    assert!(t.client.approve_milestone_release(&id, &t.freelancer_addr, &0));

    let id2 = create_funded_contract(&t, &ReleaseAuthorization::MultiSig);

    // Admin: DENY
    assert_contract_error(
        t.client.try_approve_milestone_release(&id2, &t.admin, &0),
        Error::UnauthorizedRole,
    );

    // Arbiter: DENY
    assert_contract_error(
        t.client.try_approve_milestone_release(&id2, &t.arbiter_addr, &0),
        Error::UnauthorizedRole,
    );

    // Stranger: DENY
    assert_contract_error(
        t.client.try_approve_milestone_release(&id2, &t.stranger_addr, &0),
        Error::UnauthorizedRole,
    );
}

// ===========================================================================
// 2. Release Milestone Matrix (5 Roles x 4 Modes)
// ===========================================================================

#[test]
fn matrix_release_client_only_all_roles() {
    let t = setup_full();
    let id = create_funded_contract(&t, &ReleaseAuthorization::ClientOnly);
    assert!(t.client.approve_milestone_release(&id, &t.client_addr, &0));

    // Client: ALLOW
    assert!(t.client.release_milestone(&id, &t.client_addr, &0));

    let id2 = create_funded_contract(&t, &ReleaseAuthorization::ClientOnly);
    assert!(t.client.approve_milestone_release(&id2, &t.client_addr, &0));

    // Admin: DENY
    assert_contract_error(
        t.client.try_release_milestone(&id2, &t.admin, &0),
        Error::UnauthorizedRole,
    );

    // Freelancer: DENY
    assert_contract_error(
        t.client.try_release_milestone(&id2, &t.freelancer_addr, &0),
        Error::UnauthorizedRole,
    );

    // Arbiter: DENY
    assert_contract_error(
        t.client.try_release_milestone(&id2, &t.arbiter_addr, &0),
        Error::UnauthorizedRole,
    );

    // Stranger: DENY
    assert_contract_error(
        t.client.try_release_milestone(&id2, &t.stranger_addr, &0),
        Error::UnauthorizedRole,
    );
}

#[test]
fn matrix_release_arbiter_only_all_roles() {
    let t = setup_full();
    let id = create_funded_contract(&t, &ReleaseAuthorization::ArbiterOnly);
    assert!(t.client.approve_milestone_release(&id, &t.arbiter_addr, &0));

    // Arbiter: ALLOW
    assert!(t.client.release_milestone(&id, &t.arbiter_addr, &0));

    let id2 = create_funded_contract(&t, &ReleaseAuthorization::ArbiterOnly);
    assert!(t.client.approve_milestone_release(&id2, &t.arbiter_addr, &0));

    // Client: DENY
    assert_contract_error(
        t.client.try_release_milestone(&id2, &t.client_addr, &0),
        Error::UnauthorizedRole,
    );

    // Admin: DENY
    assert_contract_error(
        t.client.try_release_milestone(&id2, &t.admin, &0),
        Error::UnauthorizedRole,
    );

    // Freelancer: DENY
    assert_contract_error(
        t.client.try_release_milestone(&id2, &t.freelancer_addr, &0),
        Error::UnauthorizedRole,
    );

    // Stranger: DENY
    assert_contract_error(
        t.client.try_release_milestone(&id2, &t.stranger_addr, &0),
        Error::UnauthorizedRole,
    );
}

#[test]
fn matrix_release_client_and_arbiter_all_roles() {
    let t = setup_full();

    // Client release
    let id1 = create_funded_contract(&t, &ReleaseAuthorization::ClientAndArbiter);
    assert!(t.client.approve_milestone_release(&id1, &t.client_addr, &0));
    assert!(t.client.release_milestone(&id1, &t.client_addr, &0));

    // Arbiter release
    let id2 = create_funded_contract(&t, &ReleaseAuthorization::ClientAndArbiter);
    assert!(t.client.approve_milestone_release(&id2, &t.arbiter_addr, &0));
    assert!(t.client.release_milestone(&id2, &t.arbiter_addr, &0));

    // Test unauthorized roles
    let id3 = create_funded_contract(&t, &ReleaseAuthorization::ClientAndArbiter);
    assert!(t.client.approve_milestone_release(&id3, &t.client_addr, &0));

    // Admin: DENY
    assert_contract_error(
        t.client.try_release_milestone(&id3, &t.admin, &0),
        Error::UnauthorizedRole,
    );

    // Freelancer: DENY
    assert_contract_error(
        t.client.try_release_milestone(&id3, &t.freelancer_addr, &0),
        Error::UnauthorizedRole,
    );

    // Stranger: DENY
    assert_contract_error(
        t.client.try_release_milestone(&id3, &t.stranger_addr, &0),
        Error::UnauthorizedRole,
    );
}

#[test]
fn matrix_release_multisig_all_roles() {
    let t = setup_full();

    // Both approve
    let id1 = create_funded_contract(&t, &ReleaseAuthorization::MultiSig);
    assert!(t.client.approve_milestone_release(&id1, &t.client_addr, &0));
    assert!(t.client.approve_milestone_release(&id1, &t.freelancer_addr, &0));

    // Client: ALLOW
    assert!(t.client.release_milestone(&id1, &t.client_addr, &0));

    // Freelancer: ALLOW
    let id2 = create_funded_contract(&t, &ReleaseAuthorization::MultiSig);
    assert!(t.client.approve_milestone_release(&id2, &t.client_addr, &0));
    assert!(t.client.approve_milestone_release(&id2, &t.freelancer_addr, &0));
    assert!(t.client.release_milestone(&id2, &t.freelancer_addr, &0));

    // Unauthorized roles
    let id3 = create_funded_contract(&t, &ReleaseAuthorization::MultiSig);
    assert!(t.client.approve_milestone_release(&id3, &t.client_addr, &0));
    assert!(t.client.approve_milestone_release(&id3, &t.freelancer_addr, &0));

    // Admin: DENY
    assert_contract_error(
        t.client.try_release_milestone(&id3, &t.admin, &0),
        Error::UnauthorizedRole,
    );

    // Arbiter: DENY
    assert_contract_error(
        t.client.try_release_milestone(&id3, &t.arbiter_addr, &0),
        Error::UnauthorizedRole,
    );

    // Stranger: DENY
    assert_contract_error(
        t.client.try_release_milestone(&id3, &t.stranger_addr, &0),
        Error::UnauthorizedRole,
    );
}

// ===========================================================================
// 3. Deposit Funds Matrix across 5 Roles
// ===========================================================================

#[test]
fn matrix_deposit_funds_all_roles() {
    let t = setup_full();
    let milestones = vec![&t.env, 500_0000000_i128];
    let id = t.client.create_contract(
        &t.client_addr,
        &t.freelancer_addr,
        &None,
        &milestones,
        &ReleaseAuthorization::ClientOnly,
    );

    let amount = 500_0000000_i128;
    StellarAssetClient::new(&t.env, &t.token_addr).mint(&t.client_addr, &amount);
    StellarAssetClient::new(&t.env, &t.token_addr).mint(&t.admin, &amount);
    StellarAssetClient::new(&t.env, &t.token_addr).mint(&t.freelancer_addr, &amount);
    StellarAssetClient::new(&t.env, &t.token_addr).mint(&t.arbiter_addr, &amount);
    StellarAssetClient::new(&t.env, &t.token_addr).mint(&t.stranger_addr, &amount);

    // Admin: DENY
    assert_contract_error(
        t.client.try_deposit_funds(&id, &t.admin, &amount),
        Error::UnauthorizedRole,
    );

    // Freelancer: DENY
    assert_contract_error(
        t.client.try_deposit_funds(&id, &t.freelancer_addr, &amount),
        Error::UnauthorizedRole,
    );

    // Arbiter: DENY
    assert_contract_error(
        t.client.try_deposit_funds(&id, &t.arbiter_addr, &amount),
        Error::UnauthorizedRole,
    );

    // Stranger: DENY
    assert_contract_error(
        t.client.try_deposit_funds(&id, &t.stranger_addr, &amount),
        Error::UnauthorizedRole,
    );

    // Client: ALLOW
    assert!(t.client.deposit_funds(&id, &t.client_addr, &amount));
}

// ===========================================================================
// 4. Issue Reputation Matrix across 5 Roles
// ===========================================================================

#[test]
fn matrix_issue_reputation_all_roles() {
    let t = setup_full();
    let milestones = vec![&t.env, 500_0000000_i128];
    let id = t.client.create_contract(
        &t.client_addr,
        &t.freelancer_addr,
        &None,
        &milestones,
        &ReleaseAuthorization::ClientOnly,
    );
    let amount = 500_0000000_i128;
    StellarAssetClient::new(&t.env, &t.token_addr).mint(&t.client_addr, &amount);
    t.client.deposit_funds(&id, &t.client_addr, &amount);
    t.client.approve_milestone_release(&id, &t.client_addr, &0);
    t.client.release_milestone(&id, &t.client_addr, &0);

    // Contract is now completed.
    let comment = String::from_str(&t.env, "Great work!");

    // Admin: DENY
    assert_contract_error(
        t.client.try_issue_reputation(&id, &t.admin, &5, &comment),
        Error::UnauthorizedRole,
    );

    // Freelancer: DENY
    assert_contract_error(
        t.client.try_issue_reputation(&id, &t.freelancer_addr, &5, &comment),
        Error::UnauthorizedRole,
    );

    // Arbiter: DENY
    assert_contract_error(
        t.client.try_issue_reputation(&id, &t.arbiter_addr, &5, &comment),
        Error::UnauthorizedRole,
    );

    // Stranger: DENY
    assert_contract_error(
        t.client.try_issue_reputation(&id, &t.stranger_addr, &5, &comment),
        Error::UnauthorizedRole,
    );

    // Client: ALLOW
    assert!(t.client.issue_reputation(&id, &t.client_addr, &5, &comment));
}

// ===========================================================================
// 5. Submit Work Evidence Matrix across 5 Roles
// ===========================================================================

#[test]
fn matrix_submit_work_evidence_all_roles() {
    let t = setup_full();
    let id = create_funded_contract(&t, &ReleaseAuthorization::ClientOnly);
    let cid = String::from_str(&t.env, "QmTestEvidence1234567890");

    // Admin: DENY
    assert_contract_error(
        t.client.try_submit_work_evidence(&id, &t.admin, &0, &cid),
        Error::UnauthorizedRole,
    );

    // Client: DENY
    assert_contract_error(
        t.client.try_submit_work_evidence(&id, &t.client_addr, &0, &cid),
        Error::UnauthorizedRole,
    );

    // Arbiter: DENY
    assert_contract_error(
        t.client.try_submit_work_evidence(&id, &t.arbiter_addr, &0, &cid),
        Error::UnauthorizedRole,
    );

    // Stranger: DENY
    assert_contract_error(
        t.client.try_submit_work_evidence(&id, &t.stranger_addr, &0, &cid),
        Error::UnauthorizedRole,
    );

    // Freelancer: ALLOW
    assert!(t.client.submit_work_evidence(&id, &t.freelancer_addr, &0, &cid));
}

// ===========================================================================
// 6. Contract Finalization Matrix across 5 Roles
// ===========================================================================

#[test]
fn matrix_finalize_contract_all_roles() {
    let t = setup_full();

    // Complete a contract
    let milestones = vec![&t.env, 500_0000000_i128];
    let id1 = t.client.create_contract(
        &t.client_addr,
        &t.freelancer_addr,
        &Some(t.arbiter_addr.clone()),
        &milestones,
        &ReleaseAuthorization::ClientOnly,
    );
    let amount = 500_0000000_i128;
    StellarAssetClient::new(&t.env, &t.token_addr).mint(&t.client_addr, &amount);
    t.client.deposit_funds(&id1, &t.client_addr, &amount);
    t.client.approve_milestone_release(&id1, &t.client_addr, &0);
    t.client.release_milestone(&id1, &t.client_addr, &0);

    // Admin (not a participant): DENY
    assert_contract_error(
        t.client.try_finalize_contract(&id1, &t.admin),
        Error::UnauthorizedRole,
    );

    // Stranger: DENY
    assert_contract_error(
        t.client.try_finalize_contract(&id1, &t.stranger_addr),
        Error::UnauthorizedRole,
    );

    // Client: ALLOW
    assert!(t.client.finalize_contract(&id1, &t.client_addr));

    // Test Freelancer finalization on another completed contract
    let id2 = t.client.create_contract(
        &t.client_addr,
        &t.freelancer_addr,
        &Some(t.arbiter_addr.clone()),
        &milestones,
        &ReleaseAuthorization::ClientOnly,
    );
    StellarAssetClient::new(&t.env, &t.token_addr).mint(&t.client_addr, &amount);
    t.client.deposit_funds(&id2, &t.client_addr, &amount);
    t.client.approve_milestone_release(&id2, &t.client_addr, &0);
    t.client.release_milestone(&id2, &t.client_addr, &0);

    // Freelancer: ALLOW
    assert!(t.client.finalize_contract(&id2, &t.freelancer_addr));

    // Test Arbiter finalization on another completed contract
    let id3 = t.client.create_contract(
        &t.client_addr,
        &t.freelancer_addr,
        &Some(t.arbiter_addr.clone()),
        &milestones,
        &ReleaseAuthorization::ClientOnly,
    );
    StellarAssetClient::new(&t.env, &t.token_addr).mint(&t.client_addr, &amount);
    t.client.deposit_funds(&id3, &t.client_addr, &amount);
    t.client.approve_milestone_release(&id3, &t.client_addr, &0);
    t.client.release_milestone(&id3, &t.client_addr, &0);

    // Arbiter: ALLOW
    assert!(t.client.finalize_contract(&id3, &t.arbiter_addr));
}

// ===========================================================================
// 7. Client Migration Matrix across 5 Roles
// ===========================================================================

#[test]
fn matrix_client_migration_all_roles() {
    let t = setup_full();
    let id = create_funded_contract(&t, &ReleaseAuthorization::ClientOnly);
    let new_client = Address::generate(&t.env);

    // Propose migration:
    // Admin: DENY
    assert_contract_error(
        t.client.try_propose_client_migration(&id, &t.admin, &new_client),
        Error::UnauthorizedRole,
    );

    // Freelancer: DENY
    assert_contract_error(
        t.client.try_propose_client_migration(&id, &t.freelancer_addr, &new_client),
        Error::UnauthorizedRole,
    );

    // Arbiter: DENY
    assert_contract_error(
        t.client.try_propose_client_migration(&id, &t.arbiter_addr, &new_client),
        Error::UnauthorizedRole,
    );

    // Stranger: DENY
    assert_contract_error(
        t.client.try_propose_client_migration(&id, &t.stranger_addr, &new_client),
        Error::UnauthorizedRole,
    );

    // Client: ALLOW
    assert!(t.client.propose_client_migration(&id, &t.client_addr, &new_client));

    // Accept migration:
    // Old Client: DENY
    assert_contract_error(
        t.client.try_accept_client_migration(&id, &t.client_addr),
        Error::UnauthorizedRole,
    );

    // Admin: DENY
    assert_contract_error(
        t.client.try_accept_client_migration(&id, &t.admin),
        Error::UnauthorizedRole,
    );

    // Freelancer: DENY
    assert_contract_error(
        t.client.try_accept_client_migration(&id, &t.freelancer_addr),
        Error::UnauthorizedRole,
    );

    // Stranger: DENY
    assert_contract_error(
        t.client.try_accept_client_migration(&id, &t.stranger_addr),
        Error::UnauthorizedRole,
    );

    // New Client: ALLOW
    assert!(t.client.accept_client_migration(&id, &new_client));
}

// ===========================================================================
// 8. Admin-Only Governance & Control Operations Matrix across 5 Roles
// ===========================================================================

#[test]
fn matrix_admin_operations_all_roles() {
    let env = Env::default();
    env.mock_all_auths_allowing_non_root_auth();
    let contract_id = env.register(Escrow, ());
    let client = EscrowClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    client.initialize(&admin);

    let client_addr = Address::generate(&env);
    let freelancer_addr = Address::generate(&env);
    let arbiter_addr = Address::generate(&env);
    let stranger_addr = Address::generate(&env);
    let new_token = env.register_stellar_asset_contract(admin.clone());

    // set_settlement_token:
    // Client: DENY
    assert_contract_error(
        client.try_set_settlement_token(&client_addr, &new_token),
        Error::UnauthorizedRole,
    );

    // Freelancer: DENY
    assert_contract_error(
        client.try_set_settlement_token(&freelancer_addr, &new_token),
        Error::UnauthorizedRole,
    );

    // Arbiter: DENY
    assert_contract_error(
        client.try_set_settlement_token(&arbiter_addr, &new_token),
        Error::UnauthorizedRole,
    );

    // Stranger: DENY
    assert_contract_error(
        client.try_set_settlement_token(&stranger_addr, &new_token),
        Error::UnauthorizedRole,
    );

    // Admin: ALLOW
    assert!(client.set_settlement_token(&admin, &new_token));

    // set_max_milestones:
    assert!(client.set_max_milestones(&10));

    // set_max_escrow_stroops:
    assert!(client.set_max_escrow_stroops(&1_000_000_0000000_i128));
}

// ===========================================================================
// 9. Error Code Assertions
// ===========================================================================

#[test]
fn matrix_error_codes_unauthorized_role() {
    let t = setup_full();
    let id = create_funded_contract(&t, &ReleaseAuthorization::ClientOnly);

    let result = t.client.try_approve_milestone_release(&id, &t.freelancer_addr, &0);
    assert_contract_error(result, Error::UnauthorizedRole);
}

#[test]
fn matrix_error_codes_already_approved() {
    let t = setup_full();
    let id = create_funded_contract(&t, &ReleaseAuthorization::ClientOnly);

    assert!(t.client.approve_milestone_release(&id, &t.client_addr, &0));

    let result = t.client.try_approve_milestone_release(&id, &t.client_addr, &0);
    assert_contract_error(result, crate::Error::AlreadyApproved);
}

#[test]
fn matrix_error_codes_insufficient_approvals() {
    let t = setup_full();
    let id = create_funded_contract(&t, &ReleaseAuthorization::ClientOnly);

    let result = t.client.try_release_milestone(&id, &t.client_addr, &0);
    assert_contract_error(result, crate::Error::InsufficientApprovals);
}

#[test]
fn matrix_error_codes_missing_arbiter() {
    let t = setup_full();
    let milestones = vec![&t.env, 500_0000000_i128];

    let result = t.client.try_create_contract(
        &t.client_addr,
        &t.freelancer_addr,
        &None,
        &milestones,
        &ReleaseAuthorization::ArbiterOnly,
    );
    assert!(result.is_err(), "ArbiterOnly mode should require arbiter at contract creation");
}
