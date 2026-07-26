// Merged imports

use crate::{
    safe_add_amounts, Contract, ContractStatus, DataKey, Escrow, EscrowArgs, EscrowClient,
    EscrowError,
};
use soroban_sdk::{contractimpl, symbol_short, Address, Env, Symbol};

// Removed obsolete duplicated `impl Escrow`

/// Resolution selected by the assigned arbiter for a disputed escrow.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DisputeResolution {
    /// Refund all remaining escrowed funds to the client.
    FullRefund,
    /// Refund 70% of the remaining balance to the client and release 30% to the freelancer.
    PartialRefund,
    /// Release all remaining escrowed funds to the freelancer.
    FullPayout,
    /// Apply a custom split of the remaining balance.
    Split(i128, i128),
}

// Removed another obsolete copied chunk

impl DisputeResolution {
    pub fn code(&self) -> u32 {
        match self {
            Self::FullRefund => 0,
            Self::PartialRefund => 1,
            Self::FullPayout => 2,
            Self::Split(_, _) => 3,
        }
    }
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
            let freelancer_payout = available
                .checked_mul(30)
                .and_then(|value| value.checked_div(100))
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
            let total = safe_add_amounts(*client_amount, *freelancer_amount)
                .ok_or(Error::PotentialOverflow)?;
            if total != available {
                return Err(Error::InvalidDisputeSplit);
            }
            Ok(DisputePayouts {
                client_payout: *client_amount,
                freelancer_payout: *freelancer_amount,
            })
        }
    }
}

#[allow(dead_code)]
pub fn final_status_after_resolution(contract: &Contract) -> ContractStatus {
    if contract.refunded_amount == contract.funded_amount {
        ContractStatus::Refunded
    } else {
        ContractStatus::Completed
    }
}

#[contractimpl]
impl Escrow {
    /// Raise a dispute on a funded or partially funded escrow.
    /// Only the client or freelancer may call this.
    pub fn raise_dispute(env: Env, contract_id: u32, caller: Address) -> bool {
        Self::require_not_paused(&env);
        caller.require_auth();

        let key = DataKey::Contract(contract_id);
        let mut contract = env
            .storage()
            .persistent()
            .get::<_, Contract>(&key)
            .unwrap_or_else(|| env.panic_with_error(Error::ContractNotFound));

        if caller != contract.client && caller != contract.freelancer {
            env.panic_with_error(Error::UnauthorizedRole);
        }
        if contract.arbiter.is_none() {
            env.panic_with_error(Error::ArbiterRequired);
        }
        if contract.status != ContractStatus::Funded
            && contract.status != ContractStatus::PartiallyFunded
        {
            env.panic_with_error(Error::InvalidState);
        }

        contract.status = ContractStatus::Disputed;
        env.storage().persistent().set(&key, &contract);

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
