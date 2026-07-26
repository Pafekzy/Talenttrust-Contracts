use crate::{
    approvals, ttl, Contract, ContractStatus, DataKey, Error, Escrow, Milestone,
    ReleaseAuthorization, REPUTATION_CREDIT_INCREMENT,
};
use soroban_sdk::{Address, Env, Symbol, Vec};

use crate::utils::now_seconds;
use crate::{
    approvals, ttl, Contract, ContractStatus, DataKey, Error, Escrow, EscrowArgs, EscrowClient,
    EscrowError, Milestone, ReleaseAuthorization,
};
use soroban_sdk::{contractimpl, symbol_short, token, Address, Env, Symbol, Vec};

#[contractimpl]
impl Escrow {
    /// Releases a specific milestone, transferring the net payout to the freelancer.
    ///
    /// Executes `SAC::transfer(from: escrow_address, to: freelancer, milestone.amount − fee)`.
    /// The protocol fee is retained inside the contract under
    /// `DataKey::AccumulatedProtocolFees` and stays commingled with the escrow balance
    /// until `withdraw_protocol_fees` is called.
    ///
    /// See [`docs/escrow/sac-custody.md`](../../../docs/escrow/sac-custody.md) for the
    /// full custody model and accounting invariant.
    ///
    /// The target milestone must be fully funded through per-milestone deposit
    /// allocation before it can be released.
    ///
    /// Requires valid, non-expired approvals based on the contract's ReleaseAuthorization mode.
    ///
    /// MultiSig semantics are client-and-freelancer approval. A MultiSig
    /// milestone can be released only by the stored client or freelancer after
    /// both of those addresses have approved the same milestone.
    ///
    /// Approvals are cleared from temporary storage after a successful release.
    /// Missing or expired approvals are fail-closed — they produce
    /// `InsufficientApprovals` and the call panics without mutating state.
    ///
    /// See `approve_milestone_release`, `get_milestone_approvals`, and
    /// `docs/escrow/approvals-and-release.md` for the full flow.
    ///
    /// # Arguments
    /// * `env` - The contract environment
    /// * `contract_id` - The contract ID
    /// * `caller` - The address of the caller (must be authorized)
    /// * `milestone_index` - The index of the milestone to release
    ///
    /// # Returns
    /// `true` if release was successful
    ///
    /// # Errors
    /// * `ContractNotFound` - If contract doesn't exist
    /// * `InvalidState` - If contract is not in Funded state
    /// * `InvalidMilestone` - If milestone index is out of bounds
    /// * `AlreadyReleased` - If milestone was already released
    /// * `AlreadyRefunded` - If milestone was already refunded
    /// * `InsufficientFunds` - If the milestone or aggregate contract balance is underfunded
    /// * `InsufficientApprovals` - If required approvals are missing
    /// * `ApprovalExpired` - If approvals have expired
    /// * `UnauthorizedRole` - If caller is not authorized to release
    ///
    /// # Security
    /// - Requires valid approvals that haven't expired
    /// - Approvals are cleared after successful release
    /// - Fail-closed: missing or expired approvals prevent release
    ///
    /// # Events
    /// Emits `("mlstn_rls", contract_id)` with payload
    /// `(milestone_index, amount, fee, new_released_amount, caller, timestamp)`
    /// on every successful release.
    ///
    /// Additionally emits `("ctrct_cmp", contract_id)` with payload
    /// `(caller, timestamp)` when the release transitions the contract to
    /// `Completed` (i.e. all milestones are released or refunded).
    pub fn release_milestone(
        env: Env,
        contract_id: u32,
        caller: Address,
        milestone_index: u32,
    ) -> bool {
        Self::require_not_paused(&env);
        // Authenticate caller before any state-dependent logic
        caller.require_auth();

        let mut contract: Contract = env
            .storage()
            .persistent()
            .get(&DataKey::Contract(contract_id))
            .unwrap_or_else(|| env.panic_with_error(EscrowError::ContractNotFound));

        // Extend TTL on contract read
        ttl::extend_contract_ttl(&env, contract_id);

        Self::require_not_finalized(&env, contract_id);

        // Verify contract is in Funded state before release (deposit transitions
        // Created → Funded when fully funded, so release must accept Funded).
        if contract.status != ContractStatus::Funded {
            env.panic_with_error(EscrowError::InvalidState);
        }

        // Check caller is authorized for this release authorization mode
        let is_client = caller == contract.client;
        let is_freelancer = caller == contract.freelancer;
        let is_arbiter = contract.arbiter.as_ref() == Some(&caller);

        match contract.release_authorization {
            ReleaseAuthorization::ClientOnly => {
                if !is_client {
                    env.panic_with_error(EscrowError::UnauthorizedRole);
                }
            }
            ReleaseAuthorization::ArbiterOnly => {
                if !is_arbiter {
                    env.panic_with_error(EscrowError::UnauthorizedRole);
                }
            }
            ReleaseAuthorization::ClientAndArbiter => {
                if !is_client && !is_arbiter {
                    env.panic_with_error(EscrowError::UnauthorizedRole);
                }
            }
            ReleaseAuthorization::MultiSig => {
                if !is_client && !is_freelancer {
                    env.panic_with_error(EscrowError::UnauthorizedRole);
                }
            }
        }

        let milestones: Vec<Milestone> = ttl::load_milestones(&env, contract_id);

        if milestone_index >= milestones.len() {
            env.panic_with_error(Error::IndexOutOfBounds);
        }

        let milestone = milestones.get(milestone_index).unwrap().clone();

        if milestone.released {
            env.panic_with_error(Error::MilestoneAlreadyReleased);
        }

        if milestone.refunded {
            env.panic_with_error(EscrowError::AlreadyRefunded);
        }

        // Check for valid approvals
        approvals::check_approvals(&env, &contract, contract_id, milestone_index)
            .unwrap_or_else(|e| env.panic_with_error(e));

        let milestone_key = Symbol::new(&env, "milestones");
        let mut milestones: Vec<Milestone> = storage::load_milestones(&env, contract_id);

        // Extend TTL on milestone read
        ttl::extend_milestone_ttl(&env, contract_id);

        if milestone_index >= milestones.len() {
            env.panic_with_error(EscrowError::IndexOutOfBounds);
        }

        let mut milestone = milestones.get(milestone_index).unwrap().clone();

        if milestone.released {
            env.panic_with_error(Error::MilestoneAlreadyReleased);
        }

        if milestone.refunded {
            env.panic_with_error(EscrowError::AlreadyRefunded);
        }

        approvals::check_approvals(&env, &contract, contract_id, milestone_index)
            .unwrap_or_else(|e| env.panic_with_error(e));

        let available_balance =
            crate::amount_validation::available_balance(contract.funded_amount, contract.released_amount, contract.refunded_amount).unwrap_or_else(|| env.panic_with_error(Error::PotentialOverflow));
        if available_balance < milestone.amount {
            env.panic_with_error(Error::InsufficientFunds);
        }

        let _release_amount = milestone.amount;
        milestone.released = true;
        milestones.set(milestone_index, milestone.clone());
        contract.released_amount = crate::amount_validation::safe_add_amounts(contract.released_amount, milestone.amount).unwrap_or_else(|| env.panic_with_error(Error::PotentialOverflow));

        // Compute the protocol fee up-front so the available-balance check can
        // account for both the net payout and the fee that stays in the contract.
        //
        // `protocol_fee` — the portion of `gross_amount` retained by the
        // protocol. Deducted from the gross milestone amount before transfer
        // so the escrow balance is never overdrawn.
        let protocol_fee: i128 = if Self::is_initialized(&env) {
            let fee_bps = Self::read_protocol_fee_bps(&env);
            if fee_bps > 0 {
                Self::calculate_protocol_fee(&env, gross_amount, fee_bps)
            } else {
                0
            }
        } else {
            0
        };

        // `net_amount` — the amount actually transferred to the freelancer
        // after deducting the protocol fee.
        let net_amount = gross_amount - protocol_fee;

        // The available balance must cover the full gross milestone amount
        // (net payout + fee) without dipping into already-accumulated fees or
        // other milestones' funds.
        let accumulated_fees: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::AccumulatedProtocolFees)
            .unwrap_or(0);
        let available_balance = contract.funded_amount
            - contract.released_amount
            - contract.refunded_amount
            - accumulated_fees;
        if available_balance < gross_amount {
            env.panic_with_error(EscrowError::InsufficientFunds);
        }

