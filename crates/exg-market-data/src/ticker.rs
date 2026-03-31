use std::collections::VecDeque;

use exg_common::{Decimal128, SymbolId, UnixMicros};
use serde::{Deserialize, Serialize};

/// 24 hours in microseconds.
const WINDOW_24H_US: u64 = 24 * 3_600 * 1_000_000;

// ── Ticker ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ticker {
    pub symbol: SymbolId,
    pub last_price: Decimal128,
    pub mark_price: Decimal128,
    pub index_price: Decimal128,
    pub high_24h: Decimal128,
    pub low_24h: Decimal128,
    pub volume_24h: Decimal128,
    pub quote_volume_24h: Decimal128,
    pub open_24h: Decimal128,
    pub price_change_24h: Decimal128,
    pub price_change_pct_24h: Decimal128,
    pub trade_count_24h: u64,
    pub best_bid: Option<Decimal128>,
    pub best_ask: Option<Decimal128>,
    pub last_updated: UnixMicros,
}

// ── TickerAggregator ───────────────────────────────────────────────────

/// 24h sliding window ticker.
pub struct TickerAggregator {
    symbol: SymbolId,
    /// Trades in the 24h window: (timestamp, price, qty).
    trades: VecDeque<(UnixMicros, Decimal128, Decimal128)>,
    current_ticker: Ticker,
}

impl TickerAggregator {
    pub fn new(symbol: SymbolId) -> Self {
        Self {
            symbol,
            trades: VecDeque::new(),
            current_ticker: Ticker {
                symbol,
                last_price: Decimal128::ZERO,
                mark_price: Decimal128::ZERO,
                index_price: Decimal128::ZERO,
                high_24h: Decimal128::ZERO,
                low_24h: Decimal128::ZERO,
                volume_24h: Decimal128::ZERO,
                quote_volume_24h: Decimal128::ZERO,
                open_24h: Decimal128::ZERO,
                price_change_24h: Decimal128::ZERO,
                price_change_pct_24h: Decimal128::ZERO,
                trade_count_24h: 0,
                best_bid: None,
                best_ask: None,
                last_updated: UnixMicros::from_micros(0),
            },
        }
    }

    /// Add a trade and update the ticker.
    pub fn add_trade(&mut self, price: Decimal128, qty: Decimal128, timestamp: UnixMicros) {
        self.trades.push_back((timestamp, price, qty));
        self.prune(timestamp);
        self.recompute(timestamp);
    }

    /// Update mark/index prices.
    pub fn update_prices(&mut self, mark_price: Decimal128, index_price: Decimal128) {
        self.current_ticker.mark_price = mark_price;
        self.current_ticker.index_price = index_price;
    }

    /// Update best bid/ask from orderbook.
    pub fn update_bbo(&mut self, best_bid: Option<Decimal128>, best_ask: Option<Decimal128>) {
        self.current_ticker.best_bid = best_bid;
        self.current_ticker.best_ask = best_ask;
    }

    /// Get current ticker snapshot.
    pub fn ticker(&self) -> &Ticker {
        &self.current_ticker
    }

    /// Prune old trades outside the 24h window.
    pub fn prune(&mut self, now: UnixMicros) {
        let cutoff = now.as_micros().saturating_sub(WINDOW_24H_US);
        while let Some(front) = self.trades.front() {
            if front.0.as_micros() < cutoff {
                self.trades.pop_front();
            } else {
                break;
            }
        }
    }

    /// Recompute all derived fields from the trade window.
    fn recompute(&mut self, now: UnixMicros) {
        let t = &mut self.current_ticker;

        if self.trades.is_empty() {
            t.high_24h = Decimal128::ZERO;
            t.low_24h = Decimal128::ZERO;
            t.volume_24h = Decimal128::ZERO;
            t.quote_volume_24h = Decimal128::ZERO;
            t.open_24h = Decimal128::ZERO;
            t.price_change_24h = Decimal128::ZERO;
            t.price_change_pct_24h = Decimal128::ZERO;
            t.trade_count_24h = 0;
            t.last_updated = now;
            return;
        }

        let mut high = Decimal128::MIN;
        let mut low = Decimal128::MAX;
        let mut volume = Decimal128::ZERO;
        let mut quote_volume = Decimal128::ZERO;

        for &(_, price, qty) in &self.trades {
            high = high.max(price);
            low = low.min(price);
            volume = volume + qty;
            quote_volume = quote_volume + price * qty;
        }

        let open = self.trades.front().unwrap().1;
        let last = self.trades.back().unwrap().1;

        t.symbol = self.symbol;
        t.last_price = last;
        t.high_24h = high;
        t.low_24h = low;
        t.volume_24h = volume;
        t.quote_volume_24h = quote_volume;
        t.open_24h = open;
        t.price_change_24h = last - open;
        t.price_change_pct_24h = if !open.is_zero() {
            (last - open) / open * Decimal128::from(100i64)
        } else {
            Decimal128::ZERO
        };
        t.trade_count_24h = self.trades.len() as u64;
        t.last_updated = now;
    }
}

