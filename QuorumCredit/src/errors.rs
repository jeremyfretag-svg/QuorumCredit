use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ContractError {
    InsufficientFunds = 1,
    /// Borrower already has an active (non-repaid, non-defaulted) loan.
    ActiveLoanExists = 2,
    /// Total vouched stake overflowed i128.
    StakeOverflow = 3,
    /// admin or token address must not be the zero address.
    ZeroAddress = 4,
    DuplicateVouch = 5,
    NoActiveLoan = 6,
    ContractPaused = 7,
    LoanPastDeadline = 8,
    PoolLengthMismatch = 9,
    PoolEmpty = 10,
    PoolBorrowerActiveLoan = 11,
    PoolInsufficientFunds = 12,
    MinStakeNotMet = 13,
    LoanExceedsMaxAmount = 14,
    InsufficientVouchers = 15,
    UnauthorizedCaller = 16,
    InvalidAmount = 17,
    InvalidStateTransition = 18,
    AlreadyInitialized = 19,
    VouchTooRecent = 20,
    VouchCooldownActive = 21,
    BorrowerHasActiveLoan = 22,
    VoucherNotWhitelisted = 23,
    Blacklisted = 24,
    TimelockNotFound = 25,
    TimelockNotReady = 26,
    TimelockExpired = 27,
    NoVouchesForBorrower = 28,
    VoucherNotFound = 29,
    /// Token address does not implement the SEP-41 token interface.
    InvalidToken = 30,
    AlreadyVoted = 31,
    SlashVoteNotFound = 32,
    SlashAlreadyExecuted = 33,
    QuorumNotMet = 34,
    AlreadyRepaid = 35,
    // #684: Admin Delegation
    PermissionNotDelegated = 36,
    // #685: Admin Veto Power
    ProposalVetoed = 37,
    /// Voucher and borrower must be different addresses.
    SelfVouchNotAllowed = 38,
    InvalidBps = 39,
    DuplicateToken = 40,
    // Task 1: Loan Cancellation
    LoanNotCancellable = 41,
    CancellationWindowExpired = 42,
    // Task 2: Large Loan Multi-Signature
    LoanTooLarge = 43,
    LargeLoanPendingApproval = 44,
    LargeLoanNotApproved = 45,
    LargeLoanDelayNotElapsed = 46,
    LargeLoanAlreadyExecuted = 47,
    // Task 3: Circular Vouch Detection
    CircularVouchDetected = 48,
    VouchDepthExceeded = 49,
    // Task 4: Loan Category
    InvalidLoanCategory = 50,
    // #642: Collateral Diversification
    SectorConcentrationTooHigh = 51,
    // #643: Loan Purpose Validation
    LoanPurposeNotAllowed = 52,
    // #645: Loan Restructuring
    RestructureRequestNotFound = 53,
    RestructureAlreadyPending = 54,
    // Dispute mechanism
    DisputeNotFound = 55,
    DisputeAlreadyResolved = 56,
    DisputeWindowExpired = 57,
    // Granular pause
    FunctionPaused = 58,
    // Admin config
    InvalidAdminThreshold = 59,
    // Voucher stake limit
    StakeLimitExceeded = 60,
}
