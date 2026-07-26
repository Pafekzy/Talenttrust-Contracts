//! Refund and cancellation entrypoints.
//!
//! This module owns the two money-movement paths that return settlement-token
//! funds to the client: `refund_unreleased_milestones` (per-milestone,
//! deadline-gated refunds) and `cancel_contract` (bulk refund of the entire
//! remaining balance before any milestone has been released). Both transfer
//! SAC tokens and mutate `Contract` accounting, so they live alongside
//! `release.rs` rather than in the crate root.
//!
//! Moved out of `lib.rs` verbatim (issue #1021 — split escrow logic into a
//! dedicated module). Behaviour, error codes, event topics, and the public
//! ABI are unchanged.

use crate::{
    ttl, Contract, ContractStatus, DataKey, Error, Escrow, EscrowArgs, EscrowClient, EscrowError,
    Milestone,
};
use soroban_sdk::{contractimpl, symbol_short, token, Address, Env, Vec};

#[contractimpl]
impl Escrow {
    /// Refunds unreleased milestones back to the client.
    ///
    /// # Arguments
    /// * `env` - The contract environment
    /// * `contract_id` - The contract ID
    /// * `milestone_indices` - Vector of milestone indices to refund
    ///
    /// # Returns
    /// The total amount refunded
    ///
    /// # Errors
    /// * `ContractNotFound` - If contract doesn't exist
    /// * `EmptyRefundRequest` - If milestone_indices is empty
    /// * `DuplicateMilestoneInRefund` - If the same milestone appears multiple times
    /// * `IndexOutOfBounds` - If any milestone index is out of bounds
    /// * `AlreadyReleased` - If any milestone was already released
    /// * `AlreadyRefunded` - If any milestone was already refunded
    /// * `InsufficientFunds` - If contract doesn't have enough balance to refund
    /// * `AlreadyFinalized` - If a finalization record already exists for this contract
    /// * `InvalidState` - If contract status is not Created, Funded, or Disputed
    pub fn refund_unreleased_milestones(
        env: Env,
        contract_id: u32,
        milestone_indices: Vec<u32>,
    ) -> i128 {
        Self::require_not_paused(&env);
        // Validate non-empty request
        if milestone_indices.is_empty() {
            env.panic_with_error(EscrowError::EmptyRefundRequest);
        }

        // Check for duplicates
        for i in 0..milestone_indices.len() {
            for j in (i + 1)..milestone_indices.len() {
                if milestone_indices.get(i).unwrap() == milestone_indices.get(j).unwrap() {
                    env.panic_with_error(EscrowError::DuplicateMilestoneInRefund);
                }
            }
        }

        let mut contract: Contract = env
            .storage()
            .persistent()
            .get(&DataKey::Contract(contract_id))
            .unwrap_or_else(|| env.panic_with_error(EscrowError::ContractNotFound));

        // Extend TTL on contract read
        ttl::extend_contract_ttl(&env, contract_id);

        Self::require_not_finalized(&env, contract_id);

        // Only allow refunds while the contract is still in an active,
        // unreleased state. Cancelled, Completed, and Refunded contracts
        // must not be refundable again.
        if contract.status != ContractStatus::Created
            && contract.status != ContractStatus::Funded
            && contract.status != ContractStatus::Disputed
        {
            env.panic_with_error(EscrowError::InvalidState);
        }

        contract.client.require_auth();

        let mut milestones: Vec<Milestone> = ttl::load_milestones(&env, contract_id);

        let mut total_refund_amount: i128 = 0;

        // Validate all milestones first
        for idx in milestone_indices.iter() {
            if idx >= milestones.len() {
                env.panic_with_error(Error::IndexOutOfBounds);
            }

            let milestone = milestones.get(idx).unwrap();

            // SECURITY: Check if milestone is already released
            if milestone.released {
                env.panic_with_error(Error::AlreadyReleased);
            }

            // SECURITY: Check if milestone is already refunded
            if milestone.refunded {
                env.panic_with_error(EscrowError::AlreadyRefunded);
            }

            // SECURITY: Check timeout refund conditions - milestone must be overdue if deadline is set
            if milestone.deadline.is_some() {
                // Milestone has a deadline - check if it's overdue
                if !Self::is_milestone_overdue(env.clone(), contract_id, idx) {
                    // Deadline set but milestone not yet overdue
                    env.panic_with_error(Error::MilestoneNotOverdue);
                }
                // SECURITY: is_milestone_overdue already verified: now > deadline AND unreleased
            }
            // If no deadline (None), allow refund anytime (backward compatibility)

            total_refund_amount += milestone.amount;
        }

        // Check if there's enough balance
        let available_balance =
            contract.funded_amount - contract.released_amount - contract.refunded_amount;
        if available_balance < total_refund_amount {
            env.panic_with_error(EscrowError::InsufficientFunds);
        }

        // Transfer tokens from contract to client
        let token = Self::read_settlement_token(&env)
            .unwrap_or_else(|| env.panic_with_error(Error::SettlementTokenNotConfigured));

        let token_client = token::Client::new(&env, &token);
        token_client.transfer(
            &env.current_contract_address(),
            &contract.client,
            &total_refund_amount,
        );

        // Mark milestones as refunded
        for idx in milestone_indices.iter() {
            let mut milestone = milestones.get(idx).unwrap();
            milestone.refunded = true;
            milestone.refunded_amount = milestone.amount;
            milestones.set(idx, milestone);
        }

        contract.refunded_amount = contract
            .refunded_amount
            .checked_add(total_refund_amount)
            .unwrap_or_else(|| env.panic_with_error(Error::InsufficientFunds));

        // Check if all unreleased milestones are refunded
        let all_refunded_or_released = milestones.iter().all(|m| m.released || m.refunded);
        if all_refunded_or_released {
            let all_refunded = milestones.iter().all(|m| m.refunded);
            if all_refunded {
                contract.status = ContractStatus::Refunded;
            } else {
                // Some released, some refunded
                contract.status = ContractStatus::Completed;
                Self::grant_pending_reputation_credit(&env, &contract.freelancer);
            }
        }

        ttl::store_milestones(&env, contract_id, &milestones);
        env.storage()
            .persistent()
            .set(&DataKey::Contract(contract_id), &contract);

        // Extend TTL on contract write (milestone TTL already extended by store_milestones)
        ttl::extend_contract_ttl(&env, contract_id);

        // Emit `refunded` event after all state mutations succeed.
        //
        // Topics : `(symbol_short!("refunded"), contract_id: u32)`
        // Data   : `(total_refund_amount: i128, new_status: ContractStatus, timestamp: u64)`
        env.events().publish(
            (symbol_short!("refunded"), contract_id),
            (
                total_refund_amount,
                contract.status,
                env.ledger().timestamp(),
            ),
        );

        total_refund_amount
    }

