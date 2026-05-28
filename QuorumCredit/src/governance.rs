use crate::errors::ContractError;
use crate::helpers::{
    add_slash_balance, config, extend_ttl, get_active_loan_record, require_not_paused, 
    require_not_paused_for, validate_loan_active,
};
use crate::types::{DataKey, DisputeRecord, DisputeResolution, PauseFlag, SlashVoteRecord, 
    TimelockAction, TimelockProposal, DEFAULT_DISPUTE_WINDOW_SECS, VouchRecord};
use soroban_sdk::panic_with_error;
use soroban_sdk::{symbol_short, Address, Env, Vec};

/// Default quorum: 50% of total vouched stake must approve.
const DEFAULT_SLASH_VOTE_QUORUM_BPS: u32 = 5_000;

/// Cast a governance vote on whether `borrower` should be slashed.
///
/// - Only active vouchers (those with a stake in `Vouches(borrower)`) may vote.
/// - Votes are weighted by the voucher's current stake.
/// - When `approve_stake * 10_000 / total_stake >= quorum_bps`, slash is auto-executed.
pub fn vote_slash(
    env: Env,
    voucher: Address,
    borrower: Address,
    approve: bool,
) -> Result<(), ContractError> {
    voucher.require_auth();
    require_not_paused(&env)?;
    // Task 1: Check granular pause for slash operations
    require_not_paused_for(&env, PauseFlag::Slash)?;

    // Borrower must have an active loan to be slashable.
    let loan = get_active_loan_record(&env, &borrower)?;
    if loan.status != crate::types::LoanStatus::Active {
        return Err(ContractError::NoActiveLoan);
    }

    // Fetch vouches and find this voucher's stake.
    let vouches: Vec<VouchRecord> = env
        .storage()
        .persistent()
        .get(&DataKey::Vouches(borrower.clone()))
        .unwrap_or(Vec::new(&env));

    let voucher_stake = vouches
        .iter()
        .find(|v| v.voucher == voucher)
        .map(|v| v.amount)
        .ok_or(ContractError::VoucherNotFound)?;

    let total_stake: i128 = vouches.iter().map(|v| v.amount).sum();

    // Load or initialise the vote record.
    let mut vote: SlashVoteRecord = env
        .storage()
        .persistent()
        .get(&DataKey::SlashVote(borrower.clone()))
        .unwrap_or(SlashVoteRecord {
            approve_stake: 0,
            reject_stake: 0,
            voters: Vec::new(&env),
            executed: false,
        });

    if vote.executed {
        return Err(ContractError::SlashAlreadyExecuted);
    }

    // Prevent double-voting.
    if vote.voters.iter().any(|v| v == voucher) {
        return Err(ContractError::AlreadyVoted);
    }

    if approve {
        vote.approve_stake += voucher_stake;
    } else {
        vote.reject_stake += voucher_stake;
    }
    vote.voters.push_back(voucher.clone());

    env.events().publish(
        (symbol_short!("gov"), symbol_short!("voted")),
        (voucher.clone(), borrower.clone(), approve, voucher_stake),
    );

    // Check quorum.
    let quorum_bps: u32 = env
        .storage()
        .instance()
        .get(&DataKey::SlashVoteQuorum)
        .unwrap_or(DEFAULT_SLASH_VOTE_QUORUM_BPS);

    let quorum_reached =
        total_stake > 0 && vote.approve_stake * 10_000 / total_stake >= quorum_bps as i128;

    if quorum_reached {
        vote.executed = true;
        env.storage()
            .persistent()
            .set(&DataKey::SlashVote(borrower.clone()), &vote);
        execute_slash(&env, &borrower)?;
    } else {
        env.storage()
            .persistent()
            .set(&DataKey::SlashVote(borrower.clone()), &vote);
    }

    Ok(())
}

/// Returns the current slash vote record for a borrower, if any.
pub fn get_slash_vote(env: Env, borrower: Address) -> Option<SlashVoteRecord> {
    env.storage()
        .persistent()
        .get(&DataKey::SlashVote(borrower))
}

