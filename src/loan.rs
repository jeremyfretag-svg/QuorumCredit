use crate::errors::ContractError;
use crate::helpers::{
    config, deduct_slash_balance, get_active_loan_record, get_latest_loan_record,
    has_active_loan, next_loan_id, register_borrower_if_needed, require_allowed_token,
    require_not_paused, require_admin_approval,
};
use crate::reputation::ReputationNftExternalClient;
use crate::types::{
    DataKey, LoanRecord, LoanStatus, SlashRecord, VouchRecord, BPS_DENOMINATOR, SLASH_ESCROW_PERIOD,
};
use soroban_sdk::{panic_with_error, symbol_short, Address, Env, Vec};

/// Calculate dynamic yield
pub fn calculate_dynamic_yield(env: &Env, borrower: &Address) -> i128 {
    let base_bps = config(env).yield_bps;

    let credit_score: i128 = env
        .storage()
        .instance()
        .get::<DataKey, Address>(&DataKey::ReputationNft)
        .map(|nft| ReputationNftExternalClient::new(env, &nft).balance(borrower) as i128)
        .unwrap_or(0);

    let default_count: i128 = env
        .storage()
        .persistent()
        .get::<DataKey, u32>(&DataKey::DefaultCount(borrower.clone()))
        .unwrap_or(0) as i128;

    (base_bps + (credit_score / 100) - (default_count * 50)).max(0)
}

/// Request loan
pub fn request_loan(
    env: Env,
    borrower: Address,
    amount: i128,
    threshold: i128,
    loan_purpose: soroban_sdk::String,
    token_addr: Address,
) -> Result<(), ContractError> {
    borrower.require_auth();
    require_not_paused(&env)?;
    register_borrower_if_needed(&env, &borrower);

    if has_active_loan(&env, &borrower) {
        return Err(ContractError::ActiveLoanExists);
    }

    let token = require_allowed_token(&env, &token_addr)?;
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
        panic_with_error!(&env, ContractError::InsufficientFunds);
    }

    let now = env.ledger().timestamp();
    let loan_id = next_loan_id(&env);

    let yield_bps = calculate_dynamic_yield(&env, &borrower);
    let total_yield = amount * yield_bps / 10_000;

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

    env.storage().persistent().set(&DataKey::Loan(loan_id), &loan);
    env.storage()
        .persistent()
        .set(&DataKey::ActiveLoan(borrower.clone()), &loan_id);
    env.storage()
        .persistent()
        .set(&DataKey::LatestLoan(borrower.clone()), &loan_id);

    token.transfer(&env.current_contract_address(), &borrower, &amount);

    env.events().publish(
        (symbol_short!("loan"), symbol_short!("created")),
        (borrower, amount),
    );

    Ok(())
}

/// Apply slash recovery to borrower when a defaulted loan is fully repaid.
pub fn apply_slash_recovery(env: &Env, borrower: &Address) -> Result<(), ContractError> {
    let cfg = config(env);
    if cfg.recovery_percentage == 0 {
        return Ok(());
    }

    let mut record: SlashRecord = match env
        .storage()
        .persistent()
        .get::<DataKey, SlashRecord>(&DataKey::SlashAudit(borrower.clone()))
    {
        Some(r) if !r.reversed => r,
        _ => return Ok(()),
    };

    if record.recovery_amount > 0 {
        return Ok(());
    }

    let recoverable = record.total_slashed * cfg.recovery_percentage as i128 / BPS_DENOMINATOR;
    if recoverable <= 0 {
        return Ok(());
    }

    deduct_slash_balance(env, recoverable)?;

    let loan = get_latest_loan_record(env, borrower).ok_or(ContractError::NoActiveLoan)?;
    let token = require_allowed_token(env, &loan.token_address)?;
    token.transfer(
        &env.current_contract_address(),
        borrower,
        &recoverable,
    );

    record.recovery_amount = recoverable;
    env.storage()
        .persistent()
        .set(&DataKey::SlashRecord(record.slash_id), &record);
    env.storage()
        .persistent()
        .set(&DataKey::SlashAudit(borrower.clone()), &record);

    env.events().publish(
        (symbol_short!("loan"), symbol_short!("recovery")),
        (borrower.clone(), recoverable),
    );

    Ok(())
}

