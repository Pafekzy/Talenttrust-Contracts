//! Dispute payout arithmetic, final-status helpers, and versioned dispute-metadata storage.
//!
//! `resolution_payouts` and `final_status_after_resolution` are storage-free.
//! Dispute records are stored under [`DataKey::Dispute`] with an explicit layout
//! marker at [`DataKey::DisputeStorageVersion`]. Reads go through
//! [`load_dispute_metadata`], which upgrades older layouts in place
//! (v0 → v1) and is a no-op when the on-ledger version already matches
//! [`DISPUTE_STORAGE_VERSION`].

use crate::{
    safe_add_amounts, Contract, ContractStatus, DataKey, DisputeConfig, DisputeMetadata,
    DisputeMetadataV0, DisputeResolution, DisputeSplit, Error, Escrow, EscrowError,
    DISPUTE_STORAGE_VERSION,
};
use soroban_sdk::{symbol_short, Address, BytesN, Env};

// ---------------------------------------------------------------------------
// disputes configuration helpers
// ---------------------------------------------------------------------------

/// Read-only getter for disputes configuration without mutating storage.
/// Returns sensible default (`partial_refund_freelancer_share_bps = 3000`, `partial_refund_client_share_bps = 7000`)
/// before initialization or if storage is unconfigured.
pub fn get_dispute_config(env: &Env) -> Option<DisputeConfig> {
    env.storage().persistent().get(&DataKey::DisputeConfigKey)
}

/// Storage writer for disputes configuration.
pub fn set_dispute_config(env: &Env, config: DisputeConfig) -> bool {
    env.storage()
        .persistent()
        .set(&DataKey::DisputeConfigKey, &config);
    true
}

/// Typed result of computing a dispute resolution's payouts, replacing the
/// previous untyped `(i128, i128)` tuple return.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DisputePayouts {
    /// Amount refunded back to the client.
    pub client_payout: i128,
    /// Amount released to the freelancer.
    pub freelancer_payout: i128,
}

#[allow(dead_code)]
pub fn resolution_payouts(
    contract: &Contract,
    resolution: &DisputeResolution,
) -> Result<DisputePayouts, Error> {
    let available = contract
        .funded_amount
        .checked_sub(contract.released_amount)
        .and_then(|value| value.checked_sub(contract.refunded_amount))
        .ok_or(Error::AccountingInvariantViolated)?;
    if available < 0 {
        return Err(Error::AccountingInvariantViolated);
    }

    match resolution {
        DisputeResolution::FullRefund => Ok(DisputePayouts {
            client_payout: available,
            freelancer_payout: 0,
        }),
        DisputeResolution::PartialRefund => {
            // freelancer gets floor(available * PARTIAL_REFUND_FREELANCER_SHARE / PARTIAL_REFUND_DENOMINATOR), client gets remainder
            let freelancer_payout = available
                .checked_mul(PARTIAL_REFUND_FREELANCER_SHARE)
                .and_then(|value| value.checked_div(PARTIAL_REFUND_DENOMINATOR))
                .ok_or(Error::PotentialOverflow)?;
            Ok(DisputePayouts {
                client_payout: available - freelancer_payout,
                freelancer_payout,
            })
        }
        DisputeResolution::FullPayout => Ok(DisputePayouts {
            client_payout: 0,
            freelancer_payout: available,
        }),
        DisputeResolution::Split(client_amount, freelancer_amount) => {
            if *client_amount < 0 || *freelancer_amount < 0 {
                return Err(Error::InvalidDisputeSplit);
            }
            if split.client_amount > MAX_SINGLE_AMOUNT_STROOPS
                || split.freelancer_amount > MAX_SINGLE_AMOUNT_STROOPS
            {
                return Err(EscrowError::InvalidDisputeSplit);
            }
            Ok(DisputePayouts {
                client_payout: *client_amount,
                freelancer_payout: *freelancer_amount,
            })
        }
    }
}

