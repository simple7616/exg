use std::collections::VecDeque;

use exg_common::{Decimal128, Side, SymbolId, TradeId, UnixMicros};
use serde::{Deserialize, Serialize};

// ── RecentTrade ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentTrade {
    pub trade_id: TradeId,
    pub symbol: SymbolId,
    pub price: Decimal128,
    pub qty: Decimal128,
    /// Taker side.
    pub side: Side,
    pub timestamp: UnixMicros,
}

// ── RecentTrades ───────────────────────────────────────────────────────

/// Bounded FIFO buffer of recent trades.
pub struct RecentTrades {
    symbol: SymbolId,
    trades: VecDeque<RecentTrade>,
    max_trades: usize,
}

impl RecentTrades {
    pub fn new(symbol: SymbolId, max_trades: usize) -> Self {
        Self {
            symbol,
            trades: VecDeque::with_capacity(max_trades.min(1024)),
            max_trades,
        }
    }

    /// Add a trade, evicting the oldest if at capacity.
    pub fn add_trade(&mut self, trade: RecentTrade) {
        if self.trades.len() >= self.max_trades {
            self.trades.pop_front();
        }
        self.trades.push_back(trade);
    }

    /// Get recent trades (newest last), up to `limit`.
    pub fn recent(&self, limit: usize) -> Vec<&RecentTrade> {
        let skip = self.trades.len().saturating_sub(limit);
        self.trades.iter().skip(skip).collect()
    }

    pub fn len(&self) -> usize {
        self.trades.len()
    }

    pub fn is_empty(&self) -> bool {
        self.trades.is_empty()
    }

    #[allow(dead_code)]
    pub fn symbol(&self) -> SymbolId {
        self.symbol
    }
}

// ═══════════════════════════════════��════════════════════════════════════
// Tests
// ════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    fn dec(s: &str) -> Decimal128 {
        s.parse().unwrap()
    }

    fn sym() -> SymbolId {
        SymbolId::new(1)
    }

    fn make_trade(id: u64, price: &str) -> RecentTrade {
        RecentTrade {
            trade_id: TradeId::new(id),
            symbol: sym(),
            price: dec(price),
            qty: dec("1"),
            side: Side::Buy,
            timestamp: UnixMicros::from_micros(id * 1_000_000),
        }
    }

    #[test]
    fn test_fifo_bounded_buffer() {
        let mut rt = RecentTrades::new(sym(), 3);

        rt.add_trade(make_trade(1, "100"));
        rt.add_trade(make_trade(2, "101"));
        rt.add_trade(make_trade(3, "102"));
        assert_eq!(rt.len(), 3);

        // Adding a 4th should evict the oldest.
        rt.add_trade(make_trade(4, "103"));
        assert_eq!(rt.len(), 3);

        let recent = rt.recent(10);
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[0].trade_id, TradeId::new(2));
        assert_eq!(recent[1].trade_id, TradeId::new(3));
        assert_eq!(recent[2].trade_id, TradeId::new(4));
    }

    #[test]
    fn test_oldest_trades_evicted() {
        let mut rt = RecentTrades::new(sym(), 2);

        rt.add_trade(make_trade(1, "100"));
        rt.add_trade(make_trade(2, "200"));
        rt.add_trade(make_trade(3, "300"));
        rt.add_trade(make_trade(4, "400"));

        assert_eq!(rt.len(), 2);
        let recent = rt.recent(10);
        assert_eq!(recent[0].trade_id, TradeId::new(3));
        assert_eq!(recent[1].trade_id, TradeId::new(4));
    }

    #[test]
    fn test_recent_limit() {
        let mut rt = RecentTrades::new(sym(), 100);
        for i in 1..=10 {
            rt.add_trade(make_trade(i, "100"));
        }

        let recent = rt.recent(3);
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[0].trade_id, TradeId::new(8));
        assert_eq!(recent[1].trade_id, TradeId::new(9));
        assert_eq!(recent[2].trade_id, TradeId::new(10));
    }

    #[test]
    fn test_empty() {
        let rt = RecentTrades::new(sym(), 10);
        assert!(rt.is_empty());
        assert_eq!(rt.len(), 0);
        assert!(rt.recent(5).is_empty());
    }
}