/// Set the quorum threshold (in basis points) required to auto-execute a slash.
/// Requires admin approval — called from admin module.
pub fn set_slash_vote_quorum(env: &Env, quorum_bps: u32) {
    if quorum_bps > 10_000 {
        panic_with_error!(env, ContractError::InvalidBps);
    }
    env.storage()
        .instance()
        .set(&DataKey::SlashVoteQuorum, &quorum_bps);
}

pub fn get_slash_vote_quorum(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&DataKey::SlashVoteQuorum)
        .unwrap_or(DEFAULT_SLASH_VOTE_QUORUM_BPS)
}

/// Execute a slash vote if quorum has been met.
/// Anyone can call this function to execute a slash once quorum is reached.
pub fn execute_slash_vote(env: Env, borrower: Address) -> Result<(), ContractError> {
    require_not_paused(&env)?;
    // Task 1: Check granular pause for slash operations
    require_not_paused_for(&env, PauseFlag::Slash)?;

    let vote: SlashVoteRecord = env
        .storage()
        .persistent()
        .get(&DataKey::SlashVote(borrower.clone()))
        .ok_or(ContractError::SlashVoteNotFound)?;

    if vote.executed {
        return Err(ContractError::SlashAlreadyExecuted);
    }

    // Get total stake for the borrower
    let vouches: Vec<VouchRecord> = env
        .storage()
        .persistent()
        .get(&DataKey::Vouches(borrower.clone()))
        .unwrap_or(Vec::new(&env));
    let total_stake: i128 = vouches.iter().map(|v| v.amount).sum();

    // Retrieve quorum threshold
    let quorum_bps: u32 = get_slash_vote_quorum(&env);

    // Calculate required quorum stake
    let quorum_stake = total_stake * quorum_bps as i128 / 10_000;

    // Check if approval stake meets quorum
    if vote.approve_stake < quorum_stake {
        return Err(ContractError::QuorumNotMet);
    }

    // Mark as executed and execute the slash
    let mut updated_vote = vote;
    updated_vote.executed = true;
    env.storage()
        .persistent()
        .set(&DataKey::SlashVote(borrower.clone()), &updated_vote);

    execute_slash(&env, &borrower)?;

    Ok(())
}

// ── Internal ──────────────────────────────────────────────────────────────────

fn execute_slash(env: &Env, borrower: &Address) -> Result<(), ContractError> {
    let cfg = config(env);

    let vouches: Vec<VouchRecord> = env
        .storage()
        .persistent()
        .get(&DataKey::Vouches(borrower.clone()))
        .unwrap_or(Vec::new(env));

    if vouches.is_empty() {
        panic!("no vouchers found for borrower");
    }

    // Mark loan as defaulted first so we can read token_address.
    let mut loan = get_active_loan_record(env, borrower)?;
    validate_loan_active(&loan)?;
    let loan_token = soroban_sdk::token::Client::new(env, &loan.token_address);

    // Use token-specific slash_bps if configured, otherwise fall back to global.
    let slash_bps = env
        .storage()
        .persistent()
        .get::<DataKey, crate::types::TokenConfig>(&DataKey::TokenConfig(
            loan.token_address.clone(),
        ))
        .map(|tc| tc.slash_bps)
        .unwrap_or(cfg.slash_bps);

    let mut total_slashed: i128 = 0;

    for v in vouches.iter() {
        if v.token != loan.token_address {
            continue;
        }
        let slash_amount = v.amount * slash_bps / 10_000;
        let remaining = v.amount - slash_amount;
        total_slashed += slash_amount;

        if remaining > 0 {
            loan_token.transfer(&env.current_contract_address(), &v.voucher, &remaining);
        }
    }

    add_slash_balance(env, total_slashed);

    loan.status = crate::types::LoanStatus::Defaulted;
    env.storage()
        .persistent()
        .set(&DataKey::Loan(loan.id), &loan);
    env.storage()
        .persistent()
        .remove(&DataKey::ActiveLoan(borrower.clone()));

    let count: u32 = env
        .storage()
        .persistent()
        .get(&DataKey::DefaultCount(borrower.clone()))
        .unwrap_or(0);
    env.storage()
        .persistent()
        .set(&DataKey::DefaultCount(borrower.clone()), &(count + 1));

    env.storage()
        .persistent()
        .remove(&DataKey::Vouches(borrower.clone()));

    env.events().publish(
        (symbol_short!("gov"), symbol_short!("slashed")),
        (borrower.clone(), total_slashed),
    );

    Ok(())
}

