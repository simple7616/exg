use exg_common::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskSnapshot {
    pub timestamp: UnixMicros,
    pub total_open_interest: Decimal128,
    pub total_long_notional: Decimal128,
    pub total_short_notional: Decimal128,
    pub insurance_fund_balance: Decimal128,
    pub liquidations_24h: u64,
    pub adl_events_24h: u64,
    pub users_near_liquidation: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PositionSummary {
    pub user_id: UserId,
    pub symbol: SymbolId,
    pub side: String,
    pub size: Decimal128,
    pub entry_price: Decimal128,
    pub mark_price: Decimal128,
    pub unrealized_pnl: Decimal128,
    pub margin_ratio: Decimal128,
}

pub struct RiskMonitor {
    liquidation_count_24h: u64,
    adl_count_24h: u64,
    /// Alert threshold for margin ratio (e.g. 0.8 = 80%).
    margin_ratio_threshold: Decimal128,
}

impl RiskMonitor {
    pub fn new(margin_ratio_threshold: Decimal128) -> Self {
        Self {
            liquidation_count_24h: 0,
            adl_count_24h: 0,
            margin_ratio_threshold,
        }
    }

    /// Record a liquidation event.
    pub fn record_liquidation(&mut self) {
        self.liquidation_count_24h += 1;
    }

    /// Record an ADL event.
    pub fn record_adl(&mut self) {
        self.adl_count_24h += 1;
    }

    /// Get the 24h liquidation count.
    pub fn get_liquidation_count(&self) -> u64 {
        self.liquidation_count_24h
    }

    /// Get the 24h ADL count.
    pub fn get_adl_count(&self) -> u64 {
        self.adl_count_24h
    }

    /// Check if a position's margin ratio exceeds the alert threshold.
    pub fn is_near_liquidation(&self, margin_ratio: Decimal128) -> bool {
        margin_ratio >= self.margin_ratio_threshold
    }

    /// Reset the 24h rolling counters.
    pub fn reset_24h_counters(&mut self) {
        self.liquidation_count_24h = 0;
        self.adl_count_24h = 0;
    }
}
