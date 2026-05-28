#[cfg(test)]
mod slash_cooldown_tests {
    use crate::{ContractError, LoanStatus, QuorumCreditContract, QuorumCreditContractClient};
    use soroban_sdk::{
        testutils::{Address as _, Ledger},
        token::StellarAssetClient,
        Address, Env, String, Vec,
    };

    struct Setup {
        env: Env,
        client: QuorumCreditContractClient<'static>,
        admin: Address,
        token_id: Address,
    }

    fn setup_with_cooldown(cooldown_secs: u64) -> Setup {
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

        let mut cfg = client.get_config();
        cfg.slash_cooldown_seconds = cooldown_secs;
        client.set_config(&Vec::from_array(&env, [admin.clone()]), &cfg);

        env.ledger().with_mut(|l| l.timestamp = 90_000);

        Setup {
            env,
            client,
            admin,
            token_id: token_id.address(),
        }
    }

    fn advance_time(s: &Setup, secs: u64) {
        let t = s.env.ledger().timestamp();
        s.env.ledger().with_mut(|l| l.timestamp = t + secs);
    }

    fn do_vouch(s: &Setup, voucher: &Address, borrower: &Address, stake: i128) {
        StellarAssetClient::new(&s.env, &s.token_id).mint(voucher, &stake);
        s.client.vouch(voucher, borrower, &stake, &s.token_id);
        advance_time(s, crate::types::DEFAULT_VOUCH_COOLDOWN_SECS + 1);
    }

    fn do_vouch_no_advance(s: &Setup, voucher: &Address, borrower: &Address, stake: i128) {
        StellarAssetClient::new(&s.env, &s.token_id).mint(voucher, &stake);
        s.client.vouch(voucher, borrower, &stake, &s.token_id);
    }

    fn do_loan(s: &Setup, borrower: &Address, amount: i128, threshold: i128) {
        s.client.request_loan(
            borrower,
            &amount,
            &threshold,
            &String::from_str(&s.env, "test"),
            &s.token_id,
        );
    }

    fn slash_borrower(s: &Setup, borrower: &Address, voucher: &Address) {
        s.client.vote_slash(voucher, borrower, &true);
    }

    #[test]
    fn test_slash_within_cooldown_rejected() {
        let s = setup_with_cooldown(3600);
        let borrower = Address::generate(&s.env);
        let voucher1 = Address::generate(&s.env);
        let voucher2 = Address::generate(&s.env);

        do_vouch(&s, &voucher1, &borrower, 1_000_000);
        do_loan(&s, &borrower, 100_000, 500_000);
        slash_borrower(&s, &borrower, &voucher1);

        do_vouch_no_advance(&s, &voucher2, &borrower, 1_000_000);
        do_loan(&s, &borrower, 100_000, 500_000);

        let result = s.client.try_vote_slash(&voucher2, &borrower, &true);
        assert_eq!(result, Err(Ok(ContractError::SlashCooldownActive)));
    }

    #[test]
    fn test_slash_after_cooldown_succeeds() {
        let s = setup_with_cooldown(3600);
        let borrower = Address::generate(&s.env);
        let voucher1 = Address::generate(&s.env);
        let voucher2 = Address::generate(&s.env);

        do_vouch(&s, &voucher1, &borrower, 1_000_000);
        do_loan(&s, &borrower, 100_000, 500_000);
        slash_borrower(&s, &borrower, &voucher1);

        advance_time(&s, 3601);
        do_vouch_no_advance(&s, &voucher2, &borrower, 1_000_000);
        do_loan(&s, &borrower, 100_000, 500_000);
        slash_borrower(&s, &borrower, &voucher2);
        assert_eq!(s.client.loan_status(&borrower), LoanStatus::Defaulted);
    }

    #[test]
    fn test_zero_cooldown_allows_immediate_reslash() {
        let s = setup_with_cooldown(0);
        let borrower = Address::generate(&s.env);
        let voucher1 = Address::generate(&s.env);
        let voucher2 = Address::generate(&s.env);

        do_vouch(&s, &voucher1, &borrower, 1_000_000);
        do_loan(&s, &borrower, 100_000, 500_000);
        slash_borrower(&s, &borrower, &voucher1);

        do_vouch_no_advance(&s, &voucher2, &borrower, 1_000_000);
        do_loan(&s, &borrower, 100_000, 500_000);
        slash_borrower(&s, &borrower, &voucher2);
        assert_eq!(s.client.loan_status(&borrower), LoanStatus::Defaulted);
    }
}
