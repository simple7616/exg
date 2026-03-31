use exg_common::{
    Decimal128, MarginMode, OrderId, OrderStatus, OrderType, Side, SymbolId, TimeInForce, TradeId,
    UnixMicros, UserId,
};
use serde::{Deserialize, Serialize};

/// Full order record tracking lifecycle from creation to terminal state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Order {
    pub order_id: OrderId,
    pub user_id: UserId,
    pub symbol: SymbolId,
    pub client_order_id: Option<u64>,
    pub side: Side,
    pub order_type: OrderType,
    pub time_in_force: TimeInForce,
    pub price: Option<Decimal128>,
    pub stop_price: Option<Decimal128>,
    pub original_qty: Decimal128,
    pub executed_qty: Decimal128,
    pub remaining_qty: Decimal128,
    pub status: OrderStatus,
    pub margin_mode: MarginMode,
    pub leverage: Option<Decimal128>,
    pub reduce_only: bool,
    /// Volume-weighted average price of all fills.
    pub avg_fill_price: Decimal128,
    /// Cumulative sum of (fill_price * fill_qty) across all fills.
    pub total_filled_quote: Decimal128,
    pub commission: Decimal128,
    pub created_at: UnixMicros,
    pub updated_at: UnixMicros,
    pub fills: Vec<FillRecord>,
}

/// Record of a single fill (partial or full execution).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FillRecord {
    pub trade_id: TradeId,
    pub price: Decimal128,
    pub qty: Decimal128,
    pub is_maker: bool,
    pub commission: Decimal128,
    pub timestamp: UnixMicros,
}
