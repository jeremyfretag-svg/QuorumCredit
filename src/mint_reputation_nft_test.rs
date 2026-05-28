/// Reputation NFT Minting Tests (Issue #471)
///
/// Verify that `mint_reputation_nft` mints an NFT for borrowers who have
/// repaid at least one loan, and rejects those who haven't.

#[cfg(test)]
mod mint_reputation_nft_tests {
    use crate::{
        reputation::{ReputationNftContract, ReputationNftContractClient},
        QuorumCreditContract, QuorumCreditContractClient,
    };
    use soroban_sdk::{
        testutils::{Address as _, Ledger},
        token::StellarAssetClient,
        Address, Env, String, Vec,
    };

    struct Setup {
        env: Env,
        client: QuorumCreditContractClient<'static>,
        nft_client: ReputationNftContractClient<'static>,
        token_id: Address,
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
        let nft_id = env.register_contract(None, ReputationNftContract);

        StellarAssetClient::new(&env, &token_id.address()).mint(&contract_id, &10_000_000);

        let client = QuorumCreditContractClient::new(&env, &contract_id);
        client.initialize(&deployer, &admins, &1, &token_id.address());

        // Register the NFT contract and set the lending contract as its minter.
        let nft_client = ReputationNftContractClient::new(&env, &nft_id);
        nft_client.initialize(&contract_id);
        client.set_reputation_nft(&admins, &nft_id);

        env.ledger().with_mut(|l| l.timestamp = 120);

        Setup { env, client, nft_client, token_id: token_id.address(), admin }
    }

    fn do_full_repay(s: &Setup, borrower: &Address, voucher: &Address) {
        let token = StellarAssetClient::new(&s.env, &s.token_id);
        token.mint(voucher, &500_000);
        s.client.vouch(voucher, borrower, &500_000, &s.token_id, &None);
        s.client.request_loan(borrower, &100_000, &500_000, &String::from_str(&s.env, "test"), &s.token_id);
        // principal 100_000 + 2% yield 2_000 = 102_000
        token.mint(borrower, &102_000);
        s.client.repay(borrower, &102_000);
    }

    /// Borrower who repaid a loan can mint a reputation NFT.
    #[test]
    fn test_mint_after_repayment_succeeds() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);

        do_full_repay(&s, &borrower, &voucher);

        assert_eq!(s.client.repayment_count(&borrower), 1);

        // Score already incremented by repay(); mint_reputation_nft adds another point.
        let score_before = s.nft_client.balance(&borrower);
        s.client.mint_reputation_nft(&borrower).unwrap();
        assert_eq!(s.nft_client.balance(&borrower), score_before + 1);
    }

    /// Borrower with no repayments cannot mint.
    #[test]
    #[should_panic]
    fn test_mint_without_repayment_fails() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        s.client.mint_reputation_nft(&borrower).unwrap();
    }

    /// Calling mint_reputation_nft without an NFT contract configured panics.
    #[test]
    #[should_panic]
    fn test_mint_without_nft_contract_fails() {
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

        // Manually set repayment_count to 1 via a full repay cycle.
        env.ledger().with_mut(|l| l.timestamp = 120);
        let borrower = Address::generate(&env);
        let voucher = Address::generate(&env);
        let token = StellarAssetClient::new(&env, &token_id.address());
        token.mint(&voucher, &500_000);
        client.vouch(&voucher, &borrower, &500_000, &token_id.address(), &None);
        client.request_loan(&borrower, &100_000, &500_000, &String::from_str(&env, "t"), &token_id.address());
        token.mint(&borrower, &102_000);
        client.repay(&borrower, &102_000);

        // No NFT contract set — should fail.
        client.mint_reputation_nft(&borrower).unwrap();
    }
}
