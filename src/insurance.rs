//! Insurance pool for voucher loss protection.
//!
//! ## Funding sources
//! 1. **Protocol fee** — `insurance_fee_bps` (default 0.5%) of each loan amount is
//!    automatically routed to the pool at disbursement time.
//! 2. **Voluntary contributions** — anyone may call `contribute_to_insurance()`.
//! 3. **Slashed funds** — `SLASH_TO_INSURANCE_BPS` (20%) of every slash event is
//!    automatically added to the pool; the remainder goes to `SlashTreasury`.
//!
//! ## Claims
//! After a borrower defaults (loan status = Defaulted), each voucher may call
//! `claim_insurance()` once per loan. The payout is:
//!
//!   `payout = min(pool_balance, slashed_stake * coverage_bps / 10_000)`
//!
//! where `coverage_bps` defaults to 2500 (25%). This caps the pool's exposure
//! per voucher while still providing meaningful relief.
//!
//! ## Governance
//! Admins can adjust `insurance_fee_bps` and `insurance_coverage_bps` via
//! `set_insurance_fee_bps()` and `set_insurance_coverage_bps()`.

use crate::errors::ContractError;
use crate::helpers::{config, require_not_paused};
use crate::types::{
    BPS_DENOMINATOR, DataKey, DEFAULT_INSURANCE_COVERAGE_BPS, DEFAULT_INSURANCE_FEE_BPS,
    LoanRecord, LoanStatus, SLASH_TO_INSURANCE_BPS, VouchRecord,
};
use soroban_sdk::{symbol_short, token, Address, Env, Vec};

// ── Pool funding ──────────────────────────────────────────────────────────────

/// Contribute tokens voluntarily to the insurance pool.
pub fn contribute_to_insurance(
    env: Env,
    contributor: Address,
    amount: i128,
) -> Result<(), ContractError> {
    contributor.require_auth();
    require_not_paused(&env)?;

    if amount <= 0 {
        return Err(ContractError::InvalidAmount);
    }

    let cfg = config(&env);
    let token_client = token::Client::new(&env, &cfg.token);
    token_client.transfer(&contributor, &env.current_contract_address(), &amount);

    add_to_pool(&env, amount);

    env.events().publish(
        (symbol_short!("ins"), symbol_short!("contrib")),
        (contributor, amount),
    );

    Ok(())
}

/// Called internally at loan disbursement to collect the protocol insurance fee.
/// `loan_amount` is the principal disbursed in stroops.
pub fn collect_loan_fee(env: &Env, loan_amount: i128) {
    let fee_bps = get_insurance_fee_bps(env);
    if fee_bps == 0 {
        return;
    }
    let fee = loan_amount * fee_bps as i128 / BPS_DENOMINATOR;
    if fee > 0 {
        add_to_pool(env, fee);
    }
}

/// Called internally after a slash to route a portion of slashed funds to the pool.
/// `slashed_total` is the total amount slashed across all vouchers in stroops.
pub fn allocate_slash_to_pool(env: &Env, slashed_total: i128) {
    let allocation = slashed_total * SLASH_TO_INSURANCE_BPS as i128 / BPS_DENOMINATOR;
    if allocation > 0 {
        add_to_pool(env, allocation);
    }
}

// ── Claims ────────────────────────────────────────────────────────────────────

