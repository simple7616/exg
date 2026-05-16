use exg_common::{
    Decimal128, MarginMode, OrderId, OrderType, Side, SymbolId, TimeInForce, UnixMicros, UserId,
};
use serde::{Deserialize, Serialize};

/// Input messages to the matching engine.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
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
    /// Stage 2: admin-injected mark/index price. Drives stop/trailing
    /// triggering + funding premium. Produced by the admin HTTP server.
    UpdateMarkPrice {
        symbol: SymbolId,
        mark_price: Decimal128,
        index_price: Decimal128,
        timestamp: UnixMicros,
    },
    /// Stage 2: admin-triggered funding rate computation.
    ComputeFunding {
        symbol: SymbolId,
        timestamp: UnixMicros,
    },
    /// Stage 3: admin-injected balance credit (bootstraps wallets for
    /// settlement; produced by the admin HTTP server). Clearing-domain —
    /// the matching engine ignores it.
    AdminCredit {
        user_id: UserId,
        amount: Decimal128,
        idempotency_key: String,
        timestamp: UnixMicros,
    },
}
