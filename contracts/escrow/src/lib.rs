//! TalentTrust escrow contract for milestone-based freelancer payments.
//!
//! The crate root exposes the Soroban contract and still owns several public
//! entrypoints directly: initialization, settlement-token binding, deposits,
//! milestone release/refund/cancel flows, reputation, work evidence, protocol
//! fee withdrawal, and dispute entrypoints. Supporting modules keep reusable
//! validation, storage, governance, and lifecycle helpers close to the paths
//! that use them.
//!
//! ## Escrow source tree map
//!
//! | Source | Responsibility | Storage keys owned or touched |
//! | --- | --- | --- |
//! | `lib.rs` | Contract wrapper plus root entrypoints for setup, custody, money movement, reads, reputation, work evidence, pause/emergency, fee withdrawal, and dispute orchestration. | `DataKey::Initialized`, `Admin`, `SettlementToken`, `Paused`, `Emergency`, `ReadinessChecklist`, `Contract(id)`, `(Contract(id), "milestones")`, `MilestoneApprovals`, `AccumulatedProtocolFees`, `ReputationIssued`, `PendingReputationCredits`, `Reputation`, `ReputationComment` |
//! | `amount_validation` | Stateless validation and checked arithmetic for stroop amounts and milestone totals. | None directly; callers write validated amounts to `Contract(id)` and milestone vectors. |
//! | `approvals` | Temporary milestone release approvals and release-authorization checks. | Temporary `DataKey::MilestoneApprovals(contract_id, milestone_index)`; reads `Contract(id)` and `(Contract(id), "milestones")`. |
//! | `deposit` | Deposit preflight and post-transfer accounting used by `deposit_funds`. | `DataKey::Contract(contract_id)` and `(DataKey::Contract(contract_id), "milestones")`. |
//! | `finalize` | Immutable finalization records, finalization guards, and final contract summaries. | `DataKey::Finalization(contract_id)`; reads `Contract(id)`, `(Contract(id), "milestones")`, `Paused`, and `Emergency`. |
//! | `migration` | Client migration proposals, acceptance checks, cancellation, and pending-migration reads. | Temporary `DataKey::PendingClientMigration(contract_id)`; reads and updates `DataKey::Contract(contract_id)`. |
//! | `rollback` | Guarded rollback of unchanged, unresolved disputes. | `DataKey::DisputeRollback(contract_id)`; reads and updates `DataKey::Contract(contract_id)` and its milestones. |
//! | `ttl` | TTL constants plus helpers for temporary and persistent storage renewal. | Extends caller-provided keys, especially `Contract(id)`, `(Contract(id), "milestones")`, `NextContractId`, participant indexes, approvals, and migrations. |
//! | `types` | Shared Soroban types, error enums, summaries, governance records, dispute records, and the canonical `DataKey` enum. | Declares storage key schema only; does not access storage itself. (New in this release: `DataKey::MaxDisputes`, `DataKey::DisputeCount(contract_id)`.) |
//! | `utils` | Small deterministic helpers shared by entrypoints, currently ledger timestamp access. | None. |
//! | `create_contract` | Contract creation, participant/milestone validation, ID allocation, and creation events. | `DataKey::Contract(id)`, `(DataKey::Contract(id), "milestones")`, `NextContractId`, and `GovernedParameters`. |
//! | `governance` | Admin-controlled protocol fee, governed parameter, readiness, and admin-rotation entrypoints. | `DataKey::Admin`, `ProtocolFeeBps`, `PendingAdmin`, `GovernedParameters`, and `ReadinessChecklist`. |
//!
//! Generate this map with `cargo doc -p escrow --no-deps` and open
//! `target/doc/escrow/index.html`.
#![no_std]
#![allow(clippy::derivable_impls)]
#![allow(clippy::manual_range_contains)]
#![allow(clippy::assertions_on_constants)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::type_complexity)]
#![allow(clippy::needless_range_loop)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::collapsible_else_if)]
#![allow(clippy::redundant_field_names)]
#![allow(clippy::ptr_arg)]
#![allow(clippy::useless_vec)]
#![allow(clippy::let_and_return)]
#![allow(clippy::inconsistent_digit_grouping)]
#![allow(clippy::int_plus_one)]
#![allow(clippy::duplicated_attributes)]
#![allow(clippy::unreadable_literal)]
#![allow(clippy::redundant_clone)]
#![allow(clippy::bool_assert_comparison)]
#![allow(clippy::needless_borrow)]
#![allow(clippy::clone_on_copy)]
#![allow(clippy::module_inception)]
#![allow(clippy::single_match)]
#![allow(clippy::useless_conversion)]

mod amount_validation;
mod approvals;
mod constants;
mod deposit;
pub mod events;
mod finalize;
mod governance;
mod migration;
mod storage;
mod ttl;
mod types;

pub use constants::*;
mod utils;

use soroban_sdk::{
    contract, contracterror, contractimpl, symbol_short, token, Address, Env, String, Symbol, Vec,
};

pub use amount_validation::accumulate_amounts;
pub use amount_validation::available_balance;
pub use amount_validation::safe_add_amounts;
pub use amount_validation::safe_subtract_amounts;
pub use amount_validation::validate_deposit_amount;
pub use amount_validation::validate_milestone_amounts;
pub use amount_validation::validate_single_amount;
pub use constants::{
    BPS_DENOMINATOR, INITIAL_CONTRACT_ID, MAX_BPS, MAX_COMMENT_BYTES, MAX_EVIDENCE_BYTES,
    MAX_RATING, MIN_RATING, PARTIAL_REFUND_DENOMINATOR, PARTIAL_REFUND_FREELANCER_SHARE,
    REPUTATION_CREDIT_INCREMENT,
};
pub use dispute::final_status_after_resolution;
pub use dispute::resolution_payouts;
pub use migration::PendingClientMigration;
pub use storage::{initialize_storage_version, ESCROW_STORAGE_VERSION};
pub use ttl::{ADMIN_ROTATION_MIN_DELAY_LEDGERS, PENDING_MIGRATION_TTL_LEDGERS};
// Canonical milestone-vector storage helpers (issue #701). Every module in
// the contract must route milestone reads/writes through these (defined in
// `ttl`) rather than constructing the composite `(DataKey::Contract(id),
// Symbol("milestones"))` key inline. Centralising access gives a single
// point of truth for the key shape, the missing-entry error path, and
// the persistent-TTL bump parameters used by every read and write.
pub use ttl::{
    load_milestones, milestone_storage_key, store_milestones, try_load_milestones,
};
// Keep shared storage keys and escrow domain types centralized in `types.rs`.
// `DisputeResolution` and `DisputeSplit` are defined once in `types.rs` and
// re-exported here; `dispute.rs` uses them via `crate::DisputeResolution`.
pub use milestones::{Milestone, MilestoneApprovals, MilestoneSummary, ReleaseAuthorization};
pub use types::{
    BatchSettlementResult, Contract, ContractBounds, ContractStatus, ContractSummary, DataKey,
    DepositMode, DisputeMetadata, DisputeMetadataV0, DisputeResolution, DisputeSplit, Error,
    GovernedParameters, Milestone, MilestoneApprovals, MilestoneSummary, PendingAdminProposal,
    ReadinessChecklist, ReleaseAuthorization, Reputation, SettlementItem, SplitAmounts,
    CONTRACT_SUMMARY_SCHEMA_VERSION, DISPUTE_STORAGE_VERSION,
};

type Error = EscrowError;

// Maximum bounds constants - re-export from amount_validation for API visibility
pub const MAX_MILESTONES: u32 = 10;
pub const MAX_SINGLE_AMOUNT_STROOPS: i128 = crate::amount_validation::MAX_SINGLE_AMOUNT_STROOPS;
pub const MAX_TOTAL_ESCROW_STROOPS: i128 = MAX_SINGLE_AMOUNT_STROOPS;

/// Default settlement limit (max single milestone amount in stroops).
/// Preserves the original hard-coded behaviour; admin may lower it via
/// [`Escrow::set_settlement_limit`] but never above this absolute ceiling.
pub const DEFAULT_SETTLEMENT_LIMIT: i128 = MAX_SINGLE_AMOUNT_STROOPS;

/// Maximum number of items accepted by [`Escrow::finalize_contracts_batch`].
///
/// Chosen to match the existing batch-create cap (10) so a single Soroban
/// invocation cannot exhaust the per-transaction compute budget.  Requests
/// larger than this are rejected with [`EscrowError::BatchSettlementTooLarge`]
/// before any storage is touched.
pub const MAX_BATCH_SETTLEMENT: u32 = 10;

#[contract]
pub struct Escrow;

mod create_contract;
mod dispute;
mod governance;

/// Governance-level errors for admin-gated operations.
#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum EscrowError {
    InvalidParticipant = 1,
    EmptyMilestones = 2,
    InvalidMilestoneAmount = 3,
    InvalidDepositAmount = 4,
    InvalidMilestone = 5,
    ContractNotFound = 6,
    EmptyRefundRequest = 7,
    DuplicateMilestoneInRefund = 8,
    AlreadyReleased = 9,
    AlreadyRefunded = 10,
    InsufficientFunds = 11,
    AlreadyInitialized = 12,
    InsufficientAccumulatedFees = 13,
    /// Returned by lifecycle entrypoints when `initialize` has not been called.
    ///
    /// All money-flow operations require initialization so the admin-controlled
    /// safety rails (pause, emergency controls, protocol fees) are always in
    /// scope before any funds can move.
    NotInitialized = 14,
    UnauthorizedRole = 15,
    ContractPaused = 16,
    EmergencyActive = 17,
    InvalidState = 18,
    InvalidRating = 19,
    SelfRating = 20,
    ReputationAlreadyIssued = 21,
    NotCompleted = 22,
    FreelancerMismatch = 23,
    InvalidStatusTransition = 24,
    ArbiterRequired = 25,
    InvalidDisputeSplit = 26,
    AccountingInvariantViolated = 27,
    PotentialOverflow = 28,
    AlreadyFinalized = 29,
    AmountMustBePositive = 30,
    /// No settlement token has been bound for custody transfers.
    SettlementTokenNotConfigured = 31,
    /// A settlement token has already been bound.
    SettlementTokenAlreadyBound = 32,
    /// The sum of milestone amounts exceeded the configured maximum or overflowed.
    TotalCapExceeded = 33,
    /// Too many milestones were provided.
    TooManyMilestones = 34,
    /// An arbiter was required by the release authorization mode but not provided.
    MissingArbiter = 35,
    /// The provided arbiter is invalid (same as client or freelancer).
    InvalidArbiter = 36,
    /// Contract is cancelled and must not accept further value-moving operations.
    ContractCancelled = 37,
    /// Contract has been refunded and is terminal for value-moving operations.
    ContractRefunded = 38,
    /// The address supplied as settlement token is not a valid token contract.
    /// The pre-bind probe called `token::Client::balance` against the escrow
    /// contract address and the call panicked — the address does not implement
    /// the SAC token interface.
    InvalidSettlementToken = 39,
    /// The address supplied as settlement token is the escrow contract itself.
    /// Binding self would create a circular custody reference and brick all
    /// transfer paths.
    SettlementTokenIsSelf = 40,
    /// The address supplied as settlement token is the escrow admin.
    /// Binding the admin as the custody asset conflates governance authority
    /// with the settlement token role.
    SettlementTokenIsAdmin = 41,
    /// Reputation feedback comment was empty.
    EmptyComment = 42,
    /// Reputation feedback comment exceeded the 200-character maximum.
    CommentTooLong = 43,
    /// Milestone rollback is not allowed in the current state.
    RollbackNotAllowed = 44,
}

impl Escrow {
    pub(crate) fn validate_contract_id_bounds(env: &Env, contract_id: u32) {
        if contract_id == 0 {
            env.panic_with_error(Error::InvalidContractId);
        }
    }

    /// Get the settlement token address from the canonical `DataKey` binding.
    pub(crate) fn read_settlement_token(env: &Env) -> Option<Address> {
        env.storage().persistent().get(&DataKey::SettlementToken)
    }

    pub(crate) fn write_settlement_token(env: &Env, token: &Address) {
        settlement::write_settlement_token(env, token);
    }

    pub(crate) fn require_initialized(env: &Env) {
        if !env
            .storage()
            .persistent()
            .get::<_, bool>(&DataKey::Initialized)
            .unwrap_or(false)
        {
            env.panic_with_error(Error::NotInitialized);
        }
    }

    pub(crate) fn is_initialized(env: &Env) -> bool {
        env.storage()
            .persistent()
            .get::<_, bool>(&DataKey::Initialized)
            .unwrap_or(false)
    }

    pub(crate) fn require_not_paused(env: &Env) {
        if env.storage().persistent().get::<_, bool>(&DataKey::Paused).unwrap_or(false) {
            env.panic_with_error(EscrowError::ContractPaused);
        }
        if env.storage().persistent().get::<_, bool>(&DataKey::Emergency).unwrap_or(false) {
            env.panic_with_error(EscrowError::EmergencyActive);
        }
    }

    pub(crate) fn require_not_finalized(env: &Env, contract_id: u32) {
        if env.storage().persistent().has(&DataKey::Finalization(contract_id)) {
            env.panic_with_error(EscrowError::AlreadyFinalized);
        }
    }

    pub(crate) fn validate_contract_id_bounds(env: &Env, contract_id: u32) {
        if contract_id == 0 {
            env.panic_with_error(EscrowError::InvalidContractId);
        }
    }

    /// Validate that a contract ID is within acceptable bounds.
    pub(crate) fn validate_contract_id_bounds(env: &Env, contract_id: u32) {
        if contract_id == 0 {
            env.panic_with_error(Error::InvalidContractId);
        }
    }

    pub(crate) fn require_party(env: &Env, contract: &Contract, caller: &Address) {
        let is_client = caller == &contract.client;
        let is_freelancer = caller == &contract.freelancer;
        let is_arbiter = contract.arbiter.as_ref() == Some(caller);

        if is_client || is_freelancer || is_arbiter {
            return;
        }

        env.panic_with_error(Error::PartyNotAuthorized);
    }

    /// Returns the current escrow state for a contract.
    ///
    /// Read-only view. Returns a sensible default when no escrow record exists
    /// instead of panicking.
    pub fn get_escrow_state(env: Env, contract_id: String) -> Contract {
        let key = DataKey::Contract(contract_id);
        env.storage()
            .persistent()
            .get(&key)
            .unwrap_or(Contract::default())
    }

    /// Read the admin-configurable settlement limit from storage, falling back
    /// to [`DEFAULT_SETTLEMENT_LIMIT`] when no value has been set.
    pub(crate) fn read_settlement_limit(env: &Env) -> i128 {
        env.storage()
            .persistent()
            .get(&DataKey::SettlementLimit)
            .unwrap_or(DEFAULT_SETTLEMENT_LIMIT)
    }
}

