/// Tests for decrease_stake() full withdrawal (amount == current stake).
/// Covers issue #482.
#[cfg(test)]
mod decrease_stake_full_withdrawal_tests {
    use crate::{QuorumCreditContract, QuorumCreditContractClient};
    use soroban_sdk::{
        testutils::{Address as _, Ledger},
        token::StellarAssetClient,
        Address, Env, Vec,
    };

    fn setup() -> (Env, QuorumCreditContractClient<'static>, Address) {
        let env = Env::default();
        env.mock_all_auths();

        let deployer = Address::generate(&env);
        let admin = Address::generate(&env);
        let token_id = env.register_stellar_asset_contract_v2(admin.clone());
        let contract_id = env.register_contract(None, QuorumCreditContract);

        StellarAssetClient::new(&env, &token_id.address()).mint(&contract_id, &10_000_000);

        let client = QuorumCreditContractClient::new(&env, &contract_id);
        client.initialize(&deployer, &Vec::from_array(&env, [admin]), &1, &token_id.address());

        env.ledger().with_mut(|l| l.timestamp = 120);

        (env, client, token_id.address())
    }

    /// decrease_stake() with amount == stake removes the vouch and returns funds.
    #[test]
    fn test_decrease_stake_full_withdrawal_removes_vouch_and_returns_funds() {
        let (env, client, token_id) = setup();
        let borrower = Address::generate(&env);
        let voucher_a = Address::generate(&env);

        // Step 1: Voucher A vouches with 1,000,000 stroops.
        StellarAssetClient::new(&env, &token_id).mint(&voucher_a, &1_000_000);
        client.vouch(&voucher_a, &borrower, &1_000_000, &token_id, &None);

        let balance_before = soroban_sdk::token::Client::new(&env, &token_id).balance(&voucher_a);

        // Step 2: Call decrease_stake() with the full 1,000,000 stroops.
        client.decrease_stake(&voucher_a, &borrower, &1_000_000);

        // Step 3: Assert vouch is removed from the list.
        assert!(
            !client.vouch_exists(&voucher_a, &borrower),
            "vouch should be removed after full withdrawal"
        );
        let vouches = client.get_vouches(&borrower);
        assert!(vouches.is_empty(), "vouch list should be empty after full withdrawal");

        // Step 4: Assert funds are returned to voucher.
        let balance_after = soroban_sdk::token::Client::new(&env, &token_id).balance(&voucher_a);
        assert_eq!(
            balance_after,
            balance_before + 1_000_000,
            "voucher should receive full stake back"
        );
    }
}
