use super::EscrowFixture;
use soroban_sdk::{testutils::Address as _, token::StellarAssetClient, vec, Address, Env, String};

#[derive(Clone, Copy)]
struct ResourceBaseline {
    max_instructions: i64,
    max_mem_bytes: i64,
    max_read_entries: u32,
    max_write_entries: u32,
    max_read_bytes: u32,
    max_write_bytes: u32,
    max_fee_total: i64,
}

#[derive(Clone, Copy)]
struct MeasuredResources {
    instructions: i64,
    mem_bytes: i64,
    read_entries: u32,
    write_entries: u32,
    read_bytes: u32,
    write_bytes: u32,
}

const CREATE_CONTRACT_BASELINE: ResourceBaseline = ResourceBaseline {
    max_instructions: 15_000_000,
    max_mem_bytes: 1_500_000,
    max_read_entries: 8,
    max_write_entries: 6,
    max_read_bytes: 8_192,
    max_write_bytes: 24_576,
    max_fee_total: 3_000_000,
};

const DEPOSIT_FUNDS_BASELINE: ResourceBaseline = ResourceBaseline {
    max_instructions: 15_000_000,
    max_mem_bytes: 1_500_000,
    max_read_entries: 12,
    max_write_entries: 6,
    max_read_bytes: 8_192,
    max_write_bytes: 24_576,
    max_fee_total: 6_000_000,
};

const RELEASE_MILESTONE_BASELINE: ResourceBaseline = ResourceBaseline {
    max_instructions: 15_000_000,
    max_mem_bytes: 1_500_000,
    max_read_entries: 14,
    max_write_entries: 6,
    max_read_bytes: 8_192,
    max_write_bytes: 24_576,
    max_fee_total: 3_000_000,
};

const REFUND_BASELINE: ResourceBaseline = ResourceBaseline {
    max_instructions: 15_000_000,
    max_mem_bytes: 1_500_000,
    max_read_entries: 10,
    max_write_entries: 6,
    max_read_bytes: 8_192,
    max_write_bytes: 24_576,
    max_fee_total: 3_000_000,
};

const CANCEL_BASELINE: ResourceBaseline = ResourceBaseline {
    max_instructions: 15_000_000,
    max_mem_bytes: 1_500_000,
    max_read_entries: 8,
    max_write_entries: 6,
    max_read_bytes: 8_192,
    max_write_bytes: 24_576,
    max_fee_total: 3_000_000,
};

const DISPUTE_BASELINE: ResourceBaseline = ResourceBaseline {
    max_instructions: 15_000_000,
    max_mem_bytes: 1_500_000,
    max_read_entries: 10,
    max_write_entries: 6,
    max_read_bytes: 8_192,
    max_write_bytes: 24_576,
    max_fee_total: 3_000_000,
};

// ---------------------------------------------------------------------------
// Reputation resource-budget baselines
// ---------------------------------------------------------------------------
// Values are set generously for the initial commit.  If the CI runner reports
// stable numbers below these thresholds they should be tightened so that a
// meaningful regression always trips an assertion.

const ISSUE_REPUTATION_BASELINE: ResourceBaseline = ResourceBaseline {
    max_instructions: 15_000_000,
    max_mem_bytes: 1_500_000,
    max_read_entries: 6,
    max_write_entries: 6,
    max_read_bytes: 8_192,
    max_write_bytes: 24_576,
    max_fee_total: 3_000_000,
};

const GET_REPUTATION_BASELINE: ResourceBaseline = ResourceBaseline {
    max_instructions: 2_000_000,
    max_mem_bytes: 500_000,
    max_read_entries: 2,
    max_write_entries: 1,
    max_read_bytes: 4_096,
    max_write_bytes: 4_096,
    max_fee_total: 500_000,
};

const GET_AVERAGE_RATING_BASELINE: ResourceBaseline = ResourceBaseline {
    max_instructions: 3_000_000,
    max_mem_bytes: 500_000,
    max_read_entries: 2,
    max_write_entries: 1,
    max_read_bytes: 4_096,
    max_write_bytes: 4_096,
    max_fee_total: 500_000,
};

const GET_REPUTATION_COMMENT_BASELINE: ResourceBaseline = ResourceBaseline {
    max_instructions: 3_000_000,
    max_mem_bytes: 500_000,
    max_read_entries: 2,
    max_write_entries: 1,
    max_read_bytes: 4_096,
    max_write_bytes: 4_096,
    max_fee_total: 800_000,
};

const GET_PENDING_REPUTATION_CREDITS_BASELINE: ResourceBaseline = ResourceBaseline {
    max_instructions: 2_000_000,
    max_mem_bytes: 500_000,
    max_read_entries: 4,
    max_write_entries: 2,
    max_read_bytes: 4_096,
    max_write_bytes: 4_096,
    max_fee_total: 500_000,
};

