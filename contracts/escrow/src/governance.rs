//! Governance and protocol-configuration entrypoints.
//!
//! This module owns admin-controlled persistent configuration:
//! `DataKey::Admin` for authorization, `ProtocolFeeBps` for release fees,
//! `GovernedParameters` for escrow caps, `ReadinessChecklist` for deployment
//! readiness state, and `PendingAdmin` for two-step admin rotation proposals.
//! Money movement for protocol-fee withdrawal remains in the crate root because
//! it performs settlement-token transfers.

use crate::ttl::ADMIN_ROTATION_MIN_DELAY_LEDGERS;
use crate::{
    DataKey, Error, Escrow, EscrowArgs, EscrowClient, GovernedParameters, PendingAdminProposal,
    ReadinessChecklist, EscrowError, DEFAULT_SETTLEMENT_LIMIT,
};
use soroban_sdk::{contractimpl, symbol_short, Address, Env, Symbol};

impl Escrow {
    /// Set the protocol fee in basis points.
    ///
    /// Admin-gated: the stored admin (under [`DataKey::Admin`]) must authorize
    /// the call and the contract must be initialized.
    ///
    /// `new_bps` must be `≤ 10_000` (100%). The fee takes effect immediately for
    /// the next `release_milestone` call. Values above 10_000 are rejected with
    /// `InvalidProtocolParameters` because a fee exceeding 100% would make every
    /// milestone release net negative for the freelancer.
    ///
    /// See [`docs/escrow/protocol-fees.md`](../../../docs/escrow/protocol-fees.md) for
    /// the basis-point model, fee formula, accrual storage, and withdrawal flow.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment
    /// * `new_bps` - Fee rate in basis points (0 to 10 000)
    ///
    /// # Returns
    /// * `bool` - `true` if set successfully
    ///
    /// # Errors
    /// * `NotInitialized` - If contract is uninitialized
    /// * `UnauthorizedRole` - If caller is not admin
    ///
    /// # Examples
    /// ```rust,ignore
    /// let client = EscrowClient::new(&env, &contract_id);
    /// let set = client.set_protocol_fee_bps(&250); // 2.5%
    /// assert!(set);
    /// ```
    ///
    /// # Events
    /// `(Symbol("protocol_fee_bps"),)` → `(old_bps, new_bps, admin, timestamp)`
    pub(crate) fn set_protocol_fee_bps_impl(env: Env, new_bps: u32) -> bool {
        Self::require_initialized(&env);
        let admin: Address = env
            .storage()
            .persistent()
            .get(&DataKey::Admin)
            .unwrap_or_else(|| env.panic_with_error(EscrowError::NotInitialized));
        admin.require_auth();

        // Reject any fee above 100 % (10_000 bps). A fee > 100 % would make every
        // milestone release impossible — the net payout would be negative.
        if new_bps > 10_000 {
            env.panic_with_error(Error::InvalidProtocolParameters);
        }

        let old_bps: u32 = env
            .storage()
            .persistent()
            .get(&DataKey::ProtocolFeeBps)
            .unwrap_or(0u32);
        env.storage()
            .persistent()
            .set(&DataKey::ProtocolFeeBps, &new_bps);

        env.events().publish(
            (Symbol::new(&env, "protocol_fee_bps"),),
            (old_bps, new_bps, admin.clone(), env.ledger().timestamp()),
        );
        true
    }

    /// Returns the stored governance admin address.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment
    ///
    /// # Returns
    /// * `Option<Address>` - `Some(Address)` of current admin, `None` if uninitialized
    ///
    /// # Examples
    /// ```rust,ignore
    /// let client = EscrowClient::new(&env, &contract_id);
    /// let admin = client.get_governance_admin();
    /// ```
    pub fn get_governance_admin(env: Env) -> Option<Address> {
        env.storage().persistent().get(&DataKey::Admin)
    }

