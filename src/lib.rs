#![no_std]

mod errors;
mod helpers;
mod insurance;
mod types;
mod vouch;

use soroban_sdk::{contract, contractimpl, symbol_short, Address, Env, String, Vec};

#[cfg(test)]
mod insurance_pool_test;

use crate::errors::ContractError;
use crate::helpers::{config, get_active_loan_record, has_active_loan, require_allowed_token, require_not_paused};
use crate::types::{
    Config, DataKey, LoanRecord, LoanStatus, VouchRecord,
    DEFAULT_LOAN_DURATION, DEFAULT_MAX_LOAN_TO_STAKE_RATIO, DEFAULT_MAX_VOUCHERS,
    DEFAULT_MIN_LOAN_AMOUNT, DEFAULT_MIN_VOUCH_AGE_SECS, DEFAULT_SLASH_BPS, DEFAULT_YIELD_BPS,
};

#[contract]
pub struct QuorumCreditContract;

#[contractimpl]
impl QuorumCreditContract {
    // ─────────────────────────────────────────────
    // Initialization
    // ─────────────────────────────────────────────

    pub fn initialize(
        env: Env,
        deployer: Address,
        admins: Vec<Address>,
        admin_threshold: u32,
        token: Address,
    ) -> Result<(), ContractError> {
        deployer.require_auth();

        if env.storage().instance().has(&DataKey::Config) {
            return Err(ContractError::AlreadyInitialized);
        }

        if admins.is_empty() || admin_threshold == 0 || admin_threshold > admins.len() {
            return Err(ContractError::InvalidAmount);
        }

        env.storage().instance().set(&DataKey::Deployer, &deployer);
        env.storage().instance().set(
            &DataKey::Config,
            &Config {
                admins,
                admin_threshold,
                token,
                allowed_tokens: Vec::new(&env),
                yield_bps: DEFAULT_YIELD_BPS,
                slash_bps: DEFAULT_SLASH_BPS,
                max_vouchers: DEFAULT_MAX_VOUCHERS,
                min_loan_amount: DEFAULT_MIN_LOAN_AMOUNT,
                loan_duration: DEFAULT_LOAN_DURATION,
                max_loan_to_stake_ratio: DEFAULT_MAX_LOAN_TO_STAKE_RATIO,
                grace_period: 0,
                min_vouch_age_secs: DEFAULT_MIN_VOUCH_AGE_SECS,
                prepayment_penalty_bps: 0,
            },
        );

        Ok(())
    }

    // ─────────────────────────────────────────────
    // Core Vouching
    // ─────────────────────────────────────────────

    pub fn vouch(
        env: Env,
        voucher: Address,
        borrower: Address,
        stake: i128,
        token: Address,
    ) -> Result<(), ContractError> {
        vouch::vouch(env, voucher, borrower, stake, token)
    }

    // ─────────────────────────────────────────────
    // Stake Management
    // ─────────────────────────────────────────────

    pub fn increase_stake(
        env: Env,
        voucher: Address,
        borrower: Address,
        additional: i128,
    ) -> Result<(), ContractError> {
        vouch::increase_stake(env, voucher, borrower, additional)
    }

    // ─────────────────────────────────────────────
    // Loans
    // ─────────────────────────────────────────────

