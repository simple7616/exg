use serde::{Deserialize, Serialize};

// ── Side ────────────────────────────────────────────────────────────────

/// Order / position side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[rkyv(derive(Debug))]
pub enum Side {
    Buy,
    Sell,
}

impl Side {
    /// Return the opposite side.
    #[inline]
    pub const fn opposite(self) -> Self {
        match self {
            Self::Buy => Self::Sell,
            Self::Sell => Self::Buy,
        }
    }

    /// Sign multiplier: Buy = +1, Sell = -1.
    #[inline]
    pub const fn sign(self) -> i8 {
        match self {
            Self::Buy => 1,
            Self::Sell => -1,
        }
    }
}

// ── OrderType ───────────────────────────────────────────────────────────

/// Supported order types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[rkyv(derive(Debug))]
pub enum OrderType {
    /// Standard limit order.
    Limit,
    /// Market order — fills at best available price.
    Market,
    /// Stop-loss market: triggers at stop_price, executes as market.
    StopMarket,
    /// Stop-loss limit: triggers at stop_price, places limit at price.
    StopLimit,
    /// Take-profit market.
    TakeProfitMarket,
    /// Take-profit limit.
    TakeProfitLimit,
    /// Trailing stop: tracks peak price with a trailing delta.
    TrailingStop,
    /// Iceberg: large order split into visible slices.
    Iceberg,
}

impl OrderType {
    /// Whether this order type requires a trigger (stop/mark) price.
    #[inline]
    pub const fn is_conditional(self) -> bool {
        matches!(
            self,
            Self::StopMarket
                | Self::StopLimit
                | Self::TakeProfitMarket
                | Self::TakeProfitLimit
                | Self::TrailingStop
        )
    }

    /// Whether this order rests on the book when not immediately matched.
    #[inline]
    pub const fn is_limit(self) -> bool {
        matches!(
            self,
            Self::Limit | Self::StopLimit | Self::TakeProfitLimit | Self::Iceberg
        )
    }
}

// ── OrderStatus ─────────────────────────────────────────────────────────

/// Order lifecycle status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[rkyv(derive(Debug))]
pub enum OrderStatus {
    /// Accepted but not yet matched.
    New,
    /// Partially filled, remaining quantity still on book.
    PartiallyFilled,
    /// Fully filled.
    Filled,
    /// Canceled by user (may have partial fills).
    Canceled,
    /// Rejected at submission (risk check, validation, etc.).
    Rejected,
    /// Expired (GTD timeout or session end).
    Expired,
    /// Pending trigger (conditional orders waiting for stop/mark price).
    PendingTrigger,
}

impl OrderStatus {
    /// Whether the order is in a terminal state (no further transitions).
    #[inline]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Filled | Self::Canceled | Self::Rejected | Self::Expired
        )
    }

    /// Whether the order is active on the book or awaiting trigger.
    #[inline]
    pub const fn is_active(self) -> bool {
        matches!(self, Self::New | Self::PartiallyFilled | Self::PendingTrigger)
    }
}

// ── TimeInForce ─────────────────────────────────────────────────────────

/// Time-in-force policies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[rkyv(derive(Debug))]
pub enum TimeInForce {
    /// Good-Till-Canceled: remains until filled or explicitly canceled.
    Gtc,
    /// Immediate-Or-Cancel: fill what you can, cancel the rest.
    Ioc,
    /// Fill-Or-Kill: fill entirely or reject entirely.
    Fok,
    /// Good-Till-Date: auto-cancel at a specified timestamp.
    Gtd,
    /// Post-Only: rejected if it would take liquidity (maker-only).
    PostOnly,
}

// ── MarginMode ──────────────────────────────────────────────────────────

/// Margin mode for a position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[rkyv(derive(Debug))]
pub enum MarginMode {
    /// All positions share the same margin pool.
    Cross,
    /// Each position has its own isolated margin.
    Isolated,
}

// ── PositionSide ────────────────────────────────────────────────────────

/// Position direction in hedge mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[rkyv(derive(Debug))]
pub enum PositionSide {
    /// Long position (ADL side = +1).
    Long,
    /// Short position (ADL side = -1).
    Short,
    /// One-way mode — side determined by net position.
    Both,
}

impl PositionSide {
    /// ADL side value: Long = +1, Short = -1, Both = 0.
    #[inline]
    pub const fn adl_side(self) -> i8 {
        match self {
            Self::Long => 1,
            Self::Short => -1,
            Self::Both => 0,
        }
    }
}

// ── SymbolType ──────────────────────────────────────────────────────────