/// Determine the final contract status after dispute resolution.
///
/// Returns [`ContractStatus::Refunded`] only when every stroop ever deposited
/// has been refunded (`refunded_amount == funded_amount`). Otherwise returns
/// [`ContractStatus::Completed`] — including the case where some funds remain
/// escrowed after a dispute resolution.
///
/// # Example
/// ```ignore
/// use soroban_sdk::{Address, Env};
/// use crate::{Contract, ContractStatus, ReleaseAuthorization};
///
/// let env = Env::default();
/// let fixture = |funded: i128, refunded: i128| Contract {
///     client: Address::generate(&env),
///     freelancer: Address::generate(&env),
///     arbiter: Some(Address::generate(&env)),
///     status: ContractStatus::Disputed,
///     total_deposited: funded,
///     funded_amount: funded,
///     released_amount: 0,
///     refunded_amount: refunded,
///     release_authorization: ReleaseAuthorization::ClientOnly,
///     reputation_issued: false,
/// };
///
/// // Full refund of the deposit lands the contract in the Refunded terminal state.
/// assert_eq!(
///     final_status_after_resolution(&fixture(100, 100)),
///     ContractStatus::Refunded,
/// );
///
/// // Partial refund plus the released remainder keeps the contract Completed.
/// assert_eq!(
///     final_status_after_resolution(&fixture(100, 60)),
///     ContractStatus::Completed,
/// );
/// ```
pub fn final_status_after_resolution(contract: &Contract) -> ContractStatus {
    if contract.refunded_amount == contract.funded_amount {
        ContractStatus::Refunded
    } else {
        ContractStatus::Completed
    }
}

// ─── Versioned dispute storage ───────────────────────────────────────────────

pub(crate) fn dispute_key(contract_id: u32) -> DataKey {
    DataKey::Dispute(contract_id)
}

pub(crate) fn dispute_version_key(contract_id: u32) -> DataKey {
    DataKey::DisputeStorageVersion(contract_id)
}

/// Returns the on-ledger dispute storage version for `contract_id`.
///
/// Missing markers are treated as version `0` (legacy / pre-versioned).
pub fn get_dispute_storage_version(env: &Env, contract_id: u32) -> u32 {
    env.storage()
        .persistent()
        .get(&dispute_version_key(contract_id))
        .unwrap_or(0)
}

        env.events().publish(
            (symbol_short!("dispute"), contract_id),
            (caller, env.ledger().timestamp()),
        );
        true
    }

    /// Resolve a disputed escrow. Only the assigned arbiter may call this.
    pub fn resolve_dispute(
        env: Env,
        contract_id: u32,
        arbiter: Address,
        resolution: DisputeResolution,
    ) -> bool {
        Self::require_not_paused(&env);
        arbiter.require_auth();

        let key = DataKey::Contract(contract_id);
        let mut contract = env
            .storage()
            .persistent()
            .get::<_, Contract>(&key)
            .unwrap_or_else(|| env.panic_with_error(Error::ContractNotFound));

        if contract.status != ContractStatus::Disputed {
            env.panic_with_error(Error::InvalidState);
        }
        if contract.arbiter.clone() != Some(arbiter.clone()) {
            env.panic_with_error(Error::UnauthorizedRole);
        }

        let payouts = resolution_payouts(&contract, &resolution)
            .unwrap_or_else(|err| env.panic_with_error(err));
        let client_payout = payouts.client_payout;
        let freelancer_payout = payouts.freelancer_payout;

        contract.refunded_amount = safe_add_amounts(contract.refunded_amount, client_payout)
            .unwrap_or_else(|| env.panic_with_error(Error::PotentialOverflow));
        contract.released_amount = safe_add_amounts(contract.released_amount, freelancer_payout)
            .unwrap_or_else(|| env.panic_with_error(Error::PotentialOverflow));

        if safe_add_amounts(contract.released_amount, contract.refunded_amount)
            != Some(contract.funded_amount)
        {
            env.panic_with_error(Error::AccountingInvariantViolated);
        }

        contract.status = final_status_after_resolution(&contract);
        env.storage().persistent().set(&key, &contract);

        env.events().publish(
            (symbol_short!("dsp_res"), contract_id),
            (
                arbiter,
                resolution.code(),
                client_payout,
                freelancer_payout,
                env.ledger().timestamp(),
            ),
        );
        true
    }
}

