#[cfg(test)]
mod vouch_conflict_detection_tests {
    use crate::errors::ContractError;
    use crate::{QuorumCreditContract, QuorumCreditContractClient};
    use soroban_sdk::{testutils::{Address as _, Ledger}, token::StellarAssetClient, Address, Env, Vec};

    fn setup(env: &Env) -> (Address, Address, Address) {
        let deployer = Address::generate(env);
        let admin = Address::generate(env);
        let token_id = env.register_stellar_asset_contract_v2(admin.clone()).address();
        let contract_id = env.register_contract(None, QuorumCreditContract);
        QuorumCreditContractClient::new(env, &contract_id).initialize(
            &deployer,
            &Vec::from_array(env, [admin.clone()]),
            &1,
            &token_id,
        );
        (contract_id, token_id, admin)
    }

    fn mint_and_vouch(env: &Env, client: &QuorumCreditContractClient, token: &Address, voucher: &Address, borrower: &Address) {
        StellarAssetClient::new(env, token).mint(voucher, &1_000_000);
        client.vouch(voucher, borrower, &500_000, token);
        // Advance past cooldown so the same voucher can vouch again
        env.ledger().with_mut(|l| l.timestamp += crate::types::DEFAULT_VOUCH_COOLDOWN_SECS + 1);
    }

    fn disburse_loan(env: &Env, client: &QuorumCreditContractClient, token: &Address, borrower: &Address) {
        StellarAssetClient::new(env, token).mint(&client.address, &10_000_000);
        client.request_loan(
            borrower,
            &100_000,
            &500_000,
            &soroban_sdk::String::from_str(env, "test"),
            token,
        );
    }

    /// #639: get/set conflict_threshold works.
    #[test]
    fn test_set_get_conflict_threshold() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, _token_id, admin) = setup(&env);
        let client = QuorumCreditContractClient::new(&env, &contract_id);

        assert_eq!(client.get_conflict_threshold(), 0);
        client.set_conflict_threshold(&Vec::from_array(&env, [admin]), &2);
        assert_eq!(client.get_conflict_threshold(), 2);
    }

    /// #639: voucher below threshold can vouch freely.
    #[test]
    fn test_vouch_below_conflict_threshold_allowed() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, token_id, admin) = setup(&env);
        let client = QuorumCreditContractClient::new(&env, &contract_id);

        // Set threshold to 2: voucher may back at most 1 active-loan borrower
        client.set_conflict_threshold(&Vec::from_array(&env, [admin.clone()]), &2);

        let voucher = Address::generate(&env);
        let borrower1 = Address::generate(&env);
        let borrower2 = Address::generate(&env);

        mint_and_vouch(&env, &client, &token_id, &voucher, &borrower1);
        disburse_loan(&env, &client, &token_id, &borrower1);

        // borrower1 has active loan; voucher backs 1 active-loan borrower → still under threshold of 2
        StellarAssetClient::new(&env, &token_id).mint(&voucher, &1_000_000);
        let result = client.try_vouch(&voucher, &borrower2, &500_000, &token_id);
        assert!(result.is_ok());
    }

    /// #639: voucher at or above threshold is rejected.
    #[test]
    fn test_vouch_at_conflict_threshold_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, token_id, admin) = setup(&env);
        let client = QuorumCreditContractClient::new(&env, &contract_id);

        // Set threshold to 1: voucher may not back any active-loan borrower
        client.set_conflict_threshold(&Vec::from_array(&env, [admin.clone()]), &1);

        let voucher = Address::generate(&env);
        let borrower1 = Address::generate(&env);
        let borrower2 = Address::generate(&env);

        mint_and_vouch(&env, &client, &token_id, &voucher, &borrower1);
        disburse_loan(&env, &client, &token_id, &borrower1);

        // borrower1 has active loan; voucher already backs 1 → at threshold → reject
        StellarAssetClient::new(&env, &token_id).mint(&voucher, &1_000_000);
        let result = client.try_vouch(&voucher, &borrower2, &500_000, &token_id);
        assert_eq!(result, Err(Ok(ContractError::VouchConflictDetected)));
    }

    /// #639: threshold=0 means no limit.
    #[test]
    fn test_conflict_threshold_zero_means_no_limit() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, token_id, _admin) = setup(&env);
        let client = QuorumCreditContractClient::new(&env, &contract_id);

        // Default threshold is 0 (no limit)
        let voucher = Address::generate(&env);
        let borrower1 = Address::generate(&env);
        let borrower2 = Address::generate(&env);

        mint_and_vouch(&env, &client, &token_id, &voucher, &borrower1);
        disburse_loan(&env, &client, &token_id, &borrower1);

        StellarAssetClient::new(&env, &token_id).mint(&voucher, &1_000_000);
        let result = client.try_vouch(&voucher, &borrower2, &500_000, &token_id);
        assert!(result.is_ok());
    }
}