fn valid_comment(env: &Env) -> String {
    String::from_str(env, "Great job!")
}

/// Complete a fully-funded fixture by approving and releasing all three
/// milestones, transitioning the contract to `Completed`.
fn complete_fixture(fixture: &EscrowFixture) {
    let escrow = fixture.escrow();
    for i in 0..3u32 {
        escrow.approve_milestone_release(&fixture.escrow_id, &fixture.client, &i);
        escrow.release_milestone(&fixture.escrow_id, &fixture.client, &i);
    }
}

fn measure_last_invocation(env: &Env) -> (MeasuredResources, i64) {
    let resources = env.cost_estimate().resources();
    let fee = env.cost_estimate().fee();

    (
        MeasuredResources {
            instructions: resources.instructions,
            mem_bytes: resources.mem_bytes,
            read_entries: resources.read_entries,
            write_entries: resources.write_entries,
            read_bytes: resources.read_bytes,
            write_bytes: resources.write_bytes,
        },
        fee.total,
    )
}

fn assert_within_baseline(
    label: &str,
    resources: MeasuredResources,
    fee_total: i64,
    baseline: ResourceBaseline,
) {
    assert!(
        resources.instructions <= baseline.max_instructions,
        "{} instruction regression: {} > {}",
        label,
        resources.instructions,
        baseline.max_instructions
    );
    assert!(
        resources.mem_bytes <= baseline.max_mem_bytes,
        "{} memory regression: {} > {}",
        label,
        resources.mem_bytes,
        baseline.max_mem_bytes
    );
    assert!(
        resources.read_entries <= baseline.max_read_entries,
        "{} read-entry regression: {} > {}",
        label,
        resources.read_entries,
        baseline.max_read_entries
    );
    assert!(
        resources.write_entries <= baseline.max_write_entries,
        "{} write-entry regression: {} > {}",
        label,
        resources.write_entries,
        baseline.max_write_entries
    );
    assert!(
        resources.read_bytes <= baseline.max_read_bytes,
        "{} read-byte regression: {} > {}",
        label,
        resources.read_bytes,
        baseline.max_read_bytes
    );
    assert!(
        resources.write_bytes <= baseline.max_write_bytes,
        "{} write-byte regression: {} > {}",
        label,
        resources.write_bytes,
        baseline.max_write_bytes
    );
    assert!(
        fee_total <= baseline.max_fee_total,
        "{} fee regression: {} > {}",
        label,
        fee_total,
        baseline.max_fee_total
    );
}

#[test]
fn create_contract_resource_baseline() {
    let fixture = EscrowFixture::builder().build();
    let escrow = fixture.escrow();

    let client_addr = Address::generate(&fixture.env);
    let freelancer_addr = Address::generate(&fixture.env);
    let _ = escrow.create_contract(
        &client_addr,
        &freelancer_addr,
        &None,
        &super::default_milestones(&fixture.env),
        &crate::ReleaseAuthorization::ClientOnly,
    );

    let (resources, fee_total) = measure_last_invocation(&fixture.env);
    assert_within_baseline(
        "create_contract",
        resources,
        fee_total,
        CREATE_CONTRACT_BASELINE,
    );
}

#[test]
fn deposit_funds_resource_baseline() {
    let fixture = EscrowFixture::builder().with_settlement_token().build();
    let escrow = fixture.escrow();
    let token = fixture.settlement_token.as_ref().unwrap();
    let total = fixture.total_amount();

    StellarAssetClient::new(&fixture.env, token).mint(&fixture.client, &total);
    let _ = escrow.deposit_funds(&fixture.escrow_id, &fixture.client, &total);

    let (resources, fee_total) = measure_last_invocation(&fixture.env);
    assert_within_baseline(
        "deposit_funds",
        resources,
        fee_total,
        DEPOSIT_FUNDS_BASELINE,
    );
}

#[test]
fn release_milestone_resource_baseline() {
    let fixture = EscrowFixture::builder().funded().build();
    let escrow = fixture.escrow();

    escrow.approve_milestone_release(&fixture.escrow_id, &fixture.client, &0);
    let _ = escrow.release_milestone(&fixture.escrow_id, &fixture.client, &0);

    let (resources, fee_total) = measure_last_invocation(&fixture.env);
    assert_within_baseline(
        "release_milestone",
        resources,
        fee_total,
        RELEASE_MILESTONE_BASELINE,
    );
}

