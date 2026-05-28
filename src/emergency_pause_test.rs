#[cfg(test)]
mod emergency_pause_tests {
    use crate::{ConfigUpdateKey, ContractError, QuorumCreditContract, QuorumCreditContractClient};
    use soroban_sdk::{
        testutils::{Address as _, Ledger},
        token::StellarAssetClient,
        Address, Env, String, Vec,
    };

    fn setup() -> (
        Env,
        QuorumCreditContractClient<'static>,
        Address,
        Address,
        Address,
        Address,
    ) {
        let env = Env::default();
        env.mock_all_auths();

        let deployer = Address::generate(&env);
        let admin1 = Address::generate(&env);
        let admin2 = Address::generate(&env);
        let voucher = Address::generate(&env);
        let borrower = Address::generate(&env);
        let admins = Vec::from_array(&env, [admin1.clone(), admin2.clone()]);
        let token = env
            .register_stellar_asset_contract_v2(admin1.clone())
            .address();

        let contract_id = env.register_contract(None, QuorumCreditContract);
        StellarAssetClient::new(&env, &token).mint(&voucher, &5_000_000);
        let client = QuorumCreditContractClient::new(&env, &contract_id);
        client.initialize(&deployer, &admins, &2, &token);
        env.ledger().with_mut(|l| l.timestamp = 90_000);

        (env, client, admin1, admin2, voucher, borrower)
    }

    #[test]
    fn test_any_admin_can_emergency_pause() {
        let (_env, client, admin1, _, _, _) = setup();
        client.emergency_pause(&admin1);
        assert!(client.get_config().emergency_pause_enabled);
    }

    #[test]
    fn test_non_admin_cannot_emergency_pause() {
        let (_env, client, _, _, _, _) = setup();
        let outsider = Address::generate(&_env);
        let result = client.try_emergency_pause(&outsider);
        assert_eq!(result, Err(Ok(ContractError::UnauthorizedCaller)));
    }

    #[test]
    fn test_paused_contract_rejects_writes() {
        let (env, client, admin1, _, voucher, borrower) = setup();
        client.emergency_pause(&admin1);

        let result = client.try_vouch(&voucher, &borrower, &1_000_000, &client.get_config().token);
        assert_eq!(result, Err(Ok(ContractError::ContractPaused)));

        let result = client.try_request_loan(
            &borrower,
            &100_000,
            &500_000,
            &String::from_str(&env, "x"),
            &client.get_config().token,
        );
        assert_eq!(result, Err(Ok(ContractError::ContractPaused)));
    }

    #[test]
    fn test_full_threshold_required_to_unpause() {
        let (_env, client, admin1, admin2, _, _) = setup();
        client.emergency_pause(&admin1);

        let one = Vec::from_array(&_env, [admin1.clone()]);
        assert!(client.try_emergency_unpause(&one).is_err());

        let two = Vec::from_array(&_env, [admin1.clone(), admin2.clone()]);
        client.emergency_unpause(&two);
        assert!(!client.get_config().emergency_pause_enabled);
    }

    #[test]
    fn test_unpaused_contract_resumes_writes() {
        let (_env, client, admin1, admin2, voucher, borrower) = setup();
        client.emergency_pause(&admin1);
        let signers = Vec::from_array(&_env, [admin1.clone(), admin2.clone()]);
        client.emergency_unpause(&signers);

        StellarAssetClient::new(&_env, &client.get_config().token).mint(&voucher, &1_000_000);
        client.vouch(&voucher, &borrower, &1_000_000, &client.get_config().token);
        assert!(client.vouch_exists(&voucher, &borrower));
    }

    #[test]
    fn test_config_update_blocked_while_emergency_paused() {
        let (_env, client, admin1, admin2, _, _) = setup();
        client.emergency_pause(&admin1);
        let signers = Vec::from_array(&_env, [admin1.clone(), admin2.clone()]);
        let result = client.try_propose_config_update(
            &admin1,
            &ConfigUpdateKey::AdminThreshold,
            &2,
        );
        assert_eq!(result, Err(Ok(ContractError::ContractPaused)));
        let _ = signers;
    }
}
