use soroban_sdk::{contracttype, symbol_short, Address, Env, Vec};

use crate::{
    safe_subtract_amounts, Contract, ContractStatus, ContractSummary, DataKey, Escrow, EscrowError,
    Milestone, MilestoneSummary, CONTRACT_SUMMARY_SCHEMA_VERSION,
};

/// Immutable metadata written when an escrow contract is closed.
///
/// The record is stored once under `DataKey::Finalization(contract_id)`.
/// After it exists, all contract-specific mutating entrypoints reject with
/// `EscrowError::AlreadyFinalized`.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FinalizationRecord {
    /// Authorized client, freelancer, or assigned arbiter that finalized.
    pub finalizer: Address,
    /// Ledger timestamp at finalization time.
    pub timestamp: u64,
    /// Snapshot of participant, milestone, and accounting state.
    pub summary: ContractSummary,
}

impl Escrow {
    fn finalization_key(contract_id: u32) -> DataKey {
        settlement::finalization_key(contract_id)
    }

    fn load_contract_for_finalization(env: &Env, contract_id: u32) -> Contract {
        env.storage()
            .persistent()
            .get::<_, Contract>(&DataKey::Contract(contract_id))
            .unwrap_or_else(|| env.panic_with_error(EscrowError::ContractNotFound))
    }

    pub(crate) fn is_finalized(env: &Env, contract_id: u32) -> bool {
        storage::is_finalized(env, contract_id)
    }

    pub(crate) fn require_not_finalized(env: &Env, contract_id: u32) {
        if Self::is_finalized(env, contract_id) {
            env.panic_with_error(EscrowError::AlreadyFinalized);
        }
    }

    pub(crate) fn require_not_paused(env: &Env) {
        if env
            .storage()
            .persistent()
            .get::<_, bool>(&DataKey::Paused)
            .unwrap_or(false)
        {
            env.panic_with_error(EscrowError::ContractPaused);
        }
        if env
            .storage()
            .persistent()
            .get::<_, bool>(&DataKey::Emergency)
            .unwrap_or(false)
        {
            env.panic_with_error(EscrowError::EmergencyActive);
        }
    }

    fn require_finalizer_role(env: &Env, contract: &Contract, finalizer: &Address) {
        let is_client = *finalizer == contract.client;
        let is_freelancer = *finalizer == contract.freelancer;
        let is_arbiter = contract.arbiter.clone().is_some_and(|a| a == *finalizer);
        if !is_client && !is_freelancer && !is_arbiter {
            env.panic_with_error(EscrowError::UnauthorizedRole);
        }
    }

    fn summarize_contract(env: &Env, contract_id: u32, contract: &Contract) -> ContractSummary {
        let milestone_key = Symbol::new(env, "milestones");
        let milestones: Vec<Milestone> = env
            .storage()
            .persistent()
            .get(&(DataKey::Contract(contract_id), milestone_key))
            .unwrap_or_else(|| env.panic_with_error(EscrowError::ContractNotFound));

        let mut total_amount: i128 = 0;
        let mut released_milestone_count: u32 = 0;
        let mut milestone_summaries = Vec::new(env);

        for (index, ms) in milestones.iter().enumerate() {
            let idx = index as u32;
            total_amount = total_amount
                .checked_add(ms.amount)
                .unwrap_or_else(|| env.panic_with_error(EscrowError::PotentialOverflow));

            if ms.released {
                released_milestone_count = released_milestone_count
                    .checked_add(1)
                    .unwrap_or_else(|| env.panic_with_error(EscrowError::PotentialOverflow));
            }

            milestone_summaries.push_back(MilestoneSummary {
                index: idx,
                amount: ms.amount,
                released: ms.released,
                refunded: ms.refunded,
            });
        }

        let reputation_issued = env
            .storage()
            .persistent()
            .get::<_, bool>(&DataKey::ReputationIssued(contract_id))
            .unwrap_or(false);

        ContractSummary {
            schema_version: CONTRACT_SUMMARY_SCHEMA_VERSION,
            client: contract.client.clone(),
            freelancer: contract.freelancer.clone(),
            arbiter: contract.arbiter.clone(),
            status: contract.status,
            reputation_issued,
            total_amount,
            funded_amount: contract.funded_amount,
            released_amount: contract.released_amount,
            refundable_balance: contract.funded_amount
                - contract.released_amount
                - contract.refunded_amount,
            released_milestone_count,
            milestones: milestone_summaries,
        }
    }
}

