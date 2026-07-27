use crate::types::{Contract, EventEntry};
use crate::DataKey;
use soroban_sdk::{symbol_short, Env};

/// Emits an indexed event on contract state changes to assist off-chain indexers
/// in cheaply reconstructing contract lifecycle history and financial balances.
///
/// # Event Specification
/// - **Topic**: `(symbol_short!("contract"), contract_id: u32)`
/// - **Payload**: `(status: u32, funded_amount: i128, released_amount: i128, refunded_amount: i128, total_deposited: i128)`
///
/// # Storage side-effect
/// Each call persists a [`EventEntry`] record under `DataKey::Event(next_id)`
/// so that off-chain callers can enumerate the event history via
/// [`crate::Escrow::get_events_page`].
pub fn emit_contract_indexed_event(env: &Env, contract_id: u32, contract: &Contract) {
    env.events().publish(
        (symbol_short!("contract"), contract_id),
        (
            contract.status as u32,
            contract.funded_amount,
            contract.released_amount,
            contract.refunded_amount,
            contract.total_deposited,
        ),
    );

    let next_id: u32 = env
        .storage()
        .persistent()
        .get(&DataKey::NextEventId)
        .unwrap_or(0);

    let entry = EventEntry {
        contract_id,
        status: contract.status as u32,
        funded_amount: contract.funded_amount,
        released_amount: contract.released_amount,
        refunded_amount: contract.refunded_amount,
        total_deposited: contract.total_deposited,
    };

    env.storage()
        .persistent()
        .set(&DataKey::Event(next_id), &entry);

    env.storage()
        .persistent()
        .set(&DataKey::NextEventId, &(next_id + 1));
}
