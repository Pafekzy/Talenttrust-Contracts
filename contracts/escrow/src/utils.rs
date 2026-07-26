use soroban_sdk::Env;

/// Returns the current ledger timestamp in seconds (Unix epoch).
///
/// This is the **single source of truth** for all time-related operations in the
/// contract.  Every entrypoint that needs the current time must call this helper;
/// direct `env.ledger().timestamp()` calls outside this module are forbidden.
///
/// # Precision and trust assumptions
///
/// Ledger timestamps are set by Stellar validator nodes when they close each
/// ledger (roughly every ~5 seconds).  The timestamp embedded in a closed ledger
/// is **consensus-driven** — no single party can manipulate it — but it reflects
/// the validator's wall clock, not a globally synchronised atomic clock.
///
/// **Do not use this for fine-grained deadlines.**  The effective resolution is
/// one ledger (~5 s) and there is no guarantee that a given second value has
/// appeared in any particular ledger.  Off-by-one-ledger variation is normal.
/// Deadlines expressed in *minutes* or *hours* are safe; deadlines shorter than
/// ~30 seconds risk non-deterministic behaviour across validators.
///
/// # Call sites
///
/// | Entrypoint / module | How `now_seconds` is used |
/// | --- | --- |
/// | [`is_milestone_overdue`](crate::Escrow::is_milestone_overdue) | Compares `now_seconds(&env) > deadline` (strictly greater) to determine timeout-refund eligibility |
/// | Event publishers (e.g. `release_milestone`, `refund_unreleased_milestones`) | Stamp events with `env.ledger().timestamp()` for off-chain indexing |
///
/// The admin-rotation timelock ([`governance.rs`](crate::governance)) and
/// migration expiry ([`migration.rs`](crate::migration)) use **ledger-sequence
/// counts** (`env.ledger().sequence()`), not wall-clock timestamps, because those
/// mechanisms measure elapsed ledgers rather than absolute time.
///
/// # Example — milestone overdue check
///
/// ```ignore
/// use crate::utils::now_seconds;
///
/// // Returns true only when now > deadline (strictly greater).
/// // At exactly the deadline (now == deadline) it returns false — the
/// // milestone is NOT overdue yet, preventing a one-second-early refund.
/// pub fn is_milestone_overdue(env: &Env, deadline: u64) -> bool {
///     now_seconds(env) > deadline
/// }
/// ```
///
/// # Testing — deterministic time control
///
/// In tests, advance the ledger timestamp with `env.ledger().with_mut()` so that
/// `now_seconds` returns a predictable value.  This is how `contracts/escrow/src/test/timeout_tests.rs`
/// exercises deadline boundaries:
///
/// ```ignore
/// use soroban_sdk::testutils::Ledger;
///
/// fn set_now(env: &Env, secs: u64) {
///     env.ledger().with_mut(|li| {
///         li.timestamp = secs;
///     });
/// }
///
/// // Example: prove the strict-inequality boundary at the deadline.
/// set_now(&env, deadline);
/// assert!(!is_milestone_overdue(&env, deadline));  // now == deadline -> false
/// set_now(&env, deadline + 1);
/// assert!(is_milestone_overdue(&env, deadline));   // now > deadline -> true
/// ```
pub fn now_seconds(env: &Env) -> u64 {
    env.ledger().timestamp()
}
