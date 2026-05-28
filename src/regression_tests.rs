/// Regression Test Suite
///
/// Each test here corresponds to a previously fixed bug. The test name and
/// comment reference the issue number so the fix can be traced back to the
/// original report. These tests run on every CI build to prevent regressions.
///
/// See docs/contract-invariants.md for the invariants these bugs violated.
#[cfg(test)]
mod regression_tests {
    use crate::{LoanStatus, QuorumCreditContract, QuorumCreditContractClient};
    use soroban_sdk::{
        testutils::{Address as _, Ledger},
        token::StellarAssetClient,
        Address, Env, String, Vec,
    };

    // ── Shared setup ──────────────────────────────────────────────────────────

    struct Setup {
        env: Env,
        client: QuorumCreditContractClient<'static>,
        contract_id: Address,
        token: Address,
        admin: Address,
    }

    fn setup() -> Setup {
        let env = Env::default();
        env.mock_all_auths();

        let deployer = Address::generate(&env);
        let admin = Address::generate(&env);
        let admins = Vec::from_array(&env, [admin.clone()]);
        let token_id = env.register_stellar_asset_contract_v2(admin.clone());
        let contract_id = env.register_contract(None, QuorumCreditContract);

        StellarAssetClient::new(&env, &token_id.address()).mint(&contract_id, &1_000_000);

        let client = QuorumCreditContractClient::new(&env, &contract_id);
        client.initialize(&deployer, &admins, &1, &token_id.address());

        env.ledger().with_mut(|l| l.timestamp = 120);

        Setup { env, client, contract_id, token: token_id.address(), admin }
    }

    fn mint(s: &Setup, to: &Address, amount: i128) {
        StellarAssetClient::new(&s.env, &s.token).mint(to, &amount);
    }

    fn purpose(env: &Env) -> String {
        String::from_str(env, "regression test")
    }

