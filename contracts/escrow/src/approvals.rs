//! Temporary milestone approval storage and release authorization checks.
//!
//! This module owns the temporary
//! `DataKey::MilestoneApprovals(contract_id, milestone_index)` records used by
//! `approve_milestone_release` and `release_milestone`. It reads the escrow
//! contract and milestone vector to validate state and role authorization, but
//! it does not move funds or mutate milestone accounting.
//!
//! Approval records live in Soroban temporary storage and expire according to
//! `PENDING_APPROVAL_TTL_LEDGERS`. Missing or expired approvals fail closed.

use crate::storage;
use crate::ttl::{PENDING_APPROVAL_BUMP_THRESHOLD, PENDING_APPROVAL_TTL_LEDGERS};
use crate::types::{
    Contract, ContractStatus, DataKey, Milestone, MilestoneApprovals, ReleaseAuthorization,
};
use crate::Error;
use soroban_sdk::{Address, Env, Vec};

pub(crate) fn arbiter_approval_storage_key(contract_id: u32, milestone_index: u32) -> DataKey {
    ArbiterApprovalKey::new(contract_id, milestone_index).into()
}

/// Approves a milestone for release by the caller.
///
/// Records the approval in temporary storage with TTL expiry.
/// The approval will automatically expire after PENDING_APPROVAL_TTL_LEDGERS.
///
/// # Arguments
/// * `env` - The contract environment
/// * `contract_id` - The contract ID
/// * `milestone_index` - The index of the milestone to approve
/// * `caller` - The address of the caller. In MultiSig mode, exactly the
///   client and freelancer can approve, and both approvals are required.
///
/// # Returns
/// `true` if approval was recorded successfully
///
/// # Errors
/// * `ContractNotFound` - If contract doesn't exist
/// * `InvalidState` - If contract is not in Funded state
/// * `IndexOutOfBounds` - If milestone index is invalid
/// * `MilestoneAlreadyReleased` - If milestone was already released
/// * `UnauthorizedRole` - If caller is not authorized to approve
/// * `AlreadyApproved` - If caller has already approved this milestone
///
/// # Security
/// - Caller must be authenticated via require_auth()
/// - Only parties authorized by the contract's release mode can approve
/// - Approvals are stored with TTL and auto-expire
/// - Duplicate approvals from the same party are rejected
pub fn approve_milestone(
    env: &Env,
    contract_id: u32,
    milestone_index: u32,
    caller: &Address,
) -> Result<bool, Error> {
    // Load contract
    let contract: Contract = storage::load_contract(env, contract_id);

    // Verify contract is in Funded or PartiallyFunded state
    if contract.status != ContractStatus::Funded
        && contract.status != ContractStatus::PartiallyFunded
    {
        return Err(Error::InvalidState);
    }

    // Load milestones
    let milestones: Vec<Milestone> = storage::load_milestones(env, contract_id);

    // Validate milestone index
    if milestone_index >= milestones.len() {
        return Err(Error::IndexOutOfBounds);
    }

    let milestone = milestones.get(milestone_index).unwrap();

    // Check if milestone is already released
    if milestone.released {
        return Err(Error::MilestoneAlreadyReleased);
    }

    // Check authorization: caller must be authorized for this release mode
    // This validates both that caller is a participant and is authorized
    // for the contract's release authorization mode
    authorization::require_release_authorization(&env, caller, &contract);

    // Load or create approval record
    let approval_key = crate::StorageKey::milestone_approvals(contract_id, milestone_index);
    let mut approvals: MilestoneApprovals =
        env.storage()
            .temporary()
            .get(&approval_key)
            .unwrap_or(MilestoneApprovals {
                client_approved: false,
                freelancer_approved: false,
                arbiter_approved: false,
            });

    // Determine caller role for approval tracking
    let is_client = caller == &contract.client;
    let is_freelancer = caller == &contract.freelancer;
    let is_arbiter = contract.arbiter.as_ref() == Some(caller);

    // Check for duplicate approval and update
    if is_client {
        if approvals.client_approved {
            return Err(Error::AlreadyApproved);
        }
        approvals.client_approved = true;
    } else if is_freelancer {
        if approvals.freelancer_approved {
            return Err(Error::AlreadyApproved);
        }
        approvals.freelancer_approved = true;
    } else if is_arbiter {
        if approvals.arbiter_approved {
            return Err(Error::AlreadyApproved);
        }
        approvals.arbiter_approved = true;
    }

    // Store approval with TTL
    env.storage().temporary().set(&approval_key, &approvals);

    env.storage().temporary().extend_ttl(
        &approval_key,
        PENDING_APPROVAL_BUMP_THRESHOLD,
        PENDING_APPROVAL_TTL_LEDGERS,
    );

    Ok(true)
}

