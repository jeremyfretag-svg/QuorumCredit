/// Issue #676: Slashing partial recovery on full repay after default.
#[cfg(test)]
mod slash_recovery_tests {
    use crate::{
        ContractError, LoanStatus, QuorumCreditContract, QuorumCreditContractClient, RedistributionRule,
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
        StellarAssetClient::new(&env, &token_id.address()).mint(&contract_id, &50_000_000);

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
        String::from_str(env, "recovery test")
    }

    fn set_recovery_bps(s: &Setup, recovery_percentage: u32) {
        let mut cfg = s.client.get_config();
        cfg.recovery_percentage = recovery_percentage;
        cfg.redistribution_rule = RedistributionRule::Treasury;
        s.client.set_config(&s.admin_vec, &cfg);
    }

    fn slash_borrower(s: &Setup, borrower: &Address, voucher: &Address, stake: i128) -> i128 {
        StellarAssetClient::new(&s.env, &s.token_id).mint(voucher, &stake);
        s.client.vouch(voucher, borrower, &stake, &s.token_id);
        s.client
            .request_loan(borrower, &100_000, &stake, &purpose(&s.env), &s.token_id);
        s.client.set_slash_vote_quorum(&s.admin_vec, &1);
        s.client.vote_slash(voucher, borrower, &true);
        let record = s.client.get_slash_audit(borrower).unwrap();
        record.total_slashed
    }

    #[test]
    fn test_full_repay_with_recovery_returns_correct_amount() {
        let s = setup();
        set_recovery_bps(&s, 2_500);
        let borrower = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);

        let total_slashed = slash_borrower(&s, &borrower, &voucher, 500_000);
        let expected_recovery = total_slashed * 2_500 / 10_000;

        let loan = s.client.get_loan(&borrower).unwrap();
        let total_owed = loan.amount + loan.total_yield;
        let treasury_before = s.client.get_slash_treasury_balance();
        let bal_before = StellarAssetClient::new(&s.env, &s.token_id).balance(&borrower);

        StellarAssetClient::new(&s.env, &s.token_id).mint(&borrower, &total_owed);
        s.client.repay(&borrower, &total_owed);

        let record = s.client.get_slash_audit(&borrower).unwrap();
        assert_eq!(record.recovery_amount, expected_recovery);
        assert_eq!(
            StellarAssetClient::new(&s.env, &s.token_id).balance(&borrower),
            bal_before + expected_recovery
        );
        assert_eq!(
            s.client.get_slash_treasury_balance(),
            treasury_before - expected_recovery
        );
        assert_eq!(s.client.get_loan(&borrower).unwrap().status, LoanStatus::Repaid);
    }

    #[test]
    fn test_zero_recovery_percentage_returns_nothing() {
        let s = setup();
        set_recovery_bps(&s, 0);
        let borrower = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);

        slash_borrower(&s, &borrower, &voucher, 500_000);
        let loan = s.client.get_loan(&borrower).unwrap();
        let total_owed = loan.amount + loan.total_yield;
        let treasury_before = s.client.get_slash_treasury_balance();
        let bal_before = StellarAssetClient::new(&s.env, &s.token_id).balance(&borrower);

        StellarAssetClient::new(&s.env, &s.token_id).mint(&borrower, &total_owed);
        s.client.repay(&borrower, &total_owed);

        let record = s.client.get_slash_audit(&borrower).unwrap();
        assert_eq!(record.recovery_amount, 0);
        assert_eq!(
            StellarAssetClient::new(&s.env, &s.token_id).balance(&borrower),
            bal_before
        );
        assert_eq!(s.client.get_slash_treasury_balance(), treasury_before);
    }

    #[test]
    fn test_partial_repay_does_not_trigger_recovery() {
        let s = setup();
        set_recovery_bps(&s, 5_000);
        let borrower = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);

        slash_borrower(&s, &borrower, &voucher, 500_000);
        let loan = s.client.get_loan(&borrower).unwrap();
        let partial = (loan.amount + loan.total_yield) / 2;

        StellarAssetClient::new(&s.env, &s.token_id).mint(&borrower, &partial);
        s.client.repay(&borrower, &partial);

        let record = s.client.get_slash_audit(&borrower).unwrap();
        assert_eq!(record.recovery_amount, 0);
        assert_eq!(s.client.get_loan(&borrower).unwrap().status, LoanStatus::Defaulted);
    }
}
