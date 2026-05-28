/// Tests for oracle credit score integration (feat/oracle-credit-score).
///
/// Covers:
/// - set_oracle() registers oracle address (admin-only)
/// - update_credit_score_from_oracle() stores score; rejects wrong oracle
/// - get_external_credit_score() returns stored score or None
/// - score out of range (> 1000) rejected with InvalidCreditScore
/// - non-oracle caller rejected with OracleUnauthorized
/// - low score (< 300) increases yield premium by 100 bps
/// - high score (>= 700) decreases yield by 50 bps
/// - neutral score (300–699) leaves yield unchanged
#[cfg(test)]
mod oracle_credit_score_tests {
    use crate::{ContractError, QuorumCreditContract, QuorumCreditContractClient};
    use soroban_sdk::{
        testutils::{Address as _, Ledger},
        token::StellarAssetClient,
        Address, Env, String, Vec,
    };

    struct Setup {
        env: Env,
        client: QuorumCreditContractClient<'static>,
        token_id: Address,
        admin: Address,
        oracle: Address,
    }

    fn setup() -> Setup {
        let env = Env::default();
        env.mock_all_auths();

        let deployer = Address::generate(&env);
        let admin = Address::generate(&env);
        let oracle = Address::generate(&env);
        let token_id = env.register_stellar_asset_contract_v2(admin.clone());
        let contract_id = env.register_contract(None, QuorumCreditContract);

        StellarAssetClient::new(&env, &token_id.address()).mint(&contract_id, &100_000_000);

        let client = QuorumCreditContractClient::new(&env, &contract_id);
        client.initialize(
            &deployer,
            &Vec::from_array(&env, [admin.clone()]),
            &1,
            &token_id.address(),
        );

        Setup { env, client, token_id: token_id.address(), admin, oracle }
    }

    fn vouch_and_advance(s: &Setup, voucher: &Address, borrower: &Address, stake: i128) {
        StellarAssetClient::new(&s.env, &s.token_id).mint(voucher, &stake);
        s.env.ledger().with_mut(|l| l.timestamp = 90_000);
        s.client.vouch(voucher, borrower, &stake, &s.token_id, &None);
    }

    // ── set_oracle ────────────────────────────────────────────────────────────

    #[test]
    fn test_set_oracle_stores_address() {
        let s = setup();
        s.client.set_oracle(&Vec::from_array(&s.env, [s.admin.clone()]), &s.oracle);
        // Verify by pushing a score — only works if oracle is registered
        let borrower = Address::generate(&s.env);
        s.client.update_credit_score_from_oracle(&s.oracle, &borrower, &500);
        let cs = s.client.get_external_credit_score(&borrower).unwrap();
        assert_eq!(cs.oracle, s.oracle);
    }

    #[test]
    fn test_set_oracle_requires_admin() {
        let s = setup();
        let rando = Address::generate(&s.env);
        // Non-admin signer → UnauthorizedCaller
        let result = s.client.try_set_oracle(
            &Vec::from_array(&s.env, [rando]),
            &s.oracle,
        );
        assert_eq!(result, Err(Ok(ContractError::UnauthorizedCaller)));
    }

    // ── update_credit_score_from_oracle ───────────────────────────────────────

    #[test]
    fn test_update_score_stores_correctly() {
        let s = setup();
        s.client.set_oracle(&Vec::from_array(&s.env, [s.admin.clone()]), &s.oracle);
        let borrower = Address::generate(&s.env);

        s.client.update_credit_score_from_oracle(&s.oracle, &borrower, &750);

        let cs = s.client.get_external_credit_score(&borrower).unwrap();
        assert_eq!(cs.score, 750);
        assert_eq!(cs.oracle, s.oracle);
    }

    #[test]
    fn test_update_score_rejects_wrong_oracle() {
        let s = setup();
        s.client.set_oracle(&Vec::from_array(&s.env, [s.admin.clone()]), &s.oracle);
        let impostor = Address::generate(&s.env);
        let borrower = Address::generate(&s.env);

        let result = s.client.try_update_credit_score_from_oracle(&impostor, &borrower, &500);
        assert_eq!(result, Err(Ok(ContractError::OracleUnauthorized)));
    }

    #[test]
    fn test_update_score_rejects_no_oracle_registered() {
        let s = setup();
        // No set_oracle called
        let borrower = Address::generate(&s.env);
        let result = s.client.try_update_credit_score_from_oracle(&s.oracle, &borrower, &500);
        assert_eq!(result, Err(Ok(ContractError::OracleUnauthorized)));
    }

