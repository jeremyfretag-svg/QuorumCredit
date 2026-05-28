#[cfg(test)]
mod admin_delegation_tests {
    use crate::{ContractError, QuorumCreditContract, QuorumCreditContractClient};
    use soroban_sdk::{
        testutils::Address as _,
        token::StellarAssetClient,
        Address, Env, String, Vec,
    };

    struct Setup {
        env: Env,
        client: QuorumCreditContractClient<'static>,
        admin: Address,
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

        StellarAssetClient::new(&env, &token_id.address()).mint(&contract_id, &10_000_000);

        let client = QuorumCreditContractClient::new(&env, &contract_id);
        client.initialize(&deployer, &admins, &1, &token_id.address());

        Setup {
            env,
            client,
            admin,
            token_id: token_id.address(),
        }
    }

    #[test]
    fn test_admin_can_delegate() {
        let s = setup();
        let delegatee = Address::generate(&s.env);
        let permissions = Vec::from_array(
            &s.env,
            [String::from_str(&s.env, "whitelist_voucher")],
        );

        s.client.delegate_permission(
            &s.admin,
            &delegatee,
            &permissions,
        );

        // Verify delegatee can use the permission
        let voucher = Address::generate(&s.env);
        s.client.whitelist_voucher_delegated(&delegatee, &voucher);
    }

    #[test]
    fn test_delegatee_can_exercise_delegated_permission() {
        let s = setup();
        let delegatee = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);
        let permissions = Vec::from_array(
            &s.env,
            [String::from_str(&s.env, "whitelist_voucher")],
        );

        s.client.delegate_permission(
            &s.admin,
            &delegatee,
            &permissions,
        );

        s.client.whitelist_voucher_delegated(&delegatee, &voucher);

        // Verify the voucher is whitelisted
        // We can't directly query whitelist status, so the test passes if no error occurred
    }

    #[test]
    #[should_panic(expected = "caller does not have whitelist_voucher permission")]
    fn test_delegatee_cannot_exercise_non_delegated_permission() {
        let s = setup();
        let delegatee = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);

        // Don't delegate anything, just try to whitelist
        s.client.whitelist_voucher_delegated(&delegatee, &voucher);
    }

    #[test]
    #[should_panic(expected = "caller does not have whitelist_voucher permission")]
    fn test_admin_can_revoke_delegation() {
        let s = setup();
        let delegatee = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);
        let permissions = Vec::from_array(
            &s.env,
            [String::from_str(&s.env, "whitelist_voucher")],
        );

        // Delegate first
        s.client.delegate_permission(
            &s.admin,
            &delegatee,
            &permissions,
        );

        // Verify it works
        s.client.whitelist_voucher_delegated(&delegatee, &voucher);

        // Revoke delegation
        s.client.revoke_delegation(&s.admin, &delegatee);

        // Try to use delegated permission again (should fail)
        let voucher2 = Address::generate(&s.env);
        s.client.whitelist_voucher_delegated(&delegatee, &voucher2);
    }

    #[test]
    #[should_panic(expected = "insufficient admin approvals")]
    fn test_non_admin_cannot_delegate() {
        let s = setup();
        let non_admin = Address::generate(&s.env);
        let delegatee = Address::generate(&s.env);
        let permissions = Vec::from_array(
            &s.env,
            [String::from_str(&s.env, "whitelist_voucher")],
        );

        // Create signers with non-admin
        let signers = Vec::from_array(&s.env, [non_admin]);

        s.client.delegate_permission(&non_admin, &delegatee, &permissions);
    }
}