    /// Request a loan. 0.5% of the loan amount is automatically routed to the insurance pool.
    pub fn request_loan(
        env: Env,
        borrower: Address,
        amount: i128,
        threshold: i128,
        loan_purpose: String,
        token_addr: Address,
    ) -> Result<(), ContractError> {
        borrower.require_auth();
        require_not_paused(&env)?;

        if has_active_loan(&env, &borrower) {
            return Err(ContractError::ActiveLoanExists);
        }

        let token_client = require_allowed_token(&env, &token_addr)?;
        let cfg = config(&env);

        if amount < cfg.min_loan_amount {
            return Err(ContractError::LoanBelowMinAmount);
        }

        let vouches: Vec<VouchRecord> = env
            .storage()
            .persistent()
            .get(&DataKey::Vouches(borrower.clone()))
            .unwrap_or(Vec::new(&env));

        let total_stake: i128 = vouches
            .iter()
            .filter(|v| v.token == token_addr)
            .map(|v| v.stake)
            .sum();

        if total_stake < threshold {
            return Err(ContractError::InsufficientFunds);
        }

        let now = env.ledger().timestamp();
        let loan_id: u64 = env
            .storage()
            .persistent()
            .get(&DataKey::LoanCounter)
            .unwrap_or(0u64)
            + 1;
        env.storage()
            .persistent()
            .set(&DataKey::LoanCounter, &loan_id);

        let total_yield = amount * cfg.yield_bps / 10_000;

        let loan = LoanRecord {
            id: loan_id,
            borrower: borrower.clone(),
            co_borrowers: Vec::new(&env),
            amount,
            amount_repaid: 0,
            total_yield,
            status: LoanStatus::Active,
            created_at: now,
            disbursement_timestamp: now,
            repayment_timestamp: None,
            deadline: now + cfg.loan_duration,
            loan_purpose,
            token_address: token_addr.clone(),
            amortization_schedule: Vec::new(&env),
            reminder_sent: false,
            risk_score: 0,
        };

        env.storage()
            .persistent()
            .set(&DataKey::Loan(loan_id), &loan);
        env.storage()
            .persistent()
            .set(&DataKey::ActiveLoan(borrower.clone()), &loan_id);

        // Auto-collect insurance fee from disbursed amount (held by contract)
        insurance::collect_loan_fee(&env, amount);

        token_client.transfer(&env.current_contract_address(), &borrower, &amount);

        env.events().publish(
            (symbol_short!("loan"), symbol_short!("created")),
            (borrower, amount),
        );

        Ok(())
    }

    /// Slash a borrower's loan (admin). Routes 20% of slashed funds to insurance pool.
    pub fn slash(
        env: Env,
        admin_signers: Vec<Address>,
        borrower: Address,
    ) -> Result<(), ContractError> {
        require_not_paused(&env)?;

        let cfg = config(&env);
        let mut approved: u32 = 0;
        for signer in admin_signers.iter() {
            if cfg.admins.iter().any(|a| a == signer) {
                signer.require_auth();
                approved += 1;
            }
        }
        if approved < cfg.admin_threshold {
            return Err(ContractError::UnauthorizedCaller);
        }

        let mut loan = get_active_loan_record(&env, &borrower)?;
        loan.status = LoanStatus::Defaulted;

        let vouches: Vec<VouchRecord> = env
            .storage()
            .persistent()
            .get(&DataKey::Vouches(borrower.clone()))
            .unwrap_or(Vec::new(&env));

        let token_client = require_allowed_token(&env, &loan.token_address)?;
        let contract = env.current_contract_address();
        let mut total_slashed: i128 = 0;

        for v in vouches.iter() {
            if v.token != loan.token_address {
                continue;
            }
            let slash_amount = v.stake * cfg.slash_bps / 10_000;
            total_slashed += slash_amount;
            // Return unslashed portion to voucher
            let returned = v.stake - slash_amount;
            if returned > 0 {
                token_client.transfer(&contract, &v.voucher, &returned);
            }
        }

        // Route 20% of total slashed to insurance pool; rest to slash treasury
        let to_pool = total_slashed * crate::types::SLASH_TO_INSURANCE_BPS as i128 / 10_000;
        let to_treasury = total_slashed - to_pool;

        insurance::allocate_slash_to_pool(&env, total_slashed);

        let treasury: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::SlashTreasury)
            .unwrap_or(0);
        env.storage()
            .persistent()
            .set(&DataKey::SlashTreasury, &(treasury + to_treasury));

        env.storage()
            .persistent()
            .set(&DataKey::Loan(loan.id), &loan);
        env.storage()
            .persistent()
            .remove(&DataKey::ActiveLoan(borrower.clone()));

        env.events().publish(
            (symbol_short!("loan"), symbol_short!("slashed")),
            (borrower, total_slashed),
        );

