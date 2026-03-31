use exg_common::{Decimal128, OrderId, Side, SymbolId, TradeId, UnixMicros, UserId};
use serde::{Deserialize, Serialize};

/// Rejection reasons for orders.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub enum RejectReason {
    InsufficientMargin,
    PositionLimitExceeded,
    PriceOutOfBand,
    SelfTradePrevented,
    RateLimitExceeded,
    PostOnlyWouldTake,
    FokNotFillable,
    SymbolSuspended,
    MarkPriceStale,
    InvalidOrder,
    DuplicateOrder,
    OrderNotFound,
}

/// Output messages from the matching engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub enum Event {
    OrderAccepted {
        order_id: OrderId,
        user_id: UserId,
        symbol: SymbolId,
        client_order_id: Option<u64>,
        timestamp: UnixMicros,
    },
    OrderRejected {
        order_id: OrderId,
        user_id: UserId,
        reason: RejectReason,
        timestamp: UnixMicros,
    },
    OrderCanceled {
        order_id: OrderId,
        user_id: UserId,
        symbol: SymbolId,
        remaining_qty: Decimal128,
        timestamp: UnixMicros,
    },
    OrderFilled {
        order_id: OrderId,
        trade_id: TradeId,
        user_id: UserId,
        symbol: SymbolId,
        side: Side,
        fill_price: Decimal128,
        fill_qty: Decimal128,
        is_maker: bool,
        remaining_qty: Decimal128,
        timestamp: UnixMicros,
    },
    TradeExecuted {
        trade_id: TradeId,
        symbol: SymbolId,
        price: Decimal128,
        qty: Decimal128,
        buyer_order_id: OrderId,
        seller_order_id: OrderId,
        buyer_user_id: UserId,
        seller_user_id: UserId,
        buyer_fee: Decimal128,
        seller_fee: Decimal128,
        timestamp: UnixMicros,
    },
    MarkPriceUpdate {
        symbol: SymbolId,
        mark_price: Decimal128,
        index_price: Decimal128,
        timestamp: UnixMicros,
    },
    FundingRateUpdate {
        symbol: SymbolId,
        funding_rate: Decimal128,
        timestamp: UnixMicros,
    },
    LiquidationOrder {
        user_id: UserId,
        symbol: SymbolId,
        side: Side,
        quantity: Decimal128,
        timestamp: UnixMicros,
    },
}
