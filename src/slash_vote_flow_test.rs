/// Issue #488: Comprehensive test for the slash vote flow.
///
/// Verifies that vote_slash() correctly accumulates votes and auto-executes
/// the slash once the default 50% quorum is reached.
///
/// Flow:
///   1. Create loan with 3 vouchers (A, B, C) each staking 1_000_000
///   2. A votes approve  → 33% < 50%, no slash yet
///   3. B votes approve  → 66% ≥ 50%, slash auto-executes
///   4. Assert loan is Defaulted and vote record is marked executed
///   5. C's vote is rejected with SlashAlreadyExecuted
#[cfg(test)]
mod slash_vote_flow_tests {
    use crate::{ContractError, LoanStatus, QuorumCreditContract, QuorumCreditContractClient};
    use soroban_sdk::{
        testutils::Address as _,
        token::StellarAssetClient,
        Address, Env, String, Vec,
    };

    fn setup() -> (Env, QuorumCreditContractClient<'static>, Address) {
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

        // Advance past MIN_VOUCH_AGE (60s)
        env.ledger().with_mut(|l| l.timestamp = 120);

        (env, client, token_id.address())
    }

    #[test]
    fn test_slash_vote_full_flow() {
        let (env, client, token_id) = setup();

        let borrower = Address::generate(&env);
        let voucher_a = Address::generate(&env);
        let voucher_b = Address::generate(&env);
        let voucher_c = Address::generate(&env);

        // Step 1: Create loan with 3 vouchers, each staking 1_000_000
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
        assert_eq!(client.loan_status(&borrower), LoanStatus::Active);

        // Step 2: A votes approve — 33% < 50% default quorum, no slash yet
        client.vote_slash(&voucher_a, &borrower, &true);
        assert_eq!(client.loan_status(&borrower), LoanStatus::Active);

        let vote = client.get_slash_vote(&borrower).unwrap();
        assert_eq!(vote.approve_stake, 1_000_000);
        assert!(!vote.executed);

        // Step 3: B votes approve — 66% ≥ 50%, slash auto-executes
        client.vote_slash(&voucher_b, &borrower, &true);
        assert_eq!(client.loan_status(&borrower), LoanStatus::Defaulted);

        let vote = client.get_slash_vote(&borrower).unwrap();
        assert!(vote.executed);
        assert_eq!(vote.approve_stake, 2_000_000);

        // Step 4: execute_slash_vote returns SlashAlreadyExecuted (idempotent guard)
        let result = client.try_execute_slash_vote(&borrower);
        assert_eq!(result, Err(Ok(ContractError::SlashAlreadyExecuted)));

        // Step 5: C's vote is also rejected with SlashAlreadyExecuted
        let result = client.try_vote_slash(&voucher_c, &borrower, &true);
        assert_eq!(result, Err(Ok(ContractError::SlashAlreadyExecuted)));
    }
}
