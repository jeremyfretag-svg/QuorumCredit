#[cfg(test)]
mod max_loan_to_stake_ratio_tests {
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
        // Set max_loan_to_stake_ratio to 100 (1:1)
        client.set_max_loan_to_stake_ratio(&Vec::from_array(env, [admin.clone()]), &100);
        // Fund contract so it can disburse loans
        StellarAssetClient::new(env, &token_id).mint(&contract_id, &10_000_000);
        let voucher = Address::generate(env);
        StellarAssetClient::new(env, &token_id).mint(&voucher, &10_000_000);
        (contract_id, token_id, admin, voucher, Address::generate(env))
    }

    /// Test that max_loan_to_stake_ratio is enforced.
    #[test]
    fn test_max_loan_to_stake_ratio_enforced() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().with_mut(|l| l.timestamp = 120);
        let (contract_id, token_id, _admin, voucher, borrower) = setup(&env);
        let client = QuorumCreditContractClient::new(&env, &contract_id);

        // Have voucher vouch with 1,000,000 stroops
        client.vouch(&voucher, &borrower, &1_000_000, &token_id, &None);

        // Attempt to request 1,100,000 stroops (exceeds 1:1 ratio)
        let result = client.try_request_loan(
            &borrower,
            &1_100_000,
            &1_000_000,
            &String::from_str(&env, "test loan"),
            &token_id,
        );
        assert_eq!(result, Err(Ok(ContractError::InsufficientFunds)));
    }

    /// Test that loan at max ratio succeeds.
    #[test]
    fn test_loan_at_max_ratio_succeeds() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().with_mut(|l| l.timestamp = 120);
        let (contract_id, token_id, _admin, voucher, borrower) = setup(&env);
        let client = QuorumCreditContractClient::new(&env, &contract_id);

        // Have voucher vouch with 1,000,000 stroops
        client.vouch(&voucher, &borrower, &1_000_000, &token_id, &None);

        // Request 1,000,000 stroops (exactly 1:1 ratio)
        let result = client.try_request_loan(
            &borrower,
            &1_000_000,
            &1_000_000,
            &String::from_str(&env, "test loan"),
            &token_id,
        );
        assert!(result.is_ok());
    }
}