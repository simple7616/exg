pub mod adl;
pub mod funding;
pub mod margin;
pub mod pre_trade;

use exg_common::{
    Decimal128, MarginMode, OrderId, OrderType, PositionSide, Side, SymbolId, UserId,
};

// ── Data Structures ────────────────────────────────────────────────────

/// Symbol configuration for risk calculations.
pub struct SymbolConfig {
    pub symbol: SymbolId,
    pub tick_size: Decimal128,
    pub lot_size: Decimal128,
    pub min_notional: Decimal128,
    pub max_leverage: Decimal128,
    pub maker_fee: Decimal128,
    pub taker_fee: Decimal128,
    pub margin_tiers: Vec<MarginTier>,
}

pub struct MarginTier {
    pub notional_floor: Decimal128,
    pub notional_cap: Decimal128,
    pub maintenance_margin_rate: Decimal128,
    /// Cumulative adjustment so that maintenance margin =
    /// notional * rate - maintenance_amount.
    pub maintenance_amount: Decimal128,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Position {
    pub user_id: UserId,
    pub symbol: SymbolId,
    pub side: PositionSide,
    /// Always positive.
    pub size: Decimal128,
    pub entry_price: Decimal128,
    pub leverage: Decimal128,
    pub margin: Decimal128,
    pub unrealized_pnl: Decimal128,
    pub accumulated_funding: Decimal128,
    pub margin_mode: MarginMode,
}

pub struct Account {
    pub user_id: UserId,
    pub wallet_balance: Decimal128,
    pub available_balance: Decimal128,
    pub frozen_balance: Decimal128,
}

pub struct OrderInfo {
    pub order_id: OrderId,
    pub user_id: UserId,
    pub symbol: SymbolId,
    pub side: Side,
    pub price: Decimal128,
    pub quantity: Decimal128,
    pub order_type: OrderType,
}

pub struct RateLimitState {
    pub orders_in_window: u32,
    pub cancels_in_window: u32,
}

pub struct RateLimitConfig {
    pub max_orders_per_second: u32,
    pub max_cancels_per_second: u32,
}
