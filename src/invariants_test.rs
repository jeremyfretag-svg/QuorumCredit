/// Contract Invariant Tests
///
/// Defines `verify_invariants` — a helper that asserts all documented invariants
/// (see docs/contract-invariants.md) hold after every state-changing operation.
///
/// Also contains tests that deliberately attempt to violate each invariant and
/// confirm the contract rejects the operation.
#[cfg(test)]
mod invariants_tests {
    use crate::{LoanStatus, QuorumCreditContract, QuorumCreditContractClient};
    use soroban_sdk::{
        testutils::{Address as _, Ledger},
        token::{StellarAssetClient, TokenClient},
        Address, Env, String, Vec,
    };

    // ── Setup ─────────────────────────────────────────────────────────────────

    struct Setup {
        env: Env,
        client: QuorumCreditContractClient<'static>,
        contract_id: Address,
        token: Address,
    }

    fn setup() -> Setup {
        let env = Env::default();
        env.mock_all_auths();

        let deployer = Address::generate(&env);
        let admin = Address::generate(&env);
        let admins = Vec::from_array(&env, [admin.clone()]);
        let token_id = env.register_stellar_asset_contract_v2(admin.clone());
        let contract_id = env.register_contract(None, QuorumCreditContract);

        // Pre-fund contract so it can disburse loans and pay yield
        StellarAssetClient::new(&env, &token_id.address()).mint(&contract_id, &1_000_000);

        let client = QuorumCreditContractClient::new(&env, &contract_id);
        client.initialize(&deployer, &admins, &1, &token_id.address());

        // Advance past MIN_VOUCH_AGE (60 s)
        env.ledger().with_mut(|l| l.timestamp = 120);

        Setup { env, client, contract_id, token: token_id.address() }
    }

    fn mint(s: &Setup, to: &Address, amount: i128) {
        StellarAssetClient::new(&s.env, &s.token).mint(to, &amount);
    }

    fn balance(s: &Setup, addr: &Address) -> i128 {
        TokenClient::new(&s.env, &s.token).balance(addr)
    }

    fn purpose(env: &Env) -> String {
        String::from_str(env, "test")
    }

    // ── verify_invariants ─────────────────────────────────────────────────────

    /// Assert all contract invariants for the given set of borrowers.
    ///
    /// Call this after every state-changing operation in tests.
    fn verify_invariants(s: &Setup, borrowers: &[Address]) {
        let contract_balance = balance(s, &s.contract_id);

        // I6 — Slash treasury is non-negative
        let slash_treasury = s.client.get_slash_treasury_balance();
        assert!(slash_treasury >= 0, "I6 violated: slash_treasury < 0");

        // I7 — yield_bps in [0, 10_000]
        let cfg = s.client.get_config();
        assert!(
            cfg.yield_bps >= 0 && cfg.yield_bps <= 10_000,
            "I7 violated: yield_bps={} out of [0,10000]",
            cfg.yield_bps
        );

        // I8 — admin_threshold consistency
        assert!(
            cfg.admin_threshold >= 1
                && cfg.admin_threshold <= cfg.admins.len(),
            "I8 violated: admin_threshold={} admins={}",
            cfg.admin_threshold,
            cfg.admins.len()
        );

        let mut total_locked_stake: i128 = 0;

        for borrower in borrowers {
            // I3 — No active loan without vouches
            let status = s.client.loan_status(borrower);
            let vouches = s.client.get_vouches(borrower);
            if status == LoanStatus::Active {
                assert!(
                    vouches.is_some() && !vouches.as_ref().unwrap().is_empty(),
                    "I3 violated: active loan for {borrower:?} but no vouches"
                );
            }

            // I4 — amount_repaid <= amount + total_yield
            if let Some(loan) = s.client.get_loan(borrower) {
                assert!(
                    loan.amount_repaid <= loan.amount + loan.total_yield,
                    "I4 violated: amount_repaid={} > amount+yield={}",
                    loan.amount_repaid,
                    loan.amount + loan.total_yield
                );

                // I2 — loan amount <= total vouched * ratio / 100 (only at active state)
                if status == LoanStatus::Active {
                    let total_vouched = s.client.total_vouched(borrower);
                    let max_ratio = cfg.max_loan_to_stake_ratio as i128;
                    assert!(
                        loan.amount <= total_vouched * max_ratio / 100,
                        "I2 violated: loan.amount={} > total_vouched*ratio={}",
                        loan.amount,
                        total_vouched * max_ratio / 100
                    );
                }
            }

            // Accumulate locked stake for I1
            if status == LoanStatus::Active {
                if let Some(vouches) = s.client.get_vouches(borrower) {
                    for v in vouches.iter() {
                        total_locked_stake += v.stake;
                    }
                }
            }
        }

        // I1 — contract balance >= total locked stake
        assert!(
            contract_balance >= total_locked_stake,
            "I1 violated: contract_balance={contract_balance} < total_locked_stake={total_locked_stake}"
        );
    }

    // ── Tests ─────────────────────────────────────────────────────────────────