#[contractimpl]
impl Escrow {
    /// Bind the single Stellar Asset Contract (SAC) token this escrow instance will custody.
    ///
    /// This is a **write-once** step: once a token is recorded under
    /// [`DataKey::SettlementToken`] all subsequent money-flow entrypoints
    /// (`deposit_funds`, `release_milestone`, `refund_unreleased_milestones`,
    /// `cancel_contract`, `withdraw_protocol_fees`) read that address to execute SAC
    /// `transfer` calls. A second call with any token address is rejected with
    /// `SettlementTokenAlreadyBound`.
    ///
    /// # Pre-bind probe (issue #723)
    ///
    /// Before persisting the token address, this entrypoint performs a **read-only
    /// probe** to verify the supplied address is a live SAC token contract:
    ///
    /// 1. Calls `token::Client::balance(env.current_contract_address())` against
    ///    the candidate address. If the address does not implement the SAC token
    ///    interface, the call panics and the bind is rejected with
    ///    `InvalidSettlementToken`.
    /// 2. Rejects `env.current_contract_address()` (the escrow contract itself)
    ///    with `SettlementTokenIsSelf` Ã¢â‚¬â€ binding self creates a circular custody
    ///    reference.
    /// 3. Rejects the stored admin address with `SettlementTokenIsAdmin` Ã¢â‚¬â€
    ///    conflating governance authority with the settlement token role is a
    ///    privilege-separation violation.
    ///
    /// # Reentrancy mitigation
    ///
    /// All downstream money-flow entrypoints (`deposit_funds`, `release_milestone`,
    /// `cancel_contract`, `refund_unreleased_milestones`) follow strict
    /// **state-before-transfer** (Checks-Effects-Interactions) ordering: contract
    /// state is finalized *before* any `token::Client::transfer` call. A
    /// malicious token contract that re-enters the escrow during a transfer will
    /// observe the already-mutated state and cannot double-spend or front-run
    /// the operation. The probe itself performs no state mutation — it only
    /// reads the token balance — so it cannot be used as a reentrancy vector.
    ///
    /// See [`docs/escrow/sac-custody.md`](../../../docs/escrow/sac-custody.md) for the
    /// full custody model, accounting invariant, and lifecycle sequence diagram.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment
    /// * `admin` - The admin address (must match stored admin)
    /// * `token` - The SAC token address
    ///
    /// # Returns
    /// * `bool` - `true` on successful settlement token binding
    ///
    /// # Errors
    /// * `NotInitialized` if `initialize` has not been called
    /// * `ContractPaused` if the contract is paused
    /// * `UnauthorizedRole` if `admin` is not the stored admin
    /// * `SettlementTokenAlreadyBound` if a token is already bound
    /// * `InvalidSettlementToken` if the probe call to `token::Client::balance` panics
    /// * `SettlementTokenIsSelf` if `token == env.current_contract_address()`
    /// * `SettlementTokenIsAdmin` if `token == stored_admin`
    ///
    /// # Examples
    /// ```rust,ignore
    /// let client = EscrowClient::new(&env, &contract_id);
    /// let bound = client.bind_settlement_token(&admin, &usdc_token_address);
    /// assert!(bound);
    /// ```
    ///
    /// # Events
    /// On a successful, authorized bind this publishes a settlement bind event
    /// with an indexed short topic for efficient off-chain querying by indexers
    /// and monitoring dashboards.
    ///
    /// * Topics: `(symbol_short!("sttl_bind"),)`
    /// * Data: `(admin: Address, token: Address, timestamp: u64)`
    ///
    /// The event only fires after the write succeeds. Rejected binds
    /// (uninitialized, unauthorized, invalid token, self, admin) panic before
    /// this point and therefore publish nothing. All payload fields are public
    /// configuration.
    pub fn bind_settlement_token(env: Env, admin: Address, token: Address) -> bool {
        Self::require_initialized(&env);
        Self::require_not_paused(&env);
        let stored_admin: Address = env
            .storage()
            .persistent()
            .get(&DataKey::Admin)
            .unwrap_or_else(|| env.panic_with_error(EscrowError::NotInitialized));

        if admin != stored_admin {
            env.panic_with_error(Error::UnauthorizedRole);
        }
        admin.require_auth();

        if Self::read_settlement_token(&env).is_some() {
            env.panic_with_error(EscrowError::SettlementTokenAlreadyBound);
        }

        // Ã¢â€â‚¬Ã¢â€â‚¬ Pre-bind probe (issue #723) Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
        //
        // Reject the escrow contract's own address Ã¢â‚¬â€ binding self would create
        // a circular custody reference and brick every transfer path.
        if token == env.current_contract_address() {
            env.panic_with_error(EscrowError::SettlementTokenIsSelf);
        }

        // Reject the admin address Ã¢â‚¬â€ conflating governance authority with the
        // settlement token role is a privilege-separation violation.
        if token == stored_admin {
            env.panic_with_error(EscrowError::SettlementTokenIsAdmin);
        }

        // Read-only probe: call `token::Client::balance` against the escrow
        // contract address. If `token` does not implement the SAC token
        // interface, the host panics and we translate that into
        /// `InvalidSettlementToken`.
        //
        // This is safe because:
        // - `balance` is a read-only entrypoint (no state mutation on the
        //   token contract).
        // - We have not yet written anything to storage Ã¢â‚¬â€ a panic here leaves
        //   no partial state.
        // - The probe cannot be used for reentrancy: it calls `balance`, not
        //   `transfer`, and the escrow has no callback the token could invoke.
        let token_client = token::Client::new(&env, &token);
        let _probe: i128 = token_client.balance(&env.current_contract_address());

        Self::write_settlement_token(&env, &token);

        // Emit after the binding write succeeds so indexers can track the bound
        // asset using an indexed short topic for efficient off-chain querying.
        env.events().publish(
            (symbol_short!("sttl_bind"),),
            (admin, token, env.ledger().timestamp()),
        );
        true
    }

    /// Deprecated thin delegate for [`bind_settlement_token`](Self::bind_settlement_token).
    ///
    /// Retained for backward compatibility with external callers that used the historical API name.
    /// Delegates directly to [`bind_settlement_token`](Self::bind_settlement_token) and inherits
    /// every security guard (`SettlementTokenAlreadyBound`, admin auth check, SAC interface probe,
    /// self/admin validation) and event emission.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment
    /// * `admin` - The admin address (must match stored admin)
    /// * `token` - The SAC token address
    ///
    /// # Returns
    /// * `bool` - `true` on successful settlement token binding
    ///
    /// # Errors
    /// * `NotInitialized` if `initialize` has not been called
    /// * `UnauthorizedRole` if `admin` is not the stored admin
    /// * `SettlementTokenAlreadyBound` if a token is already bound
    ///
    /// # Examples
    /// ```rust,ignore
    /// let client = EscrowClient::new(&env, &contract_id);
    /// let bound = client.set_settlement_token(&admin, &usdc_token_address);
    /// assert!(bound);
    /// ```
    ///
    /// # Deprecated
    /// Use [`bind_settlement_token`](Self::bind_settlement_token) instead.
    #[deprecated(note = "Use bind_settlement_token instead.")]
    pub fn set_settlement_token(env: Env, admin: Address, token: Address) -> bool {
        Self::bind_settlement_token(env, admin, token)
    }

    /// Returns the bound settlement token, or `None` if no token has been bound.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment
    ///
    /// # Returns
    /// * `Option<Address>` - `Some(Address)` with the bound SAC token address, or `None` if unbound
    ///
    /// # Examples
    /// ```rust,ignore
    /// let client = EscrowClient::new(&env, &contract_id);
    /// if let Some(token_address) = client.get_settlement_token() {
    ///     // Process bound token address
    /// }
    /// ```
    pub fn get_settlement_token(env: Env) -> Option<Address> {
        Self::read_settlement_token(&env)
    }

    /// Returns `true` exactly when a settlement token is bound.
    ///
    /// This is the recommended cheap pre-flight readiness check before calling
    /// `deposit_funds`, which panics when no settlement token has been bound.
    /// Integrators that only need to know *whether* the escrow can accept
    /// deposits Ã¢â‚¬â€ without caring about the specific token address Ã¢â‚¬â€ should use
    /// this instead of fetching and discarding the `Address` from
    /// `get_settlement_token`.
    ///
    /// Read-only and auth-free: it performs no state mutation (no TTL write is
    /// needed for the simple binding key).
    ///
    /// # Arguments
    /// * `env` - The Soroban environment
    ///
    /// # Returns
    /// * `true` if a settlement token is bound
    /// * `false` if no settlement token has been bound yet
    ///
    /// # Examples
    /// ```rust,ignore
    /// let client = EscrowClient::new(&env, &contract_id);
    /// if client.is_settlement_token_bound() {
    ///     // Safe to make deposits
    /// }
    /// ```
    pub fn is_settlement_token_bound(env: Env) -> bool {
        Self::read_settlement_token(&env).is_some()
    }

    // Ã¢â€â‚¬Ã¢â€â‚¬ Initialization Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

    /// Initializes the escrow contract with the operational admin.
    ///
    /// Single-use. Stores the admin address that controls pause, emergency,
    /// protocol-fee, and governance operations. All escrow lifecycle operations
    /// (create, deposit, release, refund, cancel) call `require_initialized`
    /// so that these safety rails are always bound before money can move.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment
    /// * `admin` - The admin address initializing the escrow contract
    ///
    /// # Returns
    /// * `bool` - `true` on successful initialization
    ///
    /// # Errors
    /// * `AlreadyInitialized` - If `initialize` has already been called
    ///
    /// # Examples
    /// ```rust,ignore
    /// let client = EscrowClient::new(&env, &contract_id);
    /// let initialized = client.initialize(&admin);
    /// assert!(initialized);
    /// ```
    pub fn initialize(env: Env, admin: Address) -> bool {
        if env
            .storage()
            .persistent()
            .get::<_, bool>(&DataKey::Initialized)
            .unwrap_or(false)
        {
            env.panic_with_error(EscrowError::AlreadyInitialized);
        }

        admin.require_auth();
        storage::initialize_storage_version(&env);
        env.storage().persistent().set(&DataKey::Initialized, &true);
        env.storage().persistent().set(&DataKey::Admin, &admin);
        env.storage()
            .persistent()
            .set(&DataKey::NextContractId, &INITIAL_CONTRACT_ID);

        let mut checklist: ReadinessChecklist = env
            .storage()
            .persistent()
            .get(&DataKey::ReadinessChecklist)
            .unwrap_or_default();
        checklist.initialized = true;
        env.storage()
            .persistent()
            .set(&DataKey::ReadinessChecklist, &checklist);

        env.events().publish(
            (symbol_short!("init"), Symbol::new(&env, "admin_set")),
            (admin.clone(), env.ledger().timestamp()),
        );

        true
    }

    /// Returns the stored governance admin address.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment
    ///
    /// # Returns
    /// * `Option<Address>` - `Some(Address)` of the admin, or `None` if uninitialized
    ///
    /// # Examples
    /// ```rust,ignore
    /// let client = EscrowClient::new(&env, &contract_id);
    /// let admin = client.get_admin();
    /// ```
    pub fn get_admin(env: Env) -> Option<Address> {
        env.storage().persistent().get(&DataKey::Admin)
    }

    /// Returns the current protocol-wide bounds used by validation paths.
    ///
    /// Callers and off-chain indexers should query this endpoint to discover
    /// the limits enforced by `create_contract`:
    ///
    /// - `max_milestones`: maximum number of milestones per contract.
    /// - `max_single_milestone_stroops`: maximum amount for any single milestone
    ///   (admin-configurable via [`set_settlement_limit`](Self::set_settlement_limit),
    ///   defaults to [`DEFAULT_SETTLEMENT_LIMIT`]).
    /// - `max_total_escrow_stroops`: maximum sum of all milestone amounts.
    /// - `max_fee_bps`: protocol fee ceiling in basis points (10 000 = 100 %).
    ///
    /// Most fields are compile-time constants. The settlement limit is read
    /// from persistent storage and may change at runtime via admin governance.
    ///
    /// # Arguments
    /// * `_env` - The Soroban environment
    ///
    /// # Returns
    /// A [`ContractBounds`] value containing only limit fields. Unlike
    /// [`get_contract_summary`], this type carries no per-contract participant
    /// or accounting data and its schema version tracks the limits API only.
    ///
    /// The function is read-only and requires no authorization.
    pub fn get_bounds(_env: Env) -> ContractBounds {
        ContractBounds {
            max_milestones: MAX_MILESTONES,
            max_single_milestone_stroops: Self::read_settlement_limit(&_env),
            max_total_escrow_stroops: MAX_TOTAL_ESCROW_STROOPS,
            max_fee_bps: MAX_BPS,
        }
    }

    /// Returns the milestone-related configuration values.
    ///
    /// Combines compile-time bounds with runtime-governed parameters. Before
    /// initialization the governed fields fall back to sensible defaults so
    /// callers can always read a complete configuration without panicking.
    pub fn get_milestones_config(env: Env) -> MilestonesConfig {
        let governed: Option<GovernedParameters> = env
            .storage()
            .persistent()
            .get(&DataKey::GovernedParameters);
        MilestonesConfig {
            max_milestones: MAX_MILESTONES,
            max_single_milestone_stroops: MAX_SINGLE_AMOUNT_STROOPS,
            max_total_escrow_stroops: governed
                .map(|p| p.max_escrow_total_stroops)
                .unwrap_or(MAX_TOTAL_ESCROW_STROOPS),
            max_fee_bps: 10_000,
            max_schedule_title_len: MAX_SCHEDULE_TITLE_LEN,
            max_schedule_description_len: MAX_SCHEDULE_DESCRIPTION_LEN,
        }
    }

    /// Returns the current mainnet readiness checklist.
    ///
    /// The checklist tracks critical configuration steps that must be completed
    /// before the escrow contract is considered ready for mainnet production:
    ///
    /// - **`initialized`**: Flipped to `true` when `initialize` completes successfully.
    ///   Ensures that an admin has been bound to the contract.
    /// - **`governed_params_set`**: Flipped to `true` when governance/protocol parameters
    ///   (such as fees and maximum caps) are configured. Flipped during `initialize_protocol_governance`
    ///   or parameter updates.
    /// - **`emergency_controls_enabled`**: Flipped to `true` when emergency pause controls are exercised
    ///   for the first time (via `activate_emergency_pause`). This verifies the operator has functioning
    ///   emergency access.
    ///
    /// # Implications for a Clean Deploy
    /// Activating the emergency pause to flip the `emergency_controls_enabled` flag leaves the contract
    /// in a paused state. To complete a clean deploy and allow normal operations, the operator must
    /// subsequently call `resolve_emergency` to unpause the contract.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment
    ///
    /// # Returns
    /// * `ReadinessChecklist` - Struct containing setup readiness booleans
    ///
    /// # Examples
    /// ```rust,ignore
    /// let client = EscrowClient::new(&env, &contract_id);
    /// let readiness = client.get_mainnet_readiness_info();
    /// assert!(readiness.initialized);
    /// ```
    pub fn get_mainnet_readiness_info(env: Env) -> ReadinessChecklist {
        env.storage()
            .persistent()
            .get(&DataKey::ReadinessChecklist)
            .unwrap_or_default()
    }

    /// Pull the settlement-token deposit from the client into the escrow contract address.
    ///
    /// Executes `SAC::transfer(from: client, to: escrow_address, amount)` and advances
    /// status from `Created` to `Funded` once the full milestone sum has been deposited.
    /// Requires `bind_settlement_token` to have been called first; panics with
    /// `SettlementTokenNotConfigured` otherwise.
    ///
    /// See [`docs/escrow/sac-custody.md`](../../../docs/escrow/sac-custody.md) for the
    /// full custody model and accounting invariant.
    ///
    /// # Arguments
    /// * `env` - The contract environment
    /// * `contract_id` - The contract ID
    /// * `caller` - The address of the caller (must be the client)
    /// * `amount` - The amount to deposit (in stroops)
    ///
    /// # Returns
    /// `true` if deposit was successful
    ///
    /// # Errors
    /// * `SettlementTokenNotConfigured` - If `bind_settlement_token` has not been called
    /// * `AmountMustBePositive` - If amount is <= 0
    /// * `ContractNotFound` - If contract doesn't exist
    /// * `InvalidState` - If contract is not in Created state
    /// * `UnauthorizedRole` - If caller is not the client
    ///
    /// # Examples
    /// ```rust,ignore
    /// let client = EscrowClient::new(&env, &contract_id);
    /// let deposited = client.deposit_funds(&1, &client_address, &1_000_0000000);
    /// assert!(deposited);
    /// ```
    pub fn deposit_funds(env: Env, contract_id: u32, caller: Address, amount: i128) -> bool {
        Self::require_initialized(&env);
        Self::require_not_paused(&env);
        Self::require_not_finalized(&env, contract_id);

        let validated = deposit::validate_deposit(&env, contract_id, &caller, amount);

        let token = validated.contract.token.clone();

        let token_client = token::Client::new(&env, &token);
        token_client.transfer(&caller, &env.current_contract_address(), &amount);

        deposit::apply_validated_deposit(&env, contract_id, caller, validated)
    }

    /// Simulate a deposit without mutating state or moving tokens.
    ///
    /// Runs the same preflight validation as [`deposit_funds`](Self::deposit_funds)
    /// — initialization check, pause guard, deposit validation, settlement-token
    /// configuration — and returns the projected [`SimulateDepositResult`] that a
    /// real deposit would produce, but without executing the SAC transfer, writing
    /// storage, or emitting events.
    ///
    /// Because the simulation never calls into the token contract, it does **not**
    /// require the caller's authorization (no `require_auth`). This makes it a cheap
    /// read-only pre-flight that callers can invoke to preview the deposit outcome
    /// before committing to the actual transaction.
    ///
    /// # Arguments
    /// * `env` - The contract environment
    /// * `contract_id` - The contract ID
    /// * `caller` - The address of the caller
    /// * `amount` - The amount to simulate depositing (in stroops)
    ///
    /// # Returns
    /// A [`SimulateDepositResult`] with the projected funded amounts and status
    ///
    /// # Errors
    /// Returns the same errors as [`deposit_funds`](Self::deposit_funds):
    /// * `NotInitialized` if `initialize` has not been called
    /// * `ContractPaused` if the contract is paused
    /// * `AmountMustBePositive` if amount is ≤ 0
    /// * `ContractNotFound` if the contract doesn't exist
    /// * `UnauthorizedRole` if `caller` is not the client
    /// * `InvalidState` if the contract is not in `Created` or `PartiallyFunded` state
    /// * `InvalidDepositAmount` if the deposit would exceed the total milestone amount
    /// * `SettlementTokenNotConfigured` if no settlement token has been bound
    pub fn simulate_deposit_funds(
        env: Env,
        contract_id: u32,
        caller: Address,
        amount: i128,
    ) -> SimulateDepositResult {
        Self::require_initialized(&env);
        Self::require_not_paused(&env);

        // Validate all the same preconditions as the real deposit path.
        let validated = deposit::validate_deposit(&env, contract_id, &caller, amount);

        // Check settlement-token configuration (same guard as deposit_funds).
        let _token = Self::read_settlement_token(&env)
            .unwrap_or_else(|| env.panic_with_error(Error::SettlementTokenNotConfigured));

        // Project the contract status that would result from the deposit.
        let projected_status = {
            let total = validated.total_amount;
            if validated.new_funded_amount == total {
                ContractStatus::Funded
            } else {
                ContractStatus::PartiallyFunded
            }
        };

        SimulateDepositResult {
            current_funded_amount: validated.contract.funded_amount,
            new_funded_amount: validated.new_funded_amount,
            projected_status,
            total_milestone_amount: validated.total_amount,
        }
    }

