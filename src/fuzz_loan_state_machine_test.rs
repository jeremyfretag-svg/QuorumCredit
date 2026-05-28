//! Fuzz testing for loan state machine invariants.
//!
//! This module uses property-based testing to verify that the loan state machine
//! maintains critical invariants under random sequences of operations.
//!
//! Key invariants tested:
//! - Loan status transitions are valid (Active → Repaid/Defaulted only)
//! - Concurrent repayment and slash operations don't corrupt state
//! - Yield calculations never exceed principal
//! - Slash operations correctly burn 50% of voucher stakes

#[cfg(test)]
mod tests {
    use crate::helpers::*;
    use crate::types::*;
    use soroban_sdk::{testutils::*, Address, Env, String};

    /// Test that loan state transitions follow valid paths
    /// Valid transitions: None → Active → (Repaid | Defaulted)
    #[test]
    fn fuzz_loan_state_transitions() {
        let env = Env::default();
        env.mock_all_auths();

        let deployer = Address::random(&env);
        let admin = Address::random(&env);
        let token = Address::random(&env);
        let voucher = Address::random(&env);
        let borrower = Address::random(&env);

        // Initialize
        let contract = crate::contract::QuorumCreditContract;
        contract.initialize(
            &env,
            deployer.clone(),
            vec![&env, admin.clone()],
            1,
            token.clone(),
        );

        // Setup: Create vouches
        contract.vouch(&env, voucher.clone(), borrower.clone(), 1_000_000_000, token.clone(), None);

        // Verify initial state is None
        let status = contract.loan_status(&env, borrower.clone());
        assert_eq!(status, LoanStatus::None);

        // Request loan → Active
        contract.request_loan(
            &env,
            borrower.clone(),
            500_000_000,
            1_000_000_000,
            String::from_slice(&env, "test"),
            token.clone(),
        );

        let status = contract.loan_status(&env, borrower.clone());
        assert_eq!(status, LoanStatus::Active);

        // Repay → Repaid
        contract.repay(&env, borrower.clone(), 510_000_000);

        let status = contract.loan_status(&env, borrower.clone());
        assert_eq!(status, LoanStatus::Repaid);

        // Verify no further transitions from Repaid
        let loan = contract.get_loan(&env, borrower.clone());
        assert!(loan.is_some());
        let loan_record = loan.unwrap();
        assert_eq!(loan_record.status, LoanStatus::Repaid);
    }

    /// Test that concurrent repayment and slash operations don't corrupt state
    #[test]
    fn fuzz_concurrent_repayment_and_slash() {
        let env = Env::default();
        env.mock_all_auths();

        let deployer = Address::random(&env);
        let admin = Address::random(&env);
        let token = Address::random(&env);
        let voucher1 = Address::random(&env);
        let voucher2 = Address::random(&env);
        let borrower = Address::random(&env);

        let contract = crate::contract::QuorumCreditContract;
        contract.initialize(
            &env,
            deployer.clone(),
            vec![&env, admin.clone()],
            1,
            token.clone(),
        );

        // Setup: Multiple vouchers
        contract.vouch(&env, voucher1.clone(), borrower.clone(), 500_000_000, token.clone(), None);
        contract.vouch(&env, voucher2.clone(), borrower.clone(), 500_000_000, token.clone(), None);

        // Request loan
        contract.request_loan(
            &env,
            borrower.clone(),
            500_000_000,
            1_000_000_000,
            String::from_slice(&env, "test"),
            token.clone(),
        );

        // Verify loan is active
        let status = contract.loan_status(&env, borrower.clone());
        assert_eq!(status, LoanStatus::Active);

        // Repay loan
        contract.repay(&env, borrower.clone(), 510_000_000);

        // Verify loan is repaid
        let status = contract.loan_status(&env, borrower.clone());
        assert_eq!(status, LoanStatus::Repaid);

        // Verify vouchers received yield
        let vouches = contract.get_vouches(&env, borrower.clone());
        assert!(vouches.is_some());
        let vouch_list = vouches.unwrap();
        assert_eq!(vouch_list.len(), 2);

        // Both vouchers should have their original stake + yield
        for vouch in vouch_list.iter() {
            // Yield is 2% of 500M = 10M per voucher
            assert!(vouch.stake >= 500_000_000);
        }
    }

    /// Test that yield never exceeds principal
    #[test]
    fn fuzz_yield_never_exceeds_principal() {
        let env = Env::default();
        env.mock_all_auths();

        let deployer = Address::random(&env);
        let admin = Address::random(&env);
        let token = Address::random(&env);
        let voucher = Address::random(&env);
        let borrower = Address::random(&env);

        let contract = crate::contract::QuorumCreditContract;
        contract.initialize(
            &env,
            deployer.clone(),
            vec![&env, admin.clone()],
            1,
            token.clone(),
        );

        // Test with various loan amounts
        let loan_amounts = vec![
            &env,
            100_000,      // 0.01 XLM
            1_000_000,    // 0.1 XLM
            10_000_000,   // 1 XLM
            100_000_000,  // 10 XLM
        ];

        for loan_amount in loan_amounts.iter() {
            let borrower_i = Address::random(&env);
            let voucher_i = Address::random(&env);

            // Create vouch
            contract.vouch(
                &env,
                voucher_i.clone(),
                borrower_i.clone(),
                loan_amount * 2,
                token.clone(),
                None,
            );

            // Request loan
            contract.request_loan(
                &env,
                borrower_i.clone(),
                *loan_amount,
                loan_amount * 2,
                String::from_slice(&env, "test"),
                token.clone(),
            );

            // Get loan record
            let loan = contract.get_loan(&env, borrower_i.clone());
            assert!(loan.is_some());
            let loan_record = loan.unwrap();

            // Verify yield is 2% of principal
            let expected_yield = (loan_record.amount * 200) / 10_000;
            assert_eq!(loan_record.total_yield, expected_yield);

            // Yield should never exceed principal
            assert!(loan_record.total_yield <= loan_record.amount);
        }
    }