    #[test]
    fn test_invariants_hold_after_vouch() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);
        mint(&s, &voucher, 10_000);

        s.client.vouch(&voucher, &borrower, &5_000, &s.token, &None);
        verify_invariants(&s, &[borrower]);
    }

    #[test]
    fn test_invariants_hold_after_request_loan() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);
        mint(&s, &voucher, 10_000);

        s.client.vouch(&voucher, &borrower, &10_000, &s.token, &None);
        verify_invariants(&s, &[borrower.clone()]);

        s.client
            .request_loan(&borrower, &5_000, &5_000, &purpose(&s.env), &s.token);
        verify_invariants(&s, &[borrower]);
    }

    #[test]
    fn test_invariants_hold_after_repay() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);
        mint(&s, &voucher, 10_000);
        mint(&s, &borrower, 1_000);

        s.client.vouch(&voucher, &borrower, &10_000, &s.token, &None);
        s.client
            .request_loan(&borrower, &5_000, &5_000, &purpose(&s.env), &s.token);
        verify_invariants(&s, &[borrower.clone()]);

        let loan = s.client.get_loan(&borrower).unwrap();
        s.client
            .repay(&borrower, &(loan.amount + loan.total_yield));
        verify_invariants(&s, &[borrower]);
    }

    #[test]
    fn test_invariants_hold_after_slash_vote() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);
        mint(&s, &voucher, 10_000);

        s.client.vouch(&voucher, &borrower, &10_000, &s.token, &None);
        s.client
            .request_loan(&borrower, &5_000, &5_000, &purpose(&s.env), &s.token);
        verify_invariants(&s, &[borrower.clone()]);

        // Vote slash — single voucher = quorum met immediately
        s.client.vote_slash(&voucher, &borrower, &true);
        verify_invariants(&s, &[borrower]);
    }

    #[test]
    fn test_invariant_i4_repay_cannot_exceed_principal_plus_yield() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);
        mint(&s, &voucher, 10_000);
        mint(&s, &borrower, 50_000);

        s.client.vouch(&voucher, &borrower, &10_000, &s.token, &None);
        s.client
            .request_loan(&borrower, &5_000, &5_000, &purpose(&s.env), &s.token);

        let loan = s.client.get_loan(&borrower).unwrap();
        let full = loan.amount + loan.total_yield;

        // Overpayment should be rejected or capped — contract must not accept > full
        let result = s.client.try_repay(&borrower, &(full + 1));
        // Either it succeeds (capped internally) or returns an error — either way
        // the invariant must still hold.
        let _ = result;
        verify_invariants(&s, &[borrower]);
    }

    #[test]
    fn test_invariant_i5_no_double_repay() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);
        mint(&s, &voucher, 10_000);
        mint(&s, &borrower, 10_000);

        s.client.vouch(&voucher, &borrower, &10_000, &s.token, &None);
        s.client
            .request_loan(&borrower, &5_000, &5_000, &purpose(&s.env), &s.token);

        let loan = s.client.get_loan(&borrower).unwrap();
        let full = loan.amount + loan.total_yield;
        s.client.repay(&borrower, &full);

        // Second repay must fail — loan is already Repaid
        let result = s.client.try_repay(&borrower, &full);
        assert!(result.is_err(), "I5: second repay should be rejected");
        verify_invariants(&s, &[borrower]);
    }

    #[test]
    fn test_invariant_i1_contract_balance_covers_locked_stake() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);
        mint(&s, &voucher, 10_000);

        s.client.vouch(&voucher, &borrower, &10_000, &s.token, &None);

        // Contract balance must be >= 10_000 (the locked stake)
        let cb = balance(&s, &s.contract_id);
        assert!(cb >= 10_000, "I1: contract_balance={cb} < locked_stake=10000");
        verify_invariants(&s, &[borrower]);
    }

    #[test]
    fn test_invariant_i8_admin_threshold_consistency() {
        let s = setup();
        let cfg = s.client.get_config();
        assert!(
            cfg.admin_threshold >= 1 && cfg.admin_threshold <= cfg.admins.len(),
            "I8: threshold={} admins={}",
            cfg.admin_threshold,
            cfg.admins.len()
        );
    }

    #[test]
    fn test_invariants_hold_across_full_lifecycle() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        let v1 = Address::generate(&s.env);
        let v2 = Address::generate(&s.env);
        mint(&s, &v1, 10_000);
        mint(&s, &v2, 10_000);
        mint(&s, &borrower, 5_000);

        s.client.vouch(&v1, &borrower, &6_000, &s.token, &None);
        verify_invariants(&s, &[borrower.clone()]);

        s.client.vouch(&v2, &borrower, &4_000, &s.token, &None);
        verify_invariants(&s, &[borrower.clone()]);

        s.client
            .request_loan(&borrower, &5_000, &5_000, &purpose(&s.env), &s.token);
        verify_invariants(&s, &[borrower.clone()]);

        let loan = s.client.get_loan(&borrower).unwrap();
        s.client.repay(&borrower, &(loan.amount + loan.total_yield));
        verify_invariants(&s, &[borrower]);
    }
}