    /// Finalize an escrow contract by writing immutable close metadata.
    ///
    /// `finalizer` must authorize the call and must be the stored client,
    /// freelancer, or assigned arbiter. Finalization is allowed only while the
    /// contract is `Completed` or `Disputed`. Once finalized, future
    /// contract-specific mutations fail with `AlreadyFinalized`.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment
    /// * `contract_id` - The contract ID to finalize
    /// * `finalizer` - The address of the finalizer (client, freelancer, or arbiter)
    ///
    /// # Returns
    /// * `bool` - `true` if finalized successfully
    ///
    /// # Errors
    /// - `ContractPaused` when pause or emergency controls are active.
    /// - `ContractNotFound` when `contract_id` is unknown.
    /// - `AlreadyFinalized` when a close record already exists.
    /// - `UnauthorizedRole` when `finalizer` is not a contract participant.
    /// - `InvalidStatusTransition` unless status is `Completed` or `Disputed`.
    ///
    /// # Examples
    /// ```rust,ignore
    /// let client = EscrowClient::new(&env, &contract_id);
    /// let finalized = client.finalize_contract(&1, &client_address);
    /// assert!(finalized);
    /// ```
    pub fn finalize_contract(env: Env, contract_id: u32, finalizer: Address) -> bool {
        finalize::finalize_contract_impl(&env, contract_id, finalizer)
    }

    /// Finalize up to [`MAX_BATCH_SETTLEMENT`] contracts in a single invocation.
    ///
    /// This is a bounded batch companion to [`finalize_contract`](Self::finalize_contract).
    /// It accepts a vector of [`SettlementItem`] entries — each pairing a `contract_id`
    /// with the `finalizer` address for that contract — and processes them one at a time
    /// using exactly the same logic as the single-item entrypoint.
    ///
    /// # Bounding
    ///
    /// The vector length is checked **before** any item is processed:
    /// - An empty vector is rejected immediately with [`EscrowError::BatchSettlementEmpty`].
    /// - A vector longer than [`MAX_BATCH_SETTLEMENT`] is rejected immediately with
    ///   [`EscrowError::BatchSettlementTooLarge`].
    ///
    /// # Per-item semantics
    ///
    /// Each item is processed independently:
    /// - Success or failure of one item does **not** affect subsequent items.
    /// - A successful item emits the same `("finalized", contract_id)` event as the
    ///   single-item entrypoint.
    /// - Failed items are recorded in the output with `success: false` and an
    ///   `error_code` matching the [`EscrowError`] discriminant that the equivalent
    ///   single-item call would have panicked with.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment
    /// * `items` - Bounded vector of [`SettlementItem`]; 1–[`MAX_BATCH_SETTLEMENT`] entries
    ///
    /// # Returns
    /// A [`Vec<BatchSettlementResult>`] with one entry per input item in the same order.
    ///
    /// # Errors (whole-call failures — panic before any item is processed)
    /// * [`EscrowError::ContractPaused`] / [`EscrowError::EmergencyActive`] — pause gate
    /// * [`EscrowError::BatchSettlementEmpty`] — `items` is empty
    /// * [`EscrowError::BatchSettlementTooLarge`] — `items.len() > MAX_BATCH_SETTLEMENT`
    ///
    /// # Per-item error codes (recorded in `BatchSettlementResult::error_code`)
    /// * [`EscrowError::ContractNotFound`] — unknown `contract_id`
    /// * [`EscrowError::AlreadyFinalized`] — contract already has a finalization record
    /// * [`EscrowError::UnauthorizedRole`] — `finalizer` is not a participant
    /// * [`EscrowError::InvalidStatusTransition`] — status is not `Completed` or `Disputed`
    ///
    /// # Examples
    /// ```rust,ignore
    /// use escrow::{EscrowClient, SettlementItem};
    /// let items = soroban_sdk::vec![
    ///     &env,
    ///     SettlementItem { contract_id: 1, finalizer: client_addr.clone() },
    ///     SettlementItem { contract_id: 2, finalizer: client_addr.clone() },
    /// ];
    /// let results = escrow_client.finalize_contracts_batch(&items);
    /// assert!(results.get(0).unwrap().success);
    /// ```
    pub fn finalize_contracts_batch(
        env: Env,
        items: Vec<SettlementItem>,
    ) -> Vec<BatchSettlementResult> {
        // ── Global guards ────────────────────────────────────────────────────
        // Run pause/emergency check before touching any item so callers get a
        // clean, actionable error rather than a partial result set.
        Self::require_not_paused(&env);

        // Reject empty vectors immediately — a zero-length batch is a caller
        // error, not a "zero successes" scenario.
        if items.is_empty() {
            env.panic_with_error(EscrowError::BatchSettlementEmpty);
        }

        // Enforce the hard cap before doing any work so the cost of an
        // over-cap call stays O(1) rather than O(cap).
        if items.len() > MAX_BATCH_SETTLEMENT {
            env.panic_with_error(EscrowError::BatchSettlementTooLarge);
        }

        // ── Per-item processing ──────────────────────────────────────────────
        let mut results: Vec<BatchSettlementResult> = Vec::new(&env);

        for i in 0..items.len() {
            let item: SettlementItem = items.get(i).unwrap();
            let contract_id = item.contract_id;
            let finalizer = item.finalizer.clone();

            // Attempt finalization using the same implementation function as
            // the single-item entrypoint.  We use `try_invoke_contract` style
            // error capture via a nested match on `finalize_contract_impl`.
            //
            // Soroban does not expose a native try/catch, so we replicate the
            // validation logic here and produce an error code on failure rather
            // than panicking.  This keeps per-item semantics identical to the
            // single-item path while allowing the batch to continue past
            // individual failures.
            let outcome = Self::try_finalize_one(&env, contract_id, finalizer);

            match outcome {
                Ok(_) => {
                    results.push_back(BatchSettlementResult {
                        index: i,
                        contract_id,
                        success: true,
                        error_code: None,
                    });
                }
                Err(code) => {
                    results.push_back(BatchSettlementResult {
                        index: i,
                        contract_id,
                        success: false,
                        error_code: Some(code),
                    });
                }
            }
        }

        results
    }

    /// Internal helper: attempt to finalize one contract, returning
    /// `Ok(())` on success or `Err(error_code)` on any per-item failure.
    ///
    /// This mirrors `finalize::finalize_contract_impl` but returns a typed
    /// `Result` instead of panicking so the batch entrypoint can continue
    /// past individual failures.
    fn try_finalize_one(env: &Env, contract_id: u32, finalizer: Address) -> Result<(), u32> {
        use crate::ContractStatus;

        // 1. Check contract exists.
        let contract: crate::Contract = match env
            .storage()
            .persistent()
            .get(&DataKey::Contract(contract_id))
        {
            Some(c) => c,
            None => return Err(EscrowError::ContractNotFound as u32),
        };

        // 2. Check not already finalized.
        if env
            .storage()
            .persistent()
            .has(&DataKey::Finalization(contract_id))
        {
            return Err(EscrowError::AlreadyFinalized as u32);
        }

        // 3. Check finalizer role (client, freelancer, or assigned arbiter).
        let is_client = finalizer == contract.client;
        let is_freelancer = finalizer == contract.freelancer;
        let is_arbiter = contract.arbiter.as_ref().is_some_and(|a| a == &finalizer);
        if !is_client && !is_freelancer && !is_arbiter {
            return Err(EscrowError::UnauthorizedRole as u32);
        }

        // 4. Check status is terminal (Completed or Disputed).
        if contract.status != ContractStatus::Completed
            && contract.status != ContractStatus::Disputed
        {
            return Err(EscrowError::InvalidStatusTransition as u32);
        }

        // 5. All checks pass — delegate to the canonical implementation which
        //    writes storage, emits events, and handles rollback cleanup.
        //    `require_auth` inside will be satisfied by `mock_all_auths` in
        //    tests; in production the caller must have authorized the finalizer.
        finalize::finalize_contract_impl(env, contract_id, finalizer);
        Ok(())
    }

    /// Restore an unchanged, unresolved dispute to its pre-dispute status.
    pub fn rollback_dispute(env: Env, contract_id: u32) -> bool {
        rollback::rollback_dispute_impl(&env, contract_id)
    }

    /// Return immutable close metadata for `contract_id`, if it has been finalized.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment
    /// * `contract_id` - The contract ID
    ///
    /// # Returns
    /// * `Option<FinalizationRecord>` - `Some(record)` if finalized, `None` otherwise
    ///
    /// # Examples
    /// ```rust,ignore
    /// let client = EscrowClient::new(&env, &contract_id);
    /// if let Some(record) = client.get_finalization_record(&1) {
    ///     // Process finalization record
    /// }
    /// ```
    pub fn get_finalization_record(
        env: Env,
        contract_id: u32,
    ) -> Option<finalize::FinalizationRecord> {
        finalize::get_finalization_record_impl(&env, contract_id)
    }

    /// Roll back a finalized escrow contract, removing its immutable close record.
    ///
    /// `admin` must authorize the call and match the stored admin. Rollback is
    /// allowed only when the contract is finalized and its status is `Completed`
    /// or `Disputed`. Removing the finalization record re-enables mutating
    /// lifecycle operations without changing any accounting fields.
    ///
    /// # Errors
    /// * `NotInitialized` - If `initialize` has not been called.
    /// * `UnauthorizedRole` - If `admin` is not the stored admin.
    /// * `RollbackNotAllowed` - If the contract is not finalized or not in a safe status.
    ///
    /// # Events
    /// `("rollback", contract_id)` -> `(admin, status, timestamp)`
    pub fn rollback_contract(env: Env, admin: Address, contract_id: u32) -> bool {
        finalize::rollback_contract_impl(&env, contract_id, admin)
    }

    /// Propose a client migration for an existing contract.
    ///
    /// Canonical public entrypoint; delegates to `propose_client_migration_impl`.
    /// The current client must authorize the call. The proposed client address
    /// must not be the freelancer or the current client. The pending migration
    /// is stored in temporary storage with TTL.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment
    /// * `contract_id` - The contract ID
    /// * `current_client` - The address of the current client
    /// * `new_client` - The proposed new client address
    ///
    /// # Returns
    /// * `bool` - `true` if migration proposed successfully
    ///
    /// # Errors
    /// * `ContractPaused` - If paused or in emergency mode
    /// * `UnauthorizedRole` - If `current_client` is not the stored client
    /// * `InvalidParticipant` - If `new_client` is current client or freelancer
    ///
    /// # Examples
    /// ```rust,ignore
    /// let client = EscrowClient::new(&env, &contract_id);
    /// let proposed = client.propose_client_migration(&1, &current_client_address, &new_client_address);
    /// assert!(proposed);
    /// ```
    pub fn propose_client_migration(
        env: Env,
        contract_id: u32,
        current_client: Address,
        new_client: Address,
    ) -> bool {
        Self::require_not_paused(&env);
        migration::propose_client_migration_impl(&env, contract_id, current_client, new_client)
    }

    /// Accept a live pending client migration and update the contract.
    ///
    /// Canonical public entrypoint; delegates to `accept_client_migration_impl`.
    /// Only the proposed client address may authorize acceptance.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment
    /// * `contract_id` - The contract ID
    /// * `new_client` - The proposed new client address accepting migration
    ///
    /// # Returns
    /// * `bool` - `true` if migration accepted successfully
    ///
    /// # Errors
    /// * `ContractPaused` - If paused or in emergency mode
    /// * `UnauthorizedRole` - If caller is not `new_client`
    /// * `InvalidState` - If no live pending migration exists
    ///
    /// # Examples
    /// ```rust,ignore
    /// let client = EscrowClient::new(&env, &contract_id);
    /// let accepted = client.accept_client_migration(&1, &new_client_address);
    /// assert!(accepted);
    /// ```
    pub fn accept_client_migration(env: Env, contract_id: u32, new_client: Address) -> bool {
        Self::require_not_paused(&env);
        migration::accept_client_migration_impl(&env, contract_id, new_client)
    }

    /// Return true if a live pending client migration exists.
    ///
    /// Canonical public entrypoint; delegates to `has_pending_client_migration_impl`.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment
    /// * `contract_id` - The contract ID
    ///
    /// # Returns
    /// * `bool` - `true` if a pending migration exists, `false` otherwise
    ///
    /// # Examples
    /// ```rust,ignore
    /// let client = EscrowClient::new(&env, &contract_id);
    /// if client.has_pending_client_migration(&1) {
    ///     // Pending migration active
    /// }
    /// ```
    pub fn has_pending_client_migration(env: Env, contract_id: u32) -> bool {
        migration::has_pending_client_migration_impl(&env, contract_id)
    }

    /// Return the live pending client migration record.
    ///
    /// Canonical public entrypoint; delegates to `get_pending_client_migration_impl`.
    /// Panics with `InvalidState` when no live pending migration exists.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment
    /// * `contract_id` - The contract ID
    ///
    /// # Returns
    /// * `PendingClientMigration` - Record containing migration details
    ///
    /// # Errors
    /// * `InvalidState` - If no pending migration exists
    ///
    /// # Examples
    /// ```rust,ignore
    /// let client = EscrowClient::new(&env, &contract_id);
    /// let pending = client.get_pending_client_migration(&1);
    /// ```
    pub fn get_pending_client_migration(env: Env, contract_id: u32) -> PendingClientMigration {
        migration::get_pending_client_migration_impl(&env, contract_id)
    }

    // ── Versioned state migration ─────────────────────────────────────────

    /// Returns the current versioned state, transparently upgrading from V1 on read.
    ///
    /// Reads the storage version marker from [`DataKey::StorageVersion`].
    /// When the marker is absent or indicates v1, the legacy [`StateV1`] layout
    /// is deserialized and promoted to [`StateV2`] (with `status` defaulting
    /// to `Created`).  When the marker indicates v2, the [`StateV2`] record
    /// is returned directly.
    ///
    /// This is a **read-only** operation — it does not persist the migrated
    /// state.  Call [`Self::migrate_state`] to commit the upgrade to storage.
    pub fn get_state(env: Env) -> StateV2 {
        let version: u32 = env
            .storage()
            .persistent()
            .get(&DataKey::StorageVersion)
            .unwrap_or(1);

        match version {
            2 => env
                .storage()
                .persistent()
                .get::<_, StateV2>(&DataKey::State)
                .unwrap_or_else(|| env.panic_with_error(Error::ContractNotFound)),
            _ => {
                let v1: StateV1 = env
                    .storage()
                    .persistent()
                    .get(&DataKey::State)
                    .unwrap_or_else(|| env.panic_with_error(Error::ContractNotFound));
                StateV2 {
                    client: v1.client,
                    freelancer: v1.freelancer,
                    status: ContractStatus::Created,
                }
            }
        }
    }

    /// Migrates legacy v1 state to the current v2 layout and persists the result.
    ///
    /// Requires admin authorization. When the storage is already at the current
    /// version this is a no-op that returns `true`.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment
    /// * `admin` - The admin address (must match stored admin)
    ///
    /// # Returns
    /// `true` on success (including no-op when already v2).
    ///
    /// # Events
    /// Emits `("state_migrated", version)` with `(admin, timestamp)` payload
    /// when an actual migration occurs.
    pub fn migrate_state(env: Env, admin: Address) -> bool {
        admin.require_auth();

        let version: u32 = env
            .storage()
            .persistent()
            .get(&DataKey::StorageVersion)
            .unwrap_or(1);

        if version >= CURRENT_MILESTONE_VERSION {
            return true;
        }

        let v1: StateV1 = env
            .storage()
            .persistent()
            .get(&DataKey::State)
            .unwrap_or_else(|| env.panic_with_error(Error::ContractNotFound));

        let v2 = StateV2 {
            client: v1.client,
            freelancer: v1.freelancer,
            status: ContractStatus::Created,
        };

        env.storage().persistent().set(&DataKey::State, &v2);
        env.storage()
            .persistent()
            .set(&DataKey::StorageVersion, &CURRENT_MILESTONE_VERSION);

        env.events().publish(
            (
                Symbol::new(&env, "state_migrated"),
                CURRENT_MILESTONE_VERSION,
            ),
            (admin, env.ledger().timestamp()),
        );

        true
    }

    /// Approves a milestone for release.
    ///
    /// Records the caller's approval in temporary storage with a TTL of
    /// `PENDING_APPROVAL_TTL_LEDGERS` (~7 days). Each call resets the TTL.
    /// Duplicate approvals from the same party are rejected.
    ///
    /// Required approvers per mode:
    /// - `ClientOnly` Ã¢â‚¬â€ client only
    /// - `ArbiterOnly` Ã¢â‚¬â€ arbiter only
    /// - `ClientAndArbiter` Ã¢â‚¬â€ client or arbiter (one is enough)
    /// - `MultiSig` Ã¢â‚¬â€ both client and freelancer must approve
    ///
    /// # Arguments
    /// * `env` - The Soroban environment
    /// * `contract_id` - The contract ID
    /// * `caller` - The address granting approval
    /// * `milestone_index` - The zero-based milestone index
    ///
    /// # Returns
    /// * `bool` - `true` if approval was recorded
    ///
    /// # Errors
    /// * `ContractPaused` - If the contract is paused while not in emergency mode
    /// * `EmergencyActive` - If the contract is in an active emergency pause
    /// * `AlreadyFinalized` - If the contract has already been finalized
    /// * Approval/auth/state errors bubbled up from `approvals::approve_milestone`
    ///
    /// # Security
    /// * Pause/emergency gate runs BEFORE finalization checks, auth, TTL extension,
    ///   and approval staging so no approval state mutates while the contract is frozen.
    ///
    /// See `docs/escrow/approvals-and-release.md` for the full flow.
    ///
    /// # Examples
    /// ```rust,ignore
    /// let client = EscrowClient::new(&env, &contract_id);
    /// let approved = client.approve_milestone_release(&1, &client_address, &0);
    /// assert!(approved);
    /// ```
    pub fn approve_milestone_release(
        env: Env,
        contract_id: u32,
        caller: Address,
        milestone_index: u32,
    ) -> bool {
        if milestone_index >= MAX_MILESTONES {
            env.panic_with_error(Error::IndexOutOfBounds);
        }
        Self::require_not_paused(&env);
        Self::require_not_finalized(&env, contract_id);
        approvals::approve_milestone(&env, contract_id, milestone_index, &caller)
            .unwrap_or_else(|e| env.panic_with_error(e))
    }

