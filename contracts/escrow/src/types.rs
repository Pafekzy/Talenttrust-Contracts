use soroban_sdk::{contracterror, contracttype, Address, BytesN, String, Vec};

// ── Indexer summary types ────────────────────────────────────────────────────

#[allow(dead_code)]
pub const CONTRACT_SUMMARY_SCHEMA_VERSION: u32 = 1;

/// Current on-ledger layout version for per-contract dispute metadata.
///
/// Bump this when introducing a new `DisputeMetadata` layout. Older layouts are
/// upgraded on read by `dispute::load_dispute_metadata`.
pub const DISPUTE_STORAGE_VERSION: u32 = 1;

/// Legacy (v0) dispute metadata layout without an embedded schema version.
///
/// Retained solely so migrate-on-read can decode pre-versioned records and
/// rewrite them as [`DisputeMetadata`].
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DisputeMetadataV0 {
    pub raised_by: Address,
    pub reason_hash: BytesN<32>,
    pub raised_at: u64,
}

/// Versioned dispute metadata stored under [`DataKey::Dispute`].
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DisputeMetadata {
    /// Must equal [`DISPUTE_STORAGE_VERSION`] after a successful write/migration.
    pub schema_version: u32,
    pub raised_by: Address,
    pub reason_hash: BytesN<32>,
    pub raised_at: u64,
}

#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MilestoneEntry {
    pub index: u32,
    pub status: u32,
    pub amount: i128,
}

/// Lightweight contract entry returned by the paginated contracts view.
///
/// Carries only the fields needed for a UI listing: the contract `id`, a
/// numeric `status` code (the `ContractStatus` discriminant), and the
/// escrow's `funded_amount` / `released_amount` in stroops.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContractEntry {
    pub id: u32,
    pub status: u32,
    pub funded_amount: i128,
    pub released_amount: i128,
}

/// Lightweight arbiter entry returned by the paginated arbiter enumeration view.
///
/// Each entry pairs a contract id with its assigned arbiter. Contracts without
/// an arbiter are omitted from the page.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArbiterEntry {
    pub contract_id: u32,
    pub arbiter: Address,
}

/// A point-in-time snapshot of the contract state.
/// This structure is used for both indexing (`get_contract_summary`) and
/// the immutable close metadata stored at finalization.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContractSummary {
    pub schema_version: u32,
    pub client: Address,
    pub freelancer: Address,
    pub arbiter: Option<Address>,
    pub status: ContractStatus,
    /// Indicates whether reputation has been issued for this contract.
    /// This is determined by reading the `DataKey::ReputationIssued` storage entry.
    pub reputation_issued: bool,
    pub total_amount: i128,
    pub funded_amount: i128,
    pub released_amount: i128,
    pub refundable_balance: i128,
    pub released_milestone_count: u32,
    pub milestones: Vec<MilestoneSummary>,
}

/// Protocol-wide bounds for contract validation.
///
/// This type carries the limits used by `create_contract` and other
/// validation paths. It is returned by `get_bounds()` for off-chain indexers
/// and client applications.
///
/// The settlement limit (`max_single_milestone_stroops`) is admin-configurable
/// at runtime; all other fields are compile-time constants.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContractBounds {
    pub max_milestones: u32,
    pub max_single_milestone_stroops: i128,
    pub max_total_escrow_stroops: i128,
    pub max_fee_bps: u32,
    /// Maximum number of disputes per contract.
    pub max_disputes: u32,
}

/// Configuration parameters for dispute resolutions.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DisputeConfig {
    /// Percentage of remaining funds allocated to the freelancer in partial refunds (in basis points, 3000 = 30%).
    pub partial_refund_freelancer_bps: u32,
    /// Percentage of remaining funds allocated to the client in partial refunds (in basis points, 7000 = 70%).
    pub partial_refund_client_bps: u32,
}

impl Default for DisputeConfig {
    fn default() -> Self {
        DisputeConfig {
            partial_refund_freelancer_bps: 3000,
            partial_refund_client_bps: 7000,
        }
    }
}