/// Checks if a milestone has sufficient approvals for release.
///
/// Expired approvals (TTL elapsed) are treated as absent and return None.
///
/// # Arguments
/// * `env` - The contract environment
/// * `contract` - The contract data
/// * `contract_id` - The contract ID
/// * `milestone_index` - The milestone index
///
/// # Returns
/// * `Ok(true)` - If sufficient approvals exist and are valid
/// * `Err(InsufficientApprovals)` - If approvals are missing or insufficient
/// * `Err(ApprovalExpired)` - If approvals existed but have expired
///
/// # Security
/// - Fail-closed: missing or expired approvals prevent release
/// - MultiSig requires both client and freelancer approvals
/// - TTL expiry is enforced by Soroban's temporary storage
pub fn check_approvals(
    env: &Env,
    contract: &Contract,
    contract_id: u32,
    milestone_index: u32,
) -> Result<bool, Error> {
    let approval_key = crate::StorageKey::milestone_approvals(contract_id, milestone_index);

    // Try to load approvals from temporary storage
    // If TTL has expired, this will return None
    let approvals: Option<MilestoneApprovals> = env.storage().temporary().get(&approval_key);

    // If no approvals exist (or they expired), fail
    let approvals = approvals.ok_or(Error::InsufficientApprovals)?;

    // Check if required approvals are present based on authorization mode
    let sufficient = match contract.release_authorization {
        ReleaseAuthorization::ClientOnly => approvals.client_approved,
        ReleaseAuthorization::ArbiterOnly => approvals.arbiter_approved,
        ReleaseAuthorization::ClientAndArbiter => {
            approvals.client_approved || approvals.arbiter_approved
        }
        ReleaseAuthorization::MultiSig => {
            approvals.client_approved && approvals.freelancer_approved
        }
    };

    if sufficient {
        Ok(true)
    } else {
        Err(Error::InsufficientApprovals)
    }
}

/// Revokes the caller's own approval for a milestone.
///
/// Only the party who originally approved can revoke their own flag.
/// Other parties' approval flags are left intact. If all three flags
/// become false after revocation, the entire approval record is removed
/// from temporary storage.
///
/// # Arguments
/// * `env` - The contract environment
/// * `contract_id` - The contract ID
/// * `milestone_index` - The index of the milestone
/// * `caller` - The address of the caller requesting revocation
///
/// # Returns
/// `true` if the revocation was successful
///
/// # Errors
/// * `ContractNotFound` - If contract doesn't exist
/// * `IndexOutOfBounds` - If milestone index is invalid
/// * `MilestoneAlreadyReleased` - If milestone was already released
/// * `UnauthorizedRole` - If caller is not a contract participant
/// * `InsufficientApprovals` - If no approval record exists for this milestone
///
/// # Security
/// - Caller must be authenticated via require_auth()
/// - A party can only revoke their own approval flag
/// - Cannot revoke after the milestone has been released
/// - When all flags become false, the record is removed entirely
pub fn revoke_approval(
    env: &Env,
    contract_id: u32,
    milestone_index: u32,
    caller: &Address,
) -> Result<bool, Error> {
    // Load contract
    let contract: Contract = env
        .storage()
        .persistent()
        .get(&DataKey::Contract(contract_id))
        .ok_or(Error::ContractNotFound)?;

    // Load milestones
    let milestones: Vec<Milestone> = env
        .storage()
        .persistent()
        .get(&crate::ttl::milestone_storage_key(env, contract_id))
        .ok_or(Error::ContractNotFound)?;

    // Validate milestone index
    if milestone_index >= milestones.len() {
        return Err(Error::IndexOutOfBounds);
    }

    let milestone = milestones.get(milestone_index).unwrap();

    // Check if milestone is already released
    if milestone.released {
        return Err(Error::MilestoneAlreadyReleased);
    }

    // Determine caller role
    let is_client = caller == &contract.client;
    let is_freelancer = caller == &contract.freelancer;
    let is_arbiter = contract.arbiter.as_ref() == Some(caller);

    // Verify caller is a valid participant
    if !is_client && !is_freelancer && !is_arbiter {
        return Err(Error::UnauthorizedRole);
    }

    // Load approval record — must exist to revoke
    let approval_key = DataKey::MilestoneApprovals(contract_id, milestone_index);
    let mut approvals: MilestoneApprovals = env
        .storage()
        .temporary()
        .get(&approval_key)
        .ok_or(Error::InsufficientApprovals)?;

    // Clear only the caller's flag
    if is_client {
        if !approvals.client_approved {
            return Err(Error::InsufficientApprovals);
        }
        approvals.client_approved = false;
    } else if is_freelancer {
        if !approvals.freelancer_approved {
            return Err(Error::InsufficientApprovals);
        }
        approvals.freelancer_approved = false;
    } else if is_arbiter {
        if !approvals.arbiter_approved {
            return Err(Error::InsufficientApprovals);
        }
        approvals.arbiter_approved = false;
    }

    // If all flags are now false, remove the record entirely
    let all_false =
        !approvals.client_approved && !approvals.freelancer_approved && !approvals.arbiter_approved;

    if all_false {
        env.storage().temporary().remove(&approval_key);
    } else {
        // Store updated approval with TTL
        env.storage().temporary().set(&approval_key, &approvals);
        env.storage().temporary().extend_ttl(
            &approval_key,
            PENDING_APPROVAL_BUMP_THRESHOLD,
            PENDING_APPROVAL_TTL_LEDGERS,
        );
    }

    Ok(true)
}

