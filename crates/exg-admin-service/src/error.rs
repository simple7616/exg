use exg_common::{SymbolId, UserId};

#[derive(Debug, thiserror::Error)]
pub enum AdminError {
    #[error("symbol not found: {0}")]
    SymbolNotFound(SymbolId),
    #[error("duplicate symbol: {0}")]
    DuplicateSymbol(String),
    #[error("user not found: {0}")]
    UserNotFound(UserId),
    #[error("invalid operation: {0}")]
    InvalidOperation(String),
}