    /// Returns the current protocol fee in basis points.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment
    ///
    /// # Returns
    /// * `u32` - Protocol fee rate in basis points (0 if unset)
    ///
    /// # Examples
    /// ```rust,ignore
    /// let client = EscrowClient::new(&env, &contract_id);
    /// let fee_bps = client.get_protocol_fee_bps();
    /// ```
    pub fn get_protocol_fee_bps(env: Env) -> u32 {
        env.storage()
            .persistent()
            .get::<_, u32>(&DataKey::ProtocolFeeBps)
            .unwrap_or(0)
    }

    /// Set the maximum events limit for queries or indexing.
    ///
    /// Admin-gated: the stored admin (under [`DataKey::Admin`]) must authorize
    /// the call and the contract must be initialized.
    ///
    /// `new_limit` must be within safe bounds (e.g., > 0 and <= 1000).
    ///
    /// # Events
    /// `(Symbol("events_limit"),)` → `(old_limit, new_limit, admin, timestamp)`
    pub fn set_events_limit(env: Env, new_limit: u32) -> bool {
        Self::require_initialized(&env);
        let admin: Address = env
            .storage()
            .persistent()
            .get(&DataKey::Admin)
            .unwrap_or_else(|| env.panic_with_error(Error::NotInitialized));
        admin.require_auth();

        if new_limit == 0 || new_limit > 1000 {
            env.panic_with_error(Error::InvalidEventsLimit);
        }

        let old_limit: u32 = Self::get_events_limit(env.clone());
        
        env.storage()
            .persistent()
            .set(&DataKey::EventsLimit, &new_limit);

        env.events().publish(
            (Symbol::new(&env, "events_limit"),),
            (old_limit, new_limit, admin, env.ledger().timestamp()),
        );
        true
    }

    /// Returns the current events limit. Default is 100.
    pub fn get_events_limit(env: Env) -> u32 {
        env.storage()
            .persistent()
            .get::<_, u32>(&DataKey::EventsLimit)
            .unwrap_or(100) // Default preserves current behaviour
    }

    // ── Two-step admin transfer ───────────────────────────────────────────────

    /// Propose a new governance admin. Stores the proposal with a timelock.
    ///
    /// # Events
    /// `(symbol_short!("admin"), Symbol("proposed"))` → `(admin, proposed, timestamp)`
    pub fn propose_governance_admin(env: Env, proposed: Address) -> bool {
        Self::require_initialized(&env);

        let admin: Address = env
            .storage()
            .persistent()
            .get(&DataKey::Admin)
            .unwrap_or_else(|| env.panic_with_error(EscrowError::ContractNotFound));
        admin.require_auth();

        env.storage().persistent().set(
            &DataKey::PendingAdmin,
            &PendingAdminProposal {
                proposed: proposed.clone(),
                proposed_at_ledger: env.ledger().sequence(),
            },
        );

        env.events().publish(
            (symbol_short!("admin"), Symbol::new(&env, "proposed")),
            (admin, proposed.clone(), env.ledger().timestamp()),
        );
        true
    }

    /// Accept a pending admin proposal, enforcing the timelock.
    ///
    /// # Events
    /// `(symbol_short!("admin"), Symbol("accepted"))` → `(old_admin, new_admin, timestamp)`
    pub fn accept_governance_admin(env: Env) -> bool {
        Self::require_initialized(&env);

        let pending: PendingAdminProposal = env
            .storage()
            .persistent()
            .get(&DataKey::PendingAdmin)
            .unwrap_or_else(|| env.panic_with_error(EscrowError::InvalidState));

        let elapsed = env
            .ledger()
            .sequence()
            .saturating_sub(pending.proposed_at_ledger);
        if elapsed < ADMIN_ROTATION_MIN_DELAY_LEDGERS {
            env.panic_with_error(EscrowError::TimelockNotElapsed);
        }

        let pending_admin = pending.proposed;
        pending_admin.require_auth();

        let old_admin: Address = env
            .storage()
            .persistent()
            .get(&DataKey::Admin)
            .unwrap_or_else(|| env.panic_with_error(EscrowError::ContractNotFound));

        env.storage()
            .persistent()
            .set(&DataKey::Admin, &pending_admin);
        env.storage().persistent().remove(&DataKey::PendingAdmin);

        env.events().publish(
            (symbol_short!("admin"), Symbol::new(&env, "accepted")),
            (old_admin, pending_admin.clone(), env.ledger().timestamp()),
        );
        true
    }

