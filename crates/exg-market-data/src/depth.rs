use std::collections::BTreeMap;

use exg_common::{Decimal128, Side, SymbolId, UnixMicros};
use serde::{Deserialize, Serialize};

// ── DepthSnapshot ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepthSnapshot {
    pub symbol: SymbolId,
    pub timestamp: UnixMicros,
    /// (price, qty) — sorted descending by price.
    pub bids: Vec<(Decimal128, Decimal128)>,
    /// (price, qty) — sorted ascending by price.
    pub asks: Vec<(Decimal128, Decimal128)>,
    pub last_update_id: u64,
}

// ── DepthDiff ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepthDiff {
    pub symbol: SymbolId,
    pub timestamp: UnixMicros,
    /// Changed bid levels (qty=0 means removed).
    pub bids: Vec<(Decimal128, Decimal128)>,
    /// Changed ask levels (qty=0 means removed).
    pub asks: Vec<(Decimal128, Decimal128)>,
    pub first_update_id: u64,
    pub last_update_id: u64,
}

// ── DepthTracker ───────────────────────────────────────────────────────

/// Tracks orderbook depth and generates snapshots/diffs.
pub struct DepthTracker {
    symbol: SymbolId,
    /// price -> qty. We use BTreeMap for sorted iteration.
    bids: BTreeMap<Decimal128, Decimal128>,
    asks: BTreeMap<Decimal128, Decimal128>,
    update_id: u64,
    last_snapshot_id: u64,
    /// Accumulated diff since last snapshot.
    diff_bids: Vec<(Decimal128, Decimal128)>,
    diff_asks: Vec<(Decimal128, Decimal128)>,
}

impl DepthTracker {
    pub fn new(symbol: SymbolId) -> Self {
        Self {
            symbol,
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            update_id: 0,
            last_snapshot_id: 0,
            diff_bids: Vec::new(),
            diff_asks: Vec::new(),
        }
    }

    /// Update a price level. qty = 0 means remove.
    pub fn update_level(&mut self, side: Side, price: Decimal128, qty: Decimal128) {
        self.update_id += 1;

        let (book, diffs) = match side {
            Side::Buy => (&mut self.bids, &mut self.diff_bids),
            Side::Sell => (&mut self.asks, &mut self.diff_asks),
        };

        if qty.is_zero() {
            book.remove(&price);
        } else {
            book.insert(price, qty);
        }

        diffs.push((price, qty));
    }

    /// Batch update multiple levels.
    pub fn update_levels(&mut self, updates: &[(Side, Decimal128, Decimal128)]) {
        for &(side, price, qty) in updates {
            self.update_level(side, price, qty);
        }
    }

    /// Get full depth snapshot (top N levels).
    pub fn snapshot(&self, levels: usize) -> DepthSnapshot {
        // Bids: highest price first (reverse iterator on BTreeMap).
        let bids: Vec<(Decimal128, Decimal128)> = self
            .bids
            .iter()
            .rev()
            .take(levels)
            .map(|(&p, &q)| (p, q))
            .collect();

        // Asks: lowest price first (forward iterator on BTreeMap).
        let asks: Vec<(Decimal128, Decimal128)> = self
            .asks
            .iter()
            .take(levels)
            .map(|(&p, &q)| (p, q))
            .collect();

        DepthSnapshot {
            symbol: self.symbol,
            timestamp: UnixMicros::from_micros(0), // caller should set
            bids,
            asks,
            last_update_id: self.update_id,
        }
    }

    /// Generate incremental diff since last snapshot.
    pub fn diff_since_snapshot(&self) -> DepthDiff {
        DepthDiff {
            symbol: self.symbol,
            timestamp: UnixMicros::from_micros(0),
            bids: self.diff_bids.clone(),
            asks: self.diff_asks.clone(),
            first_update_id: self.last_snapshot_id + 1,
            last_update_id: self.update_id,
        }
    }