        Ok(())
    }

    pub fn repay(env: Env, borrower: Address, payment: i128) -> Result<(), ContractError> {
        borrower.require_auth();
        require_not_paused(&env)?;

        let mut loan = get_active_loan_record(&env, &borrower)?;

        if payment <= 0 {
            return Err(ContractError::InvalidAmount);
        }

        let total_owed = loan.amount + loan.total_yield;
        let outstanding = total_owed - loan.amount_repaid;

        if payment > outstanding {
            return Err(ContractError::InvalidAmount);
        }

        let token_client = require_allowed_token(&env, &loan.token_address)?;
        token_client.transfer(&borrower, &env.current_contract_address(), &payment);
        loan.amount_repaid += payment;

        if loan.amount_repaid >= total_owed {
            loan.status = LoanStatus::Repaid;
            loan.repayment_timestamp = Some(env.ledger().timestamp());

            let vouches: Vec<VouchRecord> = env
                .storage()
                .persistent()
                .get(&DataKey::Vouches(borrower.clone()))
                .unwrap_or(Vec::new(&env));

            let total_stake: i128 = vouches
                .iter()
                .filter(|v| v.token == loan.token_address)
                .map(|v| v.stake)
                .sum();

            for v in vouches.iter() {
                if v.token != loan.token_address {
                    continue;
                }
                let yield_share = if total_stake > 0 {
                    loan.total_yield * v.stake / total_stake
                } else {
                    0
                };
                token_client.transfer(
                    &env.current_contract_address(),
                    &v.voucher,
                    &(v.stake + yield_share),
                );
            }

            env.storage()
                .persistent()
                .remove(&DataKey::ActiveLoan(borrower.clone()));
            env.storage()
                .persistent()
                .remove(&DataKey::Vouches(borrower.clone()));

            env.events().publish(
                (symbol_short!("loan"), symbol_short!("repaid")),
                (borrower.clone(), loan.amount),
            );
        }

        env.storage()
            .persistent()
            .set(&DataKey::Loan(loan.id), &loan);

        Ok(())
    }

    pub fn get_loan(env: Env, borrower: Address) -> Option<LoanRecord> {
        let loan_id: u64 = env
            .storage()
            .persistent()
            .get(&DataKey::ActiveLoan(borrower.clone()))?;
        env.storage().persistent().get(&DataKey::Loan(loan_id))
    }

    pub fn get_vouches(env: Env, borrower: Address) -> Vec<VouchRecord> {
        env.storage()
            .persistent()
            .get(&DataKey::Vouches(borrower))
            .unwrap_or(Vec::new(&env))
    }

    // ─────────────────────────────────────────────
    // Insurance Pool
    // ─────────────────────────────────────────────

    /// Voluntarily contribute tokens to the insurance pool.
    pub fn contribute_to_insurance(
        env: Env,
        contributor: Address,
        amount: i128,
    ) -> Result<(), ContractError> {
        insurance::contribute_to_insurance(env, contributor, amount)
    }

    /// Claim insurance payout after a borrower default.
    /// Payout = min(pool, slashed_stake × coverage_bps / 10_000). Capped at 25% by default.
    pub fn claim_insurance(
        env: Env,
        voucher: Address,
        loan_id: u64,
    ) -> Result<(), ContractError> {
        insurance::claim_insurance(env, voucher, loan_id)
    }

    /// Returns the current insurance pool balance in stroops.
    pub fn get_insurance_pool_balance(env: Env) -> i128 {
        insurance::get_insurance_pool_balance(env)
    }

    /// Set the protocol insurance fee in basis points (admin-only, max 10000).
    pub fn set_insurance_fee_bps(
        env: Env,
        admin_signers: Vec<Address>,
        fee_bps: u32,
    ) -> Result<(), ContractError> {
        insurance::set_insurance_fee_bps(env, admin_signers, fee_bps)
    }

    /// Set the insurance coverage cap in basis points (admin-only, max 10000).
    pub fn set_insurance_coverage_bps(
        env: Env,
        admin_signers: Vec<Address>,
        coverage_bps: u32,
    ) -> Result<(), ContractError> {
        insurance::set_insurance_coverage_bps(env, admin_signers, coverage_bps)
    }

    /// Returns the current insurance fee in basis points.
    pub fn get_insurance_fee_bps(env: Env) -> u32 {
        insurance::get_insurance_fee_bps_pub(env)
    }

    /// Returns the current insurance coverage cap in basis points.
    pub fn get_insurance_coverage_bps(env: Env) -> u32 {
        insurance::get_insurance_coverage_bps_pub(env)
    }
}