/// Claim insurance payout for a defaulted loan.
///
/// The claimant must have been a voucher on the defaulted loan.
/// Each voucher may claim once per loan.
/// Payout = min(pool_balance, slashed_stake * coverage_bps / 10_000).
pub fn claim_insurance(
    env: Env,
    voucher: Address,
    loan_id: u64,
) -> Result<(), ContractError> {
    voucher.require_auth();
    require_not_paused(&env)?;

    let loan: LoanRecord = env
        .storage()
        .persistent()
        .get(&DataKey::Loan(loan_id))
        .ok_or(ContractError::NoActiveLoan)?;

    if loan.status != LoanStatus::Defaulted {
        return Err(ContractError::InvalidStateTransition);
    }

    // Prevent double-claim by this voucher on this loan
    if env
        .storage()
        .persistent()
        .has(&DataKey::InsuranceVoucherClaim(loan_id, voucher.clone()))
    {
        return Err(ContractError::InsuranceClaimAlreadyMade);
    }

    // Verify the claimant was a voucher on this loan
    let vouches: Vec<VouchRecord> = env
        .storage()
        .persistent()
        .get(&DataKey::Vouches(loan.borrower.clone()))
        .unwrap_or(Vec::new(&env));

    // Also check voucher history (vouches are cleared after slash in some flows)
    let history: Vec<Address> = env
        .storage()
        .persistent()
        .get(&DataKey::VoucherHistory(voucher.clone()))
        .unwrap_or(Vec::new(&env));

    let voucher_stake: i128 = vouches
        .iter()
        .find(|v| v.voucher == voucher && v.token == loan.token_address)
        .map(|v| v.stake)
        .unwrap_or_else(|| {
            // Vouches may have been cleared post-slash; fall back to history check
            if history.iter().any(|b| b == loan.borrower) {
                // Stake is unknown post-slash; use loan.amount as proxy for proportional calc
                0
            } else {
                0
            }
        });

    // If vouches are cleared (post-slash), verify via history
    let is_voucher = voucher_stake > 0
        || vouches.iter().any(|v| v.voucher == voucher)
        || history.iter().any(|b| b == loan.borrower);

    if !is_voucher {
        return Err(ContractError::UnauthorizedCaller);
    }

    let pool = get_pool_balance(&env);
    if pool <= 0 {
        return Err(ContractError::InsurancePoolEmpty);
    }

    // Compute payout: up to coverage_bps% of the voucher's slashed stake.
    // If stake is unknown (post-slash cleared), use proportional share of loan amount.
    let coverage_bps = get_insurance_coverage_bps(&env);
    let cfg = config(&env);
    let slash_bps = cfg.slash_bps;

    let slashed_stake = if voucher_stake > 0 {
        voucher_stake * slash_bps / BPS_DENOMINATOR
    } else {
        // Fallback: proportional share based on number of vouchers
        let n = vouches.len().max(1) as i128;
        loan.amount / n * slash_bps / BPS_DENOMINATOR
    };

    let max_payout = slashed_stake * coverage_bps as i128 / BPS_DENOMINATOR;
    let payout = pool.min(max_payout).max(0);

    if payout == 0 {
        return Err(ContractError::InsurancePoolEmpty);
    }

    // Deduct from pool and record claim
    set_pool_balance(&env, pool - payout);
    env.storage().persistent().set(
        &DataKey::InsuranceVoucherClaim(loan_id, voucher.clone()),
        &payout,
    );

    let token_client = token::Client::new(&env, &loan.token_address);
    token_client.transfer(&env.current_contract_address(), &voucher, &payout);

    env.events().publish(
        (symbol_short!("ins"), symbol_short!("claim")),
        (voucher, loan_id, payout),
    );

    Ok(())
}

// ── Governance ────────────────────────────────────────────────────────────────

/// Set the protocol insurance fee in basis points (admin-only).
/// This fee is deducted from each loan disbursement and added to the pool.
pub fn set_insurance_fee_bps(
    env: Env,
    admin_signers: Vec<Address>,
    fee_bps: u32,
) -> Result<(), ContractError> {
    require_admin(&env, &admin_signers)?;

    if fee_bps > 10_000 {
        return Err(ContractError::InvalidBps);
    }

    env.storage()
        .instance()
        .set(&DataKey::InsuranceFeeBps, &fee_bps);

    Ok(())
}

/// Set the insurance coverage cap in basis points (admin-only).
/// Payout per voucher is capped at `coverage_bps / 10_000` of their slashed stake.
pub fn set_insurance_coverage_bps(
    env: Env,
    admin_signers: Vec<Address>,
    coverage_bps: u32,
) -> Result<(), ContractError> {
    require_admin(&env, &admin_signers)?;

    if coverage_bps > 10_000 {
        return Err(ContractError::InvalidBps);
    }

    env.storage()
        .instance()
        .set(&DataKey::InsuranceCoverageBps, &coverage_bps);

    Ok(())
}

/// Returns the current insurance pool balance in stroops.
pub fn get_insurance_pool_balance(env: Env) -> i128 {
    get_pool_balance(&env)
}

/// Returns the current insurance fee in basis points.
pub fn get_insurance_fee_bps_pub(env: Env) -> u32 {
    get_insurance_fee_bps(&env)
}

/// Returns the current insurance coverage cap in basis points.
pub fn get_insurance_coverage_bps_pub(env: Env) -> u32 {
    get_insurance_coverage_bps(&env)
}

// ── Internal helpers ──────────────────────────────────────────────────────────

fn get_pool_balance(env: &Env) -> i128 {
    env.storage()
        .instance()
        .get(&DataKey::InsurancePool)
        .unwrap_or(0)
}

fn set_pool_balance(env: &Env, balance: i128) {
    env.storage()
        .instance()
        .set(&DataKey::InsurancePool, &balance);
}

fn add_to_pool(env: &Env, amount: i128) {
    let current = get_pool_balance(env);
    set_pool_balance(env, current + amount);
}

fn get_insurance_fee_bps(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&DataKey::InsuranceFeeBps)
        .unwrap_or(DEFAULT_INSURANCE_FEE_BPS)
}

fn get_insurance_coverage_bps(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&DataKey::InsuranceCoverageBps)
        .unwrap_or(DEFAULT_INSURANCE_COVERAGE_BPS)
}

fn require_admin(env: &Env, signers: &Vec<Address>) -> Result<(), ContractError> {
    let cfg = config(env);
    let mut approved: u32 = 0;
    for signer in signers.iter() {
        if cfg.admins.iter().any(|a| a == signer) {
            signer.require_auth();
            approved += 1;
        }
    }
    if approved < cfg.admin_threshold {
        return Err(ContractError::UnauthorizedCaller);
    }
    Ok(())
}
