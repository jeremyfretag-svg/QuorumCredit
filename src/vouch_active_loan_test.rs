/// Tests for vouch() rejecting new vouches when borrower has an active loan.
/// Covers issue #480.
#[cfg(test)]
mod vouch_active_loan_tests {
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
    }

    fn setup() -> Setup {
        let env = Env::default();
        env.mock_all_auths();

        let deployer = Address::generate(&env);
        let admin = Address::generate(&env);
        let admins = Vec::from_array(&env, [admin.clone()]);

        let token_id = env.register_stellar_asset_contract_v2(admin.clone());
        let contract_id = env.register_contract(None, QuorumCreditContract);

        // Pre-fund contract so it can disburse the loan.
        StellarAssetClient::new(&env, &token_id.address()).mint(&contract_id, &10_000_000);

        let client = QuorumCreditContractClient::new(&env, &contract_id);
        client.initialize(&deployer, &admins, &1, &token_id.address());

        // Advance past MIN_VOUCH_AGE so vouches are eligible for loans.
        env.ledger().with_mut(|l| l.timestamp = 120);

        Setup { env, client, token_id: token_id.address() }
    }

    fn mint_and_vouch(s: &Setup, voucher: &Address, borrower: &Address, stake: i128) {
        StellarAssetClient::new(&s.env, &s.token_id).mint(voucher, &stake);
        s.client.vouch(voucher, borrower, &stake, &s.token_id, &None);
    }

    /// vouch() must return ActiveLoanExists when the borrower already has an active loan.
    #[test]
    fn test_vouch_rejected_when_borrower_has_active_loan() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        let voucher1 = Address::generate(&s.env);
        let voucher2 = Address::generate(&s.env);

        // Step 1: Create a loan for borrower.
        mint_and_vouch(&s, &voucher1, &borrower, 1_000_000);
        s.client.request_loan(
            &borrower,
            &500_000,
            &500_000,
            &String::from_str(&s.env, "test"),
            &s.token_id,
        );

        // Step 2: Attempt to add a new vouch while loan is active.
        StellarAssetClient::new(&s.env, &s.token_id).mint(&voucher2, &500_000);
        let result = s.client.try_vouch(&voucher2, &borrower, &500_000, &s.token_id, &None);

        // Step 3: Assert ActiveLoanExists is returned.
        assert_eq!(
            result,
            Err(Ok(ContractError::ActiveLoanExists)),
            "vouch() must reject when borrower has an active loan"
        );
    }

    /// After repaying the loan, a new vouch must succeed.
    #[test]
    fn test_vouch_succeeds_after_loan_repaid() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        let voucher1 = Address::generate(&s.env);
        let voucher2 = Address::generate(&s.env);

        // Step 1: Create and repay a loan.
        mint_and_vouch(&s, &voucher1, &borrower, 1_000_000);
        s.client.request_loan(
            &borrower,
            &500_000,
            &500_000,
            &String::from_str(&s.env, "test"),
            &s.token_id,
        );

        let loan = s.client.get_loan(&borrower).expect("loan should exist");
        let repayment = loan.amount + loan.total_yield;
        StellarAssetClient::new(&s.env, &s.token_id).mint(&borrower, &repayment);
        s.client.repay(&borrower, &repayment);

        // Step 4: Advance timestamp past cooldown for voucher2.
        s.env.ledger().with_mut(|l| l.timestamp += 120);

        // Step 5: Attempt to add a new vouch — must succeed.
        StellarAssetClient::new(&s.env, &s.token_id).mint(&voucher2, &500_000);
        let result = s.client.try_vouch(&voucher2, &borrower, &500_000, &s.token_id, &None);
        assert!(result.is_ok(), "vouch() must succeed after loan is repaid");
    }
}