    /// Mark current state as snapshot baseline, clearing diff buffers.
    pub fn mark_snapshot(&mut self) {
        self.last_snapshot_id = self.update_id;
        self.diff_bids.clear();
        self.diff_asks.clear();
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
    fn test_update_levels_and_snapshot() {
        let mut dt = DepthTracker::new(sym());

        dt.update_level(Side::Buy, dec("100"), dec("5"));
        dt.update_level(Side::Buy, dec("99"), dec("10"));
        dt.update_level(Side::Buy, dec("101"), dec("3"));
        dt.update_level(Side::Sell, dec("102"), dec("7"));
        dt.update_level(Side::Sell, dec("105"), dec("2"));
        dt.update_level(Side::Sell, dec("103"), dec("4"));

        let snap = dt.snapshot(10);

        // Bids: descending by price.
        assert_eq!(snap.bids.len(), 3);
        assert_eq!(snap.bids[0], (dec("101"), dec("3")));
        assert_eq!(snap.bids[1], (dec("100"), dec("5")));
        assert_eq!(snap.bids[2], (dec("99"), dec("10")));

        // Asks: ascending by price.
        assert_eq!(snap.asks.len(), 3);
        assert_eq!(snap.asks[0], (dec("102"), dec("7")));
        assert_eq!(snap.asks[1], (dec("103"), dec("4")));
        assert_eq!(snap.asks[2], (dec("105"), dec("2")));
    }

    #[test]
    fn test_remove_level() {
        let mut dt = DepthTracker::new(sym());

        dt.update_level(Side::Buy, dec("100"), dec("5"));
        dt.update_level(Side::Buy, dec("99"), dec("10"));

        // Remove price level 100.
        dt.update_level(Side::Buy, dec("100"), Decimal128::ZERO);

        let snap = dt.snapshot(10);
        assert_eq!(snap.bids.len(), 1);
        assert_eq!(snap.bids[0], (dec("99"), dec("10")));
    }

    #[test]
    fn test_diff_generation() {
        let mut dt = DepthTracker::new(sym());

        // Initial state.
        dt.update_level(Side::Buy, dec("100"), dec("5"));
        dt.update_level(Side::Sell, dec("102"), dec("3"));
        dt.mark_snapshot();

        // New updates after snapshot.
        dt.update_level(Side::Buy, dec("101"), dec("2"));
        dt.update_level(Side::Sell, dec("102"), Decimal128::ZERO); // remove

        let diff = dt.diff_since_snapshot();
        assert_eq!(diff.bids.len(), 1);
        assert_eq!(diff.bids[0], (dec("101"), dec("2")));
        assert_eq!(diff.asks.len(), 1);
        assert_eq!(diff.asks[0], (dec("102"), Decimal128::ZERO));
        assert_eq!(diff.first_update_id, 3); // after 2 initial updates
        assert_eq!(diff.last_update_id, 4); // 2 more updates

        // After mark_snapshot, diffs should be empty.
        dt.mark_snapshot();
        let diff2 = dt.diff_since_snapshot();
        assert!(diff2.bids.is_empty());
        assert!(diff2.asks.is_empty());
    }

    #[test]
    fn test_snapshot_top_n_levels() {
        let mut dt = DepthTracker::new(sym());

        for i in 1..=10 {
            let price = Decimal128::from(i as i64);
            dt.update_level(Side::Buy, price, dec("1"));
        }

        let snap = dt.snapshot(3);
        assert_eq!(snap.bids.len(), 3);
        // Top 3 bids: 10, 9, 8.
        assert_eq!(snap.bids[0].0, Decimal128::from(10i64));
        assert_eq!(snap.bids[1].0, Decimal128::from(9i64));
        assert_eq!(snap.bids[2].0, Decimal128::from(8i64));
    }

    #[test]
    fn test_batch_update() {
        let mut dt = DepthTracker::new(sym());

        dt.update_levels(&[
            (Side::Buy, dec("100"), dec("5")),
            (Side::Buy, dec("99"), dec("10")),
            (Side::Sell, dec("101"), dec("3")),
        ]);

        let snap = dt.snapshot(10);
        assert_eq!(snap.bids.len(), 2);
        assert_eq!(snap.asks.len(), 1);
    }
}