        // Transfer the net amount (gross minus fee) to the freelancer.
        // The fee portion remains in the contract's token balance and is
        // tracked separately in AccumulatedProtocolFees.
        let token = Self::read_settlement_token(&env)
            .unwrap_or_else(|| env.panic_with_error(Error::SettlementTokenNotConfigured));
        let token_client = token::Client::new(&env, &token);
        token_client.transfer(
            &env.current_contract_address(),
            &contract.freelancer,
            &net_amount,
        );

        // Accrue the fee into the protocol's accumulated balance.
        if protocol_fee > 0 {
            env.storage().persistent().set(
                &DataKey::AccumulatedProtocolFees,
                &(accumulated_fees + protocol_fee),
            );
        }

        milestone.released = true;
        // Record the funded amount on the milestone so it is self-describing.
        milestone.funded_amount = gross_amount;
        milestones.set(milestone_index, milestone.clone());
        // released_amount tracks net amounts paid out to freelancers.
        // accumulated_fees tracks protocol fees retained in the contract.
        // Together: released_amount + refunded_amount + accumulated_fees <= funded_amount.
        contract.released_amount = contract
            .released_amount
            .checked_add(net_amount)
            .unwrap_or_else(|| env.panic_with_error(EscrowError::PotentialOverflow));

        // Accounting invariant: net released + refunded + all accumulated fees
        // must never exceed the total funded amount.
        let new_accumulated = accumulated_fees + protocol_fee;
        let invariant_sum = contract.released_amount + contract.refunded_amount + new_accumulated;
        if invariant_sum > contract.funded_amount {
            env.panic_with_error(EscrowError::AccountingInvariantViolated);
        }

