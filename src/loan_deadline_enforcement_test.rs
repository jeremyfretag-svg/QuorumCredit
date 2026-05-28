#[cfg(test)]
mod loan_deadline_enforcement_tests {
    use crate::errors::ContractError;
    use crate::{QuorumCreditContract, QuorumCreditContractClient};
    use soroban_sdk::{testutils::Address as _, token::StellarAssetClient, Address, Env, String, Vec};

    fn setup(env: &Env) -> (Address, Address, Address, Address, Address) {
        let deployer = Address::generate(env);
        let admin = Address::generate(env);
        let token_id = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        let contract_id = env.register_contract(None, QuorumCreditContract);
        let client = QuorumCreditContractClient::new(env, &contract_id);
        client.initialize(&deployer, &Vec::from_array(env, [admin.clone()]), &1, &token_id);
        // Fund contract so it can disburse loans
        StellarAssetClient::new(env, &token_id).mint(&contract_id, &10_000_000);
        let voucher = Address::generate(env);
        StellarAssetClient::new(env, &token_id).mint(&voucher, &10_000_000);
        let borrower = Address::generate(env);
        StellarAssetClient::new(env, &token_id).mint(&borrower, &10_000_000);
        (contract_id, token_id, admin, voucher, borrower)
    }

    /// Test that loan deadlines are enforced.
    #[test]
    fn test_loan_deadline_enforcement() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, token_id, _admin, voucher, borrower) = setup(&env);
        let client = QuorumCreditContractClient::new(&env, &contract_id);

        // Set loan duration to 1 day
        let mut config = client.get_config();
        config.loan_duration = 24 * 60 * 60; // 1 day in seconds
        client.set_config(&Vec::from_array(env, [_admin.clone()]), &config);

        // Have voucher vouch
        client.vouch(&voucher, &borrower, &5_000_000, &token_id, &None);

        // Request loan
        client.request_loan(
            &borrower,
            &1_000_000,
            &5_000_000,
            &String::from_str(&env, "test loan"),
            &token_id,
        );

        // Advance time by 2 days
        env.ledger().with_mut(|l| l.timestamp += 2 * 24 * 60 * 60);

        // Attempt to repay
        let result = client.try_repay(&borrower, &1_000_000);
        assert_eq!(result, Err(Ok(ContractError::LoanPastDeadline)));
    }
}