    #[test]
    fn test_update_score_rejects_out_of_range() {
        let s = setup();
        s.client.set_oracle(&Vec::from_array(&s.env, [s.admin.clone()]), &s.oracle);
        let borrower = Address::generate(&s.env);

        let result = s.client.try_update_credit_score_from_oracle(&s.oracle, &borrower, &1001);
        assert_eq!(result, Err(Ok(ContractError::InvalidCreditScore)));
    }

    #[test]
    fn test_update_score_accepts_boundary_values() {
        let s = setup();
        s.client.set_oracle(&Vec::from_array(&s.env, [s.admin.clone()]), &s.oracle);
        let borrower = Address::generate(&s.env);

        // 0 and 1000 are both valid
        s.client.update_credit_score_from_oracle(&s.oracle, &borrower, &0);
        assert_eq!(s.client.get_external_credit_score(&borrower).unwrap().score, 0);

        s.client.update_credit_score_from_oracle(&s.oracle, &borrower, &1000);
        assert_eq!(s.client.get_external_credit_score(&borrower).unwrap().score, 1000);
    }

    // ── get_external_credit_score ─────────────────────────────────────────────

    #[test]
    fn test_get_score_returns_none_when_not_set() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        assert!(s.client.get_external_credit_score(&borrower).is_none());
    }

    // ── yield adjustment in request_loan ─────────────────────────────────────

    #[test]
    fn test_low_score_increases_yield() {
        let s = setup();
        s.client.set_oracle(&Vec::from_array(&s.env, [s.admin.clone()]), &s.oracle);

        let borrower = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);
        vouch_and_advance(&s, &voucher, &borrower, 10_000_000);

        // Low score (< 300) → yield += 100 bps → effective = 300 bps (3%)
        s.client.update_credit_score_from_oracle(&s.oracle, &borrower, &200);
        s.client.request_loan(
            &borrower,
            &5_000_000,
            &5_000_000,
            &String::from_str(&s.env, "test"),
            &s.token_id,
        );

        let loan = s.client.get_loan(&borrower).unwrap();
        // base 200 bps + 100 bps premium = 300 bps → 5_000_000 * 300 / 10_000 = 150_000
        assert_eq!(loan.total_yield, 150_000);
    }

    #[test]
    fn test_high_score_decreases_yield() {
        let s = setup();
        s.client.set_oracle(&Vec::from_array(&s.env, [s.admin.clone()]), &s.oracle);

        let borrower = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);
        vouch_and_advance(&s, &voucher, &borrower, 10_000_000);

        // High score (>= 700) → yield -= 50 bps → effective = 150 bps (1.5%)
        s.client.update_credit_score_from_oracle(&s.oracle, &borrower, &800);
        s.client.request_loan(
            &borrower,
            &5_000_000,
            &5_000_000,
            &String::from_str(&s.env, "test"),
            &s.token_id,
        );

        let loan = s.client.get_loan(&borrower).unwrap();
        // base 200 bps - 50 bps discount = 150 bps → 5_000_000 * 150 / 10_000 = 75_000
        assert_eq!(loan.total_yield, 75_000);
    }

    #[test]
    fn test_neutral_score_no_yield_change() {
        let s = setup();
        s.client.set_oracle(&Vec::from_array(&s.env, [s.admin.clone()]), &s.oracle);

        let borrower = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);
        vouch_and_advance(&s, &voucher, &borrower, 10_000_000);

        // Neutral score (300–699) → no adjustment → 200 bps (2%)
        s.client.update_credit_score_from_oracle(&s.oracle, &borrower, &500);
        s.client.request_loan(
            &borrower,
            &5_000_000,
            &5_000_000,
            &String::from_str(&s.env, "test"),
            &s.token_id,
        );

        let loan = s.client.get_loan(&borrower).unwrap();
        // base 200 bps → 5_000_000 * 200 / 10_000 = 100_000
        assert_eq!(loan.total_yield, 100_000);
    }

    #[test]
    fn test_no_oracle_score_uses_base_yield() {
        let s = setup();

        let borrower = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);
        vouch_and_advance(&s, &voucher, &borrower, 10_000_000);

        // No oracle score set → base yield 200 bps
        s.client.request_loan(
            &borrower,
            &5_000_000,
            &5_000_000,
            &String::from_str(&s.env, "test"),
            &s.token_id,
        );

        let loan = s.client.get_loan(&borrower).unwrap();
        assert_eq!(loan.total_yield, 100_000);
    }
}