    /// Batch variant of [`approve_milestone_release`](Self::approve_milestone_release)
    /// that accepts a bounded vector of milestone indices.
    ///
    /// If the vector length exceeds [`MAX_BATCH_APPROVALS`], the call is rejected
    /// with [`EscrowError::BatchCapExceeded`]. Per-item semantics are preserved:
    /// each milestone index goes through the same authorization logic as the
    /// single-entrypoint, and events are emitted per item.
    ///
    /// # Arguments
    /// * `env` - The contract environment
    /// * `contract_id` - The contract ID
    /// * `caller` - The address of the caller (must be authorized)
    /// * `milestone_indices` - Bounded vector of milestone indices to approve
    ///
    /// # Errors
    /// * `BatchCapExceeded` - If `milestone_indices` length exceeds the cap
    /// * All errors from [`approve_milestone_release`](Self::approve_milestone_release)
    ///
    /// # Events
    /// Emits `("approve", contract_id)` with payload
    /// `(caller, milestone_index, timestamp)` for each successfully approved milestone.
    pub fn approve_milestone_release_batch(
        env: Env,
        contract_id: u32,
        caller: Address,
        milestone_indices: Vec<u32>,
    ) -> bool {
        Self::require_not_paused(&env);
        Self::require_not_finalized(&env, contract_id);

        if milestone_indices.len() > MAX_BATCH_APPROVALS {
            env.panic_with_error(EscrowError::BatchCapExceeded);
        }

        for i in 0..milestone_indices.len() {
            let milestone_index = milestone_indices.get(i).unwrap();
            approvals::approve_milestone(&env, contract_id, milestone_index, &caller)
                .unwrap_or_else(|e| env.panic_with_error(e));

            env.events().publish(
                (symbol_short!("approve"), contract_id),
                (caller.clone(), milestone_index, env.ledger().timestamp()),
            );
        }

        true
    }

    /// Grants exactly one pending reputation credit to the freelancer.
    ///
    /// This is called exactly once when a contract successfully transitions to
    /// the `Completed` state, either through the final milestone release
    /// or via dispute resolution. Credits accumulate independently for each
    /// completed contract and are consumed one at a time by `issue_reputation`.
    /// A `Refunded` contract never calls this helper and therefore earns no credit.
    pub(crate) fn grant_pending_reputation_credit(env: &Env, freelancer: &Address) {
        let pending_key = DataKey::PendingReputationCredits(freelancer.clone());
        let pending: i128 = env.storage().persistent().get(&pending_key).unwrap_or(0);
        env.storage()
            .persistent()
            .set(&pending_key, &(pending + REPUTATION_CREDIT_INCREMENT));
    }

    /// Releases a specific milestone, transferring the net payout to the freelancer.
    ///
    /// Executes `SAC::transfer(from: escrow_address, to: freelancer, milestone.amount Ã¢Ë†â€™ fee)`.
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
    /// Missing or expired approvals are fail-closed Ã¢â‚¬â€ they produce
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
    /// # Examples
    /// ```rust,ignore
    /// let client = EscrowClient::new(&env, &contract_id);
    /// let released = client.release_milestone(&1, &client_address, &0);
    /// assert!(released);
    /// ```
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

        // Load contract, extend TTL, and assert not finalized via shared helper.
        let mut contract: Contract = Self::load_and_check_contract(&env, contract_id);

        // Verify contract is in Funded state before release (deposit transitions
        // Created Ã¢â€ â€™ Funded when fully funded, so release must accept Funded).
        if contract.status != ContractStatus::Funded {
            env.panic_with_error(Error::InvalidState);
        }

        // Check caller is authorized for this release authorization mode
        let is_client = caller == contract.client;
        let is_freelancer = caller == contract.freelancer;
        let is_arbiter = contract.arbiter.as_ref() == Some(&caller);

        match contract.release_authorization {
            ReleaseAuthorization::ClientOnly => {
                if !is_client {
                    env.panic_with_error(Error::UnauthorizedRole);
                }
            }
            ReleaseAuthorization::ArbiterOnly => {
                if !is_arbiter {
                    env.panic_with_error(Error::UnauthorizedRole);
                }
            }
            ReleaseAuthorization::ClientAndArbiter => {
                if !is_client && !is_arbiter {
                    env.panic_with_error(Error::UnauthorizedRole);
                }
            }
            ReleaseAuthorization::MultiSig => {
                if !is_client && !is_freelancer {
                    env.panic_with_error(Error::UnauthorizedRole);
                }
            }
        }

        let mut milestones: Vec<Milestone> = ttl::load_milestones(&env, contract_id);

        if milestone_index >= milestones.len() {
            env.panic_with_error(Error::IndexOutOfBounds);
        }

        let mut milestone = milestones.get(milestone_index).unwrap().clone();

        if milestone.released {
            env.panic_with_error(Error::MilestoneAlreadyReleased);
        }

        if milestone.refunded {
            env.panic_with_error(Error::AlreadyRefunded);
        }

        // Check for valid approvals
        approvals::check_approvals(&env, &contract, contract_id, milestone_index)
            .unwrap_or_else(|e| env.panic_with_error(e));

        let milestone_key = Symbol::new(&env, "milestones");
        let mut milestones: Vec<Milestone> = env
            .storage()
            .persistent()
            .get(&(DataKey::Contract(contract_id), milestone_key.clone()))
            .unwrap();

        // Extend TTL on milestone read
        ttl::extend_milestone_ttl(&env, contract_id);

        if milestone_index >= milestones.len() {
            env.panic_with_error(Error::IndexOutOfBounds);
        }

        let mut milestone = milestones.get(milestone_index).unwrap().clone();

        if milestone.released {
            env.panic_with_error(Error::MilestoneAlreadyReleased);
        }

        if milestone.refunded {
            env.panic_with_error(Error::AlreadyRefunded);
        }

        // Check contract-level funding (per-milestone funded_amount is set after
        // release, so we check the aggregate contract balance here).
        let available = available_balance(
            contract.funded_amount,
            contract.released_amount,
            contract.refunded_amount,
        )
        .unwrap_or_else(|| env.panic_with_error(Error::PotentialOverflow));
        if available < milestone.amount {
            env.panic_with_error(Error::InsufficientFunds);
        }

        let gross_amount = milestone.amount;

        // Compute the protocol fee up-front so the available-balance check can
        // account for both the net payout and the fee that stays in the contract.
        //
        /// `protocol_fee` Ã¢â‚¬â€ the portion of `gross_amount` retained by the
        /// protocol. Deducted from the gross milestone amount before transfer
        /// so the escrow balance is never overdrawn.
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

