#[derive(Debug, thiserror::Error)]
pub enum RingBufferError {
    #[error("buffer is full")]
    WouldBlock,

    #[error("buffer is empty")]
    Empty,

    #[error("message too large: {size} > slot_size {slot_size}")]
    MessageTooLarge { size: usize, slot_size: usize },

    #[error("slot_count must be a power of 2")]
    InvalidSlotCount,

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