    // ── Regression: Issue 108 — Borrower repaying another borrower's loan ────
    //
    // Bug: `repay` did not verify that the caller was the borrower on record.
    // A malicious actor could call `repay(victim_borrower, ...)` and drain the
    // victim's loan state without being the actual borrower.
    // Fix: Added `UnauthorizedCaller` check in `loan::repay`.
    #[test]
    fn regression_108_unauthorized_repay_rejected() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        let attacker = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);

        mint(&s, &voucher, 10_000);
        mint(&s, &attacker, 10_000);

        s.client.vouch(&voucher, &borrower, &10_000, &s.token, &None);
        s.client
            .request_loan(&borrower, &5_000, &5_000, &purpose(&s.env), &s.token);

        // Attacker tries to repay borrower's loan — must be rejected
        let result = s.client.try_repay(&attacker, &5_100);
        assert!(
            result.is_err(),
            "regression_108: attacker should not be able to repay another borrower's loan"
        );

        // Loan must still be active
        assert_eq!(s.client.loan_status(&borrower), LoanStatus::Active);
    }

    // ── Regression: Issue 109 — Double slash / slash after repay ─────────────
    //
    // Bug: `slash` could be called twice on the same borrower, or after the loan
    // was already repaid, leading to double-counting in the slash treasury and
    // incorrect voucher balances.
    // Fix: `SlashAlreadyExecuted` and `InvalidStateTransition` guards added.
    #[test]
    fn regression_109_slash_after_repay_rejected() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);

        mint(&s, &voucher, 10_000);
        mint(&s, &borrower, 5_000);

        s.client.vouch(&voucher, &borrower, &10_000, &s.token, &None);
        s.client
            .request_loan(&borrower, &5_000, &5_000, &purpose(&s.env), &s.token);

        let loan = s.client.get_loan(&borrower).unwrap();
        s.client.repay(&borrower, &(loan.amount + loan.total_yield));
        assert_eq!(s.client.loan_status(&borrower), LoanStatus::Repaid);

        // Slash after repay must be rejected
        let result = s.client.try_vote_slash(&voucher, &borrower, &true);
        assert!(
            result.is_err(),
            "regression_109: slash after repay should be rejected"
        );
    }

    // ── Regression: Issue 112 — Slash balance accounting / fund leakage ──────
    //
    // Bug: Slashed funds were not tracked in `SlashTreasury`, allowing them to
    // silently disappear from the contract's accounting.
    // Fix: `add_slash_balance` called in `governance::execute_slash`.
    #[test]
    fn regression_112_slash_treasury_increases_on_slash() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);

        mint(&s, &voucher, 10_000);

        s.client.vouch(&voucher, &borrower, &10_000, &s.token, &None);
        s.client
            .request_loan(&borrower, &5_000, &5_000, &purpose(&s.env), &s.token);

        let treasury_before = s.client.get_slash_treasury_balance();

        s.client.vote_slash(&voucher, &borrower, &true);

        let treasury_after = s.client.get_slash_treasury_balance();

        assert!(
            treasury_after > treasury_before,
            "regression_112: slash treasury should increase after slash (before={treasury_before}, after={treasury_after})"
        );
    }

    // ── Regression: Issue 114 — Total outflow never exceeds total inflow ──────
    //
    // Bug: Under certain conditions the contract could disburse more tokens than
    // it received, violating the solvency invariant.
    // Fix: Pre-disbursement balance check in `loan::request_loan`.
    #[test]
    fn regression_114_loan_disbursement_cannot_exceed_contract_balance() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);

        // Voucher stakes 500 — contract has 1_000_000 pre-funded + 500 stake
        mint(&s, &voucher, 500);
        s.client.vouch(&voucher, &borrower, &500, &s.token, &None);

        // Requesting a loan larger than the contract balance must fail
        let huge = 10_000_000_000i128;
        let result =
            s.client
                .try_request_loan(&borrower, &huge, &500, &purpose(&s.env), &s.token);
        assert!(
            result.is_err(),
            "regression_114: loan exceeding contract balance should be rejected"
        );
    }

    // ── Regression: Duplicate vouch rejected ─────────────────────────────────
    //
    // Bug: The same voucher could vouch for the same borrower twice, inflating
    // the total stake and allowing under-collateralised loans.
    // Fix: `DuplicateVouch` check in `vouch::vouch`.
    #[test]
    fn regression_duplicate_vouch_rejected() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);

        mint(&s, &voucher, 20_000);
        s.client.vouch(&voucher, &borrower, &5_000, &s.token, &None);

        let result = s.client.try_vouch(&voucher, &borrower, &5_000, &s.token, &None);
        assert!(
            result.is_err(),
            "regression: duplicate vouch should be rejected"
        );
    }

    // ── Regression: Zero-stake vouch rejected ────────────────────────────────
    //
    // Bug: A vouch with stake=0 was accepted, creating a phantom voucher entry
    // that could satisfy the `min_vouchers` check without providing real collateral.
    // Fix: `require_positive_amount` in `vouch::vouch`.
    #[test]
    fn regression_zero_stake_vouch_rejected() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);

        let result = s.client.try_vouch(&voucher, &borrower, &0, &s.token, &None);
        assert!(
            result.is_err(),
            "regression: zero-stake vouch should be rejected"
        );
    }

    // ── Regression: Self-vouch rejected ──────────────────────────────────────
    //
    // Bug: A borrower could vouch for themselves, bypassing the social-collateral
    // model entirely.
    // Fix: `SelfVouchNotAllowed` check in `vouch::vouch`.
    #[test]
    fn regression_self_vouch_rejected() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        mint(&s, &borrower, 10_000);

        let result = s.client.try_vouch(&borrower, &borrower, &5_000, &s.token, &None);
        assert!(
            result.is_err(),
            "regression: self-vouch should be rejected"
        );
    }

    // ── Regression: Loan request below minimum amount rejected ───────────────
    //
    // Bug: Loans below `min_loan_amount` were accepted, creating dust loans that
    // cost more in fees than they were worth.
    // Fix: `LoanBelowMinAmount` check in `loan::request_loan`.
    #[test]
    fn regression_loan_below_min_amount_rejected() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);

        mint(&s, &voucher, 10_000);
        s.client.vouch(&voucher, &borrower, &10_000, &s.token, &None);

        // DEFAULT_MIN_LOAN_AMOUNT = 100_000 stroops; request 1 stroop
        let result =
            s.client
                .try_request_loan(&borrower, &1, &1, &purpose(&s.env), &s.token);
        assert!(
            result.is_err(),
            "regression: loan below min_loan_amount should be rejected"
        );
    }

    // ── Regression: Vouch while active loan exists rejected ──────────────────
    //
    // Bug: New vouches could be added after a loan was disbursed, retroactively
    // inflating the collateral record without providing real pre-loan backing.
    // Fix: `ActiveLoanExists` check in `vouch::vouch`.
    #[test]
    fn regression_vouch_during_active_loan_rejected() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        let voucher1 = Address::generate(&s.env);
        let voucher2 = Address::generate(&s.env);

        mint(&s, &voucher1, 10_000);
        mint(&s, &voucher2, 10_000);

        s.client.vouch(&voucher1, &borrower, &10_000, &s.token, &None);
        s.client
            .request_loan(&borrower, &5_000, &5_000, &purpose(&s.env), &s.token);

        let result = s.client.try_vouch(&voucher2, &borrower, &5_000, &s.token, &None);
        assert!(
            result.is_err(),
            "regression: vouch during active loan should be rejected"
        );
    }

    // ── Regression: Contract cannot be initialized twice ─────────────────────
    //
    // Bug: `initialize` could be called a second time, overwriting the admin and
    // token configuration.
    // Fix: `AlreadyInitialized` guard in `contract::initialize`.
    #[test]
    fn regression_double_initialize_rejected() {
        let s = setup();
        let deployer2 = Address::generate(&s.env);
        let admin2 = Address::generate(&s.env);
        let admins2 = Vec::from_array(&s.env, [admin2]);

        let result = s
            .client
            .try_initialize(&deployer2, &admins2, &1, &s.token);
        assert!(
            result.is_err(),
            "regression: second initialize should be rejected"
        );
    }
}
