use crate::EscrowError;

pub const MAX_SINGLE_AMOUNT_STROOPS: i128 = 100_000_000_000_000_000;

pub fn validate_single_amount(amount: i128) -> Result<(), EscrowError> {
    if amount <= 0 {
        return Err(EscrowError::AmountMustBePositive);
    }
    Ok(())
}

pub fn validate_amount_array(amounts: &[i128]) -> Result<i128, EscrowError> {
    let mut total = 0i128;
    for amount in amounts {
        validate_single_amount(*amount)?;
        total = total
            .checked_add(*amount)
            .ok_or(EscrowError::PotentialOverflow)?;
    }
    Ok(total)
}

pub fn validate_milestone_amounts(
    amounts: &[i128],
    max_total: i128,
) -> Result<(), EscrowError> {
    for amount in amounts {
        validate_single_amount(*amount)?;
    }
    let total = validate_amount_array(amounts)?;
    if total > max_total {
        return Err(EscrowError::TotalCapExceeded);
    }
    Ok(())
}

pub fn accumulate_amounts<I>(amounts: I) -> Result<i128, EscrowError>
where
    I: Iterator<Item = i128>,
{
    let mut total = 0i128;
    for amount in amounts {
        total = total
            .checked_add(amount)
            .ok_or(EscrowError::PotentialOverflow)?;
    }
    Ok(total)
}

pub fn safe_add_amounts(a: i128, b: i128) -> Option<i128> {
    a.checked_add(b)
}

pub fn safe_subtract_amounts(a: i128, b: i128) -> Option<i128> {
    a.checked_sub(b)
}

pub fn validate_deposit_amount(
    deposit_amount: i128,
    current_deposited: i128,
    max_total: i128,
) -> Result<(), EscrowError> {
    validate_single_amount(deposit_amount)?;
    let remaining = max_total
        .checked_sub(current_deposited)
        .ok_or(EscrowError::PotentialOverflow)?;
    if deposit_amount > remaining {
        return Err(EscrowError::InvalidMilestoneAmount);
    }
    Ok(())
}

pub fn checked_available_balance(
    funded_amount: i128,
    released_amount: i128,
    refunded_amount: i128,
) -> Result<i128, EscrowError> {
    let balance = funded_amount
        .checked_sub(released_amount)
        .ok_or(EscrowError::AccountingInvariantViolated)?;
    let balance = balance
        .checked_sub(refunded_amount)
        .ok_or(EscrowError::AccountingInvariantViolated)?;
    Ok(balance)
}

/// Computes available (unreleased, unrefunded) balance with checked arithmetic,
/// guarding against underflow at extreme values.
pub fn available_balance(funded: i128, released: i128, refunded: i128) -> Option<i128> {
    funded.checked_sub(released)?.checked_sub(refunded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_single_amount() {
        assert!(validate_single_amount(1).is_ok());
        assert!(validate_single_amount(100_0000000).is_ok());
        assert!(validate_single_amount(MAX_SINGLE_AMOUNT_STROOPS).is_ok());

        assert_eq!(
            validate_single_amount(0),
            Err(EscrowError::AmountMustBePositive)
        );
        assert_eq!(
            validate_single_amount(-1),
            Err(EscrowError::AmountMustBePositive)
        );
        assert_eq!(
            validate_single_amount(MAX_SINGLE_AMOUNT_STROOPS + 1),
            Err(EscrowError::InvalidMilestoneAmount)
        );
    }

    #[test]
    fn test_validate_amount_array() {
        let amounts1 = [100_0000000, 200_0000000, 300_0000000];
        assert!(validate_amount_array(&amounts1).is_ok());
        assert_eq!(validate_amount_array(&amounts1).unwrap(), 600_0000000);

        let amounts2 = [100_0000000, 0, 300_0000000];
        assert_eq!(
            validate_amount_array(&amounts2),
            Err(EscrowError::AmountMustBePositive)
        );

        let amounts3 = [100_0000000, -50_0000000, 300_0000000];
        assert_eq!(
            validate_amount_array(&amounts3),
            Err(EscrowError::AmountMustBePositive)
        );
    }

    #[test]
    fn test_validate_milestone_amounts() {
        let max_contract_total = 1_000_000_0000000;
        let milestones1 = [100_0000000, 200_0000000, 300_0000000];
        assert!(validate_milestone_amounts(&milestones1, max_contract_total).is_ok());
        let milestones2 = [500_000_0000000, 600_000_0000000];
        assert_eq!(
            validate_milestone_amounts(&milestones2, max_contract_total),
            Err(EscrowError::TotalCapExceeded)
        );
    }

    #[test]
    fn test_validate_deposit_amount() {
        assert!(validate_deposit_amount(100, 0, 1000).is_ok());
        assert!(validate_deposit_amount(500, 500, 1000).is_ok());
        assert_eq!(
            validate_deposit_amount(0, 0, 1000),
            Err(EscrowError::AmountMustBePositive)
        );
        assert_eq!(
            validate_deposit_amount(501, 500, 1000),
            Err(EscrowError::InvalidMilestoneAmount)
        );
    }

    #[test]
    fn test_safe_arithmetic() {
        assert_eq!(safe_add_amounts(100, 200), Some(300));
        assert_eq!(safe_add_amounts(i128::MAX, 1), None);
        assert_eq!(safe_subtract_amounts(300, 100), Some(200));
        assert_eq!(safe_subtract_amounts(0, 1), Some(-1));
        assert_eq!(safe_subtract_amounts(i128::MIN, 1), None);
    }
}