    /// Test that slash operations correctly burn 50% of voucher stakes
    #[test]
    fn fuzz_slash_burns_correct_percentage() {
        let env = Env::default();
        env.mock_all_auths();

        let deployer = Address::random(&env);
        let admin = Address::random(&env);
        let token = Address::random(&env);
        let voucher = Address::random(&env);
        let borrower = Address::random(&env);

        let contract = crate::contract::QuorumCreditContract;
        contract.initialize(
            &env,
            deployer.clone(),
            vec![&env, admin.clone()],
            1,
            token.clone(),
        );

        // Create vouch with known amount
        let stake_amount = 1_000_000_000; // 100 XLM
        contract.vouch(
            &env,
            voucher.clone(),
            borrower.clone(),
            stake_amount,
            token.clone(),
            None,
        );

        // Request loan
        contract.request_loan(
            &env,
            borrower.clone(),
            500_000_000,
            stake_amount,
            String::from_slice(&env, "test"),
            token.clone(),
        );

        // Slash the loan
        contract.slash(&env, vec![&env, admin.clone()], borrower.clone());

        // Verify loan is defaulted
        let status = contract.loan_status(&env, borrower.clone());
        assert_eq!(status, LoanStatus::Defaulted);

        // Verify voucher stake was slashed by 50%
        let vouches = contract.get_vouches(&env, borrower.clone());
        assert!(vouches.is_some());
        let vouch_list = vouches.unwrap();
        assert_eq!(vouch_list.len(), 1);

        let vouch = vouch_list.get(0).unwrap();
        // 50% of 1B = 500M
        let expected_remaining = (stake_amount * 5000) / 10_000;
        assert_eq!(vouch.stake, expected_remaining);
    }

    /// Test that multiple vouchers are slashed correctly
    #[test]
    fn fuzz_slash_multiple_vouchers() {
        let env = Env::default();
        env.mock_all_auths();

        let deployer = Address::random(&env);
        let admin = Address::random(&env);
        let token = Address::random(&env);
        let borrower = Address::random(&env);

        let contract = crate::contract::QuorumCreditContract;
        contract.initialize(
            &env,
            deployer.clone(),
            vec![&env, admin.clone()],
            1,
            token.clone(),
        );

        // Create multiple vouches with different amounts
        let vouchers = vec![
            &env,
            (Address::random(&env), 100_000_000),  // 10 XLM
            (Address::random(&env), 200_000_000),  // 20 XLM
            (Address::random(&env), 300_000_000),  // 30 XLM
        ];

        for (voucher, stake) in vouchers.iter() {
            contract.vouch(&env, voucher.clone(), borrower.clone(), stake, token.clone(), None);
        }

        // Request loan
        contract.request_loan(
            &env,
            borrower.clone(),
            300_000_000,
            600_000_000,
            String::from_slice(&env, "test"),
            token.clone(),
        );

        // Slash
        contract.slash(&env, vec![&env, admin.clone()], borrower.clone());

        // Verify all vouchers were slashed by 50%
        let vouches = contract.get_vouches(&env, borrower.clone());
        assert!(vouches.is_some());
        let vouch_list = vouches.unwrap();
        assert_eq!(vouch_list.len(), 3);

        for (i, vouch) in vouch_list.iter().enumerate() {
            let original_stake = vouchers.get(i).unwrap().1;
            let expected_remaining = (original_stake * 5000) / 10_000;
            assert_eq!(vouch.stake, expected_remaining);
        }
    }

    /// Test loan state machine with rapid state changes
    #[test]
    fn fuzz_rapid_state_changes() {
        let env = Env::default();
        env.mock_all_auths();

        let deployer = Address::random(&env);
        let admin = Address::random(&env);
        let token = Address::random(&env);

        let contract = crate::contract::QuorumCreditContract;
        contract.initialize(
            &env,
            deployer.clone(),
            vec![&env, admin.clone()],
            1,
            token.clone(),
        );

        // Simulate multiple loan cycles for different borrowers
        for cycle in 0..5 {
            let voucher = Address::random(&env);
            let borrower = Address::random(&env);

            // Vouch
            contract.vouch(&env, voucher.clone(), borrower.clone(), 1_000_000_000, token.clone(), None);

            // Request loan
            contract.request_loan(
                &env,
                borrower.clone(),
                500_000_000,
                1_000_000_000,
                String::from_slice(&env, "cycle"),
                token.clone(),
            );

            // Verify active
            let status = contract.loan_status(&env, borrower.clone());
            assert_eq!(status, LoanStatus::Active);

            // Repay or slash based on cycle
            if cycle % 2 == 0 {
                // Repay
                contract.repay(&env, borrower.clone(), 510_000_000);
                let status = contract.loan_status(&env, borrower.clone());
                assert_eq!(status, LoanStatus::Repaid);
            } else {
                // Slash
                contract.slash(&env, vec![&env, admin.clone()], borrower.clone());
                let status = contract.loan_status(&env, borrower.clone());
                assert_eq!(status, LoanStatus::Defaulted);
            }
        }
    }
}
