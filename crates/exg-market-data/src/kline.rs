use std::collections::{HashMap, VecDeque};

use exg_common::{Decimal128, SymbolId, UnixMicros};
use serde::{Deserialize, Serialize};

// ── KlineInterval ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KlineInterval {
    S1,
    M1,
    M5,
    M15,
    H1,
    H4,
    D1,
}

impl KlineInterval {
    /// All intervals, ordered from smallest to largest.
    pub const ALL: [KlineInterval; 7] = [
        Self::S1,
        Self::M1,
        Self::M5,
        Self::M15,
        Self::H1,
        Self::H4,
        Self::D1,
    ];

    /// Duration in microseconds.
    pub const fn duration_us(&self) -> u64 {
        match self {
            Self::S1 => 1_000_000,
            Self::M1 => 60 * 1_000_000,
            Self::M5 => 5 * 60 * 1_000_000,
            Self::M15 => 15 * 60 * 1_000_000,
            Self::H1 => 3_600 * 1_000_000,
            Self::H4 => 4 * 3_600 * 1_000_000,
            Self::D1 => 86_400 * 1_000_000,
        }
    }

    /// Align a timestamp to the start of the interval period.
    pub fn align_timestamp(&self, ts: UnixMicros) -> UnixMicros {
        let dur = self.duration_us();
        let aligned = (ts.as_micros() / dur) * dur;
        UnixMicros::from_micros(aligned)
    }
}

// ── Kline ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Kline {
    pub symbol: SymbolId,
    pub interval: KlineInterval,
    pub open_time: UnixMicros,
    pub close_time: UnixMicros,
    pub open: Decimal128,
    pub high: Decimal128,
    pub low: Decimal128,
    pub close: Decimal128,
    /// Base asset volume.
    pub volume: Decimal128,
    /// Quote asset volume.
    pub quote_volume: Decimal128,
    pub trade_count: u64,
}

// ── KlineAggregator ────────────────────────────────────────────────────

/// Aggregates trades into klines for a single symbol.
pub struct KlineAggregator {
    symbol: SymbolId,
    /// Current open kline per interval.
    current: HashMap<KlineInterval, Kline>,
    /// Completed klines buffer (bounded FIFO).
    completed: VecDeque<Kline>,
    max_completed: usize,
}

impl KlineAggregator {
    pub fn new(symbol: SymbolId, max_completed: usize) -> Self {
        Self {
            symbol,
            current: HashMap::new(),
            completed: VecDeque::new(),
            max_completed,
        }
    }

    /// Process a trade. Returns completed klines if any interval rolled over.
    pub fn add_trade(
        &mut self,
        price: Decimal128,
        qty: Decimal128,
        timestamp: UnixMicros,
    ) -> Vec<Kline> {
        let quote_volume = price * qty;
        let mut completed = Vec::new();

        for interval in KlineInterval::ALL {
            let open_time = interval.align_timestamp(timestamp);
            let close_time =
                UnixMicros::from_micros(open_time.as_micros() + interval.duration_us() - 1);

            if let Some(existing) = self.current.get_mut(&interval) {
                if existing.open_time == open_time {
                    // Same period — update OHLCV.
                    existing.high = existing.high.max(price);
                    existing.low = existing.low.min(price);
                    existing.close = price;
                    existing.volume = existing.volume + qty;
                    existing.quote_volume = existing.quote_volume + quote_volume;
                    existing.trade_count += 1;
                    continue;
                }
                // Period rolled over — complete the old kline.
                let old = existing.clone();
                completed.push(old);
            }

            // Start new kline for this interval.
            self.current.insert(
                interval,
                Kline {
                    symbol: self.symbol,
                    interval,
                    open_time,
                    close_time,
                    open: price,
                    high: price,
                    low: price,
                    close: price,
                    volume: qty,
                    quote_volume,
                    trade_count: 1,
                },
            );
        }

        // Buffer completed klines.
        for kline in &completed {
            if self.completed.len() >= self.max_completed {
                self.completed.pop_front();
            }
            self.completed.push_back(kline.clone());
        }

        completed
    }

    /// Get current (incomplete) kline for an interval.
    pub fn current_kline(&self, interval: KlineInterval) -> Option<&Kline> {
        self.current.get(&interval)
    }

