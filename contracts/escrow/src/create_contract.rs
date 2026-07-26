pub use crate::Escrow;
use crate::{
    amount_validation, ttl, Contract, ContractStatus, DataKey, Error, Escrow, EscrowArgs,
    EscrowClient, EscrowError, GovernedParameters, Milestone, MilestoneSchedule,
    ReleaseAuthorization, MAX_MILESTONES, MAX_SCHEDULE_DESCRIPTION_LEN, MAX_SCHEDULE_TITLE_LEN,
};
use soroban_sdk::{symbol_short, Address, Env, Symbol, Vec};

impl Escrow {
    /// Creates a new escrow contract with the specified client, freelancer, and milestone amounts.
    ///
    /// This is the single canonical creation path. It enforces:
    /// - Distinct client and freelancer addresses
    /// - Arbiter presence when required by the release authorization mode
    /// - Arbiter distinctness from client and freelancer
    /// - At least one milestone with all amounts strictly positive
    /// - The `MAX_MILESTONES` cap
    /// - The governed total-escrow cap (falls back to `i128::MAX` when unset)
    /// - No contract-id collision or overflow
    ///
    /// # Arguments
    /// * `env` - The contract environment
    /// * `client` - The address of the client funding the contract
    /// * `freelancer` - The address of the freelancer performing the work
    /// * `arbiter` - Optional arbiter address for dispute resolution
    /// * `milestones` - Vector of milestone amounts (in stroops)
    /// * `release_authorization` - Authorization mode for milestone releases
    ///
    /// # Returns
    /// The unique contract ID assigned to the new escrow.
    ///
    /// # Errors
    /// * `InvalidParticipant`   - If client and freelancer are the same address
    /// * `EmptyMilestones`      - If no milestones are provided
    /// * `InvalidMilestoneAmount` - If any milestone amount is <= 0
    /// * `MissingArbiter`       - If arbiter is required but not provided
    /// * `InvalidArbiter`       - If arbiter is same as client or freelancer
    /// * `TooManyMilestones`    - If the number of milestones exceeds `MAX_MILESTONES`
    /// * `TotalCapExceeded`     - If the sum of milestone amounts exceeds the governed cap
    /// * `ContractIdOverflow`   - If the next id would exceed `u32::MAX`
    /// * `ContractIdCollision`  - If the allocated id slot is already occupied
    /// # Examples
    /// ```rust,ignore
    /// let client = EscrowClient::new(&env, &contract_id);
    /// let milestones = soroban_sdk::vec![&env, 500_0000000];
    /// let id = client.create_contract(&client_addr, &freelancer_addr, &None, &milestones, &ReleaseAuthorization::ClientOnly);
    /// assert_eq!(id, 1);
    /// ```
    pub fn create_contract(
        env: Env,
        client: Address,
        freelancer: Address,
        arbiter: Option<Address>,
        milestones: Vec<i128>,
        release_authorization: ReleaseAuthorization,
    ) -> u32 {
        Escrow::require_not_paused(&env);
        client.require_auth();

        if client == freelancer {
            env.panic_with_error(EscrowError::InvalidParticipant);
        }

        match release_authorization {
            ReleaseAuthorization::ArbiterOnly | ReleaseAuthorization::ClientAndArbiter
                if arbiter.is_none() =>
            {
                env.panic_with_error(EscrowError::MissingArbiter);
            }
            _ => {}
        }

        if let Some(ref arb) = arbiter {
            if arb == &client || arb == &freelancer {
                env.panic_with_error(EscrowError::InvalidArbiter);
            }
        }

        if milestones.is_empty() {
            env.panic_with_error(EscrowError::EmptyMilestones);
        }

        if milestones.len() > MAX_MILESTONES {
            env.panic_with_error(EscrowError::TooManyMilestones);
        }

        let max_total = env
            .storage()
            .persistent()
            .get::<_, GovernedParameters>(&DataKey::GovernedParameters)
            .map(|params| params.max_escrow_total_stroops)
            .unwrap_or(i128::MAX);

        let mut native_milestones = [0_i128; MAX_MILESTONES as usize];
        let len = milestones.len() as usize;
        for i in 0..len {
            native_milestones[i] = milestones.get(i as u32).unwrap();
        }
        match amount_validation::validate_milestone_amounts(&native_milestones[..len], max_total) {
            Ok(_) => (),
            Err(err) => match err {
                EscrowError::InvalidMilestoneAmount => {
                    env.panic_with_error(EscrowError::InvalidMilestoneAmount)
                }
                EscrowError::TotalCapExceeded => {
                    env.panic_with_error(EscrowError::TotalCapExceeded)
                }
                _ => env.panic_with_error(EscrowError::InvalidMilestoneAmount),
            },
        }

        ttl::extend_next_contract_id_ttl(&env);

        let id = Escrow::next_contract_id(&env);

        let contract = Contract {
            client: client.clone(),
            freelancer: freelancer.clone(),
            arbiter,
            status: ContractStatus::Created,
            total_deposited: 0,
            funded_amount: 0,
            released_amount: 0,
            refunded_amount: 0,
            release_authorization,
            reputation_issued: false,
        };
        env.storage()
            .persistent()
            .get(&DataKey::NextContractId)
            .unwrap_or(1);

        let mut milestone_vec: Vec<Milestone> = Vec::new(&env);
        for amount in milestones.iter() {
            milestone_vec.push_back(Milestone {
                amount,
                funded_amount: 0,
                protocol_fee: 0,
                released: false,
                refunded: false,
                work_evidence: None,
                refunded_amount: 0,
                deadline: None,
            });
        }

        let next_id = id
            .checked_add(1)
            .unwrap_or_else(|| env.panic_with_error(EscrowError::ContractIdOverflow));
        env.storage()
            .persistent()
            .set(&DataKey::NextContractId, &next_id);

        env.events().publish(
            (symbol_short!("created"), id),
            (client, freelancer, env.ledger().timestamp()),
        );

        // Emit indexed event carrying state & balances.
        crate::events::emit_contract_indexed_event(&env, id, &contract);

        id
    }

