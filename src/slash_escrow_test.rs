/// Slash Escrow Tests (Issue #550)
///
/// Verifies that slashed funds are held in escrow for a period before being burned,
/// allowing for dispute resolution.

#[cfg(test)]
mod slash_escrow_tests {
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

        // Fund contract generously
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

    /// Test slash creates escrow entry
    #[test]
    fn test_slash_creates_escrow() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);

        do_vouch(&s, &voucher, &borrower, 500_000);
        s.client.request_loan(&borrower, &100_000, &500_000, &purpose(&s.env), &s.token_id);

        // Set quorum to 1 so single voucher can slash
        s.client.set_slash_vote_quorum(&s.admin_vec, &1);

        // Vote to slash
        s.client.vote_slash(&voucher, &borrower, &true);

        // Verify escrow was created (slashed amount = 500_000 * 5000 / 10_000 = 250_000)
        // We can't directly query escrow, but we can verify the loan is defaulted
        let loan = s.client.get_loan(&borrower).unwrap();
        assert_eq!(loan.status, crate::types::LoanStatus::Defaulted, "loan should be defaulted");
    }

    /// Test release_slash_escrow fails before escrow period expires
    #[test]
    fn test_release_slash_escrow_fails_before_expiry() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);

        do_vouch(&s, &voucher, &borrower, 500_000);
        s.client.request_loan(&borrower, &100_000, &500_000, &purpose(&s.env), &s.token_id);

        s.client.set_slash_vote_quorum(&s.admin_vec, &1);
        s.client.vote_slash(&voucher, &borrower, &true);

        // Try to release escrow immediately (should fail)
        let result = s.client.try_release_slash_escrow(&s.admin_vec, &borrower);
        assert!(result.is_err(), "expected error when releasing escrow before expiry");
    }

    /// Test release_slash_escrow succeeds after escrow period expires
    #[test]
    fn test_release_slash_escrow_succeeds_after_expiry() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);

        do_vouch(&s, &voucher, &borrower, 500_000);
        s.client.request_loan(&borrower, &100_000, &500_000, &purpose(&s.env), &s.token_id);

        s.client.set_slash_vote_quorum(&s.admin_vec, &1);
        s.client.vote_slash(&voucher, &borrower, &true);

        // Advance time by 30 days + 1 second (SLASH_ESCROW_PERIOD = 30 days)
        let escrow_period: u64 = 30 * 24 * 60 * 60;
        s.env.ledger().with_mut(|l| l.timestamp = l.timestamp + escrow_period + 1);

        // Now release should succeed
        let result = s.client.try_release_slash_escrow(&s.admin_vec, &borrower);
        assert!(result.is_ok(), "expected release_slash_escrow to succeed after escrow period");
    }

    /// Test release_slash_escrow fails for non-existent escrow
    #[test]
    fn test_release_slash_escrow_fails_for_nonexistent() {
        let s = setup();
        let borrower = Address::generate(&s.env);

        // Try to release escrow for borrower with no escrow
        let result = s.client.try_release_slash_escrow(&s.admin_vec, &borrower);
        assert!(result.is_err(), "expected error when releasing non-existent escrow");
    }

    /// Test escrow period is 30 days
    #[test]
    fn test_escrow_period_is_30_days() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);

        do_vouch(&s, &voucher, &borrower, 500_000);
        s.client.request_loan(&borrower, &100_000, &500_000, &purpose(&s.env), &s.token_id);

        let slash_time = s.env.ledger().timestamp();
        s.client.set_slash_vote_quorum(&s.admin_vec, &1);
        s.client.vote_slash(&voucher, &borrower, &true);

        // Advance time by 29 days (should still fail)
        s.env.ledger().with_mut(|l| l.timestamp = slash_time + (29 * 24 * 60 * 60));
        let result = s.client.try_release_slash_escrow(&s.admin_vec, &borrower);
        assert!(result.is_err(), "expected error at 29 days");

        // Advance time by 30 days + 1 second (should succeed)
        s.env.ledger().with_mut(|l| l.timestamp = slash_time + (30 * 24 * 60 * 60) + 1);
        let result = s.client.try_release_slash_escrow(&s.admin_vec, &borrower);
        assert!(result.is_ok(), "expected success at 30 days + 1 second");
    }
}
