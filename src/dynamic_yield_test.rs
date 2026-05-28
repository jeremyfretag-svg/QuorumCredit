/// Dynamic Yield Tests (Issue #473)
///
/// Verifies calculate_dynamic_yield and that request_loan locks in the
/// dynamic yield on the LoanRecord.

#[cfg(test)]
mod dynamic_yield_tests {
    use crate::{
        reputation::{ReputationNftContract, ReputationNftContractClient},
        QuorumCreditContract, QuorumCreditContractClient,
    };
    use soroban_sdk::{
        testutils::{Address as _, Ledger},
        token::StellarAssetClient,
        Address, Env, String, Vec,
    };

    struct Setup {
        env: Env,
        client: QuorumCreditContractClient<'static>,
        token_id: Address,
        admin_vec: Vec<Address>,
    }

    fn setup() -> Setup {
        let env = Env::default();
        env.mock_all_auths();

        let deployer = Address::generate(&env);
        let admin = Address::generate(&env);
        let admins = Vec::from_array(&env, [admin.clone()]);

        let token_id = env.register_stellar_asset_contract_v2(admin.clone());
        let contract_id = env.register_contract(None, QuorumCreditContract);

        // Fund contract generously so all loans can be disbursed.
        StellarAssetClient::new(&env, &token_id.address()).mint(&contract_id, &100_000_000);

        let client = QuorumCreditContractClient::new(&env, &contract_id);
        client.initialize(&deployer, &admins, &1, &token_id.address());

        env.ledger().with_mut(|l| l.timestamp = 120);

        Setup { env, client, token_id: token_id.address(), admin_vec: admins }
    }

    fn do_vouch(s: &Setup, voucher: &Address, borrower: &Address, stake: i128) {
        StellarAssetClient::new(&s.env, &s.token_id).mint(voucher, &stake);
        s.client.vouch(voucher, borrower, &stake, &s.token_id, &None);
    }

    fn purpose(env: &Env) -> String {
        String::from_str(env, "test")
    }

    /// No score, no defaults → yield equals base (200 bps = 2%).
    #[test]
    fn test_dynamic_yield_base_case() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        // 200 bps base, credit_score=0, default_count=0 → 200
        assert_eq!(s.client.calculate_dynamic_yield(&borrower), 200);
    }

    /// Credit score of 500 → yield = 200 + (500/100) = 205 bps.
    #[test]
    fn test_dynamic_yield_increases_with_credit_score() {
        let s = setup();
        let borrower = Address::generate(&s.env);

        // Register NFT contract and mint 500 reputation points to borrower.
        let contract_id = s.client.address.clone();
        let nft_id = s.env.register_contract(None, ReputationNftContract);
        let nft = ReputationNftContractClient::new(&s.env, &nft_id);
        nft.initialize(&contract_id);
        s.client.set_reputation_nft(&s.admin_vec, &nft_id);

        // Mint 500 points directly via the NFT contract (contract is the minter).
        for _ in 0..500 {
            nft.mint(&borrower);
        }

        // 200 + (500/100) = 205
        assert_eq!(s.client.calculate_dynamic_yield(&borrower), 205);
    }

    /// 3 defaults → yield = 200 - (3*50) = 50 bps.
    #[test]
    fn test_dynamic_yield_decreases_with_defaults() {
        let s = setup();
        let borrower = Address::generate(&s.env);

        // Simulate 3 defaults by running 3 slash cycles.
        s.client.set_slash_vote_quorum(&s.admin_vec, &1);

        for _ in 0..3 {
            let voucher = Address::generate(&s.env);
            do_vouch(&s, &voucher, &borrower, 500_000);
            s.client.request_loan(&borrower, &100_000, &500_000, &purpose(&s.env), &s.token_id);
            s.client.vote_slash(&voucher, &borrower, &true);
        }

        // 200 - (3 * 50) = 50
        assert_eq!(s.client.calculate_dynamic_yield(&borrower), 50);
    }

    /// Enough defaults to push yield below 0 → floored at 0.
    #[test]
    fn test_dynamic_yield_floored_at_zero() {
        let s = setup();
        let borrower = Address::generate(&s.env);

        s.client.set_slash_vote_quorum(&s.admin_vec, &1);

        // 5 defaults → 200 - (5*50) = -50 → clamped to 0
        for _ in 0..5 {
            let voucher = Address::generate(&s.env);
            do_vouch(&s, &voucher, &borrower, 500_000);
            s.client.request_loan(&borrower, &100_000, &500_000, &purpose(&s.env), &s.token_id);
            s.client.vote_slash(&voucher, &borrower, &true);
        }

        assert_eq!(s.client.calculate_dynamic_yield(&borrower), 0);
    }

    /// total_yield on the LoanRecord reflects the dynamic bps at disbursement.
    #[test]
    fn test_loan_record_uses_dynamic_yield() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);

        // 1 default → yield_bps = 200 - 50 = 150
        s.client.set_slash_vote_quorum(&s.admin_vec, &1);
        do_vouch(&s, &voucher, &borrower, 500_000);
        s.client.request_loan(&borrower, &100_000, &500_000, &purpose(&s.env), &s.token_id);
        s.client.vote_slash(&voucher, &borrower, &true);

        // Now request a second loan with dynamic yield = 150 bps.
        let voucher2 = Address::generate(&s.env);
        do_vouch(&s, &voucher2, &borrower, 500_000);
        s.client.request_loan(&borrower, &100_000, &500_000, &purpose(&s.env), &s.token_id);

        let loan = s.client.get_loan(&borrower).unwrap();
        // 100_000 * 150 / 10_000 = 1_500
        assert_eq!(loan.total_yield, 1_500);
    }

    /// Test set_borrower_risk_score updates the risk_score field
    #[test]
    fn test_set_borrower_risk_score() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);

        do_vouch(&s, &voucher, &borrower, 500_000);
        s.client.request_loan(&borrower, &100_000, &500_000, &purpose(&s.env), &s.token_id);

        let loan_before = s.client.get_loan(&borrower).unwrap();
        assert_eq!(loan_before.risk_score, 0, "risk_score should be 0 initially");

        // Set risk_score to 50
        s.client.set_borrower_risk_score(&s.admin_vec, &borrower, &50).unwrap();

        let loan_after = s.client.get_loan(&borrower).unwrap();
        assert_eq!(loan_after.risk_score, 50, "risk_score should be updated to 50");
    }

    /// Test set_borrower_risk_score rejects invalid scores > 100
    #[test]
    fn test_set_borrower_risk_score_rejects_invalid() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);

        do_vouch(&s, &voucher, &borrower, 500_000);
        s.client.request_loan(&borrower, &100_000, &500_000, &purpose(&s.env), &s.token_id);

        // Try to set risk_score > 100
        let result = s.client.try_set_borrower_risk_score(&s.admin_vec, &borrower, &101);
        assert!(result.is_err(), "expected error for risk_score > 100");
    }
}
