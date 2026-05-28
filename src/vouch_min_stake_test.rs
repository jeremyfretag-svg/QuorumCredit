#[cfg(test)]
mod vouch_min_stake_tests {
    use crate::errors::ContractError;
    use crate::types::DEFAULT_MIN_YIELD_STAKE;
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
        client.initialize(&deployer, &Vec::from_array(env, [admin.clone()]), &1, &token_id);
        // Set min_stake to DEFAULT_MIN_YIELD_STAKE (50 stroops)
        client.set_min_stake(
            &Vec::from_array(env, [admin]),
            &DEFAULT_MIN_YIELD_STAKE,
        );
        let voucher = Address::generate(env);
        StellarAssetClient::new(env, &token_id).mint(&voucher, &1_000_000);
        (contract_id, token_id, voucher, Address::generate(env))
    }

    /// Issue #474: vouch() with 49 stroops (below DEFAULT_MIN_YIELD_STAKE) must be rejected.
    #[test]
    fn test_vouch_below_min_stake_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, token_id, voucher, borrower) = setup(&env);
        let client = QuorumCreditContractClient::new(&env, &contract_id);

        let result = client.try_vouch(&voucher, &borrower, &(DEFAULT_MIN_YIELD_STAKE - 1), &token_id, &None);
        assert_eq!(result, Err(Ok(ContractError::MinStakeNotMet)));
    }

    /// Issue #474: vouch() with exactly 50 stroops (DEFAULT_MIN_YIELD_STAKE) must succeed.
    #[test]
    fn test_vouch_exact_min_stake_accepted() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, token_id, voucher, borrower) = setup(&env);
        let client = QuorumCreditContractClient::new(&env, &contract_id);

        let result = client.try_vouch(&voucher, &borrower, &DEFAULT_MIN_YIELD_STAKE, &token_id, &None);
        assert!(result.is_ok());
    }
}
