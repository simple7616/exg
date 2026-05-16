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
    /// Monotonic per-emitted-RealizedPnl discriminator. Used to build a
    /// stable idempotency key `pnl_{seq}_{user}_{symbol}`. Advanced **iff**
    /// a `RealizedPnl` fact is actually emitted (an underfunded zero-move
    /// loss advances neither this nor emits an event). Replay (Task 5)
    /// re-applies `RealizedPnl` facts in WAL order and MUST increment this
    /// in the exact same order so keys match — this determinism is
    /// load-bearing (spec invariant 31).
    pnl_seq: u64,
}

impl PostTradeProcessor {
    pub fn new() -> Self {
        Self {
            positions: PositionManager::new(),
            ledger: Ledger::new(),
            mark_price: Decimal128::ZERO,
            funding_period_id: 0,
            pnl_seq: 0,
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
    pub fn consume(&mut self, events: &[Event], ts_param: UnixMicros) -> Vec<Event> {
        let mut out = Vec::new();
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
                let pnl =
                    self.apply_fill_to_position(*user_id, *symbol, *side, *fill_qty, *fill_price);
                if !pnl.is_zero() {
                    // Invariant 31: `pnl_seq` MUST advance in lockstep with
                    // emitted `RealizedPnl` facts so live↔replay idempotency
                    // keys match. We settle with the PROVISIONAL next seq but
                    // only COMMIT (increment) the counter when the move is
                    // non-zero and a fact is actually pushed. An underfunded
                    // loss with 0 available caps `moved` to 0 → no event AND
                    // no seq advance, exactly mirroring Task 5 replay (which
                    // increments once per replayed RealizedPnl fact).
                    let next_seq = self.pnl_seq + 1;
                    // CEO C1/C1b: record the ACTUALLY-MOVED signed amount
                    // (capped at the user's available for a loss), not the
                    // notional pnl — replay applies this fact directly.
                    let moved =
                        self.settle_realized_pnl(*user_id, *symbol, pnl, next_seq, ts_param);
                    if !moved.is_zero() {
                        self.pnl_seq = next_seq;
                        out.push(Event::RealizedPnl {
                            user_id: *user_id,
                            symbol: *symbol,
                            amount: moved,
                            timestamp: ts_param,
                        });
                    }
                }
            }
        }
        out
    }

    /// Admin-credit a user's Funding wallet (ledger journals SYSTEM→user;
    /// idempotent on `idempotency_key`). Emits the AdminCredited fact.
    pub fn handle_admin_credit(
        &mut self,
        user_id: UserId,
        amount: Decimal128,
        idempotency_key: &str,
        ts: UnixMicros,
    ) -> Vec<Event> {
        self.ledger.get_or_create_account(user_id);
        self.ledger
            .deposit(user_id, amount, idempotency_key, ts)
            .expect("admin credit deposit (amount > 0 enforced at handler)");
        vec![Event::AdminCredited {
            user_id,
            amount,
            idempotency_key: idempotency_key.to_owned(),
            timestamp: ts,
        }]
    }

