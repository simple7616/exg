use thiserror::Error;

use crate::ids::{OrderId, SymbolId, UserId};

/// Unified error type for the exchange engine.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ExgError {
    // ── Order errors ────────────────────────────────────────────────────
    #[error("order not found: {0}")]
    OrderNotFound(OrderId),

    #[error("duplicate order: {0}")]
    DuplicateOrder(OrderId),

    #[error("order already canceled: {0}")]
    OrderAlreadyCanceled(OrderId),

    #[error("order already filled: {0}")]
    OrderAlreadyFilled(OrderId),

    #[error("invalid order: {0}")]
    InvalidOrder(String),

    #[error("post-only order would take liquidity")]
    PostOnlyWouldTake,

    #[error("FOK order cannot be fully filled")]
    FokNotFillable,

    #[error("self-trade prevented for user {0}")]
    SelfTradePrevented(UserId),

    // ── Risk / margin errors ────────────────────────────────────────────
    #[error("insufficient margin: required {required}, available {available}")]
    InsufficientMargin { required: String, available: String },

    #[error("position limit exceeded for symbol {0}")]
    PositionLimitExceeded(SymbolId),

    #[error("price out of band: order_price={order_price}, mark_price={mark_price}")]
    PriceOutOfBand {
        order_price: String,
        mark_price: String,
    },

    #[error("leverage out of range: {0}")]
    InvalidLeverage(String),

    #[error("rate limit exceeded for user {0}")]
    RateLimitExceeded(UserId),

    // ── Account / ledger errors ─────────────────────────────────────────
    #[error("insufficient balance: required {required}, available {available}")]
    InsufficientBalance { required: String, available: String },

    #[error("account not found: user {0}")]
    AccountNotFound(UserId),

    #[error("duplicate journal entry: idempotency_key={0}")]
    DuplicateJournalEntry(String),

    #[error("balance invariant violated: {0}")]
    BalanceInvariantViolation(String),

    // ── Symbol / market errors ──────────────────────────────────────────
    #[error("symbol not found: {0}")]
    SymbolNotFound(SymbolId),

    #[error("symbol suspended: {0}")]
    SymbolSuspended(SymbolId),

    #[error("invalid quantity: {0}")]
    InvalidQuantity(String),

    #[error("notional too small: {0}")]
    NotionalTooSmall(String),

    // ── System errors ───────────────────────────────────────────────────
    #[error("internal error: {0}")]
    Internal(String),

    #[error("WAL error: {0}")]
    Wal(String),

    #[error("serialization error: {0}")]
    Serialization(String),

    #[error("mark price stale for symbol {0}")]
    MarkPriceStale(SymbolId),

    // ── Liquidation / ADL ─────────────────���─────────────────────────────
    #[error("liquidation triggered for user {user} on symbol {symbol}")]
    LiquidationTriggered { user: UserId, symbol: SymbolId },

    #[error("ADL triggered for user {user} on symbol {symbol}")]
    AdlTriggered { user: UserId, symbol: SymbolId },

    #[error("insurance fund depleted")]
    InsuranceFundDepleted,
}

/// Convenience type alias.
pub type ExgResult<T> = Result<T, ExgError>;
