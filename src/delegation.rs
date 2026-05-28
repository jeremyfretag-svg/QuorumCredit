//! Vouching delegation — allows a voucher to authorise a trusted delegate to
//! vouch on their behalf within configurable stake limits.
//!
//! ## Flow
//! 1. `delegate_vouching(voucher, delegate, max_stake_per_vouch, max_total_stake, expires_at)`
//! 2. `vouch_as_delegate(delegate, voucher, borrower, stake, token)`
//! 3. `revoke_delegation(voucher)`
//! 4. `get_delegation(voucher)` / `get_delegate_used_stake(voucher, delegate)`

use crate::errors::ContractError;
use crate::helpers::require_not_paused;
use crate::types::{DataKey, VouchingDelegation};
use soroban_sdk::{symbol_short, Address, Env};

/// Register a global vouching delegation.
pub fn delegate_vouching(
    env: Env,
    voucher: Address,
    delegate: Address,
    max_stake_per_vouch: i128,
    max_total_stake: i128,
    expires_at: Option<u64>,
) -> Result<(), ContractError> {
    voucher.require_auth();
    require_not_paused(&env)?;

    if voucher == delegate {
        return Err(ContractError::SelfVouchNotAllowed);
    }
    if max_stake_per_vouch <= 0 || max_total_stake <= 0 || max_stake_per_vouch > max_total_stake {
        return Err(ContractError::InvalidAmount);
    }
    if env.storage().persistent().has(&DataKey::VouchingDelegation(voucher.clone())) {
        return Err(ContractError::DelegationAlreadyExists);
    }

    env.storage().persistent().set(
        &DataKey::VouchingDelegation(voucher.clone()),
        &VouchingDelegation {
            delegate: delegate.clone(),
            max_stake_per_vouch,
            max_total_stake,
            created_at: env.ledger().timestamp(),
            expires_at,
        },
    );

    env.events().publish(
        (symbol_short!("deleg"), symbol_short!("set")),
        (voucher, delegate, max_stake_per_vouch, max_total_stake),
    );

    Ok(())
}

/// Revoke an active delegation. Only the original voucher may revoke.
pub fn revoke_delegation(env: Env, voucher: Address) -> Result<(), ContractError> {
    voucher.require_auth();
    require_not_paused(&env)?;

    if !env.storage().persistent().has(&DataKey::VouchingDelegation(voucher.clone())) {
        return Err(ContractError::DelegationNotFound);
    }

    let delegation: VouchingDelegation = env
        .storage()
        .persistent()
        .get(&DataKey::VouchingDelegation(voucher.clone()))
        .unwrap();

    env.storage().persistent().remove(&DataKey::VouchingDelegation(voucher.clone()));
    env.storage().persistent().remove(
        &DataKey::DelegateUsedStake(voucher.clone(), delegation.delegate),
    );

    env.events().publish(
        (symbol_short!("deleg"), symbol_short!("revoke")),
        voucher,
    );

    Ok(())
}

/// Vouch on behalf of `voucher` using delegated authority.
/// The delegate signs; tokens are pulled from `voucher`'s wallet.
pub fn vouch_as_delegate(
    env: Env,
    delegate: Address,
    voucher: Address,
    borrower: Address,
    stake: i128,
    token: Address,
) -> Result<(), ContractError> {
    delegate.require_auth();
    require_not_paused(&env)?;

    if stake <= 0 {
        return Err(ContractError::InsufficientFunds);
    }

    let delegation: VouchingDelegation = env
        .storage()
        .persistent()
        .get(&DataKey::VouchingDelegation(voucher.clone()))
        .ok_or(ContractError::DelegationNotFound)?;

    if delegation.delegate != delegate {
        return Err(ContractError::UnauthorizedCaller);
    }

    if let Some(exp) = delegation.expires_at {
        if env.ledger().timestamp() > exp {
            return Err(ContractError::DelegationExpired);
        }
    }

    if stake > delegation.max_stake_per_vouch {
        return Err(ContractError::DelegateStakeCapExceeded);
    }

    let used: i128 = env
        .storage()
        .persistent()
        .get(&DataKey::DelegateUsedStake(voucher.clone(), delegate.clone()))
        .unwrap_or(0);

    if used + stake > delegation.max_total_stake {
        return Err(ContractError::DelegateStakeCapExceeded);
    }

    // Execute the vouch using voucher's tokens (no second require_auth on voucher)
    crate::vouch::vouch_on_behalf(&env, voucher.clone(), borrower.clone(), stake, token.clone())?;

    env.storage().persistent().set(
        &DataKey::DelegateUsedStake(voucher.clone(), delegate.clone()),
        &(used + stake),
    );

    env.events().publish(
        (symbol_short!("deleg"), symbol_short!("vouch")),
        (delegate, voucher, borrower, stake, token),
    );

    Ok(())
}

/// Returns the active delegation for `voucher`, or `None`.
pub fn get_delegation(env: Env, voucher: Address) -> Option<VouchingDelegation> {
    env.storage().persistent().get(&DataKey::VouchingDelegation(voucher))
}

/// Returns total stake already committed by the delegate on behalf of `voucher`.
pub fn get_delegate_used_stake(env: Env, voucher: Address, delegate: Address) -> i128 {
    env.storage()
        .persistent()
        .get(&DataKey::DelegateUsedStake(voucher, delegate))
        .unwrap_or(0)
}