/// Persist current-layout dispute metadata and stamp the version marker.
pub fn store_dispute_metadata(env: &Env, contract_id: u32, meta: &DisputeMetadata) {
    let mut stored = meta.clone();
    stored.schema_version = DISPUTE_STORAGE_VERSION;
    env.storage()
        .persistent()
        .set(&dispute_key(contract_id), &stored);
    env.storage()
        .persistent()
        .set(&dispute_version_key(contract_id), &DISPUTE_STORAGE_VERSION);
}

/// Remove dispute metadata and its version marker (called on successful resolve).
pub fn remove_dispute_metadata(env: &Env, contract_id: u32) {
    let data_key = dispute_key(contract_id);
    let version_key = dispute_version_key(contract_id);
    if env.storage().persistent().has(&data_key) {
        env.storage().persistent().remove(&data_key);
    }
    if env.storage().persistent().has(&version_key) {
        env.storage().persistent().remove(&version_key);
    }
}

fn synthesize_legacy_dispute_metadata(env: &Env, contract: &Contract) -> DisputeMetadata {
    DisputeMetadata {
        schema_version: DISPUTE_STORAGE_VERSION,
        // Pre-metadata disputes only recorded the disputed status; preserve a
        // deterministic party reference so accounting identity is not lost.
        raised_by: contract.client.clone(),
        reason_hash: BytesN::from_array(env, &[0u8; 32]),
        raised_at: 0,
    }
}

/// Load dispute metadata, upgrading older layouts on read.
///
/// - **Current version:** returns the stored record unchanged (no-op).
/// - **v0:** decodes [`DisputeMetadataV0`], migrates to v1, rewrites storage.
/// - **Legacy status-only:** when the contract is `Disputed` but no dispute
///   record exists, synthesizes a v1 record and persists it.
///
/// Preserves `raised_by`, `reason_hash`, and `raised_at` across v0 → v1.
pub fn load_dispute_metadata(env: &Env, contract_id: u32) -> DisputeMetadata {
    let version = get_dispute_storage_version(env, contract_id);
    let data_key = dispute_key(contract_id);

    if version == DISPUTE_STORAGE_VERSION {
        return env
            .storage()
            .persistent()
            .get::<_, DisputeMetadata>(&data_key)
            .unwrap_or_else(|| env.panic_with_error(EscrowError::DisputeNotFound));
    }

    if version == 0 {
        if let Some(v0) = env
            .storage()
            .persistent()
            .get::<_, DisputeMetadataV0>(&data_key)
        {
            let v1 = migrate_dispute_metadata_v0_to_v1(v0);
            store_dispute_metadata(env, contract_id, &v1);
            return v1;
        }

        let contract: Contract = env
            .storage()
            .persistent()
            .get(&DataKey::Contract(contract_id))
            .unwrap_or_else(|| env.panic_with_error(Error::ContractNotFound));

        if contract.status == ContractStatus::Disputed {
            let v1 = synthesize_legacy_dispute_metadata(env, &contract);
            store_dispute_metadata(env, contract_id, &v1);
            return v1;
        }

        env.panic_with_error(EscrowError::DisputeNotFound);
    }

    env.panic_with_error(EscrowError::UnsupportedDisputeStorageVersion);
}

