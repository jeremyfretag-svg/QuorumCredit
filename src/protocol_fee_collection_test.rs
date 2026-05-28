#[cfg(test)]
mod protocol_fee_collection_tests {
    use crate::{QuorumCreditContract, QuorumCreditContractClient};
    use soroban_sdk::{testutils::Address as _, token::StellarAssetClient, Address, Env, String, Vec};

    fn setup(env: &Env) -> (Address, Address, Address, Address, Address, Address) {
        let deployer = Address::generate(env);
        let admin = Address::generate(env);
        let token_id = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        let contract_id = env.register_contract(None, QuorumCreditContract);
        let client = QuorumCreditContractClient::new(env, &contract_id);
        client.initialize(&deployer, &Vec::from_array(env, [admin.clone()]), &1, &token_id);
        // Set protocol fee to 1% (100 bps)
        client.set_protocol_fee(&Vec::from_array(env, [admin.clone()]), &100);
        // Set fee treasury
        let treasury = Address::generate(env);
        client.set_fee_treasury(&Vec::from_array(env, [admin.clone()]), &treasury);
        // Fund contract so it can disburse loans
        StellarAssetClient::new(env, &token_id).mint(&contract_id, &10_000_000);
        let voucher = Address::generate(env);
        StellarAssetClient::new(env, &token_id).mint(&voucher, &10_000_000);
        let borrower = Address::generate(env);
        StellarAssetClient::new(env, &token_id).mint(&borrower, &10_000_000);
        (contract_id, token_id, admin, voucher, borrower, treasury)
    }

    /// Test that protocol fees are correctly collected.
    #[test]
    fn test_protocol_fee_collection() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().with_mut(|l| l.timestamp = 120);
        let (contract_id, token_id, _admin, voucher, borrower, treasury) = setup(&env);
        let client = QuorumCreditContractClient::new(&env, &contract_id);

        // Have voucher vouch
        client.vouch(&voucher, &borrower, &5_000_000, &token_id, &None);

        // Request loan for 1,000,000 stroops
        client.request_loan(
            &borrower,
            &1_000_000,
            &5_000_000,
            &String::from_str(&env, "test loan"),
            &token_id,
        );

        // Repay the loan
        client.repay(&borrower, &1_010_000); // 1,000,000 + 1% fee + yield (assuming 0 yield for simplicity)
        client.repay(&treasury, &1_010_000); // 1,000,000 + 1% fee + yield (assuming 0 yield for simplicity)

        // Assert 10,000 stroops are collected as fee
        let fee_balance = client.get_fee_treasury();
        assert_eq!(fee_balance, 10_000);
    }
}