/// Input Validation Guard Tests
///
/// Tests for:
/// - set_config / set_admin_threshold: admin_threshold > admins.len() → InvalidAdminThreshold
/// - vote_slash (initialize_slash_vote): no active loan → NoActiveLoan
/// - vouch: zero or negative stake → InsufficientFunds (via require_positive_amount)
/// - increase_stake: zero or negative amount → InsufficientFunds (via require_positive_amount)
#[cfg(test)]
mod input_validation_tests {
    use crate::errors::ContractError;
    use crate::{QuorumCreditContract, QuorumCreditContractClient};
    use soroban_sdk::{
        testutils::{Address as _, Ledger},
        token::StellarAssetClient,
        Address, Env, Vec,
    };

    fn setup(env: &Env) -> (QuorumCreditContractClient<'static>, Address, Address) {
        env.mock_all_auths();

        let deployer = Address::generate(env);
        let admin = Address::generate(env);
        let admins = Vec::from_array(env, [admin.clone()]);

        let token_id = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        let contract_id = env.register_contract(None, QuorumCreditContract);

        // Fund contract so it can disburse loans
        StellarAssetClient::new(env, &token_id).mint(&contract_id, &100_000_000);

        let client = QuorumCreditContractClient::new(env, &contract_id);
        client.initialize(&deployer, &admins, &1, &token_id);

        // Advance time past vouch cooldown
        env.ledger().with_mut(|l| l.timestamp = 120);

        (client, admin, token_id)
    }

    // ── Issue: set_config allows admin_threshold > admins.len() ──────────────

    /// set_admin_threshold: threshold greater than admin count must return InvalidAdminThreshold.
    #[test]
    fn test_set_admin_threshold_exceeds_admin_count_rejected() {
        let env = Env::default();
        let (client, admin, _token_id) = setup(&env);

        let admins = Vec::from_array(&env, [admin.clone()]);

        // Only 1 admin; requesting threshold of 2 must fail.
        let result = client.try_set_admin_threshold(&admins, &2u32);
        assert_eq!(
            result,
            Err(Ok(ContractError::InvalidAdminThreshold)),
            "expected InvalidAdminThreshold when threshold exceeds admin count"
        );
    }

    /// initialize (validate_admin_config): threshold > admins.len() at init must fail.
    #[test]
    fn test_initialize_invalid_threshold_rejected() {
        let env = Env::default();
        env.mock_all_auths();

        let deployer = Address::generate(&env);
        let admin = Address::generate(&env);
        let admins = Vec::from_array(&env, [admin.clone()]);

        let token_id = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        let contract_id = env.register_contract(None, QuorumCreditContract);
        let client = QuorumCreditContractClient::new(&env, &contract_id);

        // 1 admin but threshold = 2 → must fail
        let result = client.try_initialize(&deployer, &admins, &2u32, &token_id);
        assert!(
            result.is_err(),
            "expected error when threshold exceeds admin count at initialization"
        );
    }

    // ── Issue: initialize_slash_vote for non-borrower ─────────────────────────

    /// vote_slash: borrower with no active loan must return NoActiveLoan.
    #[test]
    fn test_slash_vote_for_non_borrower_rejected() {
        let env = Env::default();
        let (client, _admin, token_id) = setup(&env);

        let voucher = Address::generate(&env);
        let borrower = Address::generate(&env);

        // Give voucher some tokens and vouch (but no loan is ever requested)
        StellarAssetClient::new(&env, &token_id).mint(&voucher, &1_000_000);
        client.vouch(&voucher, &borrower, &1_000_000, &token_id, &None);

        // borrower has no active loan → vote_slash must fail
        let result = client.try_vote_slash(&voucher, &borrower, &true);
        assert_eq!(
            result,
            Err(Ok(ContractError::NoActiveLoan)),
            "expected NoActiveLoan when borrower has no active loan"
        );
    }

    // ── Issue: vouch() accepts zero/negative stake ────────────────────────────

    /// vouch: zero stake must be rejected.
    #[test]
    fn test_vouch_zero_stake_rejected() {
        let env = Env::default();
        let (client, _admin, token_id) = setup(&env);

        let voucher = Address::generate(&env);
        let borrower = Address::generate(&env);

        let result = client.try_vouch(&voucher, &borrower, &0i128, &token_id, &None);
        assert_eq!(
            result,
            Err(Ok(ContractError::InsufficientFunds)),
            "expected InsufficientFunds for zero stake"
        );
    }

    /// vouch: negative stake must be rejected.
    #[test]
    fn test_vouch_negative_stake_rejected() {
        let env = Env::default();
        let (client, _admin, token_id) = setup(&env);

        let voucher = Address::generate(&env);
        let borrower = Address::generate(&env);

        let result = client.try_vouch(&voucher, &borrower, &-1_000_000i128, &token_id, &None);
        assert_eq!(
            result,
            Err(Ok(ContractError::InsufficientFunds)),
            "expected InsufficientFunds for negative stake"
        );
    }

    // ── Issue: increase_stake() accepts zero/negative amount ─────────────────

    /// increase_stake: zero amount must be rejected.
    #[test]
    fn test_increase_stake_zero_amount_rejected() {
        let env = Env::default();
        let (client, _admin, token_id) = setup(&env);

        let voucher = Address::generate(&env);
        let borrower = Address::generate(&env);

        StellarAssetClient::new(&env, &token_id).mint(&voucher, &1_000_000);
        client.vouch(&voucher, &borrower, &1_000_000, &token_id, &None);

        let result = client.try_increase_stake(&voucher, &borrower, &0i128);
        assert_eq!(
            result,
            Err(Ok(ContractError::InsufficientFunds)),
            "expected InsufficientFunds for zero increase amount"
        );
    }

    /// increase_stake: negative amount must be rejected.
    #[test]
    fn test_increase_stake_negative_amount_rejected() {
        let env = Env::default();
        let (client, _admin, token_id) = setup(&env);

        let voucher = Address::generate(&env);
        let borrower = Address::generate(&env);

        StellarAssetClient::new(&env, &token_id).mint(&voucher, &1_000_000);
        client.vouch(&voucher, &borrower, &1_000_000, &token_id, &None);

        let result = client.try_increase_stake(&voucher, &borrower, &-500_000i128);
        assert_eq!(
            result,
            Err(Ok(ContractError::InsufficientFunds)),
            "expected InsufficientFunds for negative increase amount"
        );
    }
}