        /// `net_amount` Ã¢â‚¬â€ the amount actually transferred to the freelancer
        /// after deducting the protocol fee.
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
        let token = contract.token.clone();
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
        milestone.protocol_fee = protocol_fee;
        milestones.set(milestone_index, milestone.clone());
        // Indexed event for off-chain milestone-history reconstruction.
        env.events().publish(
            (symbol_short!("mlstn_idx"), contract_id, milestone_index),
            (milestone.amount, true, false, env.ledger().timestamp()),
        );
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
            let old_status = contract.status.clone();
            contract.status = ContractStatus::Completed;
            Self::grant_pending_reputation_credit(&env, &contract.freelancer);
        }

        ttl::store_milestones(&env, contract_id, &milestones);
        env.storage()
            .persistent()
            .set(&DataKey::Contract(contract_id), &contract);

        events::emit_contract_indexed_event(&env, contract_id, &contract);

        // Extend TTL on contract write (milestone TTL already extended by store_milestones)
        ttl::extend_contract_ttl(&env, contract_id);

        // Ã¢â€â‚¬Ã¢â€â‚¬ Events Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
        //
        // Emitted only after all state mutations succeed (fail-closed guarantee:
        // if execution reaches here, the release was accepted). Events contain
        // no secrets Ã¢â‚¬â€ all fields are already public contract state or
        // caller-supplied arguments.

        /// `mlstn_rls` Ã¢â‚¬â€ fired on every successful milestone release.
        ///
        /// Topics : `(symbol_short!("mlstn_rls"), contract_id: u32)`
        /// Data   : `(milestone_index: u32, amount: i128, fee: i128,
        ///            new_released_amount: i128, caller: Address, timestamp: u64)`
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

        // `ctrct_cmp` Ã¢â‚¬â€ fired only when this release completes the contract.
        //
        /// Topics : `(symbol_short!("ctrct_cmp"), contract_id: u32)`
        /// Data   : `(caller: Address, timestamp: u64)`
        if all_released {
            env.events().publish(
                (symbol_short!("ctrct_cmp"), contract_id),
                (caller, env.ledger().timestamp()),
            );
        }

        true
    }

    /// Rolls back a released or refunded milestone to its prior state.
    ///
    /// Admin-guarded operation that undoes a milestone release or refund within
    /// safe contract states (`Funded` or `PartiallyFunded`). The milestone must
    /// currently be in either the released or refunded state; a milestone in the
    /// initial state (neither released nor refunded) is rejected.
    ///
    /// # Invariants Preserved
    ///
    /// The accounting invariant
    /// `released_amount + refunded_amount + accumulated_fees ≤ funded_amount`
    /// is maintained by reversing the precise amounts that were recorded when
    /// the milestone was released or refunded. No actual token transfer is
    /// performed — the caller (admin) is responsible for recovering any tokens
    /// that may have moved off-chain.
    ///
    /// # Arguments
    /// * `env` - The contract environment
    /// * `contract_id` - The contract ID
    /// * `admin` - The admin address (must match stored admin)
    /// * `milestone_index` - The index of the milestone to rollback
    ///
    /// # Returns
    /// `true` if rollback was successful
    ///
    /// # Errors
    /// * `ContractPaused` - If the contract is paused while not in emergency mode
    /// * `EmergencyActive` - If the contract is in an active emergency pause
    /// * `ContractNotFound` - If contract doesn't exist
    /// * `AlreadyFinalized` - If a finalization record already exists
    /// * `RollbackNotAllowed` - If the contract status does not allow rollback
    ///   or the milestone is not in a rollback-able state
    /// * `IndexOutOfBounds` - If milestone_index is out of bounds
    /// * `AccountingInvariantViolated` - If accounting state is inconsistent
    ///
    /// # Events
    /// Emits `("rollback", contract_id)` with payload
    /// `(milestone_index, admin, timestamp)` on every successful rollback.
    pub fn rollback_milestone(
        env: Env,
        contract_id: u32,
        admin: Address,
        milestone_index: u32,
    ) -> bool {
        Self::require_initialized(&env);
        Self::require_not_paused(&env);

        let stored_admin: Address = env
            .storage()
            .persistent()
            .get(&DataKey::Admin)
            .unwrap_or_else(|| env.panic_with_error(EscrowError::NotInitialized));

        if admin != stored_admin {
            env.panic_with_error(EscrowError::UnauthorizedRole);
        }
        admin.require_auth();

        let mut contract: Contract = env
            .storage()
            .persistent()
            .get(&DataKey::Contract(contract_id))
            .unwrap_or_else(|| env.panic_with_error(EscrowError::ContractNotFound));

        ttl::extend_contract_ttl(&env, contract_id);
        Self::require_not_finalized(&env, contract_id);

        // Only allow rollback in active non-terminal states
        if contract.status != ContractStatus::Funded
            && contract.status != ContractStatus::PartiallyFunded
        {
            env.panic_with_error(EscrowError::RollbackNotAllowed);
        }

        let mut milestones: Vec<Milestone> = ttl::load_milestones(&env, contract_id);

        if milestone_index >= milestones.len() {
            env.panic_with_error(Error::IndexOutOfBounds);
        }

        let mut milestone = milestones.get(milestone_index).unwrap();

        // Milestone must be in a rollback-able state
        if !milestone.released && !milestone.refunded {
            env.panic_with_error(EscrowError::RollbackNotAllowed);
        }

        if milestone.released {
            let net_amount = milestone
                .amount
                .checked_sub(milestone.protocol_fee)
                .unwrap_or_else(|| env.panic_with_error(EscrowError::AccountingInvariantViolated));

            contract.released_amount = contract
                .released_amount
                .checked_sub(net_amount)
                .unwrap_or_else(|| env.panic_with_error(EscrowError::AccountingInvariantViolated));

            // Reverse the protocol fee that was accrued when the milestone was released
            if milestone.protocol_fee > 0 {
                let accumulated_fees: i128 = env
                    .storage()
                    .persistent()
                    .get(&DataKey::AccumulatedProtocolFees)
                    .unwrap_or(0);
                if accumulated_fees >= milestone.protocol_fee {
                    env.storage().persistent().set(
                        &DataKey::AccumulatedProtocolFees,
                        &(accumulated_fees - milestone.protocol_fee),
                    );
                }
            }

            milestone.released = false;
            milestone.funded_amount = 0;
            milestone.protocol_fee = 0;

            // Clear approvals for this milestone
            approvals::clear_approvals(&env, contract_id, milestone_index);
        }

        if milestone.refunded {
            contract.refunded_amount = contract
                .refunded_amount
                .checked_sub(milestone.amount)
                .unwrap_or_else(|| env.panic_with_error(EscrowError::AccountingInvariantViolated));

            milestone.refunded = false;
            milestone.refunded_amount = 0;
        }

        milestones.set(milestone_index, milestone);

        ttl::store_milestones(&env, contract_id, &milestones);
        env.storage()
            .persistent()
            .set(&DataKey::Contract(contract_id), &contract);
        ttl::extend_contract_ttl(&env, contract_id);

        env.events().publish(
            (Symbol::new(&env, "rollback"), contract_id),
            (milestone_index, admin, env.ledger().timestamp()),
        );

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
    ///
    /// # Examples
    /// ```rust,ignore
    /// let client = EscrowClient::new(&env, &contract_id);
    /// let overdue = client.is_milestone_overdue(&1, &0);
    /// ```
    pub fn is_milestone_overdue(env: Env, contract_id: u32, milestone_index: u32) -> bool {
        let contract: Contract = match env
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
    ///
    /// # Examples
    /// ```rust,ignore
    /// let client = EscrowClient::new(&env, &contract_id);
    /// let indices = soroban_sdk::vec![&env, 0u32];
    /// let refunded_total = client.refund_unreleased_milestones(&1, &indices);
    /// ```
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

        // Load contract, extend TTL, and assert not finalized via shared helper.
        let mut contract: Contract = Self::load_and_check_contract(&env, contract_id);

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
                env.panic_with_error(Error::AlreadyRefunded);
            }

            // SECURITY: Check timeout refund conditions - milestone must be overdue if deadline is set
            if let Some(deadline) = milestone.deadline {
                // Milestone has a deadline - check if it's overdue
                if !Self::is_milestone_overdue(env.clone(), contract_id, idx) {
                    // Deadline set but milestone not yet overdue
                    env.panic_with_error(Error::MilestoneNotOverdue);
                }
                // SECURITY: is_milestone_overdue already verified: now > deadline AND unreleased
            }
            // If no deadline (None), allow refund anytime (backward compatibility)

            total_refund_amount = safe_add_amounts(total_refund_amount, milestone.amount)
                .unwrap_or_else(|| env.panic_with_error(Error::PotentialOverflow));
        }

        // Check if there's enough balance
        let available_balance = available_balance(
            contract.funded_amount,
            contract.released_amount,
            contract.refunded_amount,
        )
        .unwrap_or_else(|| env.panic_with_error(Error::PotentialOverflow));
        if available_balance < total_refund_amount {
            env.panic_with_error(EscrowError::InsufficientFunds);
        }

        // Transfer tokens from contract to client
        let token = contract.token.clone();

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
            let mlstn_idx_amount = milestone.amount;
            milestones.set(idx, milestone);
            // Indexed event for off-chain milestone-history reconstruction.
            env.events().publish(
                (symbol_short!("mlstn_idx"), contract_id, idx),
                (mlstn_idx_amount, false, true, env.ledger().timestamp()),
            );
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

        events::emit_contract_indexed_event(&env, contract_id, &contract);

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

    /// Checks whether a contract with the given ID exists in storage.
    ///
    /// This is a cheap, non-panicking existence probe that returns `true` if
    /// the contract record is present and `false` otherwise. Unlike `get_contract`,
    /// this function does **not** panic with `ContractNotFound` for missing IDs,
    /// making it safe for indexers and clients iterating over ID ranges.
    ///
    /// # Security
    /// This is a read-only operation that does **not** extend the contract's TTL.
    /// Probing for contract existence cannot be abused to keep entries alive.
    /// Only actual contract operations (reads/writes) extend TTL.
    ///
    /// # Arguments
    /// * `env` - The contract environment
    /// * `contract_id` - The contract ID to check
    ///
    /// # Returns
    /// * `true` if the contract exists
    /// * `false` if the contract does not exist
    ///
    /// # Examples
    /// ```rust,ignore
    /// let client = EscrowClient::new(&env, &contract_id);
    /// if client.contract_exists(&1) {
    ///     let contract = client.get_contract(&1);
    /// }
    /// ```
    pub fn contract_exists(env: Env, contract_id: u32) -> bool {
        env.storage()
            .persistent()
            .has(&DataKey::Contract(contract_id))
    }

    /// Retrieves contract information.
    ///
    /// # Arguments
    /// * `env` - The contract environment
    /// * `contract_id` - The contract ID
    ///
    /// # Returns
    /// * `Contract` - The escrow contract struct
    ///
    /// # Errors
    /// * `ContractNotFound` - If contract does not exist
    ///
    /// # Examples
    /// ```rust,ignore
    /// let client = EscrowClient::new(&env, &contract_id);
    /// let contract = client.get_contract(&1);
    /// ```
    pub fn get_contract(env: Env, contract_id: u32) -> Contract {
        let contract = env
            .storage()
            .persistent()
            .get(&DataKey::Contract(contract_id))
            .unwrap_or_else(|| env.panic_with_error(Error::ContractNotFound));

        // Extend TTL on contract read
        ttl::extend_contract_ttl(&env, contract_id);
        contract
    }

    /// Returns the next contract ID to be allocated (the high-water mark).
    ///
    /// This reader returns the current value of `NextContractId`, which represents
    /// the next ID that will be assigned when `create_contract` is called.
    /// Indexers can use this to determine the allocation high-water mark and
    /// safely iterate over the allocated ID range `[1, get_next_contract_id() - 1]`.
    ///
    /// # Security
    /// This is a read-only operation that does not mutate contract state or extend TTL.
    ///
    /// # Arguments
    /// * `env` - The contract environment
    ///
    /// # Returns
    /// The next contract ID to be allocated (always ≥ 1)
    ///
    /// # Examples
    /// ```rust,ignore
    /// let client = EscrowClient::new(&env, &contract_id);
    /// let next_id = client.get_next_contract_id();
    /// for id in 1..next_id {
    ///     if client.contract_exists(&id) {
    ///         let contract = client.get_contract(&id);
    ///     }
    /// }
    /// ```
    pub fn get_next_contract_id(env: Env) -> u32 {
        env.storage()
            .persistent()
            .get(&DataKey::NextContractId)
            .unwrap_or(INITIAL_CONTRACT_ID)
    }

    /// Returns a structured summary of the contract and its milestones.
    ///
    /// Extends contract and milestone TTL on read without requiring caller auth.
    ///
    /// # Arguments
    /// * `env` - The contract environment
    /// * `contract_id` - The contract ID
    ///
    /// # Returns
    /// The detailed `ContractSummary` for off-chain consumption
    ///
    /// # Errors
    /// * `ContractNotFound` - If contract doesn't exist
    ///
    /// # Examples
    /// ```rust,ignore
    /// let client = EscrowClient::new(&env, &contract_id);
    /// let summary = client.get_contract_summary(&1);
    /// assert_eq!(summary.schema_version, 1);
    /// ```
    pub fn get_contract_summary(env: Env, contract_id: u32) -> ContractSummary {
        let contract: Contract = env
            .storage()
            .persistent()
            .get(&DataKey::Contract(contract_id))
            .unwrap_or_else(|| env.panic_with_error(EscrowError::ContractNotFound));

        // Extend TTL on contract and milestones read
        ttl::extend_contract_and_milestones_ttl(&env, contract_id);

        let milestones = ttl::load_milestones(&env, contract_id);
        let total_amount: i128 =
            crate::amount_validation::accumulate_amounts(milestones.iter().map(|m| m.amount))
                .unwrap_or_else(|_| env.panic_with_error(EscrowError::PotentialOverflow));
        let released_milestone_count = milestones.iter().filter(|m| m.released).count() as u32;

        let mut milestone_summaries = Vec::new(&env);
        for (idx, m) in milestones.iter().enumerate() {
            milestone_summaries.push_back(MilestoneSummary {
                index: idx as u32,
                amount: m.amount,
                released: m.released,
                refunded: m.refunded,
            });
        }

        let reputation_issued = env
            .storage()
            .persistent()
            .get::<_, bool>(&DataKey::ReputationIssued(contract_id))
            .unwrap_or(contract.reputation_issued);

        let refundable_balance =
            contract.funded_amount - contract.released_amount - contract.refunded_amount;

        ContractSummary {
            schema_version: CONTRACT_SUMMARY_SCHEMA_VERSION,
            client: contract.client,
            freelancer: contract.freelancer,
            arbiter: contract.arbiter,
            status: contract.status,
            reputation_issued,
            total_amount,
            funded_amount: contract.funded_amount,
            released_amount: contract.released_amount,
            refundable_balance,
            released_milestone_count,
            milestones: milestone_summaries,
        }
    }

    /// Retrieves all milestones for a contract.
    ///
    /// # Arguments
    /// * `env` - The contract environment
    /// * `contract_id` - The contract ID
    ///
    /// # Returns
    /// * `Vec<Milestone>` - Vector of milestone items
    ///
    /// # Errors
    /// * `ContractNotFound` - If contract milestones do not exist
    ///
    /// # Examples
    /// ```rust,ignore
    /// let client = EscrowClient::new(&env, &contract_id);
    /// let milestones = client.get_milestones(&1);
    /// ```
    pub fn get_milestones(env: Env, contract_id: u32) -> Vec<Milestone> {
        let milestone_key = Symbol::new(&env, "milestones");
        let milestones = env
            .storage()
            .persistent()
            .get(&(DataKey::Contract(contract_id), milestone_key))
            .unwrap_or_else(|| env.panic_with_error(EscrowError::ContractNotFound));
        ttl::extend_milestone_ttl(&env, contract_id);
        milestones
    }

    /// Retrieves a single milestone by index for a contract.
    ///
    /// This is the bounds-checked single-item counterpart to
    /// `get_milestones`. Off-chain callers that only need one milestone's
    /// state (amount, funded/released/refunded flags, deadline, work evidence)
    /// can avoid fetching and decoding the full `Vec<Milestone>`.
    ///
    /// # Arguments
    /// * `env` - The contract environment
    /// * `contract_id` - The contract ID
    /// * `milestone_index` - The zero-based index of the milestone to read
    ///
    /// # Returns
    /// * `Some(Milestone)` if `milestone_index` is in bounds
    /// * `None` if `milestone_index` is out of bounds
    ///
    /// # Panics
    /// Panics with `ContractNotFound` if the contract's milestones were never
    /// allocated (i.e. the contract id is unknown), matching
    /// `get_milestones`.
    ///
    /// # Side effects
    /// Extends the milestones vector TTL on a successful read, consistent with
    /// `get_milestones`. Auth-free and otherwise non-mutating.
    ///
    /// # Examples
    /// ```rust,ignore
    /// let client = EscrowClient::new(&env, &contract_id);
    /// if let Some(milestone) = client.get_milestone(&1, &0) {
    ///     // Process milestone 0
    /// }
    /// ```
    pub fn get_milestone(env: Env, contract_id: u32, milestone_index: u32) -> Option<Milestone> {
        let milestone_key = Symbol::new(&env, "milestones");
        let milestones: Vec<Milestone> = env
            .storage()
            .persistent()
            .get(&(DataKey::Contract(contract_id), milestone_key))
            .unwrap_or_else(|| env.panic_with_error(EscrowError::ContractNotFound));
        ttl::extend_milestone_ttl(&env, contract_id);
        milestones.get(milestone_index)
    }

    /// Returns the schedule metadata for a single milestone, or `None` when no
    /// schedule has been stored for that index or when the contract ID is unknown.
    ///
    /// Does NOT panic for unknown contract IDs — returns `None` consistently.
    pub fn get_milestone_schedule(
        env: Env,
        contract_id: u32,
        milestone_index: u32,
    ) -> Option<MilestoneSchedule> {
        let schedule_key = Symbol::new(&env, "schedule");
        let schedules: Option<Vec<Option<MilestoneSchedule>>> = env
            .storage()
            .persistent()
            .get(&(DataKey::Contract(contract_id), schedule_key));
        match schedules {
            None => None,
            Some(s) => {
                if milestone_index >= s.len() {
                    None
                } else {
                    s.get(milestone_index)
                }
            }
        }
    }

    /// Updates the schedule metadata for a single milestone.
    ///
    /// The caller must be the stored client and must authorize the call.
    /// The target milestone must not yet be released or refunded.
    ///
    /// # Errors
    /// * `ContractNotFound` — unknown `contract_id`.
    /// * `UnauthorizedRole` — caller is not the stored client.
    /// * `IndexOutOfBounds` — `milestone_index` exceeds the milestone count.
    /// * `MilestoneAlreadyReleased` — milestone is already released.
    /// * `AlreadyRefunded` — milestone has been refunded.
    /// * `InvalidScheduleMetadata` — the schedule data fails validation.
    pub fn set_milestone_schedule(
        env: Env,
        contract_id: u32,
        caller: Address,
        milestone_index: u32,
        schedule: MilestoneSchedule,
    ) -> bool {
        Self::require_not_paused(&env);
        caller.require_auth();

        let contract: Contract = env
            .storage()
            .persistent()
            .get(&DataKey::Contract(contract_id))
            .unwrap_or_else(|| env.panic_with_error(EscrowError::ContractNotFound));
        ttl::extend_contract_ttl(&env, contract_id);

        if caller != contract.client {
            env.panic_with_error(Error::UnauthorizedRole);
        }

        let milestone_key = Symbol::new(&env, "milestones");
        let milestones: Vec<Milestone> = env
            .storage()
            .persistent()
            .get(&(DataKey::Contract(contract_id), milestone_key))
            .unwrap_or_else(|| env.panic_with_error(EscrowError::ContractNotFound));

        if milestone_index >= milestones.len() {
            env.panic_with_error(Error::IndexOutOfBounds);
        }

        let ms = milestones.get(milestone_index).unwrap();
        if ms.released {
            env.panic_with_error(Error::MilestoneAlreadyReleased);
        }
        if ms.refunded {
            env.panic_with_error(Error::AlreadyRefunded);
        }

        // Validate schedule data.
        let now = env.ledger().timestamp();
        if let Some(due) = schedule.due_date {
            if due <= now {
                env.panic_with_error(Error::InvalidScheduleMetadata);
            }
            // Check monotonicity with previous milestone (if any).
            if milestone_index > 0 {
                let prev_idx = milestone_index - 1;
                let schedule_key = Symbol::new(&env, "schedule");
                let schedules: Option<Vec<Option<MilestoneSchedule>>> = env
                    .storage()
                    .persistent()
                    .get(&(DataKey::Contract(contract_id), schedule_key.clone()));
                if let Some(ref scheds) = schedules {
                    if let Some(Some(ref prev)) = scheds.get(prev_idx) {
                        if let Some(prev_due) = prev.due_date {
                            if due <= prev_due {
                                env.panic_with_error(Error::InvalidScheduleMetadata);
                            }
                        }
                    }
                }
            }
            // Check monotonicity with next milestone (if any).
            if (milestone_index as u32) < milestones.len() - 1 {
                let next_idx = milestone_index + 1;
                let schedule_key = Symbol::new(&env, "schedule");
                let schedules: Option<Vec<Option<MilestoneSchedule>>> = env
                    .storage()
                    .persistent()
                    .get(&(DataKey::Contract(contract_id), schedule_key));
                if let Some(ref scheds) = schedules {
                    if let Some(Some(ref next)) = scheds.get(next_idx) {
                        if let Some(next_due) = next.due_date {
                            if next_due <= due {
                                env.panic_with_error(Error::InvalidScheduleMetadata);
                            }
                        }
                    }
                }
            }
        }
        if let Some(ref title) = schedule.title {
            if title.len() > MAX_SCHEDULE_TITLE_LEN as u32 {
                env.panic_with_error(Error::InvalidScheduleMetadata);
            }
        }
        if let Some(ref desc) = schedule.description {
            if desc.len() > MAX_SCHEDULE_DESCRIPTION_LEN as u32 {
                env.panic_with_error(Error::InvalidScheduleMetadata);
            }
        }

        // Store the schedule.
        let mut entry = schedule;
        entry.updated_at = now;
        let schedule_key = Symbol::new(&env, "schedule");
        let mut stored_schedules: Vec<Option<MilestoneSchedule>> = env
            .storage()
            .persistent()
            .get(&(DataKey::Contract(contract_id), schedule_key.clone()))
            .unwrap_or_else(|| {
                let mut v: Vec<Option<MilestoneSchedule>> = Vec::new(&env);
                for _ in 0..milestones.len() {
                    v.push_back(None);
                }
                v
            });
        stored_schedules.set(milestone_index, Some(entry));
        env.storage()
            .persistent()
            .set(&(DataKey::Contract(contract_id), schedule_key), &stored_schedules);

        true
    }

    /// Returns funded minus released minus refunded for `contract_id`.
    ///
    /// # Arguments
    /// * `env` - The contract environment
    /// * `contract_id` - The contract ID
    ///
    /// # Returns
    /// * `i128` - Remaining refundable balance in stroops
    ///
    /// # Errors
    /// * `ContractNotFound` - If contract does not exist
    ///
    /// # Examples
    /// ```rust,ignore
    /// let client = EscrowClient::new(&env, &contract_id);
    /// let balance = client.get_refundable_balance(&1);
    /// ```
    pub fn get_refundable_balance(env: Env, contract_id: u32) -> i128 {
        let contract: Contract = env
            .storage()
            .persistent()
            .get(&DataKey::Contract(contract_id))
            .unwrap_or_else(|| env.panic_with_error(EscrowError::ContractNotFound));
        ttl::extend_contract_ttl(&env, contract_id);
        contract.funded_amount - contract.released_amount - contract.refunded_amount
    }

    /// Retrieves approval status for a milestone.
    ///
    /// Returns `None` when no approval record exists or when the TTL has
    /// elapsed. Treat `None` and an all-`false` struct identically Ã¢â‚¬â€ neither
    /// unblocks `release_milestone`.
    ///
    /// On a successful read, this entrypoint renews the temporary approval
    /// record's TTL using `PENDING_APPROVAL_BUMP_THRESHOLD` /
    /// `PENDING_APPROVAL_TTL_LEDGERS`, consistent with the approval write path.
    /// Missing or expired entries still return `None` without writing.
    ///
    /// # Cost Semantics
    /// This is a storage-touching read of temporary state, not a zero-cost pure
    /// getter. Integrators that poll approval state should account for the host
    /// storage access and TTL bump behavior.
    ///
    /// See `approve_milestone_release` and `docs/escrow/authorization.md`.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment
    /// * `contract_id` - The contract ID
    /// * `milestone_index` - The zero-based milestone index
    ///
    /// # Returns
    /// * `Option<MilestoneApprovals>` - `Some(MilestoneApprovals)` if present, `None` if non-existent or expired
    ///
    /// # Examples
    /// ```rust,ignore
    /// let client = EscrowClient::new(&env, &contract_id);
    /// if let Some(approvals) = client.get_milestone_approvals(&1, &0) {
    ///     assert!(approvals.client_approved);
    /// }
    /// ```
    pub fn get_milestone_approvals(
        env: Env,
        contract_id: u32,
        milestone_index: u32,
    ) -> Option<MilestoneApprovals> {
        Self::get_milestone_approvals_impl(&env, contract_id, milestone_index)
    }

    /// Retrieves approval status for a milestone.
    ///
    /// Returns ledgers remaining, computed against ttl::compute_expiry.
    /// `None` when no live approval exists,
    /// distinguishing "never approved" from "approved and evicted".
    ///
    /// # Arguments
    /// * `env` - The Soroban environment
    /// * `contract_id` - The contract ID
    /// * `milestone_index` - The zero-based milestone index
    ///
    /// # Returns
    /// * `Option<u32>` - `Some(ledger_expiry)` if approval exists, `None` otherwise
    ///
    /// # Examples
    /// ```rust,ignore
    /// let client = EscrowClient::new(&env, &contract_id);
    /// if let Some(deadline) = client.get_approval_deadline(&1, &0) {
    ///     // Process deadline ledger
    /// }
    /// ```
    pub fn get_approval_deadline(env: Env, contract_id: u32, milestone_index: u32) -> Option<u32> {
        Self::get_approval_deadline_impl(&env, contract_id, milestone_index)
    }

    // Ã¢â€â‚¬Ã¢â€â‚¬ Pause / unpause Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

    /// Pause all state-changing escrow operations.
    ///
    /// Requires the stored admin's authorization. While paused, all mutating
    /// entrypoints panic with `ContractPaused`. Read-only queries are never blocked.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment
    ///
    /// # Returns
    /// * `bool` - `true` if paused successfully
    ///
    /// # Errors
    /// * `NotInitialized` - If contract is uninitialized
    /// * `UnauthorizedRole` - If caller is not admin
    ///
    /// # Examples
    /// ```rust,ignore
    /// let client = EscrowClient::new(&env, &contract_id);
    /// let paused = client.pause();
    /// assert!(paused);
    /// ```
    ///
    /// # Events
    /// Emits `("paused", timestamp)` with `(admin,)` payload.
    pub fn pause(env: Env) -> bool {
        Self::require_initialized(&env);
        let admin: Address = env.storage().persistent().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        env.storage().persistent().set(&DataKey::Paused, &true);

        env.events()
            .publish((symbol_short!("pause"), env.ledger().timestamp()), (admin,));
        true
    }

    /// Unpause operations, clearing the `Paused` flag.
    ///
    /// Blocked while `Emergency` is active Ã¢â‚¬â€ use `resolve_emergency` instead.
    /// Requires the stored admin's authorization.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment
    ///
    /// # Returns
    /// * `bool` - `true` if unpaused successfully
    ///
    /// # Errors
    /// * `NotInitialized` - If contract is uninitialized
    /// * `EmergencyActive` - If emergency controls are currently active
    /// * `UnauthorizedRole` - If caller is not admin
    ///
    /// # Examples
    /// ```rust,ignore
    /// let client = EscrowClient::new(&env, &contract_id);
    /// let unpaused = client.unpause();
    /// assert!(unpaused);
    /// ```
    ///
    /// # Events
    /// Emits `("unpaused", timestamp)` with `(admin,)` payload.
    pub fn unpause(env: Env) -> bool {
        Self::require_initialized(&env);
        if env
            .storage()
            .persistent()
            .get::<_, bool>(&DataKey::Emergency)
            .unwrap_or(false)
        {
            env.panic_with_error(EscrowError::EmergencyActive);
        }
        let admin: Address = env.storage().persistent().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        env.storage().persistent().set(&DataKey::Paused, &false);

        env.events().publish(
            (symbol_short!("unpaused"), env.ledger().timestamp()),
            (admin,),
        );
        true
    }

    /// Returns `true` if the contract is currently paused.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment
    ///
    /// # Returns
    /// * `bool` - `true` if paused, `false` otherwise
    ///
    /// # Examples
    /// ```rust,ignore
    /// let client = EscrowClient::new(&env, &contract_id);
    /// if client.is_paused() {
    ///     // Contract is currently paused
    /// }
    /// ```
    pub fn is_paused(env: Env) -> bool {
        env.storage()
            .persistent()
            .get(&DataKey::Paused)
            .unwrap_or(false)
    }

    // Ã¢â€â‚¬Ã¢â€â‚¬ Emergency pause Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

    /// Activate emergency pause, setting both `Emergency` and `Paused` flags.
    ///
    /// Requires the stored admin's authorization. While emergency is active,
    /// all mutating entrypoints panic with `EmergencyActive` or `ContractPaused`,
    /// and `unpause` is blocked.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment
    ///
    /// # Returns
    /// * `bool` - `true` if emergency pause activated
    ///
    /// # Errors
    /// * `NotInitialized` - If contract is uninitialized
    /// * `UnauthorizedRole` - If caller is not admin
    ///
    /// # Examples
    /// ```rust,ignore
    /// let client = EscrowClient::new(&env, &contract_id);
    /// let activated = client.activate_emergency_pause();
    /// assert!(activated);
    /// ```
    ///
    /// # Events
    /// Emits `("emergency", "activated")` with `(admin, timestamp)` payload.
    /// Sets `emergency_controls_enabled` in the readiness checklist.
    pub fn activate_emergency_pause(env: Env) -> bool {
        let admin: Address = env
            .storage()
            .persistent()
            .get(&DataKey::Admin)
            .unwrap_or_else(|| env.panic_with_error(EscrowError::NotInitialized));

        if env
            .storage()
            .persistent()
            .get::<_, bool>(&DataKey::Initialized)
            .unwrap_or(false)
        {
            admin.require_auth();
        }
        env.storage().persistent().set(&DataKey::Emergency, &true);
        env.storage().persistent().set(&DataKey::Paused, &true);

        let mut checklist: ReadinessChecklist = env
            .storage()
            .persistent()
            .get(&DataKey::ReadinessChecklist)
            .unwrap_or_default();
        checklist.emergency_controls_enabled = true;
        env.storage()
            .persistent()
            .set(&DataKey::ReadinessChecklist, &checklist);

        env.events().publish(
            (
                Symbol::new(&env, "emergency"),
                Symbol::new(&env, "activated"),
            ),
            (
                env.storage()
                    .persistent()
                    .get::<_, Address>(&DataKey::Admin)
                    .unwrap(),
                env.ledger().timestamp(),
            ),
        );
        true
    }

    /// Resolve emergency, clearing both `Emergency` and `Paused` flags.
    ///
    /// Requires the stored admin's authorization. After resolution, all
    /// operations resume normally.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment
    ///
    /// # Returns
    /// * `bool` - `true` if emergency resolved
    ///
    /// # Errors
    /// * `NotInitialized` - If contract is uninitialized
    /// * `UnauthorizedRole` - If caller is not admin
    ///
    /// # Examples
    /// ```rust,ignore
    /// let client = EscrowClient::new(&env, &contract_id);
    /// let resolved = client.resolve_emergency();
    /// assert!(resolved);
    /// ```
    ///
    /// # Events
    /// Emits `("emergency", "resolved")` with `(admin, timestamp)` payload.
    /// Sets `emergency_controls_enabled` in the readiness checklist.
    pub fn resolve_emergency(env: Env) -> bool {
        Self::require_initialized(&env);
        let admin: Address = env
            .storage()
            .persistent()
            .get(&DataKey::Admin)
            .unwrap_or_else(|| env.panic_with_error(EscrowError::NotInitialized));
        admin.require_auth();
        env.storage().persistent().set(&DataKey::Emergency, &false);
        env.storage().persistent().set(&DataKey::Paused, &false);

        let mut checklist: ReadinessChecklist = env
            .storage()
            .persistent()
            .get(&DataKey::ReadinessChecklist)
            .unwrap_or_default();
        checklist.emergency_controls_enabled = true;
        env.storage()
            .persistent()
            .set(&DataKey::ReadinessChecklist, &checklist);
        env.events().publish(
            (
                Symbol::new(&env, "emergency"),
                Symbol::new(&env, "resolved"),
            ),
            (admin, env.ledger().timestamp()),
        );
        true
    }

    /// Returns `true` if emergency mode is active.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment
    ///
    /// # Returns
    /// * `bool` - `true` if emergency mode is active, `false` otherwise
    ///
    /// # Examples
    /// ```rust,ignore
    /// let client = EscrowClient::new(&env, &contract_id);
    /// if client.is_emergency() {
    ///     // Emergency mode active
    /// }
    /// ```
    pub fn is_emergency(env: Env) -> bool {
        env.storage()
            .persistent()
            .get(&DataKey::Emergency)
            .unwrap_or(false)
    }

    // ── Cancel contract ──────────────────────────────────────────────────────

    /// Cancels a contract before any milestone has been released.
    ///
    /// The caller must be the stored client and must authorize the call. The
    /// contract must be in `Created` or `Funded` state, with no released
    /// balance, and the full remaining refundable balance is sent back to the
    /// client via the configured Stellar Asset Contract before the contract is
    /// marked `Cancelled`. A zero-funded cancellation does not invoke a token
    /// transfer and leaves unrelated contracts' escrowed token balances intact.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment
    /// * `contract_id` - The contract ID to cancel
    /// * `client` - Address of client canceling contract
    ///
    /// # Returns
    /// * `bool` - `true` if canceled successfully
    ///
    /// # Errors
    /// * `ContractPaused` - If the contract is paused while not in emergency mode.
    /// * `EmergencyActive` - If the contract is in an active emergency pause.
    /// * `ContractNotFound` - If the contract does not exist.
    /// * `UnauthorizedRole` - If the caller is not the stored client.
    /// * `AlreadyCancelled` - If the contract was already cancelled.
    /// * `InvalidStatusTransition` - If the contract is not `Created`/`Funded` or has already released funds.
    ///
    /// # Examples
    /// ```rust,ignore
    /// let client = EscrowClient::new(&env, &contract_id);
    /// let cancelled = client.cancel_contract(&1, &client_address);
    /// assert!(cancelled);
    /// ```
    pub fn cancel_contract(env: Env, contract_id: u32, client: Address) -> bool {
        Self::require_not_paused(&env);
        // Load contract, extend TTL, and assert not finalized via shared helper.
        let mut contract: Contract = Self::load_and_check_contract(&env, contract_id);

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

        let refund_amount = available_balance(
            contract.funded_amount,
            contract.released_amount,
            contract.refunded_amount,
        )
        .unwrap_or_else(|| env.panic_with_error(Error::PotentialOverflow));
        if refund_amount > 0 {
            let token = contract.token.clone();
            token::Client::new(&env, &token).transfer(
                &env.current_contract_address(),
                &client,
                &refund_amount,
            );
        }
            .persistent()
            .set(&DataKey::Contract(contract_id), &contract);

        events::emit_contract_indexed_event(&env, contract_id, &contract);
        ttl::extend_contract_ttl(&env, contract_id);

        env.events().publish(
            (symbol_short!("cancelled"), contract_id),
            (client, refund_amount, env.ledger().timestamp()),
        );

        true
    }

    // Ã¢â€â‚¬Ã¢â€â‚¬ Dispute management Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

    // ── Dispute management ────────────────────────────────────────────────────

    /// Opens a dispute on a funded or partially funded escrow.
    ///
    /// Persists versioned dispute metadata under [`DataKey::Dispute`] and stamps
    /// [`DataKey::DisputeStorageVersion`] with [`DISPUTE_STORAGE_VERSION`].
    pub fn raise_dispute(env: Env, contract_id: u32, caller: Address) -> bool {
        dispute::raise_dispute_impl(&env, contract_id, caller)
    }

    /// Resolves an open dispute with the arbiter-selected resolution.
    ///
    /// Ensures dispute metadata is present via migrate-on-read, then clears it.
    pub fn resolve_dispute(
        env: Env,
        contract_id: u32,
        arbiter: Address,
        resolution: DisputeResolution,
    ) -> bool {
        dispute::resolve_dispute_impl(&env, contract_id, arbiter, resolution)
    }

    /// Returns versioned dispute metadata, upgrading older layouts on read.
    pub fn get_dispute(env: Env, contract_id: u32) -> DisputeMetadata {
        dispute::get_dispute_impl(&env, contract_id)
    }

    /// Returns the on-ledger dispute storage layout version for `contract_id`.
    pub fn get_dispute_storage_version(env: Env, contract_id: u32) -> u32 {
        dispute::get_dispute_storage_version(&env, contract_id)
    }

    // ── Reputation ───────────────────────────────────────────────────────────

    /// Issues reputation credit for a completed contract.
    ///
    /// Once all milestones on a contract have been released (or a mix of
    /// released and refunded), the contract transitions to
    /// [`ContractStatus::Completed`] and the freelancer earns one
    /// *pending reputation credit*. The client must consume that credit by
    /// calling this function, which records a `rating` (1–5) and a text
    /// `comment` on-chain and updates the freelancer's cumulative
    /// [`types::Reputation`] record.
    ///
    /// # Arguments
    ///
    /// * `env` – The Soroban execution environment (injected by the runtime).
    /// * `contract_id` – The numeric ID of the completed escrow contract.
    /// * `caller` – Address of the client; must match `contract.client`.
    ///   `require_auth` is called on this address.
    /// * `rating` – Integer score in the closed range \[1, 5\] (inclusive).
    /// * `comment` – Freeform UTF-8 feedback; must be 1–200 **bytes**.
    ///   Because [`soroban_sdk::String::len`] counts UTF-8 bytes, a 3-byte
    ///   emoji occupies 3 bytes toward the 200-byte cap.
    ///
    /// # Returns
    ///
    /// `true` on success. The function panics on all error paths — it never
    /// returns `false`.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment
    /// * `contract_id` - The contract ID
    /// * `caller` - Address of client issuing reputation
    /// * `rating` - Rating integer between 1 and 5 (inclusive)
    /// * `comment` - Feedback string (1-200 bytes)
    ///
    /// # Returns
    /// * `bool` - `true` if reputation issued successfully
    ///
    /// # Errors
    ///
    /// The function panics with the following [`crate::EscrowError`] codes:
    ///
    /// | Error | Condition |
    /// |---|---|
    /// | `ContractPaused` | Contract is paused (non-emergency mode) |
    /// | `EmergencyActive` | Emergency pause is active |
    /// | `ContractNotFound` | `contract_id` does not map to an existing contract |
    /// | `UnauthorizedRole` | `caller` is not the stored client address |
    /// | `InvalidRating` | `rating < 1` or `rating > 5` |
    /// | `EmptyComment` | `comment` has zero bytes |
    /// | `CommentTooLong` | `comment` exceeds 200 bytes |
    /// | `NotCompleted` | Contract status is not `Completed` |
    /// | `ReputationAlreadyIssued` | Reputation was already issued for this contract |
    /// | `SelfRating` | `contract.client == contract.freelancer` |
    /// | `InvalidState` | No pending reputation credit exists for the freelancer |
    ///
    /// # Security
    ///
    /// * The pause/emergency gate runs **before** any contract state is read,
    ///   so a paused contract cannot have its reputation record mutated.
    /// * The 200-byte cap prevents unbounded on-chain storage growth.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use soroban_sdk::{testutils::Address as _, vec, Address, Env, String};
    /// # use escrow::{Escrow, EscrowClient, ReleaseAuthorization};
    /// let env = Env::default();
    /// env.mock_all_auths();
    ///
    /// // Deploy and initialise the contract.
    /// let escrow_id = env.register(Escrow, ());
    /// let escrow = EscrowClient::new(&env, &escrow_id);
    /// let admin = Address::generate(&env);
    /// escrow.initialize(&admin);
    ///
    /// // Create participants and a 3-milestone escrow.
    /// let client_addr = Address::generate(&env);
    /// let freelancer_addr = Address::generate(&env);
    /// let milestones = vec![&env, 200_0000000_i128, 400_0000000_i128, 600_0000000_i128];
    /// let contract_id = escrow.create_contract(
    ///     &client_addr,
    ///     &freelancer_addr,
    ///     &None,
    ///     &milestones,
    ///     &ReleaseAuthorization::ClientOnly,
    /// );
    ///
    /// // Deposit and release all milestones to reach Completed status.
    /// escrow.deposit_funds(&contract_id, &client_addr, &1_200_0000000_i128);
    /// for idx in 0_u32..3 {
    ///     escrow.approve_milestone_release(&contract_id, &client_addr, &idx);
    ///     escrow.release_milestone(&contract_id, &client_addr, &idx);
    /// }
    ///
    /// // Issue a 5-star rating with a short comment.
    /// let comment = String::from_str(&env, "Delivered on time, great communication!");
    /// let ok = escrow.issue_reputation(&contract_id, &client_addr, &5, &comment);
    /// assert!(ok);
    ///
    /// // The freelancer's reputation record is now populated.
    /// let rep = escrow.get_reputation(&freelancer_addr).unwrap();
    /// assert_eq!(rep.completed_contracts, 1);
    /// assert_eq!(rep.total_rating, 5);
    /// assert_eq!(rep.last_rating, 5);
    /// ```
    pub fn issue_reputation(
        env: Env,
        contract_id: u32,
        caller: Address,
        rating: u32,
        comment: String,
    ) -> bool {
        Self::require_not_paused(&env);
        Self::validate_contract_id_bounds(&env, contract_id);
        let mut contract: Contract = env
            .storage()
            .persistent()
            .get(&DataKey::Contract(contract_id))
            .unwrap_or_else(|| env.panic_with_error(EscrowError::ContractNotFound));
        ttl::extend_contract_ttl(&env, contract_id);

        if caller != contract.client {
            env.panic_with_error(EscrowError::UnauthorizedRole);
        }

        if rating < MIN_RATING || rating > MAX_RATING {
            env.panic_with_error(Error::InvalidRating);
        }

        if comment.len() == 0 {
            env.panic_with_error(EscrowError::EmptyComment);
        }

        if comment.len() > MAX_COMMENT_BYTES {
            env.panic_with_error(Error::CommentTooLong);
        }

        if contract.status != ContractStatus::Completed {
            env.panic_with_error(EscrowError::NotCompleted);
        }

        if contract.reputation_issued {
            env.panic_with_error(EscrowError::ReputationAlreadyIssued);
        }
        if contract.client == contract.freelancer {
            env.panic_with_error(EscrowError::SelfRating);
        }

        caller.require_auth();
        contract.reputation_issued = true;
        env.storage()
            .persistent()
            .set(&DataKey::Contract(contract_id), &contract);
        env.storage()
            .persistent()
            .set(&DataKey::ReputationIssued(contract_id), &true);
        env.storage().persistent().extend_ttl(
            &DataKey::ReputationIssued(contract_id),
            ttl::PERSISTENT_BUMP_THRESHOLD,
            ttl::PERSISTENT_TTL_LEDGERS,
        );

        let pending_key = DataKey::PendingReputationCredits(ReputationKey { user: contract.freelancer.clone() });
        let pending: i128 = env.storage().persistent().get(&pending_key).unwrap_or(0);
        if pending <= 0 {
            env.panic_with_error(EscrowError::InvalidState);
        }
        env.storage()
            .persistent()
            .set(&pending_key, &(pending - REPUTATION_CREDIT_INCREMENT));

        let rep_key = DataKey::Reputation(ReputationKey { user: contract.freelancer.clone() });
        let mut rep: types::Reputation =
            env.storage().persistent().get(&rep_key).unwrap_or_default();
        rep.completed_contracts += REPUTATION_CREDIT_INCREMENT;
        rep.total_rating += rating as i128;
        rep.last_rating = rating as i128;
        env.storage().persistent().set(&rep_key, &rep);

        let comment_key = DataKey::ReputationComment(contract_id);
        env.storage().persistent().set(&comment_key, &comment);
        env.storage().persistent().extend_ttl(
            &comment_key,
            ttl::PERSISTENT_BUMP_THRESHOLD,
            ttl::PERSISTENT_TTL_LEDGERS,
        );

        env.events().publish(
            (symbol_short!("repr_put"), contract_id),
            (
                contract.freelancer.clone(),
                rating,
                env.ledger().timestamp(),
            ),
        );

        true
    }

    /// Simulates `issue_reputation` and returns the projected reputation outcome
    /// without writing storage or emitting events.
    ///
    /// Runs the exact same validation as `issue_reputation` (pause/emergency
    /// gate, caller/role checks, rating/comment bounds, contract status,
    /// duplicate-issuance and self-rating checks) so a caller can preview
    /// whether a call would succeed and what the resulting reputation record
    /// would look like. Does not require `caller` authorization, since no
    /// state is mutated.
    ///
    /// # Errors
    /// Same as `issue_reputation`.
    pub fn simulate_issue_reputation(
        env: Env,
        contract_id: u32,
        caller: Address,
        rating: u32,
        comment: String,
    ) -> types::Reputation {
        Self::require_not_paused(&env);
        let contract: Contract = env
            .storage()
            .persistent()
            .get(&DataKey::Contract(contract_id))
            .unwrap_or_else(|| env.panic_with_error(Error::ContractNotFound));

        if caller != contract.client {
            env.panic_with_error(Error::UnauthorizedRole);
        }

        if rating < MIN_RATING || rating > MAX_RATING {
            env.panic_with_error(Error::InvalidRating);
        }

        if comment.len() == 0 {
            env.panic_with_error(Error::EmptyComment);
        }

        if comment.len() > MAX_COMMENT_BYTES {
            env.panic_with_error(Error::CommentTooLong);
        }

        if contract.status != ContractStatus::Completed {
            env.panic_with_error(Error::NotCompleted);
        }

        if contract.reputation_issued {
            env.panic_with_error(Error::ReputationAlreadyIssued);
        }
        if contract.client == contract.freelancer {
            env.panic_with_error(Error::SelfRating);
        }

        let pending_key = DataKey::PendingReputationCredits(contract.freelancer.clone());
        let pending: i128 = env.storage().persistent().get(&pending_key).unwrap_or(0);
        if pending <= 0 {
            env.panic_with_error(Error::InvalidState);
        }
        env.storage()
            .persistent()
            .set(&pending_key, &(pending - REPUTATION_CREDIT_INCREMENT));

        let rep_key = DataKey::Reputation(contract.freelancer.clone());
        let mut rep: types::Reputation =
            env.storage().persistent().get(&rep_key).unwrap_or_default();
        rep.completed_contracts += REPUTATION_CREDIT_INCREMENT;
        rep.total_rating += rating as i128;
        rep.last_rating = rating as i128;
        rep
    }

    /// Returns the written feedback provided by the client when reputation was issued.
    /// Returns `None` if reputation has not been issued for this contract.
    pub fn get_reputation_comment(env: Env, contract_id: u32) -> Option<String> {
        Self::validate_contract_id_bounds(&env, contract_id);
        let comment_key = DataKey::ReputationComment(contract_id);
        let comment: Option<String> = env.storage().persistent().get(&comment_key);
        if comment.is_some() {
            env.storage().persistent().extend_ttl(
                &comment_key,
                ttl::PERSISTENT_BUMP_THRESHOLD,
                ttl::PERSISTENT_TTL_LEDGERS,
            );
        }
        comment
    }

    /// Returns the cumulative reputation record for a freelancer address.
    ///
    /// The [`types::Reputation`] struct aggregates every rating the address has
    /// received across all completed escrow contracts:
    ///
    /// | Field | Description |
    /// |---|---|
    /// | `completed_contracts` | Number of contracts for which reputation was issued |
    /// | `total_rating` | Sum of all individual ratings (each in \[1, 5\]) |
    /// | `last_rating` | The most recent rating value |
    ///
    /// To obtain a decimal average divide `total_rating` by `completed_contracts`,
    /// or use `get_average_rating` which returns the value pre-scaled to
    /// basis points (×10 000).
    ///
    /// # Arguments
    ///
    /// * `env` – The Soroban execution environment.
    /// * `address` – The freelancer address to query.
    ///
    /// # Returns
    ///
    /// * `Some(Reputation)` – A snapshot of the freelancer's aggregate record.
    /// * `None` – No reputation entry exists yet (the address has never received
    ///   a rating, or the entry has expired from persistent storage).
    ///
    /// No authorisation is required; this is a read-only query.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use soroban_sdk::{testutils::Address as _, vec, Address, Env, String};
    /// # use escrow::{Escrow, EscrowClient, ReleaseAuthorization};
    /// let env = Env::default();
    /// env.mock_all_auths();
    ///
    /// let escrow_id = env.register(Escrow, ());
    /// let escrow = EscrowClient::new(&env, &escrow_id);
    /// escrow.initialize(&Address::generate(&env));
    ///
    /// let client_addr = Address::generate(&env);
    /// let freelancer_addr = Address::generate(&env);
    ///
    /// // Unknown address returns None.
    /// assert!(escrow.get_reputation(&freelancer_addr).is_none());
    ///
    /// // Complete a contract and issue a rating of 4.
    /// let milestones = vec![&env, 200_0000000_i128, 400_0000000_i128, 600_0000000_i128];
    /// let contract_id = escrow.create_contract(
    ///     &client_addr, &freelancer_addr, &None, &milestones,
    ///     &ReleaseAuthorization::ClientOnly,
    /// );
    /// escrow.deposit_funds(&contract_id, &client_addr, &1_200_0000000_i128);
    /// for idx in 0_u32..3 {
    ///     escrow.approve_milestone_release(&contract_id, &client_addr, &idx);
    ///     escrow.release_milestone(&contract_id, &client_addr, &idx);
    /// }
    /// escrow.issue_reputation(
    ///     &contract_id, &client_addr, &4,
    ///     &String::from_str(&env, "Solid delivery."),
    /// );
    ///
    /// let rep = escrow.get_reputation(&freelancer_addr).unwrap();
    /// assert_eq!(rep.completed_contracts, 1);
    /// assert_eq!(rep.total_rating, 4);
    /// assert_eq!(rep.last_rating, 4);
    /// ```
    pub fn get_reputation(env: Env, address: Address) -> Option<types::Reputation> {
        reputation_migration::read_reputation_with_migration(&env, &address)
    }

    /// Returns the freelancer's average rating scaled to basis points (Ãƒâ€”10 000),
    /// or `None` if no reputation record exists or no contracts have been completed.
    ///
    /// # Arguments
    ///
    /// * `env` – The Soroban execution environment.
    /// * `address` – The freelancer address to query.
    ///
    /// # Returns
    ///
    /// * `Some(scaled_avg)` – `total_rating * 10_000 / completed_contracts`.
    ///   Divide by `10_000` to recover the decimal average.
    /// * `None` – No reputation record for `address`, or
    ///   `completed_contracts == 0`.
    ///
    /// # Scaling
    ///
    /// `result = total_rating * 10_000 / completed_contracts`
    ///
    /// A raw rating of 5 on a single contract returns `50_000` (5.0000 on a
    /// 1–5 scale). Clients divide by `10_000` to recover the decimal value.
    ///
    /// Checked arithmetic is used throughout; division by zero is impossible
    /// because `None` is returned whenever `completed_contracts == 0`.
    ///
    /// No authorisation is required; this is a read-only query.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use soroban_sdk::{testutils::Address as _, vec, Address, Env, String};
    /// # use escrow::{Escrow, EscrowClient, ReleaseAuthorization};
    /// let env = Env::default();
    /// env.mock_all_auths();
    ///
    /// let escrow_id = env.register(Escrow, ());
    /// let escrow = EscrowClient::new(&env, &escrow_id);
    /// escrow.initialize(&Address::generate(&env));
    ///
    /// // No record yet → None.
    /// let unknown = Address::generate(&env);
    /// assert!(escrow.get_average_rating(&unknown).is_none());
    ///
    /// // Helper: create, fund, complete, and rate a contract.
    /// let client_addr = Address::generate(&env);
    /// let freelancer_addr = Address::generate(&env);
    /// let milestones = vec![&env, 200_0000000_i128, 400_0000000_i128, 600_0000000_i128];
    ///
    /// let cid1 = escrow.create_contract(
    ///     &client_addr, &freelancer_addr, &None, &milestones,
    ///     &ReleaseAuthorization::ClientOnly,
    /// );
    /// escrow.deposit_funds(&cid1, &client_addr, &1_200_0000000_i128);
    /// for idx in 0_u32..3 {
    ///     escrow.approve_milestone_release(&cid1, &client_addr, &idx);
    ///     escrow.release_milestone(&cid1, &client_addr, &idx);
    /// }
    /// // Rating: 3  →  3 * 10_000 / 1 = 30_000
    /// escrow.issue_reputation(&cid1, &client_addr, &3, &String::from_str(&env, "Good."));
    /// assert_eq!(escrow.get_average_rating(&freelancer_addr), Some(30_000));
    ///
    /// // A second client rates the same freelancer 5.
    /// // total_rating = 8, completed = 2  →  8 * 10_000 / 2 = 40_000
    /// let client2 = Address::generate(&env);
    /// let cid2 = escrow.create_contract(
    ///     &client2, &freelancer_addr, &None, &milestones,
    ///     &ReleaseAuthorization::ClientOnly,
    /// );
    /// escrow.deposit_funds(&cid2, &client2, &1_200_0000000_i128);
    /// for idx in 0_u32..3 {
    ///     escrow.approve_milestone_release(&cid2, &client2, &idx);
    ///     escrow.release_milestone(&cid2, &client2, &idx);
    /// }
    /// escrow.issue_reputation(&cid2, &client2, &5, &String::from_str(&env, "Outstanding!"));
    /// assert_eq!(escrow.get_average_rating(&freelancer_addr), Some(40_000));
    /// ```
    pub fn get_average_rating(env: Env, address: Address) -> Option<i128> {
        let rep: types::Reputation = env
            .storage()
            .persistent()
            .get(&DataKey::Reputation(ReputationKey { user: address }))?;

        if rep.completed_contracts == 0 {
            return None;
        }

        rep.total_rating
            .checked_mul(crate::SCALE)
            .and_then(|scaled| scaled.checked_div(rep.completed_contracts))
    }

    /// Returns the number of completed contracts awaiting a reputation rating.
    ///
    /// Each time a contract transitions to [`ContractStatus::Completed`] (all
    /// milestones released, or a mix of released and refunded) the freelancer
    /// earns one pending credit. Calling `issue_reputation` consumes
    /// exactly one credit. Fully-refunded contracts (`Refunded` status) do
    /// **not** accrue a credit.
    ///
    /// This value increments once per completed contract and decrements once
    /// per successful `issue_reputation` call. Refunded contracts do not accrue
    /// pending reputation credits.
    ///
    /// # Arguments
    ///
    /// * `env` – The Soroban execution environment.
    /// * `address` – The freelancer address to query.
    ///
    /// # Returns
    ///
    /// The number of pending credits as an `i128`. Returns `0` when no record
    /// exists. The value should not be negative under normal operation.
    ///
    /// No authorisation is required; this is a read-only query.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use soroban_sdk::{testutils::Address as _, vec, Address, Env, String};
    /// # use escrow::{Escrow, EscrowClient, ReleaseAuthorization};
    /// let env = Env::default();
    /// env.mock_all_auths();
    ///
    /// let escrow_id = env.register(Escrow, ());
    /// let escrow = EscrowClient::new(&env, &escrow_id);
    /// escrow.initialize(&Address::generate(&env));
    ///
    /// let freelancer_addr = Address::generate(&env);
    ///
    /// // No completed contracts yet → 0 credits.
    /// assert_eq!(escrow.get_pending_reputation_credits(&freelancer_addr), 0);
    ///
    /// // Complete a contract — credit increments to 1.
    /// let client_addr = Address::generate(&env);
    /// let milestones = vec![&env, 200_0000000_i128, 400_0000000_i128, 600_0000000_i128];
    /// let contract_id = escrow.create_contract(
    ///     &client_addr, &freelancer_addr, &None, &milestones,
    ///     &ReleaseAuthorization::ClientOnly,
    /// );
    /// escrow.deposit_funds(&contract_id, &client_addr, &1_200_0000000_i128);
    /// for idx in 0_u32..3 {
    ///     escrow.approve_milestone_release(&contract_id, &client_addr, &idx);
    ///     escrow.release_milestone(&contract_id, &client_addr, &idx);
    /// }
    /// assert_eq!(escrow.get_pending_reputation_credits(&freelancer_addr), 1);
    ///
    /// // Issuing reputation consumes the credit — back to 0.
    /// escrow.issue_reputation(
    ///     &contract_id, &client_addr, &5,
    ///     &String::from_str(&env, "Flawless execution."),
    /// );
    /// assert_eq!(escrow.get_pending_reputation_credits(&freelancer_addr), 0);
    /// ```
    pub fn get_pending_reputation_credits(env: Env, address: Address) -> i128 {
        env.storage()
            .persistent()
            .get(&DataKey::PendingReputationCredits(ReputationKey { user: address }))
            .unwrap_or(0)
    }

    /// Migrate the reputation storage record for `address` to the current schema version.
    ///
    /// This entrypoint is idempotent: calling it on an already-current record is a
    /// safe no-op and returns `false`. When a v1 (legacy) record is detected the
    /// migration writes a [`DataKey::ReputationStorageVersion`] marker alongside the
    /// existing data and returns `true`. All field values are preserved exactly.
    ///
    /// # When to call
    ///
    /// Existing records written before versioning was introduced are transparently
    /// upgraded on every `get_reputation` read via the migration-on-read path, so
    /// most callers never need to call this directly. This explicit entrypoint is
    /// intended for operators who want to eagerly migrate a known address (e.g. as
    /// part of a deployment runbook) and receive a clear success/no-op signal.
    ///
    /// # Arguments
    ///
    /// * `address` — The freelancer address whose reputation record should be migrated.
    ///
    /// # Returns
    ///
    /// `true` if a migration was performed; `false` if the record was already at
    /// [`REPUTATION_STORAGE_VERSION`] or no record existed (no migration needed).
    ///
    /// # Security
    ///
    /// This is a permissionless read-equivalent: it does not transfer funds,
    /// change authorizations, or mutate business state beyond writing the version
    /// marker. Pause and emergency checks are intentionally omitted so operators
    /// can still migrate records during an incident pause.
    pub fn migrate_reputation_storage(env: Env, address: Address) -> bool {
        reputation_migration::migrate_reputation_storage_impl(&env, &address)
    }

    // -----------------------------------------------------------------------
    // Work evidence
    // -----------------------------------------------------------------------

    /// Records a deliverable reference (e.g. IPFS CID or URL hash) for an
    /// unreleased milestone.
    ///
    /// Only the contract's freelancer may call this. The contract must be in
    /// `Funded` status and the target milestone must not yet be released or
    /// refunded. Evidence may be overwritten before release.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment
    /// * `contract_id` - The escrow contract to update
    /// * `caller`      - Must equal the stored `freelancer`; requires auth
    /// * `milestone_index` - Zero-based index of the milestone
    /// * `evidence`    - Deliverable reference; max 256 bytes
    ///
    /// # Returns
    /// * `bool` - `true` if work evidence was recorded successfully
    ///
    /// # Errors
    /// * `NotInitialized`     — `initialize` has not been called
    /// * `ContractPaused` / `EmergencyActive` — pause/emergency gate
    /// * `ContractNotFound`   — unknown `contract_id`
    /// * `AlreadyFinalized`   — contract has been finalized
    /// * `UnauthorizedRole`   — `caller` is not the freelancer
    /// * `InvalidState`       — contract is not `Funded`
    /// * `IndexOutOfBounds`   — `milestone_index` exceeds milestone count
    /// * `MilestoneAlreadyReleased` — milestone is already released
    /// * `AlreadyRefunded`    — milestone has been refunded
    /// * `EvidenceTooLong`    — evidence string exceeds 256 bytes
    ///
    /// # Examples
    /// ```rust,ignore
    /// let client = EscrowClient::new(&env, &contract_id);
    /// let cid = soroban_sdk::String::from_str(&env, "ipfs://Qm...");
    /// let submitted = client.submit_work_evidence(&1, &freelancer_address, &0, &cid);
    /// assert!(submitted);
    /// ```
    pub fn submit_work_evidence(
        env: Env,
        contract_id: u32,
        caller: Address,
        milestone_index: u32,
        evidence: String,
    ) -> bool {
        /// Gate: contract must have been initialized so pause and emergency rails
        /// are always in scope before any state mutation can occur.
        Self::require_initialized(&env);
        Self::require_not_paused(&env);
        caller.require_auth();

        // Load contract, extend TTL, and assert not finalized via shared helper.
        let contract: Contract = Self::load_and_check_contract(&env, contract_id);

        if caller != contract.freelancer {
            env.panic_with_error(Error::UnauthorizedRole);
        }

        if contract.status != ContractStatus::Funded {
            env.panic_with_error(EscrowError::InvalidState);
        }

        // Bound evidence to 256 bytes to prevent storage bloat.
        if evidence.len() > MAX_EVIDENCE_BYTES {
            env.panic_with_error(Error::EvidenceTooLong);
        }

        let mut milestones: Vec<Milestone> = env
            .storage()
            .persistent()
            .get(&(DataKey::Contract(contract_id), milestone_key.clone()))
            .unwrap_or_else(|| env.panic_with_error(Error::ContractNotFound));

        ttl::extend_milestone_ttl(&env, contract_id);

        if milestone_index >= milestones.len() {
            env.panic_with_error(Error::IndexOutOfBounds);
        }

        let mut milestone = milestones.get(milestone_index).unwrap();

        if milestone.released {
            env.panic_with_error(Error::MilestoneAlreadyReleased);
        }
        if milestone.refunded {
            env.panic_with_error(Error::AlreadyRefunded);
        }

        milestone.work_evidence = Some(evidence.clone());
        milestones.set(milestone_index, milestone);

        ttl::store_milestones(&env, contract_id, &milestones);

        // Extend TTL on contract write (milestone TTL already extended by store_milestones)
        ttl::extend_contract_ttl(&env, contract_id);

        env.events().publish(
            (symbol_short!("evidence"), contract_id),
            (
                milestone_index,
                contract.freelancer,
                env.ledger().timestamp(),
            ),
        );

        true
    }

    /// Returns the work evidence for a single milestone, or `None` if the
    /// milestone index is out of bounds or no evidence was submitted.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment
    /// * `contract_id` - The escrow contract ID
    /// * `milestone_index` - Zero-based index of the milestone
    ///
    /// # Returns
    /// `Some(String)` with the evidence reference if it exists,
    /// `None` when the index is out of bounds or the milestone has no evidence.
    ///
    /// # Panics
    /// Panics with `ContractNotFound` if `contract_id` was never allocated.
    ///
    /// # TTL
    /// Extends the milestones vector's persistent TTL on read,
    /// consistent with `get_milestones`.
    ///
    /// # Examples
    /// ```rust,ignore
    /// let client = EscrowClient::new(&env, &contract_id);
    /// if let Some(evidence) = client.get_work_evidence(&1, &0) {
    ///     // Process evidence string
    /// }
    /// ```
    pub fn get_work_evidence(env: Env, contract_id: u32, milestone_index: u32) -> Option<String> {
        let milestones: Vec<Milestone> = env
            .storage()
            .persistent()
            .get(&DataKey::Milestones(contract_id))
            .unwrap_or_else(|| env.panic_with_error(Error::ContractNotFound));

        ttl::extend_milestone_ttl(&env, contract_id);

        if milestone_index >= milestones.len() {
            return None;
        }

        milestones.get(milestone_index).unwrap().work_evidence
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    // Ã¢â€â‚¬Ã¢â€â‚¬ Finalization Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

    // Ã¢â€â‚¬Ã¢â€â‚¬ Governance Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

    /// Returns the total accumulated protocol fees in stroops.
    ///
    /// The balance defaults to `0` when no fees have accrued. This public
    /// reader requires no authorization and does not mutate contract state.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment
    ///
    /// # Returns
    /// The fees currently available for protocol withdrawal.
    ///
    /// See [`docs/escrow/protocol-fees.md`](../../../docs/escrow/protocol-fees.md) for
    /// storage details and the full withdrawal flow.
    ///
    /// # Examples
    /// ```rust,ignore
    /// let client = EscrowClient::new(&env, &contract_id);
    /// let fees = client.get_accumulated_protocol_fees();
    /// ```
    pub fn get_accumulated_protocol_fees(env: Env) -> i128 {
        env.storage()
            .persistent()
            .get::<_, i128>(&DataKey::AccumulatedProtocolFees)
            .unwrap_or(0)
    }

    /// Drains accrued protocol fees from the escrow contract to a treasury address.
    ///
    /// Executes `SAC::transfer(from: escrow_address, to: treasury, amount)`. Protocol
    /// fees accumulate in `DataKey::AccumulatedProtocolFees` as each milestone is
    /// released; they remain commingled with the escrow's SAC balance until this
    /// entrypoint is called.
    ///
    /// See [`docs/escrow/sac-custody.md`](../../../docs/escrow/sac-custody.md) for the
    /// full custody model, accounting invariant, and security notes on commingled fees.
    ///
    /// See [`docs/escrow/protocol-fees.md`](../../../docs/escrow/protocol-fees.md) for
    /// the complete fee lifecycle Ã¢â‚¬â€ basis-point model, accrual, withdrawal authorization,
    /// worked examples, and the release-to-withdrawal sequence diagram.
    ///
    /// Requires the stored admin's authorization. Only an amount up to the
    /// currently accumulated fees can be withdrawn.
    ///
    /// # Errors
    /// * `ContractPaused` if the contract is paused
    /// * `EmergencyActive` if the contract is in emergency pause
    /// * `UnauthorizedRole` if the caller is not the stored admin
    /// * `InsufficientAccumulatedFees` if the requested amount exceeds accrued fees
    ///
    /// # Arguments
    /// * `env` - The contract environment
    /// * `amount` - The amount of fees to withdraw
    /// * `to` - The destination address for the withdrawn fees
    ///
    /// # Returns
    /// * `bool` - `true` if fees withdrawn successfully
    ///
    /// # Errors
    /// * `NotInitialized` - If contract uninitialized
    /// * `ContractPaused` - If paused or in emergency mode
    /// * `UnauthorizedRole` - If caller is not admin
    /// * `AmountMustBePositive` - If amount <= 0
    /// * `InsufficientAccumulatedFees` - If withdrawal amount exceeds accumulated fees
    /// * `SettlementTokenNotConfigured` - If no settlement token is bound
    ///
    /// # Examples
    /// ```rust,ignore
    /// let client = EscrowClient::new(&env, &contract_id);
    /// let withdrawn = client.withdraw_protocol_fees(&50_0000000, &treasury_address);
    /// assert!(withdrawn);
    /// ```
    pub fn withdraw_protocol_fees(env: Env, amount: i128, to: Address) -> bool {
        Self::require_initialized(&env);

        // Block withdrawal while paused or in emergency Ã¢â‚¬â€ consistent with all
        // other mutating entrypoints in this contract.
        if env
            .storage()
            .persistent()
            .get::<_, bool>(&DataKey::Paused)
            .unwrap_or(false)
        {
            env.panic_with_error(EscrowError::ContractPaused);
        }

        let admin: Address = env
            .storage()
            .persistent()
            .get(&DataKey::Admin)
            .unwrap_or_else(|| env.panic_with_error(EscrowError::NotInitialized));

        admin.require_auth();

        if amount <= 0 {
            env.panic_with_error(EscrowError::AmountMustBePositive);
        }

        let accumulated: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::AccumulatedProtocolFees)
            .unwrap_or(0);

        if amount > accumulated {
            env.panic_with_error(EscrowError::InsufficientAccumulatedFees);
        }

        let token = match Self::read_settlement_token(&env) {
            Some(t) => t,
            None => env.panic_with_error(EscrowError::SettlementTokenNotConfigured),
        };

        let new_accumulated = accumulated
            .checked_sub(amount)
            .unwrap_or_else(|| env.panic_with_error(EscrowError::InsufficientAccumulatedFees));
        env.storage()
            .persistent()
            .set(&DataKey::AccumulatedProtocolFees, &new_accumulated);

        env.storage().persistent().extend_ttl(
            &DataKey::AccumulatedProtocolFees,
            ttl::PERSISTENT_BUMP_THRESHOLD,
            ttl::PERSISTENT_TTL_LEDGERS,
        );

        token_client.transfer(&env.current_contract_address(), &to, &amount);

        env.events().publish(
            (symbol_short!("fee"), symbol_short!("withdraw")),
            (admin, to, amount, env.ledger().timestamp()),
        );

        true
    }

    /// Returns the ledger sequence at which the pending admin proposal was made.
    ///
    /// Returns `None` if there is no pending proposal. This allows off-chain
    /// indexers and governance dashboards to compute the remaining timelock
    /// before the proposal can be accepted via `accept_governance_admin`.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment
    ///
    /// # Returns
    /// * `Option<u32>` - `Some(ledger_sequence)` if a proposal is active, `None` otherwise
    ///
    /// # Examples
    /// ```rust,ignore
    /// let client = EscrowClient::new(&env, &contract_id);
    /// if let Some(proposed_at) = client.get_pending_admin_proposed_at() {
    ///     // Calculate timelock remaining
    /// }
    /// ```
    pub fn get_pending_admin_proposed_at(env: Env) -> Option<u32> {
        let proposal: Option<PendingAdminProposal> =
            env.storage().persistent().get(&DataKey::PendingAdmin);
        proposal.map(|p| p.proposed_at_ledger)
    }

    // Ã¢â€â‚¬Ã¢â€â‚¬ Protocol fee helpers Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

    /// Reads the stored protocol fee in basis points (0 = no fee).
    ///
    /// See [`docs/escrow/protocol-fees.md`](../../../docs/escrow/protocol-fees.md) for
    /// the full basis-point model, formula, and fee lifecycle.
    pub(crate) fn read_protocol_fee_bps(env: &Env) -> u32 {
        env.storage()
            .persistent()
            .get(&DataKey::ProtocolFeeBps)
            .unwrap_or(0)
    }

    /// Computes the protocol fee for a given `amount` at `fee_bps` basis points.
    ///
    /// Uses integer **floor division**: `fee = amount * fee_bps / BPS_DENOMINATOR`.
    /// The result always rounds down — it never rounds up — so the freelancer
    /// receives at least `amount - fee` stroops and the protocol receives at most
    /// the floored value. Callers must ensure `fee <= amount` holds; this is
    /// guaranteed for any `fee_bps` in `[0, 10_000]` and a non-negative `amount`.
    ///
    /// # Basis-point unit
    /// `10_000 bps = 100%`. The maximum configurable rate is `10_000`. A rate of
    /// `0` is the default and disables fee collection entirely.
    ///
    /// See [`docs/escrow/protocol-fees.md`](../../../docs/escrow/protocol-fees.md) for
    /// the full formula, rounding rules, worked numeric examples, and the sequence
    /// diagram from release through treasury withdrawal.
    ///
    /// # Short-circuit
    /// Returns `0` immediately when `fee_bps == 0`, skipping the multiplication.
    ///
    /// # Panics
    /// Panics with `PotentialOverflow` (error code 28) if `amount * fee_bps`
    /// overflows `i128`. Callers should keep `amount` well below `i128::MAX /
    /// fee_bps` to avoid this guard.
    ///
    /// # Examples
    /// ```rust,ignore
    /// let fee = Escrow::calculate_protocol_fee(&env, 100_0000000, 250); // 2.5% fee
    /// assert_eq!(fee, 2_5000000);
    /// ```
    pub fn calculate_protocol_fee(env: &Env, amount: i128, fee_bps: u32) -> i128 {
        if fee_bps == 0 {
            return 0;
        }
        let product = amount
            .checked_mul(fee_bps as i128)
            .unwrap_or_else(|| env.panic_with_error(Error::PotentialOverflow));
        product / BPS_DENOMINATOR as i128
    }

    // Ã¢â€â‚¬Ã¢â€â‚¬ Internal guards Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

    /// Panics with `NotInitialized` unless `initialize` has been called.
    pub(crate) fn require_initialized(env: &Env) {
        if !env
            .storage()
            .persistent()
            .get::<_, bool>(&DataKey::Initialized)
            .unwrap_or(false)
        {
            env.panic_with_error(EscrowError::NotInitialized);
        }
    }

    pub(crate) fn is_initialized(env: &Env) -> bool {
        env.storage()
            .persistent()
            .get::<_, bool>(&DataKey::Initialized)
            .unwrap_or(false)
    }

    // -----------------------------------------------------------------------
    // Dispute management
    // -----------------------------------------------------------------------

    /// Opens a dispute for a funded or partially funded escrow contract.
    ///
    /// This entrypoint transitions the contract status to `Disputed`, preventing
    /// further milestone releases until an assigned arbiter resolves the dispute.
    /// Only the client or freelancer can open a dispute, and an arbiter must be
    /// assigned to the contract.
    ///
    /// # Arguments
    /// * `env` - The contract environment
    /// * `contract_id` - The contract ID
    /// * `caller` - The address opening the dispute (must be client or freelancer)
    ///
    /// # Returns
    /// `true` if the dispute was successfully opened
    ///
    /// # Errors
    /// * `NotInitialized` - If `initialize` has not been called
    /// * `ContractNotFound` - If contract doesn't exist
    /// * `UnauthorizedRole` - If caller is not client or freelancer
    /// * `ArbiterRequired` - If no arbiter is assigned to the contract
    /// * `InvalidState` - If contract is not in a disputable state
    /// * `ContractPaused` - If pause or emergency controls are active
    /// * `AlreadyFinalized` - If contract has been finalized
    ///
    /// # Security
    /// - Only contract parties (client/freelancer) can open disputes
    /// - Requires arbiter assignment for resolution
    /// - Blocks milestone releases while disputed
    /// - Respects pause and emergency controls
    pub fn raise_dispute(env: Env, contract_id: u32, caller: Address) -> bool {
        /// Gate: contract must have been initialized so pause and emergency rails
        /// are always in scope before any state mutation can occur.
        Self::require_initialized(&env);
        Self::require_not_paused(&env);
        caller.require_auth();

        let mut contract: Contract = env
            .storage()
            .persistent()
            .get(&DataKey::Contract(contract_id))
            .unwrap_or_else(|| env.panic_with_error(Error::ContractNotFound));

        ttl::extend_contract_ttl(&env, contract_id);
        Self::require_not_finalized(&env, contract_id);

        // Verify caller is client or freelancer
        if caller != contract.client && caller != contract.freelancer {
            env.panic_with_error(Error::UnauthorizedRole);
        }

        // Require arbiter assignment
        if contract.arbiter.is_none() {
            env.panic_with_error(Error::ArbiterRequired);
        }

        // Verify contract is in a disputable state (Funded or PartiallyFunded)
        match contract.status {
            ContractStatus::Funded | ContractStatus::PartiallyFunded => {}
            _ => env.panic_with_error(Error::InvalidState),
        }

        contract.status = ContractStatus::Disputed;
        env.storage()
            .persistent()
            .set(&DataKey::Contract(contract_id), &contract);

        events::emit_contract_indexed_event(&env, contract_id, &contract);

        ttl::extend_contract_ttl(&env, contract_id);

        env.events().publish(
            (symbol_short!("dispute"), symbol_short!("opened")),
            (contract_id, caller),
        );

        true
    }

    /// Resolves an open dispute by applying the arbiter-selected resolution.
    ///
    /// This entrypoint applies the dispute resolution (FullRefund, PartialRefund,
    /// FullPayout, or custom Split) to the remaining escrowed balance. The resolution
    /// must be authorized by the assigned arbiter and must conserve the available funds.
    ///
    /// # Arguments
    /// * `env` - The contract environment
    /// * `contract_id` - The contract ID
    /// * `arbiter` - The arbiter address (must match contract's assigned arbiter)
    /// * `resolution` - The resolution decision (FullRefund, PartialRefund, FullPayout, or Split)
    ///
    /// # Returns
    /// `true` if the dispute was successfully resolved
    ///
    /// # Errors
    /// * `NotInitialized` - If `initialize` has not been called
    /// * `ContractNotFound` - If contract doesn't exist
    /// * `UnauthorizedRole` - If caller is not the assigned arbiter
    /// * `InvalidStatusTransition` - If contract is not in Disputed state
    /// * `InvalidDisputeSplit` - If custom split doesn't match available balance
    /// * `AccountingInvariantViolated` - If accounting state is inconsistent
    /// * `PotentialOverflow` - If amount calculations would overflow
    /// * `ContractPaused` - If pause or emergency controls are active
    /// * `AlreadyFinalized` - If contract has been finalized
    ///
    /// # Security
    /// - Only the assigned arbiter can resolve disputes
    /// - Split amounts must exactly match available balance
    /// - Updates released_amount and refunded_amount atomically
    /// - Emits dispute resolution event for indexers
    /// - Sets final contract status based on resolution outcome
    pub fn resolve_dispute(
        env: Env,
        contract_id: u32,
        arbiter: Address,
        resolution: DisputeResolution,
    ) -> bool {
        /// Gate: contract must have been initialized so pause and emergency rails
        /// are always in scope before any state mutation can occur.
        Self::require_initialized(&env);
        Self::require_not_paused(&env);
        arbiter.require_auth();

        let mut contract: Contract = env
            .storage()
            .persistent()
            .get(&DataKey::Contract(contract_id))
            .unwrap_or_else(|| env.panic_with_error(Error::ContractNotFound));

        ttl::extend_contract_ttl(&env, contract_id);
        Self::require_not_finalized(&env, contract_id);

        // Verify contract is in Disputed state
        if contract.status != ContractStatus::Disputed {
            env.panic_with_error(Error::InvalidStatusTransition);
        }

        // Verify caller is the assigned arbiter
        match &contract.arbiter {
            Some(contract_arbiter) if *contract_arbiter == arbiter => {}
            _ => env.panic_with_error(Error::UnauthorizedRole),
        }

        // Compute payouts based on resolution
        let (client_payout, freelancer_payout) =
            dispute::resolution_payouts(&contract, &resolution)
                .unwrap_or_else(|e| env.panic_with_error(e));

        // Update contract accounting
        contract.refunded_amount = contract
            .refunded_amount
            .checked_add(client_payout)
            .unwrap_or_else(|| env.panic_with_error(Error::PotentialOverflow));
        contract.released_amount = contract
            .released_amount
            .checked_add(freelancer_payout)
            .unwrap_or_else(|| env.panic_with_error(Error::PotentialOverflow));

        // Set final status
        contract.status = dispute::final_status_after_resolution(&contract);
        if contract.status == ContractStatus::Completed {
            Self::grant_pending_reputation_credit(&env, &contract.freelancer);
        }

        env.storage()
            .persistent()
            .set(&DataKey::Contract(contract_id), &contract);

        events::emit_contract_indexed_event(&env, contract_id, &contract);

        ttl::extend_contract_ttl(&env, contract_id);

        env.events().publish(
            (symbol_short!("dispute"), symbol_short!("resolved")),
            (contract_id, resolution.code()),
        );

        true
    }
}

#[cfg(test)]
mod test;