/// ── Issue 109: Slash Proposal Confirmation Window ──
///
/// Implements a two-step slash with timelock pattern:
/// 1. propose_slash: Admin creates a proposal, sets execution time (eta)
/// 2. execute_slash_proposal: After delay, anyone can execute
///
/// Propose a slash action with a delay before execution.
/// This implements the "confirmation window" for the slash action.
pub fn propose_slash(
    env: Env,
    proposer: Address,
    borrower: Address,
    delay_secs: u64,
) -> Result<u64, ContractError> {
    proposer.require_auth();
    require_not_paused(&env)?;
    // Task 1: Check granular pause for slash operations
    require_not_paused_for(&env, PauseFlag::Slash)?;

    // Get or initialize timelock counter
    let proposal_id: u64 = env
        .storage()
        .instance()
        .get(&DataKey::TimelockCounter)
        .unwrap_or(0u64)
        .checked_add(1)
        .expect("proposal ID overflow");

    let eta = env.ledger().timestamp() + delay_secs;

    let proposal = TimelockProposal {
        id: proposal_id,
        action: TimelockAction::Slash(borrower.clone()),
        proposer: proposer.clone(),
        eta,
        executed: false,
        cancelled: false,
    };

    env.storage()
        .instance()
        .set(&DataKey::Timelock(proposal_id), &proposal);
    env.storage()
        .instance()
        .set(&DataKey::TimelockCounter, &proposal_id);

    env.events().publish(
        (symbol_short!("gov"), symbol_short!("proposed")),
        (proposal_id, proposer, borrower, eta),
    );

    Ok(proposal_id)
}

/// Execute a previously proposed slash action after the delay has passed.
pub fn execute_slash_proposal(env: Env, proposal_id: u64) -> Result<(), ContractError> {
    require_not_paused(&env)?;
    // Task 1: Check granular pause for slash operations
    require_not_paused_for(&env, PauseFlag::Slash)?;

    // Get the proposal
    let mut proposal: TimelockProposal = env
        .storage()
        .instance()
        .get(&DataKey::Timelock(proposal_id))
        .ok_or(ContractError::TimelockNotFound)?;

    // Check proposal state
    if proposal.executed {
        return Err(ContractError::SlashAlreadyExecuted);
    }
    if proposal.cancelled {
        return Err(ContractError::TimelockNotFound);
    }

    // Check delay has passed
    if env.ledger().timestamp() < proposal.eta {
        return Err(ContractError::TimelockNotReady);
    }

    // Check expiry (72 hours from eta)
    const TIMELOCK_EXPIRY: u64 = 72 * 60 * 60;
    if env.ledger().timestamp() > proposal.eta + TIMELOCK_EXPIRY {
        return Err(ContractError::TimelockExpired);
    }

    // Extract borrower from the Slash action
    if let TimelockAction::Slash(borrower) = &proposal.action {
        // Mark as executed before calling execute_slash to prevent reentrancy
        proposal.executed = true;
        env.storage()
            .instance()
            .set(&DataKey::Timelock(proposal_id), &proposal);

        // Execute the slash
        execute_slash(&env, borrower)?;

        env.events().publish(
            (symbol_short!("gov"), symbol_short!("executed")),
            (proposal_id, borrower.clone()),
        );

        Ok(())
    } else {
        Err(ContractError::NoActiveLoan) // Only Slash actions supported in this release
    }
}