        // Clear approvals after successful release
        approvals::clear_approvals(&env, contract_id, milestone_index);

        // Check if all milestones are released or refunded; if so, complete.
        let all_released = milestones.iter().all(|m| m.released || m.refunded);
        if all_released {
            contract.status = ContractStatus::Completed;
            let pending_key = DataKey::PendingReputationCredits(contract.freelancer.clone());
            let pending: i128 = env.storage().persistent().get(&pending_key).unwrap_or(0);
            env.storage().persistent().set(&pending_key, &(pending + crate::REPUTATION_CREDIT_INCREMENT));
        }

        ttl::store_milestones(&env, contract_id, &milestones);
        env.storage()
            .persistent()
            .set(&DataKey::Contract(contract_id), &contract);

        crate::events::emit_contract_indexed_event(env, contract_id, &contract);

        ttl::extend_contract_and_milestones_ttl(env, contract_id);

        // ── Events ──────────────────────────────────────────────────────────
        //
        // Emitted only after all state mutations succeed (fail-closed guarantee:
        // if execution reaches here, the release was accepted). Events contain
        // no secrets — all fields are already public contract state or
        // caller-supplied arguments.

        // `mlstn_rls` — fired on every successful milestone release.
        //
        // Topics : `(symbol_short!("mlstn_rls"), contract_id: u32)`
        // Data   : `(milestone_index: u32, amount: i128, fee: i128,
        //            new_released_amount: i128, caller: Address, timestamp: u64)`
        env.events().publish(
            (symbol_short!("mlstn_rls"), contract_id),
            (
                milestone_index,
                gross_amount,
                protocol_fee,
                contract.released_amount,
                caller.clone(),
                env.ledger().timestamp(),
            ),
        );

        // `ctrct_cmp` — fired only when this release completes the contract.
        //
        // Topics : `(symbol_short!("ctrct_cmp"), contract_id: u32)`
        // Data   : `(caller: Address, timestamp: u64)`
        if all_released {
            env.events().publish(
                (symbol_short!("ctrct_cmp"), contract_id),
                (caller, env.ledger().timestamp()),
            );
        }

        true
    }

    /// Checks if a specific milestone is overdue based on its deadline.
    ///
    /// A milestone is considered overdue if:
    /// - It has a deadline set (Some value)
    /// - The current time is strictly greater than the deadline (now > deadline)
    /// - The milestone has not been released
    ///
    /// # Arguments
    /// * `env` - The contract environment
    /// * `contract_id` - The contract ID
    /// * `milestone_index` - The index of the milestone to check
    ///
    /// # Returns
    /// `true` if the milestone is overdue, `false` otherwise
    ///
    /// # Note
    /// - Returns `false` if milestone has no deadline (None)
    /// - Returns `false` if milestone is already released
    /// - Boundary condition: at exactly the deadline (now == deadline), returns `false`
    ///   because the deadline hasn't passed yet (uses strictly > comparison)
    ///
    /// # Security
    /// Uses `now_seconds(&env)` which is the single source of truth for ledger time.
    /// Time cannot be manipulated by contract callers.
    pub fn is_milestone_overdue(env: Env, contract_id: u32, milestone_index: u32) -> bool {
        // Existence probe only — `is_milestone_overdue` never reads a `Contract`
        // field, but a missing contract still means "not overdue".
        let _contract: Contract = match env
            .storage()
            .persistent()
            .get(&DataKey::Contract(contract_id))
        {
            Some(c) => c,
            None => return false, // Contract not found, not overdue
        };

        let milestone_key = Symbol::new(&env, "milestones");
        let milestones: Vec<Milestone> = match env
            .storage()
            .persistent()
            .get(&(DataKey::Contract(contract_id), milestone_key))
        {
            Some(m) => m,
            None => return false, // No milestones, not overdue
        };

        if milestone_index >= milestones.len() {
            return false; // Index out of bounds, not overdue
        }

        let milestone = milestones.get(milestone_index).unwrap();

        // Return false if already released
        if milestone.released {
            return false;
        }

        // Return false if no deadline set
        match milestone.deadline {
            None => false,
            Some(deadline) => {
                // Overdue if now > deadline (strictly greater)
                now_seconds(&env) > deadline
            }
        }
    }
}
