/// Repayment Reminder Tests (Issue #470)
///
/// Verify that `emit_repayment_reminders` emits events only for active loans
/// whose deadline is within 7 days.

#[cfg(test)]
mod repayment_reminder_tests {
    use crate::{QuorumCreditContract, QuorumCreditContractClient};
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

        StellarAssetClient::new(&env, &token_id.address()).mint(&contract_id, &10_000_000);

        let client = QuorumCreditContractClient::new(&env, &contract_id);
        client.initialize(&deployer, &admins, &1, &token_id.address());

        // Start after MIN_VOUCH_AGE (60s)
        env.ledger().with_mut(|l| l.timestamp = 120);

        Setup { env, client, token_id: token_id.address() }
    }

    fn do_vouch(s: &Setup, voucher: &Address, borrower: &Address, stake: i128) {
        StellarAssetClient::new(&s.env, &s.token_id).mint(voucher, &stake);
        s.client.vouch(voucher, borrower, &stake, &s.token_id, &None);
    }

    fn purpose(env: &Env) -> String {
        String::from_str(env, "test loan")
    }

    /// A loan with deadline within 7 days should produce a reminder event.
    #[test]
    fn test_reminder_emitted_for_loan_near_deadline() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);

        do_vouch(&s, &voucher, &borrower, 500_000);
        s.client.request_loan(&borrower, &100_000, &500_000, &purpose(&s.env), &s.token_id);

        // Advance time so deadline is 3 days away (within 7-day window).
        // Default loan_duration = 30 days. Created at t=120.
        // deadline = 120 + 30*24*3600 = 2_592_120
        // Set now = deadline - 3 days
        let loan = s.client.get_loan(&borrower).unwrap();
        let three_days: u64 = 3 * 24 * 60 * 60;
        s.env.ledger().with_mut(|l| l.timestamp = loan.deadline - three_days);

        let events_before = s.env.events().all().len();
        s.client.emit_repayment_reminders();
        let events_after = s.env.events().all().len();

        assert!(
            events_after > events_before,
            "expected a reminder event to be emitted"
        );
    }

    /// A loan with deadline more than 7 days away should NOT produce a reminder.
    #[test]
    fn test_no_reminder_for_loan_far_from_deadline() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);

        do_vouch(&s, &voucher, &borrower, 500_000);
        s.client.request_loan(&borrower, &100_000, &500_000, &purpose(&s.env), &s.token_id);

        // Stay at t=120 — deadline is 30 days away, well outside the 7-day window.
        let events_before = s.env.events().all().len();
        s.client.emit_repayment_reminders();
        let events_after = s.env.events().all().len();

        assert_eq!(
            events_after, events_before,
            "expected no reminder event for loan far from deadline"
        );
    }

    /// No loans → no events emitted.
    #[test]
    fn test_no_reminders_when_no_loans() {
        let s = setup();
        let events_before = s.env.events().all().len();
        s.client.emit_repayment_reminders();
        let events_after = s.env.events().all().len();
        assert_eq!(events_after, events_before, "expected no events when no loans exist");
    }

    /// Test send_repayment_reminder marks reminder_sent as true
    #[test]
    fn test_send_repayment_reminder_marks_sent() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);

        do_vouch(&s, &voucher, &borrower, 500_000);
        s.client.request_loan(&borrower, &100_000, &500_000, &purpose(&s.env), &s.token_id);

        let loan = s.client.get_loan(&borrower).unwrap();
        assert!(!loan.reminder_sent, "reminder should not be sent initially");

        // Send reminder
        s.client.send_repayment_reminder(&loan.id).unwrap();

        let loan_after = s.client.get_loan(&borrower).unwrap();
        assert!(loan_after.reminder_sent, "reminder should be marked as sent");
    }

    /// Test send_repayment_reminder emits event
    #[test]
    fn test_send_repayment_reminder_emits_event() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);

        do_vouch(&s, &voucher, &borrower, 500_000);
        s.client.request_loan(&borrower, &100_000, &500_000, &purpose(&s.env), &s.token_id);

        let loan = s.client.get_loan(&borrower).unwrap();
        let events_before = s.env.events().all().len();
        s.client.send_repayment_reminder(&loan.id).unwrap();
        let events_after = s.env.events().all().len();

        assert!(events_after > events_before, "expected reminder event to be emitted");
    }

    /// Test send_repayment_reminder fails if already sent
    #[test]
    fn test_send_repayment_reminder_fails_if_already_sent() {
        let s = setup();
        let borrower = Address::generate(&s.env);
        let voucher = Address::generate(&s.env);

        do_vouch(&s, &voucher, &borrower, 500_000);
        s.client.request_loan(&borrower, &100_000, &500_000, &purpose(&s.env), &s.token_id);

        let loan = s.client.get_loan(&borrower).unwrap();
        s.client.send_repayment_reminder(&loan.id).unwrap();

        // Try to send again
        let result = s.client.try_send_repayment_reminder(&loan.id);
        assert!(result.is_err(), "expected error when sending reminder twice");
    }
}
