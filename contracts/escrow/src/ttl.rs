//! Deterministic TTL / expiration policy for transient and persistent storage.

use crate::{DataKey, EscrowError, Milestone};
use soroban_sdk::{Env, IntoVal, Symbol, TryFromVal, Val, Vec};

pub const LEDGERS_PER_DAY: u32 = 17_280;
pub const PENDING_APPROVAL_TTL_LEDGERS: u32 = LEDGERS_PER_DAY * 7;
pub const PENDING_APPROVAL_BUMP_THRESHOLD: u32 = LEDGERS_PER_DAY;
/// Minimum TTL for a milestone approval entry (1 day).
///
/// This is the shortest lifetime we assign to a temporary approval. After this
/// many ledgers without a bump the entry is eligible for eviction by the host.
pub const MIN_APPROVAL_TTL: u32 = LEDGERS_PER_DAY;

/// Minimum ledgers that must elapse between proposing and finalising a
/// treasury / admin rotation. At ~5 s per ledger this is roughly 2 days,
/// giving stakeholders time to react to an unexpected proposal.
pub const ADMIN_ROTATION_MIN_DELAY_LEDGERS: u32 = LEDGERS_PER_DAY * 2;
pub const PENDING_MIGRATION_TTL_LEDGERS: u32 = LEDGERS_PER_DAY * 21;
pub const PENDING_MIGRATION_BUMP_THRESHOLD: u32 = LEDGERS_PER_DAY * 3;
pub const PERSISTENT_TTL_LEDGERS: u32 = LEDGERS_PER_DAY * 30;
pub const PERSISTENT_BUMP_THRESHOLD: u32 = LEDGERS_PER_DAY * 7;

pub fn compute_expiry(env: &Env, ttl_ledgers: u32) -> u32 {
    env.ledger().sequence().saturating_add(ttl_ledgers)
}

pub fn store_with_ttl<K, V>(env: &Env, key: &K, value: &V, ttl_ledgers: u32)
where
    K: IntoVal<Env, Val>,
    V: IntoVal<Env, Val>,
{
    let storage = env.storage().temporary();
    storage.set(key, value);
    storage.extend_ttl(key, ttl_ledgers, ttl_ledgers);
}

pub fn read_if_live<K, V>(env: &Env, key: &K) -> Option<V>
where
    K: IntoVal<Env, Val>,
    V: TryFromVal<Env, Val>,
{
    env.storage().temporary().get(key)
}

pub fn remove_transient<K>(env: &Env, key: &K)
where
    K: IntoVal<Env, Val>,
{
    env.storage().temporary().remove(key);
}

pub fn load_milestones(env: &Env, contract_id: u32) -> Vec<Milestone> {
    let key = MilestonesKey::new(contract_id);
    let milestones: Vec<Milestone> = env
        .storage()
        .persistent()
        .get(&key)
        .unwrap_or_else(|| env.panic_with_error(EscrowError::ContractNotFound));
    extend_milestone_ttl(env, contract_id);
    milestones
}

pub fn store_milestones(env: &Env, contract_id: u32, milestones: &Vec<Milestone>) {
    let key = milestone_storage_key(env, contract_id);
    let milestones: Option<Vec<Milestone>> = env.storage().persistent().get(&key);
    if milestones.is_some() {
        extend_milestone_ttl(env, contract_id);
    }
    milestones
}

/// Persists `milestones` for `contract_id` under the canonical composite
/// key and bumps the persistent TTL.
///
/// This is the **single canonical write path** for milestone vectors.
/// Every entrypoint that mutates milestone state (e.g. `release_milestone`,
/// `refund_unreleased_milestones`, `submit_work_evidence`, approval flows,
/// creation) must funnel through this helper so the lives of three
/// concerns stay in lock-step:
///
/// 1. **Composite key** — built once via [`milestone_storage_key`].
/// 2. **Atomic write + TTL bump** — the TTL is bumped in the same
///    logical step as the write, so a freshly-stored vector cannot be
///    archived in the same ledger window.
/// 3. **Bump parameters** — `PERSISTENT_BUMP_THRESHOLD` /
///    `PERSISTENT_TTL_LEDGERS`, identical to the read path's bump.
///
/// # Arguments
/// * `env`         - The contract environment.
/// * `contract_id` - The `u32` identifier previously allocated by
///                    [`crate::create_contract`].
/// * `milestones`  - The new vector to persist.
///
/// # See also
/// - [`load_milestones`] — the symmetric read path.
pub fn store_milestones(env: &Env, contract_id: u32, milestones: &Vec<Milestone>) {
    let key = MilestonesKey::new(contract_id);
    env.storage().persistent().set(&key, milestones);
    extend_milestone_ttl(env, contract_id);
}

pub(crate) fn milestone_storage_key(_env: &Env, contract_id: u32) -> DataKey {
    DataKey::Milestones(contract_id)
}

pub fn extend_contract_ttl(env: &Env, contract_id: u32) {
    env.storage().persistent().extend_ttl(
        &DataKey::Contract(contract_id),
        PERSISTENT_BUMP_THRESHOLD,
        PERSISTENT_TTL_LEDGERS,
    );
}

pub fn extend_milestone_ttl(env: &Env, contract_id: u32) {
    env.storage().persistent().extend_ttl(
        &MilestonesKey::new(contract_id),
        PERSISTENT_BUMP_THRESHOLD,
        PERSISTENT_TTL_LEDGERS,
    );
}

pub fn extend_contract_and_milestones_ttl(env: &Env, contract_id: u32) {
    extend_contract_ttl(env, contract_id);
    extend_milestone_ttl(env, contract_id);
}

pub fn extend_next_contract_id_ttl(env: &Env) {
    let key = DataKey::NextContractId;
    env.storage().persistent().extend_ttl(&key, 0, 100);
}