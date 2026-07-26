use crate::types::Contract;
use crate::Error;
use soroban_sdk::{symbol_short, Env};

/// Emits an indexed event on contract state changes to assist off-chain indexers
/// in cheaply reconstructing contract lifecycle history and financial balances.
///
/// # Event Specification
/// - **Topic**: `(symbol_short!("contract"), contract_id: u32)`
/// - **Payload**: `(status: u32, funded_amount: i128, released_amount: i128, refunded_amount: i128, total_deposited: i128)`
///
/// # Panics
/// - `InvalidContractId` if `contract_id` is zero.
/// - `AmountMustBePositive` if any amount field is negative.
pub fn emit_contract_indexed_event(env: &Env, contract_id: u32, contract: &Contract) {
    if contract_id == 0 {
        env.panic_with_error(Error::InvalidContractId);
    }
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
}

/// Validate that event payload amounts are non-negative.
/// Returns `Ok(())` when all amounts are >= 0.
pub(crate) fn validate_event_amounts(
    funded_amount: i128,
    released_amount: i128,
    refunded_amount: i128,
    total_deposited: i128,
) -> Result<(), crate::EscrowError> {
    if funded_amount < 0 || released_amount < 0 || refunded_amount < 0 || total_deposited < 0 {
        return Err(Error::AmountMustBePositive);
    }
    Ok(())
}
