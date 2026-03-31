use crate::withdrawal::WithdrawalStatus;

#[derive(Debug, thiserror::Error)]
pub enum WalletError {
    #[error("withdrawal not found: {0}")]
    WithdrawalNotFound(u64),
    #[error("invalid status transition: {from:?} -> {to:?}")]
    InvalidStatusTransition {
        from: WithdrawalStatus,
        to: WithdrawalStatus,
    },
    #[error("insufficient balance")]
    InsufficientBalance,
    #[error("address not found")]
    AddressNotFound,
    #[error("duplicate deposit: {tx_hash}")]
    DuplicateDeposit { tx_hash: String },
    #[error("internal error: {0}")]
    Internal(String),
}
