#[cfg(test)]
mod config_update_voting_tests {
    use crate::{ConfigUpdateKey, ContractError, QuorumCreditContract, QuorumCreditContractClient};
    use soroban_sdk::{testutils::Address as _, Address, Env, Vec};

    fn setup_multisig() -> (
        Env,
        QuorumCreditContractClient<'static>,
        Address,
        Address,
        Address,
    ) {
        let env = Env::default();
        env.mock_all_auths();

        let deployer = Address::generate(&env);
        let admin1 = Address::generate(&env);
        let admin2 = Address::generate(&env);
        let admins = Vec::from_array(&env, [admin1.clone(), admin2.clone()]);
        let token = env
            .register_stellar_asset_contract_v2(admin1.clone())
            .address();

        let contract_id = env.register_contract(None, QuorumCreditContract);
        let client = QuorumCreditContractClient::new(&env, &contract_id);
        client.initialize(&deployer, &admins, &2, &token);

        (env, client, admin1, admin2, deployer)
    }

    #[test]
    fn test_proposal_created_by_admin() {
        let (_env, client, admin1, _, _) = setup_multisig();
        let id = client.propose_config_update(
            &admin1,
            &ConfigUpdateKey::AdminThreshold,
            &2,
        );
        let p = client.get_config_update_proposal(&id).unwrap();
        assert_eq!(p.proposer, admin1);
        assert_eq!(p.new_value, 2);
    }

    #[test]
    fn test_approval_from_non_admin_rejected() {
        let (_env, client, admin1, _, _) = setup_multisig();
        let outsider = Address::generate(&_env);
        let id = client.propose_config_update(
            &admin1,
            &ConfigUpdateKey::AdminThreshold,
            &2,
        );
        let result = client.try_approve_config_update(&outsider, &id);
        assert_eq!(result, Err(Ok(ContractError::UnauthorizedCaller)));
    }

    #[test]
    fn test_double_approval_rejected() {
        let (_env, client, admin1, _, _) = setup_multisig();
        let id = client.propose_config_update(
            &admin1,
            &ConfigUpdateKey::AdminThreshold,
            &2,
        );
        client.approve_config_update(&admin1, &id);
        let result = client.try_approve_config_update(&admin1, &id);
        assert_eq!(result, Err(Ok(ContractError::AlreadyVoted)));
    }

    #[test]
    fn test_finalize_before_threshold_rejected() {
        let (_env, client, admin1, _, _) = setup_multisig();
        let id = client.propose_config_update(
            &admin1,
            &ConfigUpdateKey::AdminThreshold,
            &2,
        );
        client.approve_config_update(&admin1, &id);
        let result = client.try_finalize_config_update(&id);
        assert_eq!(result, Err(Ok(ContractError::UnauthorizedCaller)));
    }

    #[test]
    fn test_finalize_after_threshold_applies_change() {
        let (env, client, admin1, admin2, _) = setup_multisig();
        let id = client.propose_config_update(
            &admin1,
            &ConfigUpdateKey::AdminThreshold,
            &1,
        );
        client.approve_config_update(&admin1, &id);
        client.approve_config_update(&admin2, &id);
        client.finalize_config_update(&id);
        assert_eq!(client.get_config().admin_threshold, 1);
        let _ = env;
    }
}