/// Simulated outcome of creating a contract without actually writing to storage.
///
/// This type is returned by `simulate_create_contract` and represents the
/// projected state that would result from a contract creation. The operation
/// performs all validation checks but makes no storage writes or events.
///
/// # Read-only guarantee
/// - No storage mutations
/// - No events emitted
/// - All validation from `create_contract` is applied
/// - Outcome matches what `create_contract` would produce
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SimulateCreateContractOutcome {
    /// The contract ID that would be assigned
    pub contract_id: u32,
    /// The client address
    pub client: Address,
    /// The freelancer address
    pub freelancer: Address,
    /// The optional arbiter address
    pub arbiter: Option<Address>,
    /// The release authorization mode
    pub release_authorization: ReleaseAuthorization,
    /// The milestone amounts (in stroops)
    pub milestones: Vec<i128>,
    /// Total escrow amount across all milestones
    pub total_amount: i128,
}

// ── Core contract state ──────────────────────────────────────────────────────

/// Typed storage key for contract-owned entries that previously used ad-hoc
/// tuple keys such as `(DataKey::Contract(id), Symbol("milestones"))`.
///
/// The variants are intentionally narrow and match the storage shapes already
/// used by the escrow contract so the public behavior stays unchanged while the
/// call sites become clearer and more type-safe.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StorageKey {
    Contract(u32),
    ContractMilestones(u32),
    MilestoneApprovals(u32, u32),
    Finalization(u32),
    PendingClientMigration(u32),
}

impl StorageKey {
    pub fn contract(contract_id: u32) -> Self {
        Self::Contract(contract_id)
    }

    pub fn contract_milestones(contract_id: u32) -> Self {
        Self::ContractMilestones(contract_id)
    }

    pub fn milestone_approvals(contract_id: u32, milestone_index: u32) -> Self {
        Self::MilestoneApprovals(contract_id, milestone_index)
    }

    pub fn finalization(contract_id: u32) -> Self {
        Self::Finalization(contract_id)
    }

    pub fn pending_client_migration(contract_id: u32) -> Self {
        Self::PendingClientMigration(contract_id)
    }
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    // Admin / pause / emergency
    Initialized,
    Admin,
    Paused,
    Emergency,
    SettlementToken,
    // Contract storage
    Contract(u32),
    ContractSchemaVersion(u32),
    NextContractId,
    Milestones(u32),
    MilestoneReleased(u32, u32),
    MilestoneApprovals(u32, u32),
    Finalization(u32),
    // Reputation
    ReputationIssued(u32),
    PendingReputationCredits(ReputationKey),
    Reputation(ReputationKey),
    ReputationComment(u32),
    /// Monotonically-increasing schema version for reputation storage.
    /// Absent means v1 (original layout). Present value equals
    /// [`REPUTATION_STORAGE_VERSION`].
    ReputationStorageVersion(Address),
    // Client migration
    PendingClientMigration(u32),
    // Settlement token
    SettlementToken,
    // Finalization
    Finalization(u32),
    // Protocol / governance
    GovernanceAdmin,
    PendingGovernanceAdmin,
    ProtocolParameters,
    ProtocolFeeBps,
    PendingAdmin,
    AccumulatedProtocolFees,
    GovernedParameters,
    ReadinessChecklist,
    // Configurable limits
    MaxMilestones,
    MaxEscrowStroops,
    // Settlement storage
    SettlementToken,
    // Disputes: versioned metadata + per-contract layout marker
    Dispute(u32),
    DisputeStorageVersion(u32),
}

