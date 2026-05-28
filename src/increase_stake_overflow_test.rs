/// Tests that increase_stake() returns StakeOverflow when the addition would overflow i128.
#[cfg(test)]
mod increase_stake_overflow_tests {
    use crate::{ContractError, QuorumCreditContract, QuorumCreditContractClient};
    use crate::types::{DataKey, VouchRecord};
    use soroban_sdk::{
        testutils::Address as _, token::StellarAssetClient, Address, Env, Vec,
    };

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
        let borrower = Address::generate(env);
        (contract_id, token_id, voucher, borrower)
    }

    /// Inject a vouch record with stake = i128::MAX - 1 directly into storage,
    /// then verify increase_stake(2) returns StakeOverflow and increase_stake(1) succeeds.
    #[test]
    fn test_increase_stake_overflow_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, token_id, voucher, borrower) = setup(&env);
        let client = QuorumCreditContractClient::new(&env, &contract_id);

        // Inject a near-max vouch directly (bypassing token transfer).
        let initial_stake = i128::MAX - 1;
        env.as_contract(&contract_id, || {
            let mut vouches: Vec<VouchRecord> = Vec::new(&env);
            vouches.push_back(VouchRecord {
                voucher: voucher.clone(),
                stake: initial_stake,
                vouch_timestamp: 0,
                token: token_id.clone(),
                expiry_timestamp: None,
                delegate: None,
                chain_id: 0,
            });
            env.storage()
                .persistent()
                .set(&DataKey::Vouches(borrower.clone()), &vouches);
        });

        // Mint enough for the transfer (only 2 stroops needed).
        StellarAssetClient::new(&env, &token_id).mint(&voucher, &2);

        // Attempt to increase by 2 — must overflow.
        let result = client.try_increase_stake(&voucher, &borrower, &2);
        assert_eq!(result, Err(Ok(ContractError::StakeOverflow)));

        // Increase by 1 — must succeed (i128::MAX - 1 + 1 = i128::MAX).
        let result = client.try_increase_stake(&voucher, &borrower, &1);
        assert!(result.is_ok(), "increase_stake(1) must succeed when result equals i128::MAX");
    }
}
