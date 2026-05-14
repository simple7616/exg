use exg_common::{Decimal128, Side, SymbolId, TradeId, UnixMicros};

use crate::depth::DepthTracker;
use crate::kline::{Kline, KlineAggregator};
use crate::recent_trades::{RecentTrade, RecentTrades};
use crate::ticker::TickerAggregator;

/// Default max completed klines kept in memory per symbol.
const DEFAULT_MAX_COMPLETED_KLINES: usize = 1000;
/// Default max recent trades per symbol.
const DEFAULT_MAX_RECENT_TRADES: usize = 500;

/// Market data service combining all aggregators for a single symbol.
pub struct MarketDataService {
    pub klines: KlineAggregator,
    pub ticker: TickerAggregator,
    pub depth: DepthTracker,
    pub recent_trades: RecentTrades,
}

impl MarketDataService {
    pub fn new(symbol: SymbolId) -> Self {
        Self {
            klines: KlineAggregator::new(symbol, DEFAULT_MAX_COMPLETED_KLINES),
            ticker: TickerAggregator::new(symbol),
            depth: DepthTracker::new(symbol),
            recent_trades: RecentTrades::new(symbol, DEFAULT_MAX_RECENT_TRADES),
        }
    }

    /// Process a trade event. Returns completed klines (if any interval rolled over).
    pub fn on_trade(
        &mut self,
        trade_id: TradeId,
        price: Decimal128,
        qty: Decimal128,
        taker_side: Side,
        timestamp: UnixMicros,
    ) -> Vec<Kline> {
        // Update all aggregators.
        let completed = self.klines.add_trade(price, qty, timestamp);
        self.ticker.add_trade(price, qty, timestamp);
        self.recent_trades.add_trade(RecentTrade {
            trade_id,
            symbol: self.ticker.ticker().symbol,
            price,
            qty,
            side: taker_side,
            timestamp,
        });

        // Update BBO from depth (best bid/ask).
        let snap = self.depth.snapshot(1);
        let best_bid = snap.bids.first().map(|&(p, _)| p);
        let best_ask = snap.asks.first().map(|&(p, _)| p);
        self.ticker.update_bbo(best_bid, best_ask);

        completed
    }

    /// Process depth updates.
    pub fn on_depth_update(&mut self, updates: &[(Side, Decimal128, Decimal128)]) {
        self.depth.update_levels(updates);

        // Refresh BBO on the ticker.
        let snap = self.depth.snapshot(1);
        let best_bid = snap.bids.first().map(|&(p, _)| p);
        let best_ask = snap.asks.first().map(|&(p, _)| p);
        self.ticker.update_bbo(best_bid, best_ask);
    }

    /// Update mark/index prices.
    pub fn on_mark_price(&mut self, mark: Decimal128, index: Decimal128) {
        self.ticker.update_prices(mark, index);
    }
}

// ════════════════════════════════════════════════════════════════════════
// Tests
// ════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kline::KlineInterval;

    fn dec(s: &str) -> Decimal128 {
        s.parse().unwrap()
    }

    fn sym() -> SymbolId {
        SymbolId::new(1)
    }

    #[test]
    fn test_on_trade_updates_all_aggregators() {
        let mut svc = MarketDataService::new(sym());

        let ts = UnixMicros::from_micros(1_000_000_000_000);
        let completed = svc.on_trade(TradeId::new(1), dec("50000"), dec("1.5"), Side::Buy, ts);
        assert!(completed.is_empty()); // first trade, no rollover

        // Kline updated.
        let kline = svc.klines.current_kline(KlineInterval::M1).unwrap();
        assert_eq!(kline.open, dec("50000"));
        assert_eq!(kline.volume, dec("1.5"));

        // Ticker updated.
        assert_eq!(svc.ticker.ticker().last_price, dec("50000"));
        assert_eq!(svc.ticker.ticker().trade_count_24h, 1);

        // Recent trades updated.
        assert_eq!(svc.recent_trades.len(), 1);
        let recent = svc.recent_trades.recent(10);
        assert_eq!(recent[0].trade_id, TradeId::new(1));
        assert_eq!(recent[0].side, Side::Buy);
    }

    #[test]
    fn test_on_depth_update_refreshes_bbo() {
        let mut svc = MarketDataService::new(sym());

        svc.on_depth_update(&[
            (Side::Buy, dec("49999"), dec("10")),
            (Side::Sell, dec("50001"), dec("5")),
        ]);

        let t = svc.ticker.ticker();
        assert_eq!(t.best_bid, Some(dec("49999")));
        assert_eq!(t.best_ask, Some(dec("50001")));
    }

    #[test]
    fn test_on_mark_price() {
        let mut svc = MarketDataService::new(sym());
        svc.on_mark_price(dec("50001.23"), dec("50000.89"));

        let t = svc.ticker.ticker();
        assert_eq!(t.mark_price, dec("50001.23"));
        assert_eq!(t.index_price, dec("50000.89"));
    }

    #[test]
    fn test_full_flow() {
        let mut svc = MarketDataService::new(sym());

        // Set up depth.
        svc.on_depth_update(&[
            (Side::Buy, dec("49990"), dec("100")),
            (Side::Sell, dec("50010"), dec("50")),
        ]);

        // Set mark/index prices.
        svc.on_mark_price(dec("50000"), dec("49999"));

        // Process trades.
        let ts_base = 1_000_000_000_000u64;
        svc.on_trade(
            TradeId::new(1),
            dec("50000"),
            dec("1"),
            Side::Buy,
            UnixMicros::from_micros(ts_base),
        );
        svc.on_trade(
            TradeId::new(2),
            dec("50100"),
            dec("2"),
            Side::Sell,
            UnixMicros::from_micros(ts_base + 500_000),
        );

        // Verify state.
        let t = svc.ticker.ticker();
        assert_eq!(t.last_price, dec("50100"));
        assert_eq!(t.mark_price, dec("50000"));
        assert_eq!(t.best_bid, Some(dec("49990")));
        assert_eq!(t.best_ask, Some(dec("50010")));
        assert_eq!(t.trade_count_24h, 2);

        assert_eq!(svc.recent_trades.len(), 2);

        let snap = svc.depth.snapshot(10);
        assert_eq!(snap.bids.len(), 1);
        assert_eq!(snap.asks.len(), 1);
    }
}
