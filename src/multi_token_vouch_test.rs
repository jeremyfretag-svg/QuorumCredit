#[cfg(test)]
mod multi_token_vouch_tests {
    use crate::{QuorumCreditContract, QuorumCreditContractClient};
    use soroban_sdk::{
        testutils::{Address as _, Ledger},
        token::StellarAssetClient,
        Address, Env, String, Vec,
    };

    #[test]
    fn test_vouch_multiple_tokens_both_recorded_xlm_loan_succeeds() {
        let env = Env::default();
        env.mock_all_auths();

        let deployer = Address::generate(&env);
        let admin = Address::generate(&env);
        let admins = Vec::from_array(&env, [admin.clone()]);

        let xlm_id = env.register_stellar_asset_contract_v2(admin.clone());
        let usdc_id = env.register_stellar_asset_contract_v2(admin.clone());
        let contract_id = env.register_contract(None, QuorumCreditContract);

        // Fund contract with enough XLM to disburse the loan
        StellarAssetClient::new(&env, &xlm_id.address()).mint(&contract_id, &10_000_000);
        StellarAssetClient::new(&env, &usdc_id.address()).mint(&contract_id, &10_000_000);

        let client = QuorumCreditContractClient::new(&env, &contract_id);
        client.initialize(&deployer, &admins, &1, &xlm_id.address());

        // Add USDC to allowed tokens
        client.add_allowed_token(&admins, &usdc_id.address());

        // Advance ledger past MIN_VOUCH_AGE
        env.ledger().with_mut(|l| l.timestamp = 120);

        let voucher_a = Address::generate(&env);
        let voucher_b = Address::generate(&env);
        let borrower = Address::generate(&env);

        // Voucher A vouches with XLM
        StellarAssetClient::new(&env, &xlm_id.address()).mint(&voucher_a, &1_000_000);
        client.vouch(&voucher_a, &borrower, &1_000_000, &xlm_id.address(), &None);

        // Voucher B vouches with USDC
        StellarAssetClient::new(&env, &usdc_id.address()).mint(&voucher_b, &1_000_000);
        client.vouch(&voucher_b, &borrower, &1_000_000, &usdc_id.address(), &None);

        // Assert both vouches are recorded with the correct token
        let vouches = client.get_vouches(&borrower);
        assert_eq!(vouches.len(), 2);

        let vouch_a = vouches.iter().find(|v| v.voucher == voucher_a).unwrap();
        assert_eq!(vouch_a.token, xlm_id.address());
        assert_eq!(vouch_a.stake, 1_000_000);

        let vouch_b = vouches.iter().find(|v| v.voucher == voucher_b).unwrap();
        assert_eq!(vouch_b.token, usdc_id.address());
        assert_eq!(vouch_b.stake, 1_000_000);

        // Request loan with XLM threshold — only XLM vouches count
        let purpose = String::from_str(&env, "test");
        client.request_loan(&borrower, &500_000, &500_000, &purpose, &xlm_id.address());

        let loan = client.get_loan(&borrower).unwrap();
        assert_eq!(loan.token_address, xlm_id.address());
        assert_eq!(loan.amount, 500_000);
    }
}