/// Cancel a pending slash proposal (only by proposer or admin).
pub fn cancel_slash_proposal(
    env: Env,
    caller: Address,
    proposal_id: u64,
) -> Result<(), ContractError> {
    caller.require_auth();
    // Task 1: Check granular pause for slash operations
    require_not_paused_for(&env, PauseFlag::Slash)?;

    let mut proposal: TimelockProposal = env
        .storage()
        .instance()
        .get(&DataKey::Timelock(proposal_id))
        .ok_or(ContractError::NoActiveLoan)?;

    // Only proposer can cancel
    if caller != proposal.proposer {
        return Err(ContractError::UnauthorizedCaller);
    }

    if proposal.executed || proposal.cancelled {
        return Err(ContractError::SlashAlreadyExecuted);
    }

    proposal.cancelled = true;
    env.storage()
        .instance()
        .set(&DataKey::Timelock(proposal_id), &proposal);

    env.events().publish(
        (symbol_short!("gov"), symbol_short!("cancelled")),
        (proposal_id, caller),
    );

    Ok(())
}

/// Get a timelock proposal by ID.
pub fn get_timelock_proposal(env: Env, proposal_id: u64) -> Option<TimelockProposal> {
    env.storage()
        .instance()
        .get(&DataKey::Timelock(proposal_id))
}


// ── Governance Token Voting ───────────────────────────────────────────────────

/// Propose a governance change (token holder only).
pub fn propose_governance_change(
    env: Env,
    proposer: Address,
    description: soroban_sdk::String,
    voting_period_secs: u64,
) -> Result<u64, ContractError> {
    proposer.require_auth();
    require_not_paused(&env)?;

    // Check if governance token is set
    let gov_token_addr: Address = env
        .storage()
        .instance()
        .get(&DataKey::GovernanceTokenAddress)
        .ok_or(ContractError::InvalidToken)?;

    // Check proposer has governance tokens
    let gov_token = soroban_sdk::token::Client::new(&env, &gov_token_addr);
    let balance = gov_token.balance(&proposer);
    if balance == 0 {
        return Err(ContractError::InsufficientFunds);
    }

    let proposal_id: u64 = env
        .storage()
        .instance()
        .get(&DataKey::GovernanceProposalCounter)
        .unwrap_or(0u64)
        .checked_add(1)
        .expect("proposal ID overflow");

    let voting_end = env.ledger().timestamp() + voting_period_secs;

    let proposal = crate::types::GovernanceProposal {
        id: proposal_id,
        proposer: proposer.clone(),
        description,
        approve_votes: 0,
        reject_votes: 0,
        voters: Vec::new(&env),
        voting_end,
        executed: false,
    };

    env.storage()
        .instance()
        .set(&DataKey::GovernanceProposal(proposal_id), &proposal);
    env.storage()
        .instance()
        .set(&DataKey::GovernanceProposalCounter, &proposal_id);

    env.events().publish(
        (symbol_short!("gov"), symbol_short!("proposed")),
        (proposal_id, proposer, voting_end),
    );

    Ok(proposal_id)
}

/// Vote on a governance proposal (token holder only).
pub fn vote_on_governance_change(
    env: Env,
    voter: Address,
    proposal_id: u64,
    approve: bool,
) -> Result<(), ContractError> {
    voter.require_auth();
    require_not_paused(&env)?;

    // Check if governance token is set
    let gov_token_addr: Address = env
        .storage()
        .instance()
        .get(&DataKey::GovernanceTokenAddress)
        .ok_or(ContractError::InvalidToken)?;

    // Check voter has governance tokens
    let gov_token = soroban_sdk::token::Client::new(&env, &gov_token_addr);
    let balance = gov_token.balance(&voter);
    if balance == 0 {
        return Err(ContractError::InsufficientFunds);
    }

    let mut proposal: crate::types::GovernanceProposal = env
        .storage()
        .instance()
        .get(&DataKey::GovernanceProposal(proposal_id))
        .ok_or(ContractError::TimelockNotFound)?;

    // Check voting period hasn't ended
    if env.ledger().timestamp() > proposal.voting_end {
        return Err(ContractError::TimelockExpired);
    }

    // Prevent double voting
    if proposal.voters.iter().any(|v| v == voter) {
        return Err(ContractError::AlreadyVoted);
    }

    if approve {
        proposal.approve_votes += balance;
    } else {
        proposal.reject_votes += balance;
    }
    proposal.voters.push_back(voter.clone());

    env.storage()
        .instance()
        .set(&DataKey::GovernanceProposal(proposal_id), &proposal);

    env.events().publish(
        (symbol_short!("gov"), symbol_short!("voted")),
        (voter, proposal_id, approve, balance),
    );

    Ok(())
}

