//! Stage 3 — post-trade pipeline (clearing bounded context).
//!
//! Owned by the matching OS thread. `consume` reacts to engine events
//! (positions + ledger money moves, emitting fact events); `apply_event`
//! re-applies the WAL on replay. No I/O, no locks — single-thread.

use exg_common::{Decimal128, MarginMode, PositionSide, Side, SymbolId, UnixMicros, UserId};
use exg_ledger::Ledger;
use exg_protocol::Event;

use crate::position::PositionManager;

pub struct PostTradeProcessor {
    positions: PositionManager,
    ledger: Ledger,
    // Read in Task 4 (funding settlement); held now so the struct shape is
    // stable across the Stage 3 task sequence.
    #[expect(dead_code, reason = "consumed by funding settlement in Task 4")]
    mark_price: Decimal128,
    #[expect(dead_code, reason = "consumed by funding settlement in Task 4")]
    funding_period_id: u64,
}

impl PostTradeProcessor {
    pub fn new() -> Self {
        Self {
            positions: PositionManager::new(),
            ledger: Ledger::new(),
            mark_price: Decimal128::ZERO,
            funding_period_id: 0,
        }
    }

    /// Read-only accessors for tests / boot invariant checks.
    pub fn ledger(&self) -> &Ledger {
        &self.ledger
    }
    pub fn positions(&self) -> &PositionManager {
        &self.positions
    }

    /// Map a fill `Side` to the `PositionSide` it opens.
    fn opening_side(side: Side) -> PositionSide {
        match side {
            Side::Buy => PositionSide::Long,
            Side::Sell => PositionSide::Short,
        }
    }

    /// Live path: react to engine events. Stage 2 (this task): positions
    /// ONLY (money in Tasks 3-4). Positions project from `OrderFilled`
    /// ONLY — `TradeExecuted` describes the same trade and would
    /// double-count (spec invariant 34).
    pub fn consume(&mut self, events: &[Event], _ts_param: UnixMicros) -> Vec<Event> {
        let out = Vec::new();
        for e in events {
            if let Event::OrderFilled {
                user_id,
                symbol,
                side,
                fill_qty,
                fill_price,
                ..
            } = e
            {
                self.apply_fill_to_position(*user_id, *symbol, *side, *fill_qty, *fill_price);
                // RealizedPnl emission added in Task 3.
            }
        }
        out
    }

    /// Position-keeping: a fill in the position's direction (or no
    /// position) increases; an opposite fill reduces/closes (flipping if
    /// it exceeds current size). Returns the signed realized PnL produced
    /// by a reduction (0 when only opening/increasing) — Task 3 consumes it.
    fn apply_fill_to_position(
        &mut self,
        user_id: UserId,
        symbol: SymbolId,
        side: Side,
        qty: Decimal128,
        price: Decimal128,
    ) -> Decimal128 {
        let fill_side = Self::opening_side(side);
        let cur = self
            .positions
            .get_position(user_id, symbol)
            .map(|p| (p.side, p.size));
        match cur {
            None => {
                self.positions.open_or_increase(
                    user_id,
                    symbol,
                    fill_side,
                    qty,
                    price,
                    Decimal128::ONE,
                    MarginMode::Cross,
                );
                Decimal128::ZERO
            }
            Some((pos_side, pos_size)) if pos_side == fill_side || pos_size.is_zero() => {
                self.positions.open_or_increase(
                    user_id,
                    symbol,
                    fill_side,
                    qty,
                    price,
                    Decimal128::ONE,
                    MarginMode::Cross,
                );
                Decimal128::ZERO
            }
            Some((_pos_side, pos_size)) => {
                let reduce_qty = qty.min(pos_size);
                let (pnl, _) = self
                    .positions
                    .reduce_or_close(user_id, symbol, reduce_qty, price)
                    .expect("reduce_or_close on an existing position");
                let leftover = qty - reduce_qty;
                if leftover.is_positive() {
                    self.positions.open_or_increase(
                        user_id,
                        symbol,
                        fill_side,
                        leftover,
                        price,
                        Decimal128::ONE,
                        MarginMode::Cross,
                    );
                }
                pnl
            }
        }
    }
}

impl Default for PostTradeProcessor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dec(s: &str) -> Decimal128 {
        s.parse().unwrap()
    }
    fn ts() -> UnixMicros {
        UnixMicros::from_micros(1_700_000_000_000_000)
    }

    fn filled(user: u64, side: Side, qty: &str, price: &str) -> Event {
        Event::OrderFilled {
            order_id: exg_common::OrderId::new(user),
            trade_id: exg_common::TradeId::new(1),
            user_id: UserId::new(user),
            symbol: SymbolId::new(1),
            side,
            fill_price: dec(price),
            fill_qty: dec(qty),
            is_maker: false,
            remaining_qty: Decimal128::ZERO,
            timestamp: ts(),
        }
    }

    #[test]
    fn fill_opens_long_position_no_money() {
        let mut pt = PostTradeProcessor::new();
        let out = pt.consume(&[filled(42, Side::Buy, "2", "60000")], ts());
        let pos = pt
            .positions()
            .get_position(UserId::new(42), SymbolId::new(1))
            .unwrap();
        assert_eq!(pos.size, dec("2"));
        assert_eq!(pos.side, PositionSide::Long);
        assert_eq!(pos.entry_price, dec("60000"));
        assert!(!out.iter().any(|e| matches!(e, Event::RealizedPnl { .. })));
    }

    #[test]
    fn same_side_fill_increases_weighted_avg_entry() {
        let mut pt = PostTradeProcessor::new();
        pt.consume(&[filled(42, Side::Buy, "2", "60000")], ts());
        pt.consume(&[filled(42, Side::Buy, "2", "62000")], ts());
        let pos = pt
            .positions()
            .get_position(UserId::new(42), SymbolId::new(1))
            .unwrap();
        assert_eq!(pos.size, dec("4"));
        assert_eq!(pos.entry_price, dec("61000")); // (2*60000 + 2*62000)/4
    }

    #[test]
    fn opposite_fill_reduces_position() {
        let mut pt = PostTradeProcessor::new();
        pt.consume(&[filled(42, Side::Buy, "3", "60000")], ts());
        pt.consume(&[filled(42, Side::Sell, "1", "61000")], ts());
        let pos = pt
            .positions()
            .get_position(UserId::new(42), SymbolId::new(1))
            .unwrap();
        assert_eq!(pos.size, dec("2"));
    }

    #[test]
    fn trade_executed_does_not_double_count_position() {
        let mut pt = PostTradeProcessor::new();
        pt.consume(
            &[
                filled(42, Side::Buy, "2", "60000"),
                Event::TradeExecuted {
                    trade_id: exg_common::TradeId::new(1),
                    symbol: SymbolId::new(1),
                    price: dec("60000"),
                    qty: dec("2"),
                    buyer_order_id: exg_common::OrderId::new(42),
                    seller_order_id: exg_common::OrderId::new(43),
                    buyer_user_id: UserId::new(42),
                    seller_user_id: UserId::new(43),
                    buyer_fee: Decimal128::ZERO,
                    seller_fee: Decimal128::ZERO,
                    timestamp: ts(),
                },
            ],
            ts(),
        );
        let pos = pt
            .positions()
            .get_position(UserId::new(42), SymbolId::new(1))
            .unwrap();
        assert_eq!(pos.size, dec("2"), "TradeExecuted must not double-count");
    }
}
