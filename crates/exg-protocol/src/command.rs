use exg_common::{
    Decimal128, MarginMode, OrderId, OrderType, Side, SymbolId, TimeInForce, UnixMicros, UserId,
};
use serde::{Deserialize, Serialize};

/// Input messages to the matching engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub enum Command {
    NewOrder {
        order_id: OrderId,
        user_id: UserId,
        symbol: SymbolId,
        side: Side,
        order_type: OrderType,
        time_in_force: TimeInForce,
        /// `None` for market orders.
        price: Option<Decimal128>,
        quantity: Decimal128,
        /// For conditional orders (stop-loss, take-profit).
        stop_price: Option<Decimal128>,
        /// For trailing stop orders.
        trailing_delta: Option<Decimal128>,
        /// For iceberg orders — the visible slice size.
        visible_quantity: Option<Decimal128>,
        reduce_only: bool,
        margin_mode: MarginMode,
        leverage: Option<Decimal128>,
        client_order_id: Option<u64>,
        timestamp: UnixMicros,
    },
    CancelOrder {
        order_id: OrderId,
        user_id: UserId,
        symbol: SymbolId,
        timestamp: UnixMicros,
    },
    AmendOrder {
        order_id: OrderId,
        user_id: UserId,
        symbol: SymbolId,
        new_price: Option<Decimal128>,
        new_quantity: Option<Decimal128>,
        timestamp: UnixMicros,
    },
    CancelAllOrders {
        user_id: UserId,
        symbol: SymbolId,
        timestamp: UnixMicros,
    },
}