    /// Get recent completed klines for an interval (newest last).
    pub fn recent_klines(&self, interval: KlineInterval, limit: usize) -> Vec<&Kline> {
        self.completed
            .iter()
            .filter(|k| k.interval == interval)
            .rev()
            .take(limit)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
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

    #[test]
    fn test_single_trade_creates_kline() {
        let mut agg = KlineAggregator::new(sym(), 100);
        let ts = UnixMicros::from_micros(1_000_000_000_000); // 1_000_000 seconds
        let completed = agg.add_trade(dec("50000"), dec("1.5"), ts);

        // First trade — no completed klines yet.
        assert!(completed.is_empty());

        let kline = agg.current_kline(KlineInterval::M1).unwrap();
        assert_eq!(kline.open, dec("50000"));
        assert_eq!(kline.high, dec("50000"));
        assert_eq!(kline.low, dec("50000"));
        assert_eq!(kline.close, dec("50000"));
        assert_eq!(kline.volume, dec("1.5"));
        assert_eq!(kline.quote_volume, dec("75000"));
        assert_eq!(kline.trade_count, 1);
        assert_eq!(kline.symbol, sym());
    }

    #[test]
    fn test_multiple_trades_same_interval() {
        let mut agg = KlineAggregator::new(sym(), 100);
        let base = 1_000_000_000_000u64;

        agg.add_trade(dec("100"), dec("1"), UnixMicros::from_micros(base));
        agg.add_trade(
            dec("110"),
            dec("2"),
            UnixMicros::from_micros(base + 100_000),
        );
        agg.add_trade(
            dec("90"),
            dec("0.5"),
            UnixMicros::from_micros(base + 200_000),
        );
        agg.add_trade(
            dec("105"),
            dec("3"),
            UnixMicros::from_micros(base + 300_000),
        );

        let kline = agg.current_kline(KlineInterval::S1).unwrap();
        assert_eq!(kline.open, dec("100"));
        assert_eq!(kline.high, dec("110"));
        assert_eq!(kline.low, dec("90"));
        assert_eq!(kline.close, dec("105"));
        assert_eq!(kline.trade_count, 4);
    }

    #[test]
    fn test_interval_rollover_emits_completed() {
        let mut agg = KlineAggregator::new(sym(), 100);

        // Trade in second 0.
        let t0 = UnixMicros::from_micros(0);
        let completed = agg.add_trade(dec("100"), dec("1"), t0);
        assert!(completed.is_empty());

        // Trade in second 1 — S1 rolls over, M1 stays.
        let t1 = UnixMicros::from_micros(1_000_000);
        let completed = agg.add_trade(dec("200"), dec("2"), t1);

        // S1 should have rolled over.
        assert!(completed.iter().any(|k| k.interval == KlineInterval::S1));

        let s1_completed = completed
            .iter()
            .find(|k| k.interval == KlineInterval::S1)
            .unwrap();
        assert_eq!(s1_completed.open, dec("100"));
        assert_eq!(s1_completed.close, dec("100"));
        assert_eq!(s1_completed.volume, dec("1"));

        // Current S1 should be the new period.
        let current_s1 = agg.current_kline(KlineInterval::S1).unwrap();
        assert_eq!(current_s1.open, dec("200"));
    }

    #[test]
    fn test_volume_and_quote_volume_accumulate() {
        let mut agg = KlineAggregator::new(sym(), 100);
        let base = 60_000_000u64; // 60 seconds in micros (start of minute 1)

        // Trade 1: price=100, qty=2 → quote=200
        agg.add_trade(dec("100"), dec("2"), UnixMicros::from_micros(base));
        // Trade 2: price=150, qty=3 → quote=450
        agg.add_trade(
            dec("150"),
            dec("3"),
            UnixMicros::from_micros(base + 100_000),
        );

        let kline = agg.current_kline(KlineInterval::M1).unwrap();
        assert_eq!(kline.volume, dec("5"));
        assert_eq!(kline.quote_volume, dec("650"));
    }

    #[test]
    fn test_align_timestamp() {
        // 90 seconds = 1 minute + 30 seconds
        let ts = UnixMicros::from_micros(90_000_000);

        assert_eq!(
            KlineInterval::S1.align_timestamp(ts),
            UnixMicros::from_micros(90_000_000)
        );
        assert_eq!(
            KlineInterval::M1.align_timestamp(ts),
            UnixMicros::from_micros(60_000_000)
        );
        assert_eq!(
            KlineInterval::M5.align_timestamp(ts),
            UnixMicros::from_micros(0)
        );
        assert_eq!(
            KlineInterval::H1.align_timestamp(ts),
            UnixMicros::from_micros(0)
        );

        // 2 hours + 30 minutes
        let ts2 = UnixMicros::from_micros(9_000_000_000);
        assert_eq!(
            KlineInterval::H1.align_timestamp(ts2),
            UnixMicros::from_micros(7_200_000_000)
        );
        assert_eq!(
            KlineInterval::H4.align_timestamp(ts2),
            UnixMicros::from_micros(0)
        );
    }

    #[test]
    fn test_recent_klines() {
        let mut agg = KlineAggregator::new(sym(), 100);

        // Create 3 completed S1 klines by trading in seconds 0, 1, 2, 3.
        for i in 0..4u64 {
            let price = Decimal128::from((100 + i as i64) as i64);
            agg.add_trade(price, dec("1"), UnixMicros::from_micros(i * 1_000_000));
        }

        let recent = agg.recent_klines(KlineInterval::S1, 10);
        assert_eq!(recent.len(), 3);
        // Verify ordering (oldest first).
        assert_eq!(recent[0].open, dec("100"));
        assert_eq!(recent[1].open, dec("101"));
        assert_eq!(recent[2].open, dec("102"));
    }
}