    /// Cancel a pending governance admin proposal, aborting a two-step transfer.
    ///
    /// Only the current admin (the address stored under [`DataKey::Admin`]) may
    /// cancel, and the contract must be initialized. On success the pending
    /// proposal is removed so the previously proposed address can no longer call
    /// [`Escrow::accept_governance_admin`] — a subsequent accept panics with
    /// [`EscrowError::InvalidState`].
    ///
    /// # Errors
    /// * [`EscrowError::NotInitialized`] — `initialize` has not been called.
    /// * [`EscrowError::InvalidState`] — there is no pending proposal to cancel.
    ///
    /// # Events
    /// `(symbol_short!("admin"), Symbol("cancelled"))` → `(admin, cancelled_proposal, timestamp)`
    pub fn cancel_governance_admin_proposal(env: Env) -> bool {
        Self::require_initialized(&env);

        let admin: Address = env
            .storage()
            .persistent()
            .get(&DataKey::Admin)
            .unwrap_or_else(|| env.panic_with_error(EscrowError::ContractNotFound));
        admin.require_auth();

        let pending: PendingAdminProposal = env
            .storage()
            .persistent()
            .get(&DataKey::PendingAdmin)
            .unwrap_or_else(|| env.panic_with_error(EscrowError::InvalidState));

        env.storage().persistent().remove(&DataKey::PendingAdmin);

        env.events().publish(
            (symbol_short!("admin"), Symbol::new(&env, "cancelled")),
            (admin, pending.proposed, env.ledger().timestamp()),
        );
        true
    }

    /// Return the currently pending admin address, if any.
    pub fn get_pending_governance_admin(env: Env) -> Option<Address> {
        let proposal: Option<PendingAdminProposal> =
            env.storage().persistent().get(&DataKey::PendingAdmin);
        proposal.map(|p| p.proposed)
    }

    /// Internal: return the current admin address.
    pub(crate) fn get_governance_admin_impl(env: Env) -> Option<Address> {
        env.storage().persistent().get(&DataKey::Admin)
    }

    /// Set both governance parameters at once and update the readiness checklist.
    ///
    /// Sets `protocol_fee_bps` (must be `≤ MAX_FEE_BPS`) and `max_escrow_total_stroops`
    /// atomically. Also flips `ReadinessChecklist::governed_params_set` to `true`.
    ///
    /// See [`docs/escrow/protocol-fees.md`](../../../docs/escrow/protocol-fees.md) for
    /// the full basis-point model and fee lifecycle.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment
    /// * `admin` - The admin address updating parameters
    /// * `protocol_fee_bps` - New fee in basis points
    /// * `max_escrow_total_stroops` - Maximum total escrow capacity in stroops
    ///
    /// # Returns
    /// * `bool` - `true` if parameters set successfully
    ///
    /// # Errors
    /// * `NotInitialized` - If contract uninitialized
    /// * `UnauthorizedRole` - If caller is not admin
    /// * `InvalidProtocolParameters` - If `protocol_fee_bps > 10_000`
    ///
    /// # Examples
    /// ```rust,ignore
    /// let client = EscrowClient::new(&env, &contract_id);
    /// let set = client.set_governed_params(&admin, &200, &1_000_000_0000000);
    /// assert!(set);
    /// ```
    pub fn set_governed_params(
        env: Env,
        admin: Address,
        protocol_fee_bps: u32,
        max_escrow_total_stroops: i128,
    ) -> bool {
        if !env
            .storage()
            .persistent()
            .get::<_, bool>(&crate::DataKey::Initialized)
            .unwrap_or(false)
        {
            env.panic_with_error(EscrowError::NotInitialized);
        }

        let stored_admin: Address = env
            .storage()
            .persistent()
            .get(&DataKey::Admin)
            .unwrap_or_else(|| env.panic_with_error(EscrowError::NotInitialized));

        if admin != stored_admin {
            env.panic_with_error(EscrowError::UnauthorizedRole);
        }
        admin.require_auth();

        if protocol_fee_bps > MAX_BPS {
            env.panic_with_error(Error::InvalidProtocolParameters);
        }

        let params = GovernedParameters {
            protocol_fee_bps,
            max_escrow_total_stroops,
        };
        env.storage()
            .persistent()
            .set(&DataKey::GovernedParameters, &params);
        env.storage()
            .persistent()
            .set(&DataKey::ProtocolFeeBps, &protocol_fee_bps);

        let mut checklist: ReadinessChecklist = env
            .storage()
            .persistent()
            .get(&DataKey::ReadinessChecklist)
            .unwrap_or_default();
        checklist.governed_params_set = true;
        env.storage()
            .persistent()
            .set(&DataKey::ReadinessChecklist, &checklist);

        true
    }