    /// Settle a signed realized PnL vs the system Funding pool, returning
    /// the **actually-moved** signed amount (the value recorded in the
    /// RealizedPnl fact event). CEO review C1/C1b: a loss the user cannot
    /// cover MUST NOT panic the single-threaded exchange and MUST NOT
    /// drive `verify_account_invariant` (forbids negative user available)
    /// to fail. Uses the new capped-debit ledger primitive
    /// `settle_realized_pnl_capped`: profit → SYSTEM→user; loss →
    /// move only `min(loss, user.Funding.available)` (user floored at 0),
    /// uncollected remainder = implicit bad debt absorbed by the SYSTEM
    /// Funding pool (allowed negative). Returns the signed moved amount.
    fn settle_realized_pnl(
        &mut self,
        user_id: UserId,
        symbol: SymbolId,
        pnl: Decimal128,
        seq_tag: u64,
        ts: UnixMicros,
    ) -> Decimal128 {
        if pnl.is_zero() {
            return Decimal128::ZERO;
        }
        // Invariant 31 (live↔replay key lockstep): a loss the user cannot
        // cover at all (`Funding.available == 0`) moves 0 — `cover =
        // min(owed, 0) = 0`. We MUST NOT call `settle_realized_pnl_capped`
        // for it: that op records `key` into the ledger's idempotency set
        // UNCONDITIONALLY (operations.rs check_idempotency inserts on first
        // call), poisoning the key even though nothing moved and no fact is
        // emitted. Replay (Task 5) has NO RealizedPnl fact for this fill so
        // it never settles and never consumes the key → live would diverge
        // from replay (same seq reused later hits the poisoned key → real
        // settlement silently no-ops). Short-circuit read-only BEFORE
        // building the key or touching the ledger: no key consumed, no seq
        // advance, no event — exactly mirroring replay.
        if pnl.is_negative() {
            let available = self
                .ledger
                .get_balance(user_id, exg_ledger::WalletType::Funding)
                .map(|b| b.available)
                .unwrap_or(Decimal128::ZERO);
            if available.is_zero() {
                return Decimal128::ZERO;
            }
        }
        let key = format!("pnl_{seq_tag}_{}_{}", user_id.value(), symbol.value());
        self.ledger.get_or_create_account(user_id);
        // settle_realized_pnl_capped: signed pnl; never errors on
        // insufficiency, never drives user available negative; returns the
        // signed amount actually moved (== pnl for profit or covered loss,
        // == -(available) for an underfunded loss). Idempotent on `key`.
        let moved = self
            .ledger
            .settle_realized_pnl_capped(user_id, pnl, &key, ts)
            .expect("settle_realized_pnl_capped is infallible for a known account");
        // CEO review C4: realized-PnL audit line (spec invariant 39).
        if !moved.is_zero() {
            tracing::info!(
                target: "post_trade",
                user_id = user_id.value(),
                symbol = symbol.value(),
                %moved,
                "realized pnl"
            );
        }
        moved
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
    fn admin_credit_deposits_funding_wallet() {
        let mut pt = PostTradeProcessor::new();
        let out = pt.handle_admin_credit(UserId::new(42), dec("5000"), "ac_1", ts());
        assert!(matches!(out[0], Event::AdminCredited { .. }));
        let bal = pt
            .ledger()
            .get_balance(UserId::new(42), exg_ledger::WalletType::Funding)
            .unwrap();
        assert_eq!(bal.available, dec("5000"));
        pt.ledger().verify_all_invariants().unwrap();
    }

    #[test]
    fn realized_profit_credits_user() {
        let mut pt = PostTradeProcessor::new();
        pt.handle_admin_credit(UserId::new(42), dec("10000"), "ac_2", ts());
        // open long 1 @60000, close 1 @61000 → +1000 profit
        let out = pt.consume(
            &[
                filled(42, Side::Buy, "1", "60000"),
                filled(42, Side::Sell, "1", "61000"),
            ],
            ts(),
        );
        let pnl = out
            .iter()
            .find_map(|e| match e {
                Event::RealizedPnl { amount, .. } => Some(*amount),
                _ => None,
            })
            .expect("RealizedPnl emitted");
        assert_eq!(pnl, dec("1000"));
        let bal = pt
            .ledger()
            .get_balance(UserId::new(42), exg_ledger::WalletType::Funding)
            .unwrap();
        assert_eq!(bal.available, dec("11000")); // 10000 + 1000 profit
        pt.ledger().verify_all_invariants().unwrap();
    }

    #[test]
    fn realized_loss_debits_user() {
        let mut pt = PostTradeProcessor::new();
        pt.handle_admin_credit(UserId::new(42), dec("10000"), "ac_3", ts());
        // long 1 @60000, close 1 @59000 → -1000 loss
        let out = pt.consume(
            &[
                filled(42, Side::Buy, "1", "60000"),
                filled(42, Side::Sell, "1", "59000"),
            ],
            ts(),
        );
        let pnl = out
            .iter()
            .find_map(|e| match e {
                Event::RealizedPnl { amount, .. } => Some(*amount),
                _ => None,
            })
            .unwrap();
        assert_eq!(pnl, dec("-1000"));
        let bal = pt
            .ledger()
            .get_balance(UserId::new(42), exg_ledger::WalletType::Funding)
            .unwrap();
        assert_eq!(bal.available, dec("9000"));
        pt.ledger().verify_all_invariants().unwrap();
    }

    #[test]
    fn zero_move_loss_does_not_advance_pnl_seq() {
        let mut pt = PostTradeProcessor::new();
        // User 7 has NO admin credit → 0 Funding available. Open long
        // 1 @60000, close 1 @59000 → pnl -1000 but available 0 → capped
        // moved == 0 → NO RealizedPnl event and pnl_seq must NOT advance
        // (Stage 3 allows zero-balance opens, so this path is reachable).
        let out1 = pt.consume(
            &[
                filled(7, Side::Buy, "1", "60000"),
                filled(7, Side::Sell, "1", "59000"),
            ],
            ts(),
        );
        assert!(
            !out1.iter().any(|e| matches!(e, Event::RealizedPnl { .. })),
            "underfunded loss with 0 available emits no RealizedPnl"
        );
        pt.ledger().verify_all_invariants().unwrap();

        // Now fund + a profitable round trip → exactly one RealizedPnl.
        // Because the zero-move loss did NOT advance pnl_seq, this is the
        // FIRST emitted fact (idempotency key pnl_1_7_1).
        pt.handle_admin_credit(UserId::new(7), dec("10000"), "k", ts());
        let out2 = pt.consume(
            &[
                filled(7, Side::Buy, "1", "60000"),
                filled(7, Side::Sell, "1", "61000"),
            ],
            ts(),
        );
        let realized: Vec<Decimal128> = out2
            .iter()
            .filter_map(|e| match e {
                Event::RealizedPnl { amount, .. } => Some(*amount),
                _ => None,
            })
            .collect();
        assert_eq!(realized.len(), 1, "exactly one RealizedPnl emitted");
        assert_eq!(realized[0], dec("1000"), "profit settles correctly");
        let bal = pt
            .ledger()
            .get_balance(UserId::new(7), exg_ledger::WalletType::Funding)
            .unwrap();
        assert_eq!(bal.available, dec("11000")); // 10000 credit + 1000 profit
        pt.ledger().verify_all_invariants().unwrap();
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