/// Canonical contract error type for all entrypoint-facing errors.
#[contracterror(export = false)]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum EscrowError {
    /// The specified milestone index is out of bounds.
    IndexOutOfBounds = 3,
    AlreadyReleased = 4,
    EmptyRefundRequest = 6,
    DuplicateMilestoneInRefund = 7,
    AlreadyRefunded = 8,
    InsufficientFunds = 9,
    ContractNotFound = 10,
    UnauthorizedRole = 11,
    MissingArbiter = 12,
    InvalidArbiter = 13,
    InvalidParticipants = 14,
    AmountMustBePositive = 15,
    InvalidState = 16,
    MilestoneAlreadyReleased = 17,
    AlreadyApproved = 18,
    InsufficientApprovals = 20,
    FreelancerMismatch = 21,
    InvalidRating = 22,
    ReputationAlreadyIssued = 23,
    EmptyMilestones = 25,
    InvalidMilestoneAmount = 26,
    ContractIdCollision = 27,
    ContractIdOverflow = 28,
    EmptyComment = 29,
    CommentTooLong = 30,
    InvalidParticipant = 31,
    InvalidDepositAmount = 32,
    InvalidMilestone = 33,
    AlreadyInitialized = 34,
    InsufficientAccumulatedFees = 35,
    NotInitialized = 36,
    ContractPaused = 37,
    EmergencyActive = 38,
    SelfRating = 39,
    NotCompleted = 40,
    InvalidStatusTransition = 41,
    ArbiterRequired = 42,
    InvalidDisputeSplit = 43,
    AccountingInvariantViolated = 44,
    PotentialOverflow = 45,
    AlreadyFinalized = 46,
    /// The work evidence string exceeds the maximum length limit.
    EvidenceTooLong = 47,
    TimelockNotElapsed = 48,
    InvalidProtocolParameters = 49,
    /// The contract has already been cancelled.
    AlreadyCancelled = 50,
    /// The escrow cap would be exceeded by this operation.
    EscrowCapExceeded = 51,
    SettlementTokenNotConfigured = 52,
    MilestoneNotOverdue = 53,
    /// Milestone rollback is not allowed in the current state.
    RollbackNotAllowed = 54,
}

#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContractStatus {
    Created = 0,
    Accepted = 1,
    Funded = 2,
    Completed = 3,
    Disputed = 4,
    Cancelled = 5,
    Refunded = 6,
    PartiallyFunded = 7,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Contract {
    pub client: Address,
    pub freelancer: Address,
    pub arbiter: Option<Address>,
    pub status: ContractStatus,
    pub total_deposited: i128,
    pub funded_amount: i128,
    pub released_amount: i128,
    pub refunded_amount: i128,
    pub release_authorization: ReleaseAuthorization,
    pub reputation_issued: bool,
    pub token: Address,
}

/// Bounded snapshot of the escrow instance's settlement configuration.
///
/// All fields are read directly from persistent storage so indexers can inspect
/// settlement readiness and accrued fees without reconstructing state from
/// events or contract activity.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Milestone {
    pub amount: i128,
    pub funded_amount: i128,
    pub protocol_fee: i128,
    pub released: bool,
    pub refunded: bool,
    pub work_evidence: Option<String>,
    pub refunded_amount: i128,
    /// Optional Unix timestamp (seconds) after which the client may claim
    /// a timeout refund for this milestone without arbiter involvement.
    /// None means no deadline — the milestone never expires.
    pub deadline: Option<u64>,
}

/// Projected outcome of a milestone release simulation.
///
/// Returned by `simulate_release_milestone`. When `would_succeed` is `false`,
/// `error_code` contains the numeric code of the error that the real
/// `release_milestone` would panic with.
#[contracttype]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SimulatedRelease {
    /// Whether the real `release_milestone` would succeed.
    pub would_succeed: bool,
    /// Gross milestone amount (before fee deduction).
    pub gross_amount: i128,
    /// Protocol fee that would be retained.
    pub protocol_fee: i128,
    /// Net amount that would be transferred to the freelancer.
    pub net_amount: i128,
    /// The contract's `released_amount` after the projected release.
    pub projected_released_amount: i128,
    /// Whether this release would transition the contract to `Completed`.
    pub would_complete_contract: bool,
    /// Error code matching the corresponding entrypoint error, `None` on success.
    pub error_code: Option<u32>,
}

#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DepositMode {
    ExactTotal = 0,
    Incremental = 1,
}

// ── Simulation result types ───────────────────────────────────────────────────

/// Result of a simulated deposit operation.
///
/// Returned by [`simulate_deposit_funds`](crate::Escrow::simulate_deposit_funds)
/// to let callers preview the state transition that a real `deposit_funds`
/// call would produce, without executing any token transfer or writing storage.
///
/// All amounts are in stroops.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SimulateDepositResult {
    /// The `funded_amount` on the contract before the simulated deposit.
    pub current_funded_amount: i128,
    /// The `funded_amount` that would result after the deposit.
    pub new_funded_amount: i128,
    /// The contract status that would result after the deposit.
    pub projected_status: ContractStatus,
    /// The sum of all milestone amounts for the contract.
    pub total_milestone_amount: i128,
}

// ── Governance / readiness ───────────────────────────────────────────────────