#[test]
fn refund_resource_baseline() {
    let fixture = EscrowFixture::builder().funded().build();
    let escrow = fixture.escrow();

    let _ =
        escrow.refund_unreleased_milestones(&fixture.escrow_id, &vec![&fixture.env, 0_u32, 1, 2]);

    let (resources, fee_total) = measure_last_invocation(&fixture.env);
    assert_within_baseline("refund", resources, fee_total, REFUND_BASELINE);
}

#[test]
fn cancel_resource_baseline() {
    let fixture = EscrowFixture::builder().build();
    let escrow = fixture.escrow();

    let _ = escrow.cancel_contract(&fixture.escrow_id, &fixture.client);

    let (resources, fee_total) = measure_last_invocation(&fixture.env);
    assert_within_baseline("cancel", resources, fee_total, CANCEL_BASELINE);
}

#[test]
fn dispute_resource_baseline() {
    let builder = EscrowFixture::builder();
    let client = Address::generate(builder.env());
    let freelancer = Address::generate(builder.env());
    let arbiter = Address::generate(builder.env());
    let fixture = builder
        .with_participants(client, freelancer, Some(arbiter))
        .with_settlement_token()
        .build();
    let escrow = fixture.escrow();
    let token = fixture.settlement_token.as_ref().unwrap();
    let total = fixture.total_amount();

    StellarAssetClient::new(&fixture.env, token).mint(&fixture.client, &total);
    escrow.deposit_funds(&fixture.escrow_id, &fixture.client, &total);
    let _ = escrow.raise_dispute(&fixture.escrow_id, &fixture.client);

    let (resources, fee_total) = measure_last_invocation(&fixture.env);
    assert_within_baseline("dispute", resources, fee_total, DISPUTE_BASELINE);
}

// ---------------------------------------------------------------------------
// Reputation resource-budget tests
// ---------------------------------------------------------------------------

#[test]
fn issue_reputation_resource_baseline() {
    let fixture = EscrowFixture::builder().funded().build();
    complete_fixture(&fixture);
    let escrow = fixture.escrow();

    let _ = escrow.issue_reputation(
        &fixture.escrow_id,
        &fixture.client,
        &5,
        &valid_comment(&fixture.env),
    );

    let (resources, fee_total) = measure_last_invocation(&fixture.env);
    assert_within_baseline(
        "issue_reputation",
        resources,
        fee_total,
        ISSUE_REPUTATION_BASELINE,
    );
}

#[test]
fn get_reputation_resource_baseline() {
    let fixture = EscrowFixture::builder().funded().build();
    complete_fixture(&fixture);
    let escrow = fixture.escrow();

    escrow.issue_reputation(
        &fixture.escrow_id,
        &fixture.client,
        &5,
        &valid_comment(&fixture.env),
    );

    let _ = escrow.get_reputation(&fixture.freelancer);

    let (resources, fee_total) = measure_last_invocation(&fixture.env);
    assert_within_baseline(
        "get_reputation",
        resources,
        fee_total,
        GET_REPUTATION_BASELINE,
    );
}

#[test]
fn get_average_rating_resource_baseline() {
    let fixture = EscrowFixture::builder().funded().build();
    complete_fixture(&fixture);
    let escrow = fixture.escrow();

    escrow.issue_reputation(
        &fixture.escrow_id,
        &fixture.client,
        &5,
        &valid_comment(&fixture.env),
    );

    let _ = escrow.get_average_rating(&fixture.freelancer);

    let (resources, fee_total) = measure_last_invocation(&fixture.env);
    assert_within_baseline(
        "get_average_rating",
        resources,
        fee_total,
        GET_AVERAGE_RATING_BASELINE,
    );
}

#[test]
fn get_reputation_comment_resource_baseline() {
    let fixture = EscrowFixture::builder().funded().build();
    complete_fixture(&fixture);
    let escrow = fixture.escrow();

    escrow.issue_reputation(
        &fixture.escrow_id,
        &fixture.client,
        &5,
        &valid_comment(&fixture.env),
    );

    let _ = escrow.get_reputation_comment(&fixture.escrow_id);

    let (resources, fee_total) = measure_last_invocation(&fixture.env);
    assert_within_baseline(
        "get_reputation_comment",
        resources,
        fee_total,
        GET_REPUTATION_COMMENT_BASELINE,
    );
}

#[test]
fn get_pending_reputation_credits_resource_baseline() {
    let fixture = EscrowFixture::builder().funded().build();
    complete_fixture(&fixture);

    let escrow = fixture.escrow();
    let _ = escrow.get_pending_reputation_credits(&fixture.freelancer);

    let (resources, fee_total) = measure_last_invocation(&fixture.env);
    assert_within_baseline(
        "get_pending_reputation_credits",
        resources,
        fee_total,
        GET_PENDING_REPUTATION_CREDITS_BASELINE,
    );
}