/// Raise a dispute and persist versioned metadata under the current layout.
pub fn raise_dispute_impl(env: &Env, contract_id: u32, caller: Address) -> bool {
    Escrow::require_initialized(env);
    Escrow::require_not_paused(env);
    caller.require_auth();

    let mut contract: Contract = env
        .storage()
        .persistent()
        .get(&DataKey::Contract(contract_id))
        .unwrap_or_else(|| env.panic_with_error(Error::ContractNotFound));

    crate::ttl::extend_contract_ttl(env, contract_id);
    Escrow::require_not_finalized(env, contract_id);

    if caller != contract.client && caller != contract.freelancer {
        env.panic_with_error(Error::UnauthorizedRole);
    }
    if contract.arbiter.is_none() {
        env.panic_with_error(Error::ArbiterRequired);
    }
    match contract.status {
        ContractStatus::Funded | ContractStatus::PartiallyFunded => {}
        _ => env.panic_with_error(Error::InvalidState),
    }

    contract.status = ContractStatus::Disputed;
    env.storage()
        .persistent()
        .set(&DataKey::Contract(contract_id), &contract);

    let meta = DisputeMetadata {
        schema_version: DISPUTE_STORAGE_VERSION,
        raised_by: caller.clone(),
        reason_hash: BytesN::from_array(env, &[0u8; 32]),
        raised_at: env.ledger().timestamp(),
    };
    store_dispute_metadata(env, contract_id, &meta);

    crate::ttl::extend_contract_ttl(env, contract_id);

    env.events().publish(
        (symbol_short!("dispute"), symbol_short!("opened")),
        (contract_id, caller),
    );

    true
}

/// Resolve a dispute after ensuring metadata is present (migrating if needed).
pub fn resolve_dispute_impl(
    env: &Env,
    contract_id: u32,
    arbiter: Address,
    resolution: DisputeResolution,
) -> bool {
    Escrow::require_initialized(env);
    Escrow::require_not_paused(env);
    arbiter.require_auth();

    let mut contract: Contract = env
        .storage()
        .persistent()
        .get(&DataKey::Contract(contract_id))
        .unwrap_or_else(|| env.panic_with_error(Error::ContractNotFound));

    crate::ttl::extend_contract_ttl(env, contract_id);
    Escrow::require_not_finalized(env, contract_id);

    if contract.status != ContractStatus::Disputed {
        env.panic_with_error(Error::InvalidStatusTransition);
    }
    match &contract.arbiter {
        Some(contract_arbiter) if *contract_arbiter == arbiter => {}
        _ => env.panic_with_error(Error::UnauthorizedRole),
    }

    // Migrate-on-read / validate dispute metadata exists before mutating funds.
    let _meta = load_dispute_metadata(env, contract_id);

    let (client_payout, freelancer_payout) =
        resolution_payouts(&contract, &resolution).unwrap_or_else(|e| env.panic_with_error(e));

    contract.refunded_amount = safe_add_amounts(contract.refunded_amount, client_payout)
        .unwrap_or_else(|| env.panic_with_error(Error::PotentialOverflow));
    contract.released_amount = safe_add_amounts(contract.released_amount, freelancer_payout)
        .unwrap_or_else(|| env.panic_with_error(Error::PotentialOverflow));

    if safe_add_amounts(contract.released_amount, contract.refunded_amount)
        != Some(contract.funded_amount)
    {
        env.panic_with_error(Error::AccountingInvariantViolated);
    }

    contract.status = final_status_after_resolution(&contract);
    if contract.status == ContractStatus::Completed {
        Escrow::grant_pending_reputation_credit(env, &contract.freelancer);
    }
    env.storage()
        .persistent()
        .set(&DataKey::Contract(contract_id), &contract);

    remove_dispute_metadata(env, contract_id);
    crate::ttl::extend_contract_ttl(env, contract_id);

    env.events().publish(
        (symbol_short!("dispute"), symbol_short!("resolved")),
        (contract_id, resolution.code()),
    );

    true
}

/// Public read entrypoint helper: returns migrated dispute metadata.
pub fn get_dispute_impl(env: &Env, contract_id: u32) -> DisputeMetadata {
    load_dispute_metadata(env, contract_id)
}