    /// Creates a new escrow contract with per-milestone schedule metadata.
    ///
    /// Accepts the same parameters as [`create_contract`] plus a `schedules` vector
    /// that carries optional due-date, title, and description for each milestone.
    ///
    /// * `schedules` — Length must match `milestones`. Each entry's `due_date`
    ///   must be strictly in the future and strictly increasing (skipping `None`
    ///   entries). `title` and `description` are bounded by
    ///   [`MAX_SCHEDULE_TITLE_LEN`] and [`MAX_SCHEDULE_DESCRIPTION_LEN`].
    ///   Pass an empty vec when no schedule metadata is needed.
    pub fn create_contract_with_schedules(
        env: Env,
        client: Address,
        freelancer: Address,
        arbiter: Option<Address>,
        milestones: Vec<i128>,
        release_authorization: ReleaseAuthorization,
        schedules: Vec<Option<MilestoneSchedule>>,
    ) -> u32 {
        // Delegate to the base creation logic.
        let id = Self::create_contract(
            env.clone(),
            client,
            freelancer,
            arbiter,
            milestones.clone(),
            release_authorization,
        );

        // Validate and persist milestone schedule metadata.
        if schedules.len() > 0 {
            if schedules.len() != milestones.len() {
                env.panic_with_error(Error::InvalidScheduleMetadata);
            }
            let now = env.ledger().timestamp();
            let mut prev_due: Option<u64> = None;
            for i in 0..schedules.len() {
                if let Some(ref sched) = schedules.get(i) {
                    if let Some(due) = sched.due_date {
                        if due <= now {
                            env.panic_with_error(Error::InvalidScheduleMetadata);
                        }
                        if let Some(prev) = prev_due {
                            if due <= prev {
                                env.panic_with_error(Error::InvalidScheduleMetadata);
                            }
                        }
                        prev_due = Some(due);
                    }
                    if let Some(ref title) = sched.title {
                        if title.len() > MAX_SCHEDULE_TITLE_LEN as u32 {
                            env.panic_with_error(Error::InvalidScheduleMetadata);
                        }
                    }
                    if let Some(ref desc) = sched.description {
                        if desc.len() > MAX_SCHEDULE_DESCRIPTION_LEN as u32 {
                            env.panic_with_error(Error::InvalidScheduleMetadata);
                        }
                    }
                }
            }
            // Store schedules keyed by contract id.
            let schedule_key = Symbol::new(&env, "schedule");
            let mut stored_schedules: Vec<Option<MilestoneSchedule>> = Vec::new(&env);
            for i in 0..schedules.len() {
                let mut entry = schedules.get(i);
                if let Some(ref mut s) = entry {
                    s.updated_at = now;
                }
                stored_schedules.push_back(entry);
            }
            env.storage()
                .persistent()
                .set(&(DataKey::Contract(id), schedule_key), &stored_schedules);
        }

        id
    }
}

/// Returns the next available contract ID and asserts it is not already occupied.
///
/// # Errors
/// * `ContractIdCollision` - If the allocated id slot is already occupied
pub(crate) fn next_contract_id(env: &Env) -> u32 {
    let id: u32 = env
        .storage()
        .persistent()
        .get(&DataKey::NextContractId)
        .unwrap_or(INITIAL_CONTRACT_ID);

    if env
        .storage()
        .persistent()
        .get::<_, Contract>(&DataKey::Contract(id))
        .is_some()
    {
        env.panic_with_error(Error::ContractIdCollision);
    }

    id
}
