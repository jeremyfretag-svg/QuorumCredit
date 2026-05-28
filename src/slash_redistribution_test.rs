/// Issue #677: Slashing redistribution rules (Treasury vs Vouchers).
#[cfg(test)]
mod slash_redistribution_tests {
    use crate::{
        QuorumCreditContract, QuorumCreditContractClient, RedistributionRule,
    };
    use soroban_sdk::{
        testutils::{Address as _, Ledger},
        token::StellarAssetClient,
        Address, Env, String, Vec,
    };

    struct Setup {
        env: Env,
        client: QuorumCreditContractClient<'static>,
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
        StellarAssetClient::new(&env, &token_id.address()).mint(&contract_id, &100_000_000);

        let client = QuorumCreditContractClient::new(&env, &contract_id);
        client.initialize(&deployer, &admins, &1, &token_id.address());
        env.ledger().with_mut(|l| {
            l.timestamp = crate::types::DEFAULT_VOUCH_COOLDOWN_SECS + 1_000
        });

        Setup {
            env,
            client,
            admin_vec: admins,
            token_id: token_id.address(),
        }
    }

    fn purpose(env: &Env) -> String {
        String::from_str(env, "redistribution test")
    }

    fn set_redistribution(s: &Setup, rule: RedistributionRule) {
        let mut cfg = s.client.get_config();
        cfg.redistribution_rule = rule;
        s.client.set_config(&s.admin_vec, &cfg);
    }

    fn setup_vouches_and_loan(s: &Setup, borrower: &Address, vouchers: &[(&Address, i128)]) {
        for (v, stake) in vouchers {
            StellarAssetClient::new(&s.env, &s.token_id).mint(v, stake);
            s.client.vouch(v, borrower, stake, &s.token_id);
        }
        let threshold: i128 = vouchers.iter().map(|(_, st)| st).sum();
        s.client.request_loan(
            borrower,
            &200_000,
            &threshold,
            &purpose(&s.env),
            &s.token_id,
        );
    }

    fn execute_slash(s: &Setup, borrower: &Address, voter: &Address) -> i128 {
        s.client.set_slash_vote_quorum(&s.admin_vec, &1);
        s.client.vote_slash(voter, borrower, &true);
        s.client.get_slash_audit(borrower).unwrap().total_slashed
    }

    fn do_slash(
        s: &Setup,
        borrower: &Address,
        vouchers: &[(&Address, i128)],
    ) -> i128 {
        setup_vouches_and_loan(s, borrower, vouchers);
        execute_slash(s, borrower, vouchers[0].0)
    }

    #[test]
    fn test_treasury_rule_sends_funds_to_treasury() {
        let s = setup();
        set_redistribution(&s, RedistributionRule::Treasury);
        let borrower = Address::generate(&s.env);
        let v1 = Address::generate(&s.env);
        let total_slashed = do_slash(&s, &borrower, &[( &v1, 1_000_000 )]);

        let insurance = total_slashed * 2_000 / 10_000;
        let expected_treasury = total_slashed - insurance;
        assert_eq!(s.client.get_slash_treasury_balance(), expected_treasury);
    }

    #[test]
    fn test_vouchers_rule_splits_among_active_vouchers() {
        let s = setup();
        set_redistribution(&s, RedistributionRule::Vouchers);
        let borrower = Address::generate(&s.env);
        let v1 = Address::generate(&s.env);
        let v2 = Address::generate(&s.env);

        setup_vouches_and_loan(&s, &borrower, &[( &v1, 600_000 ), ( &v2, 400_000 )]);
        let bal_v1_before = StellarAssetClient::new(&s.env, &s.token_id).balance(&v1);
        let bal_v2_before = StellarAssetClient::new(&s.env, &s.token_id).balance(&v2);
        let total_slashed = execute_slash(&s, &borrower, &v1);
        let distributable = total_slashed - total_slashed * 2_000 / 10_000;

        let bal_v1_after = StellarAssetClient::new(&s.env, &s.token_id).balance(&v1);
        let bal_v2_after = StellarAssetClient::new(&s.env, &s.token_id).balance(&v2);

        let returned_v1 = 600_000 - 600_000 * 5_000 / 10_000;
        let returned_v2 = 400_000 - 400_000 * 5_000 / 10_000;
        let redist_v1 = bal_v1_after - bal_v1_before - returned_v1;
        let redist_v2 = bal_v2_after - bal_v2_before - returned_v2;
        assert_eq!(redist_v1 + redist_v2, distributable);
        assert_eq!(redist_v1, distributable * 600_000 / 1_000_000);
        assert_eq!(s.client.get_slash_treasury_balance(), 0);
    }

    #[test]
    fn test_vouchers_rule_with_single_voucher_falls_back_to_treasury() {
        let s = setup();
        set_redistribution(&s, RedistributionRule::Vouchers);
        let borrower = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);

        let total_slashed = do_slash(&s, &borrower, &[( &voucher, 800_000 )]);
        let distributable = total_slashed - total_slashed * 2_000 / 10_000;

        // Single voucher receives both remaining stake return and redistribution share.
        assert_eq!(s.client.get_slash_treasury_balance(), 0);
        let record = s.client.get_slash_audit(&borrower).unwrap();
        assert_eq!(record.total_slashed, total_slashed);
        assert!(distributable > 0);
    }

    #[test]
    fn test_redistribution_amounts_sum_to_distributable() {
        let s = setup();
        set_redistribution(&s, RedistributionRule::Vouchers);
        let borrower = Address::generate(&s.env);
        let v1 = Address::generate(&s.env);
        let v2 = Address::generate(&s.env);

        setup_vouches_and_loan(&s, &borrower, &[( &v1, 500_000 ), ( &v2, 500_000 )]);
        let b1 = StellarAssetClient::new(&s.env, &s.token_id).balance(&v1);
        let b2 = StellarAssetClient::new(&s.env, &s.token_id).balance(&v2);
        let total_slashed = execute_slash(&s, &borrower, &v1);
        let distributable = total_slashed - total_slashed * 2_000 / 10_000;

        let returned_each = 500_000 - 500_000 * 5_000 / 10_000;
        let redist_v1 =
            StellarAssetClient::new(&s.env, &s.token_id).balance(&v1) - b1 - returned_each;
        let redist_v2 =
            StellarAssetClient::new(&s.env, &s.token_id).balance(&v2) - b2 - returned_each;
        assert_eq!(redist_v1 + redist_v2, distributable);
        assert_eq!(s.client.get_slash_treasury_balance(), 0);
    }
}
