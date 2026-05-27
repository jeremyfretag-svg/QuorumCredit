#[cfg(test)]
mod vouch_age_yield_tests {
    use crate::{QuorumCreditContract, QuorumCreditContractClient};
    use soroban_sdk::{
        testutils::{Address as _, Ledger},
        token::StellarAssetClient,
        Address, Env, String, Vec,
    };

    const ONE_DAY: u64 = 24 * 60 * 60;
    const THIRTY_DAYS: u64 = 30 * ONE_DAY;

    struct Setup {
        env: Env,
        client: QuorumCreditContractClient<'static>,
        token: Address,
        admin: Address,
    }

    fn setup() -> Setup {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(1_000_000);

        let deployer = Address::generate(&env);
        let admin = Address::generate(&env);
        let token = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        let contract_id = env.register_contract(None, QuorumCreditContract);
        let client = QuorumCreditContractClient::new(&env, &contract_id);
        client.initialize(
            &deployer,
            &Vec::from_array(&env, [admin.clone()]),
            &1,
            &token,
        );
        Setup { env, client, token, admin }
    }

    fn mint(env: &Env, token: &Address, to: &Address, amount: i128) {
        StellarAssetClient::new(env, token).mint(to, &amount);
    }

    /// A vouch that is 30+ days old earns more yield than a fresh vouch.
    #[test]
    fn test_older_vouch_earns_higher_yield() {
        let s = setup();

        let voucher_old = Address::generate(&s.env);
        let voucher_new = Address::generate(&s.env);
        let borrower_old = Address::generate(&s.env);
        let borrower_new = Address::generate(&s.env);

        let stake: i128 = 10_000_000; // 1 XLM
        let loan_amount: i128 = 5_000_000; // 0.5 XLM

        // Fund contract for yield payouts
        mint(&s.env, &s.token, &s.admin, 10_000_000);
        StellarAssetClient::new(&s.env, &s.token).transfer(
            &s.admin,
            &s.client.address,
            &10_000_000,
        );

        // ── Old vouch: created at t=1_000_000, loan at t=1_000_000 + 35 days ──
        mint(&s.env, &s.token, &voucher_old, stake);
        s.client.vouch(&voucher_old, &borrower_old, &stake, &s.token, &None);

        s.env.ledger().set_timestamp(1_000_000 + 35 * ONE_DAY);
        mint(&s.env, &s.token, &borrower_old, loan_amount);
        s.client.request_loan(
            &borrower_old,
            &loan_amount,
            &stake,
            &String::from_str(&s.env, "old vouch test"),
            &s.token,
        );
        let loan_old = s.client.get_loan(&borrower_old).unwrap();

        // ── New vouch: created and loaned at same timestamp ──
        s.env.ledger().set_timestamp(1_000_000 + 36 * ONE_DAY);
        mint(&s.env, &s.token, &voucher_new, stake);
        s.client.vouch(&voucher_new, &borrower_new, &stake, &s.token, &None);
        mint(&s.env, &s.token, &borrower_new, loan_amount);
        s.client.request_loan(
            &borrower_new,
            &loan_amount,
            &stake,
            &String::from_str(&s.env, "new vouch test"),
            &s.token,
        );
        let loan_new = s.client.get_loan(&borrower_new).unwrap();

        // Old vouch (35 days old) should earn more total_yield than new vouch (0 days old)
        assert!(
            loan_old.total_yield > loan_new.total_yield,
            "older vouch should earn higher yield: old={} new={}",
            loan_old.total_yield,
            loan_new.total_yield
        );
    }

    /// A borrower with prior successful repayments earns higher yield for their vouchers.
    #[test]
    fn test_repayment_history_increases_yield() {
        let s = setup();

        let voucher = Address::generate(&s.env);
        let borrower = Address::generate(&s.env);
        let stake: i128 = 10_000_000;
        let loan_amount: i128 = 5_000_000;

        // Fund contract
        mint(&s.env, &s.token, &s.admin, 50_000_000);
        StellarAssetClient::new(&s.env, &s.token).transfer(
            &s.admin,
            &s.client.address,
            &50_000_000,
        );

        // ── First loan: no prior repayments ──
        mint(&s.env, &s.token, &voucher, stake);
        s.client.vouch(&voucher, &borrower, &stake, &s.token, &None);
        mint(&s.env, &s.token, &borrower, loan_amount + 200);
        s.client.request_loan(
            &borrower,
            &loan_amount,
            &stake,
            &String::from_str(&s.env, "first loan"),
            &s.token,
        );
        let first_loan = s.client.get_loan(&borrower).unwrap();
        let first_yield = first_loan.total_yield;

        // Repay first loan fully
        let total_owed = first_loan.amount + first_loan.total_yield;
        s.client.repay(&borrower, &total_owed);

        // ── Second loan: 1 prior repayment → reputation bonus ──
        s.env.ledger().set_timestamp(1_000_000 + ONE_DAY * 2);
        mint(&s.env, &s.token, &voucher, stake);
        s.client.vouch(&voucher, &borrower, &stake, &s.token, &None);
        mint(&s.env, &s.token, &borrower, loan_amount + 200);
        s.client.request_loan(
            &borrower,
            &loan_amount,
            &stake,
            &String::from_str(&s.env, "second loan"),
            &s.token,
        );
        let second_loan = s.client.get_loan(&borrower).unwrap();
        let second_yield = second_loan.total_yield;

        assert!(
            second_yield > first_yield,
            "borrower with repayment history should earn higher yield: first={} second={}",
            first_yield,
            second_yield
        );
    }

    /// A fresh vouch (age < 7 days) earns only the base yield rate.
    #[test]
    fn test_fresh_vouch_earns_base_yield_only() {
        let s = setup();

        let voucher = Address::generate(&s.env);
        let borrower = Address::generate(&s.env);
        let stake: i128 = 10_000_000;
        let loan_amount: i128 = 5_000_000;

        mint(&s.env, &s.token, &s.admin, 10_000_000);
        StellarAssetClient::new(&s.env, &s.token).transfer(
            &s.admin,
            &s.client.address,
            &10_000_000,
        );

        // Vouch and loan at same timestamp (age = 0)
        mint(&s.env, &s.token, &voucher, stake);
        s.client.vouch(&voucher, &borrower, &stake, &s.token, &None);
        mint(&s.env, &s.token, &borrower, loan_amount);
        s.client.request_loan(
            &borrower,
            &loan_amount,
            &stake,
            &String::from_str(&s.env, "fresh vouch"),
            &s.token,
        );

        let loan = s.client.get_loan(&borrower).unwrap();
        // base yield = 200 bps = 2% of loan_amount
        let expected_base_yield = loan_amount * 200 / 10_000;
        assert_eq!(
            loan.total_yield, expected_base_yield,
            "fresh vouch should earn exactly base yield"
        );
    }
}