/// Execute a governance proposal after voting period ends.
pub fn execute_governance_change(env: Env, proposal_id: u64) -> Result<(), ContractError> {
    require_not_paused(&env)?;

    let mut proposal: crate::types::GovernanceProposal = env
        .storage()
        .instance()
        .get(&DataKey::GovernanceProposal(proposal_id))
        .ok_or(ContractError::TimelockNotFound)?;

    // Check voting period has ended
    if env.ledger().timestamp() <= proposal.voting_end {
        return Err(ContractError::TimelockNotReady);
    }

    // Check proposal hasn't been vetoed (#685)
    if proposal.vetoed {
        return Err(ContractError::ProposalVetoed);
    }

    // Check proposal hasn't been executed
    if proposal.executed {
        return Err(ContractError::SlashAlreadyExecuted);
    }

    // Check if approve votes exceed reject votes (simple majority)
    if proposal.approve_votes <= proposal.reject_votes {
        return Err(ContractError::QuorumNotMet);
    }

    proposal.executed = true;
    env.storage()
        .instance()
        .set(&DataKey::GovernanceProposal(proposal_id), &proposal);

    env.events().publish(
        (symbol_short!("gov"), symbol_short!("executed")),
        (proposal_id, proposal.approve_votes, proposal.reject_votes),
    );

    Ok(())
}

/// Get a governance proposal by ID.
pub fn get_governance_proposal(
    env: Env,
    proposal_id: u64,
) -> Option<crate::types::GovernanceProposal> {
    env.storage()
        .instance()
        .get(&DataKey::GovernanceProposal(proposal_id))
}

/// Set the governance token address (admin only).
pub fn set_governance_token(env: &Env, token_addr: Address) {
    env.storage()
        .instance()
        .set(&DataKey::GovernanceTokenAddress, &token_addr);
}

/// Get the governance token address.
pub fn get_governance_token(env: Env) -> Option<Address> {
    env.storage()
        .instance()
        .get(&DataKey::GovernanceTokenAddress)
}

// ── Task 4: Dispute Mechanism for Defaulted Loans ───────────────────────────

