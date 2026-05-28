/// Yield Reserve Solvency Tests (Issue #549)
///
/// Verifies that the contract checks yield reserve balance before disbursing loans
/// and prevents over-promising yield.

#[cfg(test)]
mod yield_reserve_solvency_tests {
    use crate::{QuorumCreditContract, QuorumCreditContractClient};
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

        // Fund contract with enough for loans
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

    /// Test get_yield_reserve_balance returns 0 initially
    #[test]
    fn test_yield_reserve_balance_initial() {
        let s = setup();
        assert_eq!(s.client.get_yield_reserve_balance(), 0, "yield reserve should be 0 initially");
    }

    /// Test set_yield_reserve updates the balance
    #[test]
    fn test_set_yield_reserve() {
        let s = setup();
        s.client.set_yield_reserve(&s.admin_vec, &1_000_000).unwrap();
        assert_eq!(s.client.get_yield_reserve_balance(), 1_000_000, "yield reserve should be updated");
    }

    /// Test request_loan fails if yield reserve is insufficient
    #[test]
    fn test_request_loan_fails_insufficient_yield_reserve() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);

        do_vouch(&s, &voucher, &borrower, 500_000);

        // Set yield reserve to 0 (insufficient for any loan)
        s.client.set_yield_reserve(&s.admin_vec, &0).unwrap();

        // Try to request loan with 2% yield
        // Loan amount = 100_000, yield = 100_000 * 200 / 10_000 = 2_000
        let result = s.client.try_request_loan(&borrower, &100_000, &500_000, &purpose(&s.env), &s.token_id);
        assert!(result.is_err(), "expected error when yield reserve is insufficient");
    }

    /// Test request_loan succeeds if yield reserve is sufficient
    #[test]
    fn test_request_loan_succeeds_sufficient_yield_reserve() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);

        do_vouch(&s, &voucher, &borrower, 500_000);

        // Set yield reserve to 10_000 (enough for 100_000 loan at 2% yield)
        // Loan amount = 100_000, yield = 100_000 * 200 / 10_000 = 2_000
        s.client.set_yield_reserve(&s.admin_vec, &10_000).unwrap();

        let result = s.client.try_request_loan(&borrower, &100_000, &500_000, &purpose(&s.env), &s.token_id);
        assert!(result.is_ok(), "expected loan to succeed with sufficient yield reserve");
    }

    /// Test yield reserve is not decremented on loan disbursement
    /// (it's just a check, not a transfer)
    #[test]
    fn test_yield_reserve_not_decremented_on_loan() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);

        do_vouch(&s, &voucher, &borrower, 500_000);

        // Set yield reserve to 100_000
        s.client.set_yield_reserve(&s.admin_vec, &100_000).unwrap();

        // Request loan
        s.client.request_loan(&borrower, &100_000, &500_000, &purpose(&s.env), &s.token_id).unwrap();

        // Yield reserve should still be 100_000 (not decremented)
        assert_eq!(s.client.get_yield_reserve_balance(), 100_000, "yield reserve should not be decremented");
    }

    /// Test multiple loans with limited yield reserve
    #[test]
    fn test_multiple_loans_with_limited_yield_reserve() {
        let s = setup();

        // Set yield reserve to 5_000 (enough for 2 loans of 100_000 each at 2% yield)
        s.client.set_yield_reserve(&s.admin_vec, &5_000).unwrap();

        // First loan: 100_000 * 200 / 10_000 = 2_000 yield
        let borrower1 = Address::generate(&s.env);
        let voucher1 = Address::generate(&s.env);
        do_vouch(&s, &voucher1, &borrower1, 500_000);
        s.client.request_loan(&borrower1, &100_000, &500_000, &purpose(&s.env), &s.token_id).unwrap();

        // Second loan: 100_000 * 200 / 10_000 = 2_000 yield
        let borrower2 = Address::generate(&s.env);
        let voucher2 = Address::generate(&s.env);
        do_vouch(&s, &voucher2, &borrower2, 500_000);
        s.client.request_loan(&borrower2, &100_000, &500_000, &purpose(&s.env), &s.token_id).unwrap();

        // Third loan would need 2_000 more yield, but reserve is only 5_000 - 2_000 - 2_000 = 1_000
        let borrower3 = Address::generate(&s.env);
        let voucher3 = Address::generate(&s.env);
        do_vouch(&s, &voucher3, &borrower3, 500_000);
        let result = s.client.try_request_loan(&borrower3, &100_000, &500_000, &purpose(&s.env), &s.token_id);
        assert!(result.is_err(), "expected third loan to fail due to insufficient yield reserve");
    }
}