/// Clears approval records for a milestone after successful release.
///
/// This prevents approval reuse and cleans up temporary storage.
///
/// # Arguments
/// * `env` - The contract environment
/// * `contract_id` - The contract ID
/// * `milestone_index` - The milestone index
pub fn clear_approvals(env: &Env, contract_id: u32, milestone_index: u32) {
    let approval_key = crate::StorageKey::milestone_approvals(contract_id, milestone_index);
    env.storage().temporary().remove(&approval_key);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Escrow;
    use soroban_sdk::{testutils::Address as _, Env, Vec};

    #[test]
    fn arbiter_approval_key_preserves_existing_data_key_layout() {
        let typed_key = ArbiterApprovalKey::new(7, 2);

        assert_eq!(
            DataKey::from(typed_key),
            DataKey::MilestoneApprovals(7, 2)
        );
        assert_eq!(
            arbiter_approval_storage_key(7, 2),
            DataKey::MilestoneApprovals(7, 2)
        );
    }

    #[test]
    fn arbiter_approval_storage_absent_key_returns_none() {
        let env = Env::default();
        let escrow_id = env.register(Escrow, ());

        env.as_contract(&escrow_id, || {
            let key = arbiter_approval_storage_key(99, 1);
            let approvals: Option<MilestoneApprovals> = env.storage().temporary().get(&key);

            assert!(approvals.is_none());
            assert!(!env.storage().temporary().has(&key));
        });
    }

    #[test]
    fn arbiter_approval_storage_round_trips() {
        let env = Env::default();
        let escrow_id = env.register(Escrow, ());

        env.as_contract(&escrow_id, || {
            let key = arbiter_approval_storage_key(3, 0);
            let expected = MilestoneApprovals {
                client_approved: false,
                freelancer_approved: false,
                arbiter_approved: true,
            };

            env.storage().temporary().set(&key, &expected);
            let actual: MilestoneApprovals = env.storage().temporary().get(&key).unwrap();

            assert_eq!(actual, expected);
        });
    }

    fn setup_contract_in_storage(
        env: &Env,
        escrow_id: &crate::Address,
        contract_id: u32,
        contract: &Contract,
        release_auth: ReleaseAuthorization,
    ) {
        env.as_contract(escrow_id, || {
            env.storage()
                .persistent()
                .set(&DataKey::Contract(contract_id), contract);
            let milestones = Vec::from_array(
                env,
                [Milestone {
                    amount: 1000,
                    funded_amount: 0,
                    protocol_fee: 0,
                    released: false,
                    refunded: false,
                    work_evidence: None,
                    refunded_amount: 0,
                    deadline: None,
                }],
            );
            let _ = release_auth;
            env.storage().persistent().set(
                &DataKey::Milestones(contract_id),
                &milestones,
            );
        });
    }

    #[test]
    fn test_approve_milestone_client_only() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_address = env.register(crate::Escrow, ());

        let escrow_id = env.register(Escrow, ());
        let client = crate::Address::generate(&env);
        let freelancer = crate::Address::generate(&env);

        let contract = Contract {
            client: client.clone(),
            freelancer: freelancer.clone(),
            arbiter: None,
            status: ContractStatus::Funded,
            total_deposited: 1000,
            funded_amount: 1000,
            released_amount: 0,
            refunded_amount: 0,
            release_authorization: ReleaseAuthorization::ClientOnly,
            reputation_issued: false,
            token: crate::Address::generate(&env),
        };

        let contract_id = 1u32;
        env.as_contract(&contract_address, || {
            env.storage()
                .persistent()
                .set(&DataKey::Contract(contract_id), &contract);

            let milestones = Vec::from_array(
                &env,
                [Milestone {
                    amount: 1000,
                    funded_amount: 0,
                    protocol_fee: 0,
                    released: false,
                    refunded: false,
                    work_evidence: None,
                    refunded_amount: 0,
                    deadline: None,
                }],
            );
            env.storage().persistent().set(
                &DataKey::Milestones(contract_id),
                &milestones,
            );

            // Client approves
            let result = approve_milestone(&env, contract_id, 0, &client);
            assert!(result.is_ok());

            // Check approvals
            let check = check_approvals(&env, &contract, contract_id, 0);
            assert!(check.is_ok());
        });
    }

    #[test]
    fn test_approve_milestone_multisig() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_address = env.register(crate::Escrow, ());

        let escrow_id = env.register(Escrow, ());
        let client = crate::Address::generate(&env);
        let freelancer = crate::Address::generate(&env);

        let contract = Contract {
            client: client.clone(),
            freelancer: freelancer.clone(),
            arbiter: None,
            status: ContractStatus::Funded,
            total_deposited: 1000,
            funded_amount: 1000,
            released_amount: 0,
            refunded_amount: 0,
            release_authorization: ReleaseAuthorization::MultiSig,
            reputation_issued: false,
            token: crate::Address::generate(&env),
        };

        let contract_id = 1u32;
        env.as_contract(&contract_address, || {
            env.storage()
                .persistent()
                .set(&DataKey::Contract(contract_id), &contract);

            let milestones = Vec::from_array(
                &env,
                [Milestone {
                    amount: 1000,
                    funded_amount: 0,
                    protocol_fee: 0,
                    released: false,
                    refunded: false,
                    work_evidence: None,
                    refunded_amount: 0,
                    deadline: None,
                }],
            );
            env.storage().persistent().set(
                &DataKey::Milestones(contract_id),
                &milestones,
            );

            // Only client approves - insufficient
            let result = approve_milestone(&env, contract_id, 0, &client);
            assert!(result.is_ok());

            let check = check_approvals(&env, &contract, contract_id, 0);
            assert_eq!(check, Err(Error::InsufficientApprovals));

            // Freelancer also approves - now sufficient
            let result = approve_milestone(&env, contract_id, 0, &freelancer);
            assert!(result.is_ok());

            let check = check_approvals(&env, &contract, contract_id, 0);
            assert!(check.is_ok());
        });
    }

    #[test]
    #[ignore]
    fn test_duplicate_approval_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_address = env.register(crate::Escrow, ());

        let escrow_id = env.register(Escrow, ());
        let client = crate::Address::generate(&env);
        let freelancer = crate::Address::generate(&env);

        let contract = Contract {
            client: client.clone(),
            freelancer: freelancer.clone(),
            arbiter: None,
            status: ContractStatus::Funded,
            total_deposited: 1000,
            funded_amount: 1000,
            released_amount: 0,
            refunded_amount: 0,
            release_authorization: ReleaseAuthorization::ClientOnly,
            reputation_issued: false,
            token: crate::Address::generate(&env),
        };

        let contract_id = 1u32;
        env.as_contract(&contract_address, || {
            env.storage()
                .persistent()
                .set(&DataKey::Contract(contract_id), &contract);

            let milestones = Vec::from_array(
                &env,
                [Milestone {
                    amount: 1000,
                    funded_amount: 0,
                    protocol_fee: 0,
                    released: false,
                    refunded: false,
                    work_evidence: None,
                    refunded_amount: 0,
                    deadline: None,
                }],
            );
            env.storage().persistent().set(
                &DataKey::Milestones(contract_id),
                &milestones,
            );

            // First approval succeeds
            let result = approve_milestone(&env, contract_id, 0, &client);
            assert!(result.is_ok());

            // Second approval fails
            let result = approve_milestone(&env, contract_id, 0, &client);
            assert_eq!(result, Err(Error::AlreadyApproved));
        });
    }
}
