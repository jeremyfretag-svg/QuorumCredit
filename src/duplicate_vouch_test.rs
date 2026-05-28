#[cfg(test)]
mod duplicate_vouch_tests {
    use crate::errors::ContractError;
    use crate::{QuorumCreditContract, QuorumCreditContractClient};
    use soroban_sdk::{testutils::Address as _, token::StellarAssetClient, Address, Env, Vec};

    fn setup(env: &Env) -> (Address, Address, Address, Address) {
        let deployer = Address::generate(env);
        let admin = Address::generate(env);
        let token_id = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        let contract_id = env.register_contract(None, QuorumCreditContract);
        let client = QuorumCreditContractClient::new(env, &contract_id);
        client.initialize(&deployer, &Vec::from_array(env, [admin]), &1, &token_id);
        let voucher = Address::generate(env);
        StellarAssetClient::new(env, &token_id).mint(&voucher, &10_000_000);
        (contract_id, token_id, voucher, Address::generate(env))
    }

    /// Issue #476: second vouch from same voucher+borrower pair must return DuplicateVouch.
    #[test]
    fn test_duplicate_vouch_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, token_id, voucher, borrower) = setup(&env);
        let client = QuorumCreditContractClient::new(&env, &contract_id);

        client.vouch(&voucher, &borrower, &1_000_000, &token_id, &None);

        let result = client.try_vouch(&voucher, &borrower, &1_000_000, &token_id, &None);
        assert_eq!(result, Err(Ok(ContractError::DuplicateVouch)));
    }

    /// Issue #476: increase_stake() is the correct path to add more stake to an existing vouch.
    #[test]
    fn test_increase_stake_succeeds_after_vouch() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, token_id, voucher, borrower) = setup(&env);
        let client = QuorumCreditContractClient::new(&env, &contract_id);

        client.vouch(&voucher, &borrower, &1_000_000, &token_id, &None);

        let result = client.try_increase_stake(&voucher, &borrower, &500_000);
        assert!(result.is_ok());
    }
}