// ── MarkPriceCalculator ───────────────────────────────────────────────

/// Mark price calculator using Median(last, index, fair) model with stale detection.
pub struct MarkPriceCalculator {
    symbol: SymbolId,
    last_price: Decimal128,
    index_price: Decimal128,
    index_last_updated: UnixMicros,
    fair_price: Decimal128,
    mark_price: Decimal128,
    is_stale: bool,
    max_stale_us: u64,
}

impl MarkPriceCalculator {
    /// Create a new calculator.
    ///
    /// `max_stale_secs` is the maximum age of the index price before it is
    /// considered stale. Default recommendation: 30 seconds.
    pub fn new(symbol: SymbolId, max_stale_secs: u64) -> Self {
        Self {
            symbol,
            last_price: Decimal128::ZERO,
            index_price: Decimal128::ZERO,
            index_last_updated: UnixMicros::from_micros(0),
            fair_price: Decimal128::ZERO,
            mark_price: Decimal128::ZERO,
            is_stale: false,
            max_stale_us: max_stale_secs * 1_000_000,
        }
    }

    /// Update index price from external feed.
    pub fn update_index_price(&mut self, price: Decimal128, timestamp: UnixMicros) {
        self.index_price = price;
        self.index_last_updated = timestamp;
    }

    /// Update last trade price.
    pub fn update_last_price(&mut self, price: Decimal128) {
        self.last_price = price;
    }

    /// Update fair price (from funding rate premium).
    pub fn update_fair_price(&mut self, price: Decimal128) {
        self.fair_price = price;
    }

    /// Recalculate mark price using Median(last, index, fair).
    ///
    /// Also checks staleness of the index price.
    pub fn recalculate(&mut self, now: UnixMicros) -> Decimal128 {
        // Check staleness.
        let age = now.as_micros().saturating_sub(self.index_last_updated.as_micros());
        self.is_stale = age > self.max_stale_us;

        // Median of three values.
        self.mark_price = median3(self.last_price, self.index_price, self.fair_price);
        self.mark_price
    }

    pub fn mark_price(&self) -> Decimal128 {
        self.mark_price
    }

    pub fn is_stale(&self) -> bool {
        self.is_stale
    }

    pub fn symbol(&self) -> SymbolId {
        self.symbol
    }
}

/// Compute median of three Decimal128 values.
fn median3(a: Decimal128, b: Decimal128, c: Decimal128) -> Decimal128 {
    if (a >= b && a <= c) || (a <= b && a >= c) {
        a
    } else if (b >= a && b <= c) || (b <= a && b >= c) {
        b
    } else {
        c
    }
}