/// Finalize an escrow contract by writing immutable close metadata.
///
/// `finalizer` must authorize the call and must be the stored client,
/// freelancer, or assigned arbiter. Finalization is allowed only while the
/// contract is `Completed` or `Disputed`. Once finalized, future
/// contract-specific mutations fail with `AlreadyFinalized`.
///
/// # Errors
/// - `ContractPaused` when pause or emergency controls are active.
/// - `ContractNotFound` when `contract_id` is unknown.
/// - `AlreadyFinalized` when a close record already exists.
/// - `UnauthorizedRole` when `finalizer` is not a contract participant.
/// - `InvalidStatusTransition` unless status is `Completed` or `Disputed`.
pub fn finalize_contract_impl(env: &Env, contract_id: u32, finalizer: Address) -> bool {
    Escrow::require_not_paused(&env);
    finalizer.require_auth();

    let contract = Escrow::load_contract_for_finalization(&env, contract_id);
    Escrow::require_not_finalized(&env, contract_id);
    Escrow::require_finalizer_role(&env, &contract, &finalizer);

    if contract.status != ContractStatus::Completed && contract.status != ContractStatus::Disputed {
        env.panic_with_error(Error::InvalidStatusTransition);
    }

    let record = FinalizationRecord {
        finalizer: finalizer.clone(),
        timestamp: env.ledger().timestamp(),
        summary: Escrow::summarize_contract(&env, contract_id, &contract),
    };

    settlement::write_finalization(&env, contract_id, &record);

    if contract.status == ContractStatus::Disputed {
        crate::rollback::clear_dispute_rollback(env, contract_id);
    }

    env.events().publish(
        (symbol_short!("finalized"), contract_id),
        (finalizer, record.timestamp),
    );

    crate::events::emit_contract_indexed_event(env, contract_id, &contract);

    true
}

/// Return immutable close metadata for `contract_id`, if it has been finalized.
pub fn get_finalization_record_impl(env: &Env, contract_id: u32) -> Option<FinalizationRecord> {
    settlement::read_finalization(env, contract_id)
}

/// Roll back a finalized contract by removing its immutable close record.
///
/// `admin` must be the stored admin and authorize the call. Rollback is only
/// safe while the contract is finalized and in either `Completed` or `Disputed`
/// status; no accounting fields are modified.
///
/// # Errors
/// - `NotInitialized` if the contract has not been initialized.
/// - `UnauthorizedRole` if `admin` is not the stored admin.
/// - `RollbackNotAllowed` if the contract is not finalized or not in a safe status.
pub fn rollback_contract_impl(env: &Env, contract_id: u32, admin: Address) -> bool {
    Escrow::require_initialized(env);

    admin.require_auth();

    let stored_admin: Address = env
        .storage()
        .persistent()
        .get(&DataKey::Admin)
        .unwrap_or_else(|| env.panic_with_error(EscrowError::NotInitialized));
    if admin != stored_admin {
        env.panic_with_error(EscrowError::UnauthorizedRole);
    }

    let contract = Escrow::load_contract_for_finalization(env, contract_id);

    if !Escrow::is_finalized(env, contract_id) {
        env.panic_with_error(EscrowError::RollbackNotAllowed);
    }

    if contract.status != ContractStatus::Completed && contract.status != ContractStatus::Disputed {
        env.panic_with_error(EscrowError::RollbackNotAllowed);
    }

    let status = contract.status;

    env.storage()
        .persistent()
        .remove(&Escrow::finalization_key(contract_id));

    crate::ttl::extend_contract_ttl(env, contract_id);

    env.events().publish(
        (symbol_short!("rollback"), contract_id),
        (admin, status, env.ledger().timestamp()),
    );

    true
}
