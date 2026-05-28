#[cfg(test)]
mod max_loan_amount_tests {
    use crate::errors::ContractError;
    use crate::{QuorumCreditContract, QuorumCreditContractClient};
    use soroban_sdk::{testutils::Address as _, token::StellarAssetClient, Address, Env, String, Vec};

    const MAX: i128 = 1_000_000;

    fn setup(env: &Env) -> (Address, Address, Address, Address, Address) {
        let deployer = Address::generate(env);
        let admin = Address::generate(env);
        let token_id = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        let contract_id = env.register_contract(None, QuorumCreditContract);
        let client = QuorumCreditContractClient::new(env, &contract_id);
        client.initialize(&deployer, &Vec::from_array(env, [admin.clone()]), &1, &token_id);
        // Set max_loan_amount to 1,000,000 stroops
        client.set_max_loan_amount(&Vec::from_array(env, [admin.clone()]), &MAX);
        // Fund contract so it can disburse loans
        StellarAssetClient::new(env, &token_id).mint(&contract_id, &10_000_000);
        let voucher = Address::generate(env);
        StellarAssetClient::new(env, &token_id).mint(&voucher, &10_000_000);
        (contract_id, token_id, admin, voucher, Address::generate(env))
    }

    /// Issue #475: request_loan() with 1,000,001 stroops must be rejected.
    #[test]
    fn test_loan_exceeds_max_amount_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().with_mut(|l| l.timestamp = 120);
        let (contract_id, token_id, _admin, voucher, borrower) = setup(&env);
        let client = QuorumCreditContractClient::new(&env, &contract_id);

        client.vouch(&voucher, &borrower, &5_000_000, &token_id, &None);

        let result = client.try_request_loan(
            &borrower,
            &(MAX + 1),
            &5_000_000,
            &String::from_str(&env, "test loan"),
            &token_id,
        );
        assert_eq!(result, Err(Ok(ContractError::LoanExceedsMaxAmount)));
    }

    /// Issue #475: request_loan() with exactly 1,000,000 stroops must succeed.
    #[test]
    fn test_loan_at_max_amount_accepted() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().with_mut(|l| l.timestamp = 120);
        let (contract_id, token_id, _admin, voucher, borrower) = setup(&env);
        let client = QuorumCreditContractClient::new(&env, &contract_id);

        client.vouch(&voucher, &borrower, &5_000_000, &token_id, &None);

        let result = client.try_request_loan(
            &borrower,
            &MAX,
            &5_000_000,
            &String::from_str(&env, "test loan"),
            &token_id,
        );
        assert!(result.is_ok());
    }
}
