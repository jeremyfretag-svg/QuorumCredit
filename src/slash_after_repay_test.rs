/// Slash-After-Repay Test
///
/// Verifies that `vote_slash` cannot be called on a loan that has already been
/// fully repaid. After repayment the loan status is `Repaid` and any attempt
/// to slash must panic with "loan already repaid".
#[cfg(test)]
mod slash_after_repay_tests {
    use crate::{LoanStatus, QuorumCreditContract, QuorumCreditContractClient};
    use soroban_sdk::{
        testutils::{Address as _, Ledger},
        token::StellarAssetClient,
        Address, Env, String, Vec,
    };

    struct Setup {
        env: Env,
        client: QuorumCreditContractClient<'static>,
        admin: Address,
        admin_vec: Vec<Address>,
        token_id: Address,
    }

    fn setup() -> Setup {
        let env = Env::default();
        env.mock_all_auths();

        let deployer = Address::generate(&env);
        let admin = Address::generate(&env);
        let admins = Vec::from_array(&env, [admin.clone()]);

        let token_id = env.register_stellar_asset_contract_v2(admin.clone());
        let contract_id = env.register_contract(None, QuorumCreditContract);

        // Fund the contract so it can disburse loans.
        StellarAssetClient::new(&env, &token_id.address()).mint(&contract_id, &10_000_000);

        let client = QuorumCreditContractClient::new(&env, &contract_id);
        client.initialize(&deployer, &admins, &1, &token_id.address());

        // Advance past MIN_VOUCH_AGE (60 s) so vouches are eligible.
        env.ledger().with_mut(|l| l.timestamp = 120);

        Setup {
            env,
            client,
            admin,
            admin_vec: admins,
            token_id: token_id.address(),
        }
    }

    /// Helper: mint tokens to voucher and vouch for borrower.
    fn do_vouch(s: &Setup, voucher: &Address, borrower: &Address, stake: i128) {
        StellarAssetClient::new(&s.env, &s.token_id).mint(voucher, &stake);
        s.client.vouch(voucher, borrower, &stake, &s.token_id, &None);
    }

    /// Helper: request a loan for borrower (loan amount = 100_000, threshold = stake).
    fn do_loan(s: &Setup, borrower: &Address, threshold: i128) {
        s.client.request_loan(
            borrower,
            &100_000,
            &threshold,
            &String::from_str(&s.env, "test purpose"),
            &s.token_id,
        );
    }

    /// Helper: fully repay the active loan for borrower.
    fn do_repay(s: &Setup, borrower: &Address) {
        let loan = s.client.get_loan(borrower).expect("loan must exist");
        let total_owed = loan.amount + loan.total_yield;
        // Mint repayment amount to borrower so they can pay.
        StellarAssetClient::new(&s.env, &s.token_id).mint(borrower, &total_owed);
        s.client.repay(borrower, &total_owed);
    }

    // ── Primary test ──────────────────────────────────────────────────────────

    /// Repay a loan then attempt to slash — must panic with "loan already repaid".
    #[test]
    #[should_panic(expected = "loan already repaid")]
    fn test_slash_after_repay_panics() {
        let s = setup();
        // Set quorum to 1 bps so a single voucher vote would trigger slash immediately.
        s.client.set_slash_vote_quorum(&s.admin_vec, &1);

        let voucher = Address::generate(&s.env);
        let borrower = Address::generate(&s.env);
        let stake = 1_000_000i128;

        do_vouch(&s, &voucher, &borrower, stake);
        do_loan(&s, &borrower, stake);

        // Confirm loan is active before repayment.
        assert_eq!(s.client.loan_status(&borrower), LoanStatus::Active);

        // Fully repay the loan.
        do_repay(&s, &borrower);

        // Confirm loan is now repaid.
        assert_eq!(s.client.loan_status(&borrower), LoanStatus::Repaid);

        // Attempt to slash a repaid loan — must panic with "loan already repaid".
        s.client.vote_slash(&voucher, &borrower, &true);
    }

    // ── Preservation tests ────────────────────────────────────────────────────

    /// Sanity check: slash on an active (non-repaid) loan still works normally.
    #[test]
    fn test_slash_on_active_loan_succeeds() {
        let s = setup();
        // Set quorum to 1 bps so a single voucher vote triggers slash immediately.
        s.client.set_slash_vote_quorum(&s.admin_vec, &1);

        let voucher = Address::generate(&s.env);
        let borrower = Address::generate(&s.env);
        let stake = 1_000_000i128;

        do_vouch(&s, &voucher, &borrower, stake);
        do_loan(&s, &borrower, stake);

        assert_eq!(s.client.loan_status(&borrower), LoanStatus::Active);

        // Slash without repaying — must succeed and mark loan as Defaulted.
        s.client.vote_slash(&voucher, &borrower, &true);

        assert_eq!(s.client.loan_status(&borrower), LoanStatus::Defaulted);
    }

    /// Sanity check: slash on a borrower with no loan at all returns an error.
    #[test]
    fn test_slash_with_no_loan_returns_error() {
        let s = setup();

        let voucher = Address::generate(&s.env);
        let borrower = Address::generate(&s.env);

        do_vouch(&s, &voucher, &borrower, 1_000_000);

        // No loan requested — vote_slash must return an error, not panic.
        let result = s.client.try_vote_slash(&voucher, &borrower, &true);
        assert!(result.is_err(), "slash with no loan must return an error");
    }
}
