/// Issue #489: Test that execute_slash_vote() enforces quorum correctly.
///
/// Scenario:
///   - Quorum set to 50% (5_000 bps)
///   - 3 vouchers A, B, C each stake 1_000_000 → total 3_000_000
///   - Only A votes approve (33%) → execute_slash_vote returns QuorumNotMet
///   - B also votes approve (66%) → vote_slash auto-executes the slash
///   - execute_slash_vote now returns SlashAlreadyExecuted (idempotent guard)
#[cfg(test)]
mod execute_slash_vote_quorum_tests {
    use crate::{ContractError, QuorumCreditContract, QuorumCreditContractClient};
    use soroban_sdk::{
        testutils::Address as _,
        token::StellarAssetClient,
        Address, Env, String, Vec,
    };

    fn setup() -> (
        Env,
        QuorumCreditContractClient<'static>,
        Address, // admin
        Address, // token_id
    ) {
        let env = Env::default();
        env.mock_all_auths();

        let deployer = Address::generate(&env);
        let admin = Address::generate(&env);
        let admins = Vec::from_array(&env, [admin.clone()]);
        let token_id = env.register_stellar_asset_contract_v2(admin.clone());
        let contract_id = env.register_contract(None, QuorumCreditContract);

        // Fund contract so it can disburse the loan
        StellarAssetClient::new(&env, &token_id.address()).mint(&contract_id, &10_000_000);

        let client = QuorumCreditContractClient::new(&env, &contract_id);
        client.initialize(&deployer, &admins, &1, &token_id.address());

        // Advance past MIN_VOUCH_AGE (60s)
        env.ledger().with_mut(|l| l.timestamp = 120);

        (env, client, admin, token_id.address())
    }

    /// Verifies the full quorum lifecycle for execute_slash_vote():
    ///
    /// 1. With only A's vote (33%), execute_slash_vote returns QuorumNotMet.
    /// 2. After B votes (66% ≥ 50%), vote_slash auto-executes the slash.
    /// 3. Calling execute_slash_vote again returns SlashAlreadyExecuted (idempotent guard).
    #[test]
    fn test_execute_slash_vote_quorum_check() {
        let (env, client, admin, token_id) = setup();

        let borrower = Address::generate(&env);
        let voucher_a = Address::generate(&env);
        let voucher_b = Address::generate(&env);
        let voucher_c = Address::generate(&env);

        // Step 1: Set quorum to 50%
        let admins = Vec::from_array(&env, [admin.clone()]);
        client.set_slash_vote_quorum(&admins, &5_000);
        assert_eq!(client.get_slash_vote_quorum(), 5_000);

        // Step 2: Create loan with 3 vouchers, each staking 1_000_000
        for voucher in [&voucher_a, &voucher_b, &voucher_c] {
            StellarAssetClient::new(&env, &token_id).mint(voucher, &1_000_000);
            client.vouch(voucher, &borrower, &1_000_000, &token_id, &None);
        }
        client.request_loan(
            &borrower,
            &500_000,
            &3_000_000,
            &String::from_str(&env, "test loan"),
            &token_id,
        );

        // Step 3: Only A votes approve — 1/3 ≈ 33% < 50% quorum, no auto-execute
        client.vote_slash(&voucher_a, &borrower, &true);
        assert_eq!(client.loan_status(&borrower), crate::LoanStatus::Active);

        // Step 4: execute_slash_vote must return QuorumNotMet (33% < 50%)
        let result = client.try_execute_slash_vote(&borrower);
        assert_eq!(result, Err(Ok(ContractError::QuorumNotMet)));

        // Step 5: B votes approve — 2/3 ≈ 66% ≥ 50%, vote_slash auto-executes
        client.vote_slash(&voucher_b, &borrower, &true);
        assert_eq!(client.loan_status(&borrower), crate::LoanStatus::Defaulted);

        let vote = client.get_slash_vote(&borrower).unwrap();
        assert!(vote.executed);

        // Step 6: execute_slash_vote is now idempotent — returns SlashAlreadyExecuted
        let result = client.try_execute_slash_vote(&borrower);
        assert_eq!(result, Err(Ok(ContractError::SlashAlreadyExecuted)));
    }
}
