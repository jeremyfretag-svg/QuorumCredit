/// Insurance Pool Tests (Issue #472)
///
/// Covers: contribute, claim after default, double-claim rejection,
/// claim by non-voucher rejection, and empty pool rejection.

#[cfg(test)]
mod insurance_tests {
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

        StellarAssetClient::new(&env, &token_id.address()).mint(&contract_id, &10_000_000);

        let client = QuorumCreditContractClient::new(&env, &contract_id);
        client.initialize(&deployer, &admins, &1, &token_id.address());

        // Set quorum to 1 bps so a single voucher vote triggers slash.
        client.set_slash_vote_quorum(&admins, &1);

        env.ledger().with_mut(|l| l.timestamp = 120);

        Setup { env, client, token_id: token_id.address(), admin_vec: admins }
    }

    fn do_vouch(s: &Setup, voucher: &Address, borrower: &Address, stake: i128) {
        StellarAssetClient::new(&s.env, &s.token_id).mint(voucher, &stake);
        s.client.vouch(voucher, borrower, &stake, &s.token_id, &None);
    }

    /// Trigger a slash so the loan is Defaulted and voucher history is recorded.
    fn do_slash(s: &Setup, voucher: &Address, borrower: &Address) {
        s.client.vote_slash(voucher, borrower, &true);
    }

    // ── contribute ────────────────────────────────────────────────────────────

    #[test]
    fn test_contribute_increases_pool_balance() {
        let s = setup();
        let contributor = Address::generate(&s.env);
        StellarAssetClient::new(&s.env, &s.token_id).mint(&contributor, &500_000);

        assert_eq!(s.client.get_insurance_pool_balance(), 0);
        s.client.contribute_to_insurance(&contributor, &500_000).unwrap();
        assert_eq!(s.client.get_insurance_pool_balance(), 500_000);
    }

    #[test]
    #[should_panic]
    fn test_contribute_zero_amount_fails() {
        let s = setup();
        let contributor = Address::generate(&s.env);
        s.client.contribute_to_insurance(&contributor, &0).unwrap();
    }

    // ── claim ─────────────────────────────────────────────────────────────────

    #[test]
    fn test_claim_after_default_pays_voucher() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);
        let contributor = Address::generate(&s.env);

        // Fund the insurance pool.
        StellarAssetClient::new(&s.env, &s.token_id).mint(&contributor, &200_000);
        s.client.contribute_to_insurance(&contributor, &200_000).unwrap();

        // Set up loan and trigger default.
        do_vouch(&s, &voucher, &borrower, 500_000);
        s.client.request_loan(
            &borrower, &100_000, &500_000,
            &String::from_str(&s.env, "test"), &s.token_id,
        );
        let loan = s.client.get_loan(&borrower).unwrap();
        do_slash(&s, &voucher, &borrower);

        assert_eq!(s.client.loan_status(&borrower), crate::LoanStatus::Defaulted);

        let token = StellarAssetClient::new(&s.env, &s.token_id);
        let balance_before = token.balance(&voucher);

        s.client.claim_insurance(&voucher, &loan.id).unwrap();

        let balance_after = token.balance(&voucher);
        // Payout = min(pool=200_000, loan.amount=100_000) = 100_000
        assert_eq!(balance_after - balance_before, 100_000);
        assert_eq!(s.client.get_insurance_pool_balance(), 100_000);
    }

    #[test]
    #[should_panic]
    fn test_double_claim_fails() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);
        let contributor = Address::generate(&s.env);

        StellarAssetClient::new(&s.env, &s.token_id).mint(&contributor, &500_000);
        s.client.contribute_to_insurance(&contributor, &500_000).unwrap();

        do_vouch(&s, &voucher, &borrower, 500_000);
        s.client.request_loan(
            &borrower, &100_000, &500_000,
            &String::from_str(&s.env, "test"), &s.token_id,
        );
        let loan = s.client.get_loan(&borrower).unwrap();
        do_slash(&s, &voucher, &borrower);

        s.client.claim_insurance(&voucher, &loan.id).unwrap();
        // Second claim must fail.
        s.client.claim_insurance(&voucher, &loan.id).unwrap();
    }

    #[test]
    #[should_panic]
    fn test_claim_by_non_voucher_fails() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);
        let outsider = Address::generate(&s.env);
        let contributor = Address::generate(&s.env);

        StellarAssetClient::new(&s.env, &s.token_id).mint(&contributor, &500_000);
        s.client.contribute_to_insurance(&contributor, &500_000).unwrap();

        do_vouch(&s, &voucher, &borrower, 500_000);
        s.client.request_loan(
            &borrower, &100_000, &500_000,
            &String::from_str(&s.env, "test"), &s.token_id,
        );
        let loan = s.client.get_loan(&borrower).unwrap();
        do_slash(&s, &voucher, &borrower);

        // Outsider was not a voucher — must fail.
        s.client.claim_insurance(&outsider, &loan.id).unwrap();
    }

    #[test]
    #[should_panic]
    fn test_claim_on_empty_pool_fails() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);

        do_vouch(&s, &voucher, &borrower, 500_000);
        s.client.request_loan(
            &borrower, &100_000, &500_000,
            &String::from_str(&s.env, "test"), &s.token_id,
        );
        let loan = s.client.get_loan(&borrower).unwrap();
        do_slash(&s, &voucher, &borrower);

        // Pool is empty — must fail.
        s.client.claim_insurance(&voucher, &loan.id).unwrap();
    }

    #[test]
    #[should_panic]
    fn test_claim_on_active_loan_fails() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);
        let contributor = Address::generate(&s.env);

        StellarAssetClient::new(&s.env, &s.token_id).mint(&contributor, &500_000);
        s.client.contribute_to_insurance(&contributor, &500_000).unwrap();

        do_vouch(&s, &voucher, &borrower, 500_000);
        s.client.request_loan(
            &borrower, &100_000, &500_000,
            &String::from_str(&s.env, "test"), &s.token_id,
        );
        let loan = s.client.get_loan(&borrower).unwrap();

        // Loan is still Active — claim must fail.
        s.client.claim_insurance(&voucher, &loan.id).unwrap();
    }
}