/// Repay loan (active or defaulted).
pub fn repay(env: Env, borrower: Address, payment: i128) -> Result<(), ContractError> {
    borrower.require_auth();
    require_not_paused(&env)?;

    let mut loan = match get_active_loan_record(&env, &borrower) {
        Ok(l) => l,
        Err(ContractError::NoActiveLoan) => {
            let l = get_latest_loan_record(&env, &borrower).ok_or(ContractError::NoActiveLoan)?;
            if l.status != LoanStatus::Defaulted {
                return Err(ContractError::NoActiveLoan);
            }
            l
        }
        Err(e) => return Err(e),
    };

    let was_defaulted = loan.status == LoanStatus::Defaulted;

    if payment <= 0 {
        panic_with_error!(&env, ContractError::InvalidAmount);
    }

    let total_owed = loan.amount + loan.total_yield;
    let outstanding = total_owed - loan.amount_repaid;

    if payment > outstanding {
        panic_with_error!(&env, ContractError::InvalidAmount);
    }

    let token = require_allowed_token(&env, &loan.token_address)?;
    token.transfer(&borrower, &env.current_contract_address(), &payment);

    loan.amount_repaid += payment;

    let now = env.ledger().timestamp();
    let cfg = config(&env);

    let mut penalty: i128 = 0;
    if !was_defaulted && now < loan.deadline && cfg.prepayment_penalty_bps > 0 {
        let remaining_principal =
            loan.amount - (loan.amount_repaid * loan.amount / total_owed);
        penalty = remaining_principal * cfg.prepayment_penalty_bps as i128 / BPS_DENOMINATOR;
    }

    let fully_repaid = loan.amount_repaid >= total_owed;

    if fully_repaid && was_defaulted {
        loan.status = LoanStatus::Repaid;
        loan.repayment_timestamp = Some(now);
        apply_slash_recovery(&env, &borrower)?;
        env.events().publish(
            (symbol_short!("loan"), symbol_short!("repaid")),
            (borrower.clone(), loan.amount),
        );
    } else if fully_repaid {
        loan.status = LoanStatus::Repaid;
        loan.repayment_timestamp = Some(now);

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

        let total_yield_pool = loan.total_yield + penalty;

        for v in vouches.iter() {
            if v.token != loan.token_address {
                continue;
            }

            let share = if total_stake > 0 {
                total_yield_pool * v.stake / total_stake
            } else {
                0
            };

            token.transfer(
                &env.current_contract_address(),
                &v.voucher,
                &(v.stake + share),
            );

            let mut stats: crate::types::VoucherStats = env
                .storage()
                .persistent()
                .get(&DataKey::VoucherStats(v.voucher.clone()))
                .unwrap_or(crate::types::VoucherStats {
                    successful_vouches: 0,
                    total_vouches_slashed: 0,
                    total_yield_earned: 0,
                    total_slashed: 0,
                });
            stats.successful_vouches += 1;
            stats.total_yield_earned += share;
            env.storage()
                .persistent()
                .set(&DataKey::VoucherStats(v.voucher.clone()), &stats);
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

/// Eligibility check
pub fn is_eligible(env: Env, borrower: Address, threshold: i128, token: Address) -> bool {
    if threshold <= 0 {
        return false;
    }

    if has_active_loan(&env, &borrower) {
        return false;
    }

    let vouches: Vec<VouchRecord> = env
        .storage()
        .persistent()
        .get(&DataKey::Vouches(borrower))
        .unwrap_or(Vec::new(&env));

    let total: i128 = vouches
        .iter()
        .filter(|v| v.token == token)
        .map(|v| v.stake)
        .sum();

    total >= threshold
}

/// Partial repay (FIXED DIRECTION BUG)
pub fn repay_partial(
    env: Env,
    borrower: Address,
    payment: i128,
    token: Address,
) -> Result<(), ContractError> {
    borrower.require_auth();
    require_not_paused(&env)?;

    let mut loan = get_active_loan_record(&env, &borrower)?;

    if payment <= 0 {
        panic_with_error!(&env, ContractError::InvalidAmount);
    }

    let token_client = require_allowed_token(&env, &token)?;

    // FIX: transfer should be FROM borrower TO contract
    token_client.transfer(&borrower, &env.current_contract_address(), &payment);

    loan.amount_repaid += payment;

    env.storage()
        .persistent()
        .set(&DataKey::Loan(loan.id), &loan);

    env.events().publish(
        (symbol_short!("loan"), symbol_short!("prt_rep")),
        (borrower, payment),
    );

    Ok(())
}

/// Set yield reserve
pub fn set_yield_reserve(
    env: Env,
    admins: Vec<Address>,
    amount: i128,
) -> Result<(), ContractError> {
    require_admin_approval(&env, &admins);

    if amount < 0 {
        return Err(ContractError::InvalidAmount);
    }

    env.storage()
        .persistent()
        .set(&DataKey::YieldReserve, &amount);

    Ok(())
}

/// Get yield reserve
pub fn get_yield_reserve_balance(env: Env) -> i128 {
    env.storage()
        .persistent()
        .get(&DataKey::YieldReserve)
        .unwrap_or(0)
}

/// Set borrower risk score
pub fn set_borrower_risk_score(
    env: Env,
    admins: Vec<Address>,
    borrower: Address,
    risk_score: u32,
) -> Result<(), ContractError> {
    require_admin_approval(&env, &admins);

    if risk_score > 100 {
        return Err(ContractError::InvalidAmount);
    }

    let mut loan = get_active_loan_record(&env, &borrower)?;
    loan.risk_score = risk_score;

    env.storage()
        .persistent()
        .set(&DataKey::Loan(loan.id), &loan);

    Ok(())
}

pub fn release_slash_escrow(
    env: Env,
    admin_signers: Vec<Address>,
    borrower: Address,
) -> Result<(), ContractError> {
    require_admin_approval(&env, &admin_signers);
    let (amount, release_ts): (i128, u64) = env
        .storage()
        .persistent()
        .get(&DataKey::SlashEscrow(borrower.clone()))
        .ok_or(ContractError::NoActiveLoan)?;
    if env.ledger().timestamp() < release_ts {
        return Err(ContractError::TimelockNotReady);
    }
    crate::helpers::add_slash_balance(&env, amount);
    env.storage()
        .persistent()
        .remove(&DataKey::SlashEscrow(borrower));
    Ok(())
}

pub fn get_loan(env: Env, borrower: Address) -> Option<LoanRecord> {
    if let Ok(loan) = get_active_loan_record(&env, &borrower) {
        return Some(loan);
    }
    get_latest_loan_record(&env, &borrower)
}

pub fn get_loan_by_id(env: Env, loan_id: u64) -> Option<LoanRecord> {
    env.storage().persistent().get(&DataKey::Loan(loan_id))
}

pub fn loan_status(env: Env, borrower: Address) -> LoanStatus {
    get_loan(env, borrower)
        .map(|l| l.status)
        .unwrap_or(LoanStatus::None)
}

pub fn repayment_count(env: Env, borrower: Address) -> u32 {
    env.storage()
        .persistent()
        .get(&DataKey::RepaymentCount(borrower))
        .unwrap_or(0)
}

pub fn loan_count(env: Env, borrower: Address) -> u32 {
    env.storage()
        .persistent()
        .get(&DataKey::LoanCount(borrower))
        .unwrap_or(0)
}

pub fn default_count(env: Env, borrower: Address) -> u32 {
    env.storage()
        .persistent()
        .get(&DataKey::DefaultCount(borrower))
        .unwrap_or(0)
}

pub fn register_referral(
    _env: Env,
    _borrower: Address,
    _referrer: Address,
) -> Result<(), ContractError> {
    Err(ContractError::InvalidStateTransition)
}

pub fn get_referrer(_env: Env, _borrower: Address) -> Option<Address> {
    None
}

pub fn add_co_borrower(
    _env: Env,
    _borrower: Address,
    _co_borrower: Address,
) -> Result<(), ContractError> {
    Err(ContractError::InvalidStateTransition)
}

pub fn refinance_loan(
    _env: Env,
    _borrower: Address,
    _new_amount: i128,
    _new_threshold: i128,
    _new_token: Address,
) -> Result<(), ContractError> {
    Err(ContractError::InvalidStateTransition)
}

pub fn deposit_collateral(
    _env: Env,
    _borrower: Address,
    _amount: i128,
    _token: Address,
) -> Result<(), ContractError> {
    Err(ContractError::InvalidStateTransition)
}

pub fn get_borrower_collateral(_env: Env, _borrower: Address) -> i128 {
    0
}

pub fn emit_repayment_reminders(_env: Env) {}
pub fn mint_reputation_nft(_env: Env, _borrower: Address) -> Result<(), ContractError> {
    Ok(())
}
pub fn send_repayment_reminder(_env: Env, _loan_id: u64) -> Result<(), ContractError> {
    Ok(())
}

pub fn request_extension(
    _env: Env,
    _borrower: Address,
    _extension_secs: u64,
) -> Result<(), ContractError> {
    Err(ContractError::InvalidStateTransition)
}

pub fn approve_extension(
    _env: Env,
    _voucher: Address,
    _borrower: Address,
) -> Result<(), ContractError> {
    Err(ContractError::InvalidStateTransition)
}

pub fn get_extension_request(
    _env: Env,
    _borrower: Address,
) -> Option<crate::types::LoanExtensionRequest> {
    None
}
