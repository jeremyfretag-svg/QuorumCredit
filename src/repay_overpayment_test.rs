/// Issue #477: repay() overpayment handling tests.
///
/// The contract rejects payment > outstanding (principal + yield - already_repaid)
/// with InvalidAmount, protecting borrowers from accidental overpayment.
/// Exact repayment marks the loan as Repaid.
#[cfg(test)]
mod repay_overpayment_tests {
    use crate::errors::ContractError;
    use crate::{LoanStatus, QuorumCreditContract, QuorumCreditContractClient};
    use soroban_sdk::{
        testutils::Address as _, token::StellarAssetClient, Address, Env, String, Vec,
    };

    /// principal=1_000_000, yield_bps=200 → yield=20_000, total_owed=1_020_000
    const PRINCIPAL: i128 = 1_000_000;
    const YIELD: i128 = 20_000;
    const TOTAL_OWED: i128 = PRINCIPAL + YIELD;

    fn setup(env: &Env) -> (Address, Address, Address, Address) {
        let deployer = Address::generate(env);
        let admin = Address::generate(env);
        let token_id = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        let contract_id = env.register_contract(None, QuorumCreditContract);
        let client = QuorumCreditContractClient::new(env, &contract_id);
        client.initialize(&deployer, &Vec::from_array(env, [admin]), &1, &token_id);
        // Fund contract for disbursement + yield payouts
        StellarAssetClient::new(env, &token_id).mint(&contract_id, &10_000_000);
        let voucher = Address::generate(env);
        StellarAssetClient::new(env, &token_id).mint(&voucher, &10_000_000);
        (contract_id, token_id, voucher, Address::generate(env))
    }

    fn open_loan(
        client: &QuorumCreditContractClient,
        env: &Env,
        token_id: &Address,
        voucher: &Address,
        borrower: &Address,
    ) {
        client.vouch(voucher, borrower, &5_000_000, token_id, &None);
        client.request_loan(
            borrower,
            &PRINCIPAL,
            &5_000_000,
            &String::from_str(env, "test loan"),
            token_id,
        );
        // Give borrower enough to attempt repayment
        StellarAssetClient::new(env, token_id).mint(borrower, &2_000_000);
    }

    /// Issue #477: repay() with payment > total_owed must be rejected with InvalidAmount.
    /// The contract protects borrowers from accidental overpayment.
    #[test]
    fn test_overpayment_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().with_mut(|l| l.timestamp = 120);
        let (contract_id, token_id, voucher, borrower) = setup(&env);
        let client = QuorumCreditContractClient::new(&env, &contract_id);

        open_loan(&client, &env, &token_id, &voucher, &borrower);

        // 1,100,000 > 1,020,000 (total owed) — must be rejected
        let result = client.try_repay(&borrower, &1_100_000);
        assert_eq!(result, Err(Ok(ContractError::InvalidAmount)));
    }

    /// Issue #477: repay() with exact total_owed marks loan as Repaid.
    #[test]
    fn test_exact_repayment_marks_loan_repaid() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().with_mut(|l| l.timestamp = 120);
        let (contract_id, token_id, voucher, borrower) = setup(&env);
        let client = QuorumCreditContractClient::new(&env, &contract_id);

        open_loan(&client, &env, &token_id, &voucher, &borrower);

        client.repay(&borrower, &TOTAL_OWED);

        let loan = client.get_loan(&borrower).expect("loan record should exist");
        assert_eq!(loan.status, LoanStatus::Repaid);
        assert_eq!(loan.amount_repaid, TOTAL_OWED);
    }
}
