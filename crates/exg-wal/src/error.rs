#[derive(Debug, thiserror::Error)]
pub enum WalError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("corrupt record at sequence {sequence}: {reason}")]
    Corrupt { sequence: u64, reason: String },
    #[error("sequence gap: expected {expected}, found {found}")]
    SequenceGap { expected: u64, found: u64 },
    #[error("payload too large: {size} bytes")]
    PayloadTooLarge { size: usize },
}