    /// Retrieve the current governed parameters.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment
    ///
    /// # Returns
    /// * `Option<GovernedParameters>` - `Some(GovernedParameters)` if set, `None` otherwise
    ///
    /// # Examples
    /// ```rust,ignore
    /// let client = EscrowClient::new(&env, &contract_id);
    /// if let Some(params) = client.get_governed_parameters() {
    ///     assert_eq!(params.protocol_fee_bps, 200);
    /// }
    /// ```
    pub fn get_governed_parameters(env: Env) -> Option<GovernedParameters> {
        env.storage().persistent().get(&DataKey::GovernedParameters)
    }

    // ── Settlement limit ──────────────────────────────────────────────────────

    /// Set the per-milestone settlement limit (max single milestone amount in stroops).
    ///
    /// Admin-gated: the stored admin must authorize the call and the contract
    /// must be initialized.
    ///
    /// `limit` must satisfy `1 ≤ limit ≤ DEFAULT_SETTLEMENT_LIMIT`. The
    /// default (when no value has been set) preserves the original hard-coded
    /// behaviour.
    ///
    /// Takes effect immediately for the next `create_contract` call.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment
    /// * `admin` - The admin address (must match stored admin)
    /// * `limit` - The new settlement limit in stroops
    ///
    /// # Errors
    /// * `NotInitialized` if `initialize` has not been called
    /// * `UnauthorizedRole` if `admin` is not the stored admin
    /// * `SettlementLimitOutOfBounds` if `limit` is outside `[1, DEFAULT_SETTLEMENT_LIMIT]`
    ///
    /// # Events
    /// `(Symbol("settlement_limit"),)` → `(old_limit, new_limit, admin, timestamp)`
    pub fn set_settlement_limit(env: Env, admin: Address, limit: i128) -> bool {
        Self::require_initialized(&env);
        let stored_admin: Address = env
            .storage()
            .persistent()
            .get(&DataKey::Admin)
            .unwrap_or_else(|| env.panic_with_error(Error::NotInitialized));
        if admin != stored_admin {
            env.panic_with_error(EscrowError::UnauthorizedRole);
        }
        admin.require_auth();

        if limit < 1 || limit > DEFAULT_SETTLEMENT_LIMIT {
            env.panic_with_error(EscrowError::SettlementLimitOutOfBounds);
        }

        let old_limit: i128 = Self::read_settlement_limit(&env);
        env.storage()
            .persistent()
            .set(&DataKey::SettlementLimit, &limit);

        env.events().publish(
            (Symbol::new(&env, "settlement_limit"),),
            (old_limit, limit, admin, env.ledger().timestamp()),
        );
        true
    }

    /// Returns the current settlement limit in stroops.
    ///
    /// Defaults to [`DEFAULT_SETTLEMENT_LIMIT`] when no value has been set.
    pub fn get_settlement_limit(env: Env) -> i128 {
        Self::read_settlement_limit(&env)
    }
}