// ════════════════════════════════════════════════════════════════════════
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

    const HOUR_US: u64 = 3_600 * 1_000_000;

    #[test]
    fn test_24h_ohlcv() {
        let mut agg = TickerAggregator::new(sym());
        let base = HOUR_US; // start at 1 hour

        agg.add_trade(dec("100"), dec("1"), UnixMicros::from_micros(base));
        agg.add_trade(dec("120"), dec("2"), UnixMicros::from_micros(base + 1_000_000));
        agg.add_trade(dec("80"), dec("0.5"), UnixMicros::from_micros(base + 2_000_000));
        agg.add_trade(dec("110"), dec("1.5"), UnixMicros::from_micros(base + 3_000_000));

        let t = agg.ticker();
        assert_eq!(t.open_24h, dec("100"));
        assert_eq!(t.high_24h, dec("120"));
        assert_eq!(t.low_24h, dec("80"));
        assert_eq!(t.last_price, dec("110"));
        assert_eq!(t.volume_24h, dec("5"));
        // quote = 100*1 + 120*2 + 80*0.5 + 110*1.5 = 100 + 240 + 40 + 165 = 545
        assert_eq!(t.quote_volume_24h, dec("545"));
        assert_eq!(t.trade_count_24h, 4);
    }

    #[test]
    fn test_trade_outside_24h_pruned() {
        let mut agg = TickerAggregator::new(sym());

        // Old trade at t=0.
        agg.add_trade(dec("50"), dec("10"), UnixMicros::from_micros(0));

        // New trade at t=25h → old trade should be pruned.
        let t_25h = 25 * HOUR_US;
        agg.add_trade(dec("100"), dec("1"), UnixMicros::from_micros(t_25h));

        let t = agg.ticker();
        assert_eq!(t.trade_count_24h, 1);
        assert_eq!(t.open_24h, dec("100"));
        assert_eq!(t.volume_24h, dec("1"));
    }

    #[test]
    fn test_price_change_percentage() {
        let mut agg = TickerAggregator::new(sym());
        let base = HOUR_US;

        agg.add_trade(dec("200"), dec("1"), UnixMicros::from_micros(base));
        agg.add_trade(dec("250"), dec("1"), UnixMicros::from_micros(base + 1_000_000));

        let t = agg.ticker();
        assert_eq!(t.price_change_24h, dec("50"));
        assert_eq!(t.price_change_pct_24h, dec("25"));
    }

    #[test]
    fn test_mark_index_price_update() {
        let mut agg = TickerAggregator::new(sym());
        agg.update_prices(dec("50001.23"), dec("50000.89"));

        let t = agg.ticker();
        assert_eq!(t.mark_price, dec("50001.23"));
        assert_eq!(t.index_price, dec("50000.89"));
    }

    #[test]
    fn test_bbo_update() {
        let mut agg = TickerAggregator::new(sym());
        agg.update_bbo(Some(dec("49999")), Some(dec("50001")));

        let t = agg.ticker();
        assert_eq!(t.best_bid, Some(dec("49999")));
        assert_eq!(t.best_ask, Some(dec("50001")));
    }

    // ── MarkPriceCalculator tests ────────────────────────────────────

    #[test]
    fn test_mark_price_normal_calculation() {
        let mut calc = MarkPriceCalculator::new(sym(), 30);

        calc.update_last_price(dec("50100"));
        calc.update_index_price(dec("50000"), UnixMicros::from_micros(100_000_000));
        calc.update_fair_price(dec("50050"));

        let mark = calc.recalculate(UnixMicros::from_micros(110_000_000));

        // Median(50100, 50000, 50050) = 50050
        assert_eq!(mark, dec("50050"));
        assert!(!calc.is_stale());
    }

    #[test]
    fn test_mark_price_stale_detection() {
        let mut calc = MarkPriceCalculator::new(sym(), 30);

        // Index updated at t=0.
        calc.update_index_price(dec("50000"), UnixMicros::from_micros(0));
        calc.update_last_price(dec("50100"));
        calc.update_fair_price(dec("50050"));

        // Now = 31 seconds later (31_000_000 us > 30_000_000 us).
        calc.recalculate(UnixMicros::from_micros(31_000_000));
        assert!(calc.is_stale());

        // Now = 29 seconds later — not stale.
        calc.update_index_price(dec("50000"), UnixMicros::from_micros(31_000_000));
        calc.recalculate(UnixMicros::from_micros(60_000_000));
        assert!(!calc.is_stale());
    }

    #[test]
    fn test_mark_price_median_ordering() {
        let mut calc = MarkPriceCalculator::new(sym(), 30);
        let now = UnixMicros::from_micros(1_000_000);

        // Case 1: last is the median.
        calc.update_last_price(dec("50050"));
        calc.update_index_price(dec("50000"), now);
        calc.update_fair_price(dec("50100"));
        assert_eq!(calc.recalculate(now), dec("50050"));

        // Case 2: index is the median.
        calc.update_last_price(dec("49900"));
        calc.update_index_price(dec("50000"), now);
        calc.update_fair_price(dec("50100"));
        assert_eq!(calc.recalculate(now), dec("50000"));

        // Case 3: fair is the median.
        calc.update_last_price(dec("49900"));
        calc.update_index_price(dec("50100"), now);
        calc.update_fair_price(dec("50000"));
        assert_eq!(calc.recalculate(now), dec("50000"));

        // Case 4: all equal.
        calc.update_last_price(dec("50000"));
        calc.update_index_price(dec("50000"), now);
        calc.update_fair_price(dec("50000"));
        assert_eq!(calc.recalculate(now), dec("50000"));
    }
}
