/// Voucher reputation/history tracking tests (Issue #602)
///
/// Verifies that `VoucherStats` is correctly updated on:
/// - Successful loan repayment (successful_vouches, total_yield_earned)
/// - Slash execution (total_vouches_slashed, total_slashed)
/// - No history (zeroed defaults)
#[cfg(test)]
mod voucher_stats_tests {
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
    }

    fn setup() -> Setup {
        let env = Env::default();
        env.mock_all_auths();

        let deployer = Address::generate(&env);
        let admin = Address::generate(&env);
        let admins = Vec::from_array(&env, [admin.clone()]);

        let token_id = env.register_stellar_asset_contract_v2(admin.clone());
        let contract_id = env.register_contract(None, QuorumCreditContract);

        // Fund contract for loan disbursement + yield payouts
        StellarAssetClient::new(&env, &token_id.address()).mint(&contract_id, &100_000_000);

        let client = QuorumCreditContractClient::new(&env, &contract_id);
        client.initialize(&deployer, &admins, &1, &token_id.address());

        // Advance past MIN_VOUCH_AGE (60s)
        env.ledger().with_mut(|l| l.timestamp = 120);

        Setup { env, client, token_id: token_id.address() }
    }

    fn do_vouch(s: &Setup, voucher: &Address, borrower: &Address, stake: i128) {
        StellarAssetClient::new(&s.env, &s.token_id).mint(voucher, &stake);
        s.client.vouch(voucher, borrower, &stake, &s.token_id, &None);
    }

    fn purpose(env: &Env) -> String {
        String::from_str(env, "test loan")
    }

    /// A voucher with no activity should return zeroed stats.
    #[test]
    fn test_get_voucher_stats_default_zeroed() {
        let s = setup();
        let voucher = Address::generate(&s.env);
        let stats = s.client.get_voucher_stats(&voucher);
        assert_eq!(stats.successful_vouches, 0);
        assert_eq!(stats.total_vouches_slashed, 0);
        assert_eq!(stats.total_yield_earned, 0);
        assert_eq!(stats.total_slashed, 0);
    }

    /// After a successful repayment, the voucher's successful_vouches and
    /// total_yield_earned should be incremented.
    #[test]
    fn test_voucher_stats_incremented_on_repay() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);
        let stake = 1_000_000i128;
        let loan_amount = 500_000i128;
        let yield_bps = 200i128;
        let total_yield = loan_amount * yield_bps / 10_000; // 10_000
        let total_repay = loan_amount + total_yield;

        do_vouch(&s, &voucher, &borrower, stake);
        s.client.request_loan(&borrower, &loan_amount, &stake, &purpose(&s.env), &s.token_id);

        StellarAssetClient::new(&s.env, &s.token_id).mint(&borrower, &total_repay);
        s.client.repay(&borrower, &total_repay);

        let stats = s.client.get_voucher_stats(&voucher);
        assert_eq!(stats.successful_vouches, 1, "should record one successful vouch");
        assert_eq!(stats.total_vouches_slashed, 0, "no slashes");
        assert_eq!(stats.total_yield_earned, total_yield, "yield earned should match");
        assert_eq!(stats.total_slashed, 0, "no slashed amount");
    }

    /// After a slash, the voucher's total_vouches_slashed and total_slashed
    /// should be incremented.
    #[test]
    fn test_voucher_stats_incremented_on_slash() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);
        let stake = 1_000_000i128;
        let loan_amount = 500_000i128;

        do_vouch(&s, &voucher, &borrower, stake);
        s.client.request_loan(&borrower, &loan_amount, &stake, &purpose(&s.env), &s.token_id);

        // Trigger slash via vote (voucher votes approve, quorum met)
        s.client.vote_slash(&voucher, &borrower, &true);

        let stats = s.client.get_voucher_stats(&voucher);
        assert_eq!(stats.successful_vouches, 0, "no successful repayments");
        assert_eq!(stats.total_vouches_slashed, 1, "should record one slash");
        assert!(stats.total_slashed > 0, "slashed amount should be positive");
        assert_eq!(stats.total_yield_earned, 0, "no yield earned");
    }

    /// Stats accumulate correctly across multiple loans for the same voucher.
    #[test]
    fn test_voucher_stats_accumulate_across_multiple_loans() {
        let s = setup();
        let voucher = Address::generate(&s.env);
        let stake = 1_000_000i128;
        let loan_amount = 500_000i128;
        let yield_bps = 200i128;
        let total_yield = loan_amount * yield_bps / 10_000;
        let total_repay = loan_amount + total_yield;

        // First loan: repaid
        let borrower1 = Address::generate(&s.env);
        do_vouch(&s, &voucher, &borrower1, stake);
        s.client.request_loan(&borrower1, &loan_amount, &stake, &purpose(&s.env), &s.token_id);
        StellarAssetClient::new(&s.env, &s.token_id).mint(&borrower1, &total_repay);
        s.client.repay(&borrower1, &total_repay);

        // Advance time past vouch cooldown for second vouch
        s.env.ledger().with_mut(|l| l.timestamp += 24 * 60 * 60 + 1);

        // Second loan: slashed
        let borrower2 = Address::generate(&s.env);
        do_vouch(&s, &voucher, &borrower2, stake);
        s.client.request_loan(&borrower2, &loan_amount, &stake, &purpose(&s.env), &s.token_id);
        s.client.vote_slash(&voucher, &borrower2, &true);

        let stats = s.client.get_voucher_stats(&voucher);
        assert_eq!(stats.successful_vouches, 1, "one successful repayment");
        assert_eq!(stats.total_vouches_slashed, 1, "one slash");
        assert_eq!(stats.total_yield_earned, total_yield, "yield from first loan");
        assert!(stats.total_slashed > 0, "slashed amount from second loan");
    }

    /// With two vouchers, each gets independent stats.
    #[test]
    fn test_voucher_stats_independent_per_voucher() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        let voucher1 = Address::generate(&s.env);
        let voucher2 = Address::generate(&s.env);
        let stake1 = 600_000i128;
        let stake2 = 400_000i128;
        let loan_amount = 500_000i128;
        let total_stake = stake1 + stake2;
        let yield_bps = 200i128;
        let total_yield = loan_amount * yield_bps / 10_000;
        let total_repay = loan_amount + total_yield;

        do_vouch(&s, &voucher1, &borrower, stake1);
        do_vouch(&s, &voucher2, &borrower, stake2);
        s.client.request_loan(&borrower, &loan_amount, &total_stake, &purpose(&s.env), &s.token_id);

        StellarAssetClient::new(&s.env, &s.token_id).mint(&borrower, &total_repay);
        s.client.repay(&borrower, &total_repay);

        let stats1 = s.client.get_voucher_stats(&voucher1);
        let stats2 = s.client.get_voucher_stats(&voucher2);

        assert_eq!(stats1.successful_vouches, 1);
        assert_eq!(stats2.successful_vouches, 1);

        // Yield is proportional to stake
        let expected_yield1 = total_yield * stake1 / total_stake;
        let expected_yield2 = total_yield * stake2 / total_stake;
        assert_eq!(stats1.total_yield_earned, expected_yield1);
        assert_eq!(stats2.total_yield_earned, expected_yield2);

        // No slashes
        assert_eq!(stats1.total_vouches_slashed, 0);
        assert_eq!(stats2.total_vouches_slashed, 0);
    }
}