    /// Cancels a contract before any milestone has been released.
    ///
    /// The caller must be the stored client and must authorize the call. The
    /// contract must be in `Created` or `Funded` state, with no released
    /// balance, and the full remaining refundable balance is sent back to the
    /// client via the configured Stellar Asset Contract before the contract is
    /// marked `Cancelled`. A zero-funded cancellation does not invoke a token
    /// transfer and leaves unrelated contracts' escrowed token balances intact.
    ///
    /// # Errors
    /// * `ContractPaused` - If the contract is paused while not in emergency mode.
    /// * `EmergencyActive` - If the contract is in an active emergency pause.
    /// * `ContractNotFound` - If the contract does not exist.
    /// * `UnauthorizedRole` - If the caller is not the stored client.
    /// * `AlreadyCancelled` - If the contract was already cancelled.
    /// * `InvalidStatusTransition` - If the contract is not `Created`/`Funded` or has already released funds.
    pub fn cancel_contract(env: Env, contract_id: u32, client: Address) -> bool {
        Self::require_not_paused(&env);
        let mut contract: Contract = env
            .storage()
            .persistent()
            .get(&DataKey::Contract(contract_id))
            .unwrap_or_else(|| env.panic_with_error(EscrowError::ContractNotFound));
        ttl::extend_contract_ttl(&env, contract_id);

        Self::require_not_finalized(&env, contract_id);

        if client != contract.client {
            env.panic_with_error(EscrowError::UnauthorizedRole);
        }

        if contract.status == ContractStatus::Cancelled {
            env.panic_with_error(Error::AlreadyCancelled);
        }

        if contract.status != ContractStatus::Created && contract.status != ContractStatus::Funded {
            env.panic_with_error(EscrowError::InvalidStatusTransition);
        }

        if contract.released_amount != 0 {
            env.panic_with_error(EscrowError::InvalidStatusTransition);
        }

        client.require_auth();

        let refund_amount =
            contract.funded_amount - contract.released_amount - contract.refunded_amount;
        if refund_amount > 0 {
            let token = Self::read_settlement_token(&env)
                .unwrap_or_else(|| env.panic_with_error(EscrowError::NotInitialized));
            token::Client::new(&env, &token).transfer(
                &env.current_contract_address(),
                &client,
                &refund_amount,
            );
        }

        contract.refunded_amount = contract
            .refunded_amount
            .checked_add(refund_amount)
            .unwrap_or_else(|| env.panic_with_error(EscrowError::InsufficientFunds));
        contract.status = ContractStatus::Cancelled;

        env.storage()
            .persistent()
            .set(&DataKey::Contract(contract_id), &contract);
        ttl::extend_contract_ttl(&env, contract_id);

        env.events().publish(
            (symbol_short!("cancelled"), contract_id),
            (client, refund_amount, env.ledger().timestamp()),
        );

        true
    }
}