/// Readiness checklist stored under [`DataKey::ReadinessChecklist`].
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadinessChecklist {
    pub initialized: bool,
    pub governed_params_set: bool,
    pub emergency_controls_enabled: bool,
}

impl Default for ReadinessChecklist {
    fn default() -> Self {
        ReadinessChecklist {
            initialized: false,
            governed_params_set: false,
            emergency_controls_enabled: false,
        }
    }
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GovernedParameters {
    pub protocol_fee_bps: u32,
    pub max_escrow_total_stroops: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingAdminProposal {
    pub proposed: Address,
    pub proposed_at_ledger: u32,
}

// ── Reputation ───────────────────────────────────────────────────────────────

/// Cumulative on-chain reputation record for a freelancer.
///
/// A `Reputation` entry is created (or updated) each time
/// `issue_reputation` is called for a completed contract.  It
/// aggregates ratings across all contracts the freelancer has participated in,
/// enabling clients and integrations to derive an average score at any time.
///
/// # Stored under
///
/// [`DataKey::Reputation`]`(freelancer_address)` in persistent storage.
///
/// # Average rating
///
/// To compute the decimal average:
/// ```text
/// average = total_rating as f64 / completed_contracts as f64
/// ```
/// Or, to avoid floating-point arithmetic on-chain, use
/// `get_average_rating` which returns
/// `total_rating * 10_000 / completed_contracts` (basis-point precision).
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
/// escrow.issue_reputation(&contract_id, &client_addr, &5, &String::from_str(&env, "Excellent!"));
///
/// let rep = escrow.get_reputation(&freelancer_addr).unwrap();
/// assert_eq!(rep.completed_contracts, 1);
/// assert_eq!(rep.total_rating, 5);
/// assert_eq!(rep.last_rating, 5);
/// ```
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct Reputation {
    /// Number of escrow contracts for which reputation has been issued.
    pub completed_contracts: i128,
    /// Sum of all individual ratings received (each rating is in \[1, 5\]).
    pub total_rating: i128,
    /// The most recent individual rating value.
    pub last_rating: i128,
}

// ── Dispute Resolution ───────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DisputeSplit {
    pub client_amount: i128,
    pub freelancer_amount: i128,
}

pub type SplitAmounts = DisputeSplit;

// ── Milestone schedule metadata ───────────────────────────────────────────

/// Maximum byte length for a milestone schedule title.
pub const MAX_SCHEDULE_TITLE_LEN: u32 = 64;
/// Maximum byte length for a milestone schedule description.
pub const MAX_SCHEDULE_DESCRIPTION_LEN: u32 = 256;

/// Milestone-related configuration values.
///
/// Combines compile‑time bounds with runtime‑governed parameters, providing
/// a single read‑only view for callers that need to discover how milestones
/// are constrained.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MilestonesConfig {
    /// Maximum number of milestones per contract (compile‑time constant).
    pub max_milestones: u32,
    /// Maximum amount allowed for a single milestone in stroops (compile‑time constant).
    pub max_single_milestone_stroops: i128,
    /// Maximum total escrow amount in stroops.
    /// This is the runtime‑governed cap (falls back to the compile‑time bound when unset).
    pub max_total_escrow_stroops: i128,
    /// Maximum protocol fee in basis points (10_000 = 100 %).
    /// This is the runtime‑governed cap (falls back to 10_000 when unset).
    pub max_fee_bps: u32,
    /// Maximum byte length for a milestone schedule title.
    pub max_schedule_title_len: u32,
    /// Maximum byte length for a milestone schedule description.
    pub max_schedule_description_len: u32,
}

/// Per-milestone schedule metadata stored alongside the milestone vector.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MilestoneSchedule {
    /// Optional Unix timestamp (seconds) for the expected delivery deadline.
    /// `None` means no deadline — the milestone never expires for schedule
    /// purposes (distinct from the timeout-refund deadline on `Milestone`).
    pub due_date: Option<u64>,
    /// Optional short title for the milestone (e.g. "Phase 1").
    /// Max byte length is [`MAX_SCHEDULE_TITLE_LEN`].
    pub title: Option<String>,
    /// Optional longer description of the milestone deliverable.
    /// Max byte length is [`MAX_SCHEDULE_DESCRIPTION_LEN`].
    pub description: Option<String>,
    /// Ledger timestamp of the last schedule update.  Stamped by the contract
    /// on create / set — caller-supplied values are overwritten.
    pub updated_at: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DisputeResolution {
    FullRefund,
    PartialRefund,
    FullPayout,
    Split(DisputeSplit),
}