/// Dispute a slash/default decision.
/// The borrower can file a dispute within the dispute window after being slashed.
pub fn dispute_slash(
    env: Env,
    borrower: Address,
    evidence_hash: soroban_sdk::String,
) -> Result<u64, ContractError> {
    borrower.require_auth();
    require_not_paused(&env)?;
    // Task 1: Check granular pause for slash operations
    require_not_paused_for(&env, PauseFlag::Slash)?;

    // Validate evidence hash is not empty
    assert!(
        evidence_hash.len() > 0,
        "evidence hash cannot be empty"
    );

    // Get the borrower's latest loan to verify it was defaulted
    let loan = crate::helpers::get_latest_loan_record(&env, &borrower)
        .ok_or(ContractError::NoActiveLoan)?;

    // Loan must be defaulted to file a dispute
    if loan.status != crate::types::LoanStatus::Defaulted {
        return Err(ContractError::NoActiveLoan);
    }

    // Check dispute window
    let dispute_window: u64 = env
        .storage()
        .instance()
        .get(&DataKey::DisputeWindowSecs)
        .unwrap_or(DEFAULT_DISPUTE_WINDOW_SECS);

    // We need to track when the slash happened - for now, use the loan's repayment_timestamp
    // or created_at as a proxy. In a real implementation, we'd store the slash timestamp.
    let slash_timestamp = loan.repayment_timestamp.unwrap_or(loan.created_at);
    let now = env.ledger().timestamp();

    if now > slash_timestamp + dispute_window {
        return Err(ContractError::DisputeWindowExpired);
    }

    // Generate dispute ID
    let dispute_id: u64 = env
        .storage()
        .instance()
        .get(&DataKey::DisputeCounter)
        .unwrap_or(0u64)
        .checked_add(1)
        .expect("dispute ID overflow");

    let dispute = DisputeRecord {
        borrower: borrower.clone(),
        loan_id: loan.id,
        evidence_hash,
        disputed_at: now,
        resolved: false,
        resolved_at: None,
        resolution: None,
        voters: Vec::new(&env),
        approve_votes: 0,
        reject_votes: 0,
    };

    env.storage()
        .persistent()
        .set(&DataKey::Dispute(dispute_id), &dispute);
    extend_ttl(&env, &DataKey::Dispute(dispute_id));
    env.storage()
        .instance()
        .set(&DataKey::DisputeCounter, &dispute_id);

    env.events().publish(
        (symbol_short!("gov"), symbol_short!("disputed")),
        (dispute_id, borrower, loan.id),
    );

    Ok(dispute_id)
}

/// Vote on a dispute resolution.
/// Vouchers who previously staked for this borrower can vote on whether to uphold or reject the dispute.
pub fn vote_dispute(
    env: Env,
    voucher: Address,
    dispute_id: u64,
    approve: bool, // true = uphold dispute (reverse slash), false = reject dispute
) -> Result<(), ContractError> {
    voucher.require_auth();
    require_not_paused(&env)?;
    // Task 1: Check granular pause for slash operations
    require_not_paused_for(&env, PauseFlag::Slash)?;

    let mut dispute: DisputeRecord = env
        .storage()
        .persistent()
        .get(&DataKey::Dispute(dispute_id))
        .ok_or(ContractError::DisputeNotFound)?;

    if dispute.resolved {
        return Err(ContractError::DisputeAlreadyResolved);
    }

    // Get the vouches for this borrower to verify the voter was a voucher
    let vouches: Vec<VouchRecord> = env
        .storage()
        .persistent()
        .get(&DataKey::Vouches(dispute.borrower.clone()))
        .unwrap_or(Vec::new(&env));

    let voucher_stake = vouches
        .iter()
        .find(|v| v.voucher == voucher)
        .map(|v| v.amount)
        .ok_or(ContractError::VoucherNotFound)?;

    // Prevent double voting
    if dispute.voters.iter().any(|v| v == voucher) {
        return Err(ContractError::AlreadyVoted);
    }

    if approve {
        dispute.approve_votes += voucher_stake;
    } else {
        dispute.reject_votes += voucher_stake;
    }
    dispute.voters.push_back(voucher.clone());

    env.storage()
        .persistent()
        .set(&DataKey::Dispute(dispute_id), &dispute);
    extend_ttl(&env, &DataKey::Dispute(dispute_id));

    env.events().publish(
        (symbol_short!("gov"), symbol_short!("disp_vote")),
        (dispute_id, voucher, approve, voucher_stake),
    );

    Ok(())
}