/// Type of trading pair / contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[rkyv(derive(Debug))]
pub enum SymbolType {
    /// Spot trading pair (e.g. BTC/USDT).
    Spot,
    /// Linear perpetual contract settled in quote currency (e.g. USDT-margined).
    PerpetualLinear,
    /// Inverse perpetual contract settled in base currency (e.g. coin-margined).
    PerpetualInverse,
}

// ── SymbolStatus ────────────────────────────────────────────────────────

/// Trading status of a symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[rkyv(derive(Debug))]
pub enum SymbolStatus {
    /// Normal trading.
    Trading,
    /// Pre-launch / auction phase.
    PreTrading,
    /// Halted — no new orders accepted, existing orders remain.
    Halted,
    /// Settling — only close/reduce orders accepted.
    Settling,
    /// Delisted — all positions force-closed.
    Delisted,
}

impl SymbolStatus {
    /// Whether new orders can be placed.
    #[inline]
    pub const fn accepts_new_orders(self) -> bool {
        matches!(self, Self::Trading)
    }

    /// Whether reduce-only orders are accepted.
    #[inline]
    pub const fn accepts_reduce_only(self) -> bool {
        matches!(self, Self::Trading | Self::Settling)
    }
}

// ══════════════════════════════════════════════════════════════════════════
// Tests
// ══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_side_opposite() {
        assert_eq!(Side::Buy.opposite(), Side::Sell);
        assert_eq!(Side::Sell.opposite(), Side::Buy);
    }

    #[test]
    fn test_side_sign() {
        assert_eq!(Side::Buy.sign(), 1);
        assert_eq!(Side::Sell.sign(), -1);
    }

    #[test]
    fn test_side_serde() {
        let json = serde_json::to_string(&Side::Buy).unwrap();
        assert_eq!(json, "\"BUY\"");
        let parsed: Side = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, Side::Buy);
    }

    #[test]
    fn test_order_type_conditional() {
        assert!(!OrderType::Limit.is_conditional());
        assert!(!OrderType::Market.is_conditional());
        assert!(OrderType::StopMarket.is_conditional());
        assert!(OrderType::TrailingStop.is_conditional());
    }

    #[test]
    fn test_order_type_is_limit() {
        assert!(OrderType::Limit.is_limit());
        assert!(OrderType::StopLimit.is_limit());
        assert!(OrderType::Iceberg.is_limit());
        assert!(!OrderType::Market.is_limit());
        assert!(!OrderType::StopMarket.is_limit());
    }

    #[test]
    fn test_order_status_terminal() {
        assert!(OrderStatus::Filled.is_terminal());
        assert!(OrderStatus::Canceled.is_terminal());
        assert!(OrderStatus::Rejected.is_terminal());
        assert!(OrderStatus::Expired.is_terminal());
        assert!(!OrderStatus::New.is_terminal());
        assert!(!OrderStatus::PartiallyFilled.is_terminal());
        assert!(!OrderStatus::PendingTrigger.is_terminal());
    }

    #[test]
    fn test_order_status_active() {
        assert!(OrderStatus::New.is_active());
        assert!(OrderStatus::PartiallyFilled.is_active());
        assert!(OrderStatus::PendingTrigger.is_active());
        assert!(!OrderStatus::Filled.is_active());
    }

    #[test]
    fn test_position_side_adl() {
        assert_eq!(PositionSide::Long.adl_side(), 1);
        assert_eq!(PositionSide::Short.adl_side(), -1);
        assert_eq!(PositionSide::Both.adl_side(), 0);
    }

    #[test]
    fn test_symbol_status_accepts_orders() {
        assert!(SymbolStatus::Trading.accepts_new_orders());
        assert!(!SymbolStatus::Halted.accepts_new_orders());
        assert!(!SymbolStatus::Settling.accepts_new_orders());
        assert!(SymbolStatus::Settling.accepts_reduce_only());
    }

    #[test]
    fn test_margin_mode_serde() {
        let json = serde_json::to_string(&MarginMode::Cross).unwrap();
        assert_eq!(json, "\"CROSS\"");
        let parsed: MarginMode = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, MarginMode::Cross);
    }

    #[test]
    fn test_time_in_force_serde() {
        let json = serde_json::to_string(&TimeInForce::Gtc).unwrap();
        assert_eq!(json, "\"GTC\"");
    }

    #[test]
    fn test_symbol_type_serde() {
        let json = serde_json::to_string(&SymbolType::PerpetualLinear).unwrap();
        assert_eq!(json, "\"PERPETUAL_LINEAR\"");
    }
}
