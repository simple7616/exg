pub mod decimal;
pub mod error;
pub mod ids;
pub mod types;

// Re-exports for convenience.
pub use decimal::Decimal128;
pub use error::{ExgError, ExgResult};
pub use ids::{AccountId, OrderId, SnowflakeGen, SymbolId, TradeId, UnixMicros, UserId};
pub use types::{
    MarginMode, OrderStatus, OrderType, PositionSide, Side, SymbolStatus, SymbolType, TimeInForce,
};