/// Resolve a dispute - can be called by anyone after voting is complete.
/// If approved, the slash is reversed and the borrower is restored.
pub fn resolve_dispute(env: Env, dispute_id: u64) -> Result<(), ContractError> {
    require_not_paused(&env)?;
    // Task 1: Check granular pause for slash operations
    require_not_paused_for(&env, PauseFlag::Slash)?;

    let mut dispute: DisputeRecord = env
        .storage()
        .persistent()
        .get(&DataKey::Dispute(dispute_id))
        .ok_or(ContractError::DisputeNotFound)?;

    if dispute.resolved {
        return Err(ContractError::DisputeAlreadyResolved);
    }

    // Get total stake for quorum check
    let vouches: Vec<VouchRecord> = env
        .storage()
        .persistent()
        .get(&DataKey::Vouches(dispute.borrower.clone()))
        .unwrap_or(Vec::new(&env));
    let total_stake: i128 = vouches.iter().map(|v| v.amount).sum();

    // Simple majority: approve votes must exceed reject votes
    let resolution = if dispute.approve_votes > dispute.reject_votes && total_stake > 0 {
        DisputeResolution::Upheld
    } else {
        DisputeResolution::Rejected
    };

    dispute.resolved = true;
    dispute.resolved_at = Some(env.ledger().timestamp());
    dispute.resolution = Some(resolution.clone());

    env.storage()
        .persistent()
        .set(&DataKey::Dispute(dispute_id), &dispute);
    extend_ttl(&env, &DataKey::Dispute(dispute_id));

    // If dispute is upheld, reverse the slash by restoring the loan status
    // and returning the slashed funds (simplified - in production would need more complex logic)
    if let DisputeResolution::Upheld = resolution {
        // Restore the loan to active status (simplified)
        // In a full implementation, this would also restore the vouchers' stakes
        if let Some(mut loan) = env.storage().persistent().get::<DataKey, crate::types::LoanRecord>(
            &DataKey::Loan(dispute.loan_id),
        ) {
            loan.status = crate::types::LoanStatus::Active;
            env.storage()
                .persistent()
                .set(&DataKey::Loan(dispute.loan_id), &loan);
            extend_ttl(&env, &DataKey::Loan(dispute.loan_id));
            
            // Restore active loan mapping
            env.storage()
                .persistent()
                .set(&DataKey::ActiveLoan(dispute.borrower.clone()), &dispute.loan_id);
            extend_ttl(&env, &DataKey::ActiveLoan(dispute.borrower.clone()));
        }

        env.events().publish(
            (symbol_short!("gov"), symbol_short!("disp_up")),
            (dispute_id, dispute.borrower),
        );
    } else {
        env.events().publish(
            (symbol_short!("gov"), symbol_short!("disp_rej")),
            (dispute_id, dispute.borrower),
        );
    }

    Ok(())
}

/// Get a dispute record by ID.
pub fn get_dispute(env: Env, dispute_id: u64) -> Option<DisputeRecord> {
    env.storage()
        .persistent()
        .get(&DataKey::Dispute(dispute_id))
}

/// Set the dispute window (in seconds).
pub fn set_dispute_window(env: Env, admin_signers: Vec<Address>, window_secs: u64) {
    crate::helpers::require_admin_approval(&env, &admin_signers);
    env.storage()
        .instance()
        .set(&DataKey::DisputeWindowSecs, &window_secs);
    env.events().publish(
        (symbol_short!("admin"), symbol_short!("disp_win")),
        (admin_signers.get(0).unwrap(), window_secs),
    );
}

/// Get the current dispute window.
pub fn get_dispute_window(env: Env) -> u64 {
    env.storage()
        .instance()
        .get(&DataKey::DisputeWindowSecs)
        .unwrap_or(DEFAULT_DISPUTE_WINDOW_SECS)
}

/// Veto a governance proposal (#685)
pub fn veto_proposal(env: Env, proposal_id: u64) -> Result<(), ContractError> {
    let cfg = config(&env);
    let veto_admin = cfg.veto_admin.ok_or(ContractError::UnauthorizedCaller)?;
    veto_admin.require_auth();

    let mut proposal: GovernanceProposal = env
        .storage()
        .instance()
        .get(&DataKey::GovernanceProposal(proposal_id))
        .ok_or(ContractError::TimelockNotFound)?;

    if proposal.executed || proposal.vetoed {
        return Err(ContractError::InvalidStateTransition);
    }

    proposal.vetoed = true;
    env.storage()
        .instance()
        .set(&DataKey::GovernanceProposal(proposal_id), &proposal);

    env.events().publish(
        (symbol_short!("gov"), symbol_short!("vetoed")),
        (proposal_id, veto_admin),
    );

    Ok(())
}
