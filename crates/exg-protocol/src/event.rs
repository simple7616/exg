use exg_common::{
    Decimal128, OrderId, OrderType, Side, SymbolId, TimeInForce, TradeId, UnixMicros, UserId,
};
use serde::{Deserialize, Serialize};

/// Rejection reasons for orders.
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
pub enum Event {
    OrderAccepted {
        order_id: OrderId,
        user_id: UserId,
        symbol: SymbolId,
        client_order_id: Option<u64>,
        timestamp: UnixMicros,
        // Stage 1b: fields needed to rebuild a BookOrder during WAL replay.
        side: Side,
        order_type: OrderType,
        time_in_force: TimeInForce,
        /// Effective price recorded at accept time. For limit-like orders this is the
        /// submitted price; for market-like orders it is the sentinel (Decimal128::MAX for
        /// buy, Decimal128::ZERO for sell — same as `BookOrder.price`).
        price: Decimal128,
        /// Original submitted quantity.
        quantity: Decimal128,
        stop_price: Option<Decimal128>,
        reduce_only: bool,
        /// Iceberg visible slice size. `None` for non-iceberg orders.
        visible_quantity: Option<Decimal128>,
        /// Trailing-stop offset. `None` for non-trailing orders.
        trailing_delta: Option<Decimal128>,
        /// Trailing-stop reference price at accept time (= engine.mark_price at the time).
        /// `None` for non-trailing orders. Cannot be reconstructed at replay time because
        /// mark_price may have moved.
        trailing_peak_price: Option<Decimal128>,
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
    /// Stage 3: admin balance credit applied to the ledger (fact).
    /// Carries `idempotency_key` so replay re-applies with the exact
    /// original key (self-describing fact — like RealizedPnl/FundingSettled).
    AdminCredited {
        user_id: UserId,
        amount: Decimal128,
        idempotency_key: String,
        timestamp: UnixMicros,
    },
    /// Stage 3: realized PnL on a position reduce/close (fact).
    /// `amount` is signed: positive = profit (credit), negative = loss.
    RealizedPnl {
        user_id: UserId,
        symbol: SymbolId,
        amount: Decimal128,
        timestamp: UnixMicros,
    },
    /// Stage 3: funding payment settled for one position (fact).
    /// `amount` is signed: positive = user paid, negative = user received.
    FundingSettled {
        user_id: UserId,
        symbol: SymbolId,
        funding_period_id: u64,
        amount: Decimal128,
        timestamp: UnixMicros,
    },
}
