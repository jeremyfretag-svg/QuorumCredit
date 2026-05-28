#[cfg(test)]
mod governance_veto_tests {
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
        gov_token_id: Address,
    }

    fn setup() -> Setup {
        let env = Env::default();
        env.mock_all_auths();

        let deployer = Address::generate(&env);
        let admin = Address::generate(&env);
        let admins = Vec::from_array(&env, [admin.clone()]);

        let token_id = env.register_stellar_asset_contract_v2(admin.clone());
        let gov_token_id = env.register_stellar_asset_contract_v2(admin.clone());
        let contract_id = env.register_contract(None, QuorumCreditContract);

        StellarAssetClient::new(&env, &token_id.address()).mint(&contract_id, &10_000_000);

        let client = QuorumCreditContractClient::new(&env, &contract_id);
        client.initialize(&deployer, &admins, &1, &token_id.address());
        client.set_governance_token(&admins, &gov_token_id.address());

        Setup {
            env,
            client,
            admin,
            token_id: token_id.address(),
            gov_token_id: gov_token_id.address(),
        }
    }

    fn propose_governance(s: &Setup, proposer: &Address) -> u64 {
        // Fund proposer with governance token
        StellarAssetClient::new(&s.env, &s.gov_token_id).mint(proposer, &1_000);

        // Advance time
        s.env.ledger().with_mut(|l| l.timestamp += 100);

        let description = String::from_str(&s.env, "test proposal");
        let result = s.client.try_propose_governance_change(proposer, &description);

        // Extract proposal_id from result
        if let Ok(id) = result {
            id
        } else {
            panic!("Failed to propose governance change");
        }
    }

    #[test]
    fn test_veto_admin_can_veto_active_proposal() {
        let s = setup();
        let veto_admin = Address::generate(&s.env);
        let proposer = Address::generate(&s.env);

        // Set veto admin
        s.client.set_veto_admin(&s.admin, &Some(veto_admin.clone()));

        // Propose governance change
        let proposal_id = propose_governance(&s, &proposer);

        // Veto the proposal
        let result = s.client.try_veto_proposal(&veto_admin, &proposal_id);
        assert!(result.is_ok(), "veto_proposal should succeed");

        // Verify proposal is vetoed
        if let Some(proposal) = s.client.get_governance_proposal(&proposal_id) {
            assert!(proposal.vetoed, "proposal should be vetoed");
        }
    }

    #[test]
    fn test_vetoed_proposal_cannot_be_executed() {
        let s = setup();
        let veto_admin = Address::generate(&s.env);
        let proposer = Address::generate(&s.env);

        // Set veto admin
        s.client.set_veto_admin(&s.admin, &Some(veto_admin.clone()));

        // Propose governance change
        let proposal_id = propose_governance(&s, &proposer);

        // Veto the proposal
        s.client.veto_proposal(&veto_admin, &proposal_id).unwrap();

        // Advance time past voting period
        s.env.ledger().with_mut(|l| l.timestamp += 10_000);

        // Try to execute (should fail)
        let result = s.client.try_execute_governance_change(&proposal_id);
        assert!(
            matches!(result, Err(Ok(ContractError::ProposalVetoed))),
            "execute_governance_change should fail with ProposalVetoed"
        );
    }

    #[test]
    fn test_non_veto_admin_is_rejected() {
        let s = setup();
        let veto_admin = Address::generate(&s.env);
        let wrong_admin = Address::generate(&s.env);
        let proposer = Address::generate(&s.env);

        // Set veto admin
        s.client.set_veto_admin(&s.admin, &Some(veto_admin.clone()));

        // Propose governance change
        let proposal_id = propose_governance(&s, &proposer);

        // Try to veto with wrong address (should fail due to auth)
        let result = s.client.try_veto_proposal(&wrong_admin, &proposal_id);
        assert!(result.is_err(), "veto_proposal should fail with wrong admin");
    }

    #[test]
    fn test_main_admin_can_set_and_clear_veto_admin() {
        let s = setup();
        let veto_admin = Address::generate(&s.env);
        let proposer = Address::generate(&s.env);

        // Set veto admin
        s.client.set_veto_admin(&s.admin, &Some(veto_admin.clone()));

        // Propose governance change
        let proposal_id = propose_governance(&s, &proposer);

        // Veto should work
        s.client.veto_proposal(&veto_admin, &proposal_id).unwrap();

        // Clear veto admin
        s.client.set_veto_admin(&s.admin, &None);

        // Propose another governance change
        let proposal_id2 = propose_governance(&s, &proposer);

        // Try to veto with the old admin (should fail because no veto admin is set)
        let result = s.client.try_veto_proposal(&veto_admin, &proposal_id2);
        assert!(
            matches!(result, Err(Ok(ContractError::UnauthorizedCaller))),
            "veto_proposal should fail with UnauthorizedCaller"
        );
    }

    #[test]
    fn test_no_veto_admin_configured_returns_clear_error() {
        let s = setup();
        let proposer = Address::generate(&s.env);
        let random_address = Address::generate(&s.env);

        // Don't set veto admin (it defaults to None)
        // Propose governance change
        let proposal_id = propose_governance(&s, &proposer);

        // Try to veto (should fail with UnauthorizedCaller when no veto admin is set)
        let result = s.client.try_veto_proposal(&random_address, &proposal_id);
        assert!(
            matches!(result, Err(Ok(ContractError::UnauthorizedCaller))),
            "veto_proposal should fail with UnauthorizedCaller when no veto admin is set"
        );
    }
}
