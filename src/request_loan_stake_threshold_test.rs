/// Comprehensive tests for request_loan() stake threshold check.
/// Covers issue #481.
#[cfg(test)]
mod request_loan_stake_threshold_tests {
    use crate::{ContractError, LoanStatus, QuorumCreditContract, QuorumCreditContractClient};
    use soroban_sdk::{
        testutils::{Address as _, Ledger},
        token::StellarAssetClient,
        Address, Env, String, Vec,
    };

    struct Setup {
        env: Env,
        client: QuorumCreditContractClient<'static>,
        token_id: Address,
    }

    fn setup() -> Setup {
        let env = Env::default();
        env.mock_all_auths();

        let deployer = Address::generate(&env);
        let admin = Address::generate(&env);
        let admins = Vec::from_array(&env, [admin.clone()]);

        let token_id = env.register_stellar_asset_contract_v2(admin.clone());
        let contract_id = env.register_contract(None, QuorumCreditContract);

        StellarAssetClient::new(&env, &token_id.address()).mint(&contract_id, &10_000_000);

        let client = QuorumCreditContractClient::new(&env, &contract_id);
        client.initialize(&deployer, &admins, &1, &token_id.address());

        // Advance past MIN_VOUCH_AGE so vouches are eligible.
        env.ledger().with_mut(|l| l.timestamp = 120);

        Setup { env, client, token_id: token_id.address() }
    }

    /// request_loan() must return InsufficientFunds when total stake < threshold.
    #[test]
    fn test_request_loan_rejected_when_stake_below_threshold() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        let voucher_a = Address::generate(&s.env);

        // Step 1: Voucher A vouches with 1,000,000 stroops.
        StellarAssetClient::new(&s.env, &s.token_id).mint(&voucher_a, &1_000_000);
        s.client.vouch(&voucher_a, &borrower, &1_000_000, &s.token_id, &None);

        // Step 2: Attempt loan with threshold of 2,000,000 stroops.
        let result = s.client.try_request_loan(
            &borrower,
            &1_000_000,
            &2_000_000,
            &String::from_str(&s.env, "test"),
            &s.token_id,
        );

        // Step 3: Assert InsufficientFunds is returned.
        assert_eq!(
            result,
            Err(Ok(ContractError::InsufficientFunds)),
            "request_loan() must reject when total stake is below threshold"
        );
    }

    /// request_loan() must succeed when total stake meets the threshold exactly.
    #[test]
    fn test_request_loan_succeeds_when_stake_meets_threshold() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        let voucher_a = Address::generate(&s.env);

        // Step 1: Voucher A vouches with 1,000,000 stroops.
        StellarAssetClient::new(&s.env, &s.token_id).mint(&voucher_a, &1_000_000);
        s.client.vouch(&voucher_a, &borrower, &1_000_000, &s.token_id, &None);

        // Step 4: Request with threshold of 1,000,000 stroops — must succeed.
        let result = s.client.try_request_loan(
            &borrower,
            &500_000,
            &1_000_000,
            &String::from_str(&s.env, "test"),
            &s.token_id,
        );

        // Step 5: Assert success.
        assert!(result.is_ok(), "request_loan() must succeed when stake meets threshold");

        let loan_status = s.client.loan_status(&borrower);
        assert_eq!(loan_status, LoanStatus::Active, "loan should be active after disbursement");
    }
}