impl DisputeResolution {
    pub fn code(&self) -> u32 {
        match self {
            Self::FullRefund => 0,
            Self::PartialRefund => 1,
            Self::FullPayout => 2,
            Self::Split(_) => 3,
        }
    }
}

/// Outcome of a dispute, combining open-state and all resolution variants in one
/// enum so that `DisputeRecord` can store the outcome without `Option<DisputeResolution>`
/// (which cannot be stored in a `#[contracttype]` struct because Soroban contracttype
/// enums use env-based serialization, not the XDR `From<T>` trait needed by `Option<T>`).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DisputeOutcome {
    /// The dispute has been raised but not yet resolved by the arbiter.
    Open,
    /// All remaining funds returned to the client.
    FullRefund,
    /// 70 % to client, 30 % to freelancer (floor-rounded).
    PartialRefund,
    /// All remaining funds released to the freelancer.
    FullPayout,
    /// Arbiter-specified custom split.
    Split(DisputeSplit),
}

impl DisputeOutcome {
    /// Convert to the equivalent `DisputeResolution`, or `None` if still open.
    pub fn as_resolution(&self) -> Option<DisputeResolution> {
        match self {
            Self::Open => None,
            Self::FullRefund => Some(DisputeResolution::FullRefund),
            Self::PartialRefund => Some(DisputeResolution::PartialRefund),
            Self::FullPayout => Some(DisputeResolution::FullPayout),
            Self::Split(s) => Some(DisputeResolution::Split(s.clone())),
        }
    }

    /// Build a `DisputeOutcome` from a `DisputeResolution`.
    pub fn from_resolution(r: &DisputeResolution) -> Self {
        match r {
            DisputeResolution::FullRefund => Self::FullRefund,
            DisputeResolution::PartialRefund => Self::PartialRefund,
            DisputeResolution::FullPayout => Self::FullPayout,
            DisputeResolution::Split(s) => Self::Split(s.clone()),
        }
    }
}

/// Typed record for a dispute lifecycle entry.
///
/// Written to `DataKey::Dispute(contract_id)` by `raise_dispute` and updated
/// in-place by `resolve_dispute`. Absent for contracts that were never disputed.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DisputeRecord {
    /// Party (client or freelancer) that raised the dispute.
    pub raised_by: Address,
    /// Ledger timestamp when the dispute was raised.
    pub raised_at: u64,
    /// `Open` while the dispute is pending; replaced with the resolution variant
    /// once the arbiter calls `resolve_dispute`.
    pub outcome: DisputeOutcome,
    /// Ledger timestamp when the dispute was resolved, or `None` while open.
    pub resolved_at: Option<u64>,
}

// ── Batch settlement ─────────────────────────────────────────────────────────

/// A single item in a [`Escrow::finalize_contracts_batch`] request.
///
/// Each entry pairs a `contract_id` with the `finalizer` address that is
/// authorizing closure of that contract.  The finalizer must be the stored
/// client, freelancer, or assigned arbiter — the same role check as the
/// single-item [`Escrow::finalize_contract`] entrypoint.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SettlementItem {
    /// The escrow contract to finalize.
    pub contract_id: u32,
    /// The address authorizing finalization (client, freelancer, or arbiter).
    pub finalizer: Address,
}

/// Per-item outcome returned by [`Escrow::finalize_contracts_batch`].
///
/// Every item in the input vector produces exactly one `BatchSettlementResult`
/// at the same position.  Inspect `success` first; when `false`, `error_code`
/// carries the numeric discriminant of the [`EscrowError`] that would have
/// been returned by the equivalent single-item call.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BatchSettlementResult {
    /// Zero-based position of this item in the input vector.
    pub index: u32,
    /// The contract ID from the corresponding [`SettlementItem`].
    pub contract_id: u32,
    /// `true` when finalization succeeded; `false` on any per-item error.
    pub success: bool,
    /// Error discriminant when `success` is `false`; `None` on success.
    pub error_code: Option<u32>,
}
