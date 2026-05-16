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
    /// Latest mark price tracked from `MarkPriceUpdate`; the notional base
    /// for funding settlement (Task 4).
    mark_price: Decimal128,
    /// Monotonic funding batch counter; advanced per settled funding tick.
    /// Part of the deterministic idempotency key so live↔replay align.
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
            match e {
                Event::OrderFilled {
                    user_id,
                    symbol,
                    side,
                    fill_qty,
                    fill_price,
                    ..
                } => {
                    let pnl = self.apply_fill_to_position(
                        *user_id,
                        *symbol,
                        *side,
                        *fill_qty,
                        *fill_price,
                    );
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
                Event::MarkPriceUpdate { mark_price, .. } => {
                    self.mark_price = *mark_price;
                }
                Event::FundingRateUpdate { funding_rate, .. } => {
                    out.extend(self.settle_funding(*funding_rate, ts_param));
                }
                _ => {}
            }
        }
        out
    }

    /// Settle funding for every open position at the current mark.
    /// `payment = size * mark_price * signed_rate` (>0 user pays, <0
    /// receives); routed through the SAME capped-debit primitive as
    /// realized PnL so a Long that cannot cover a payment never panics the
    /// exchange. One atomic batch per tick (spec invariant 33);
    /// `verify_all_invariants` after (spec invariant 32). Returns one
    /// `FundingSettled` fact per position that actually moved money.
    fn settle_funding(&mut self, rate: Decimal128, ts: UnixMicros) -> Vec<Event> {
        // Snapshot (user,symbol,size,side) first — the capped primitive
        // borrows the ledger mutably; avoid aliasing the positions iter.
        let rows: Vec<(UserId, SymbolId, Decimal128, PositionSide)> = self
            .positions
            .all_positions()
            .filter(|p| !p.size.is_zero())
            .map(|p| (p.user_id, p.symbol, p.size, p.side))
            .collect();
        // CEO review C3: a funding tick with open positions but no mark
        // price set would charge everyone 0 and look like success — a
        // silent correctness failure. Warn + skip (no period bump, no
        // events).
        if !rows.is_empty() && self.mark_price.is_zero() {
            tracing::warn!(
                target: "post_trade",
                open_positions = rows.len(),
                "funding tick skipped: mark_price unset"
            );
            return Vec::new();
        }
        self.funding_period_id += 1;
        let period = self.funding_period_id;
        let mark = self.mark_price;
        let mut out = Vec::new();
        let mut settled_count = 0u64;
        let mut total_abs = Decimal128::ZERO;
        for (user_id, symbol, size, side) in rows {
            // notional always positive (size is magnitude); a Short pays a
            // sign-flipped amount so long/short directions net.
            let notional = size * mark;
            let signed_rate = match side {
                PositionSide::Long | PositionSide::Both => rate,
                PositionSide::Short => -rate,
            };
            let payment = notional * signed_rate; // >0 user pays, <0 receives
            if payment.is_zero() {
                continue;
            }
            // CEO C1/C1b: route funding through the SAME capped-debit
            // primitive as realized PnL. A Long that cannot cover a
            // funding payment must NOT panic the exchange; the moved
            // amount (capped) is what the FundingSettled fact records.
            // Deterministic idempotency key so live↔replay align.
            //
            // Sign bridge: `settle_realized_pnl_capped` uses the PnL
            // convention (signed > 0 = CREDIT user / signed < 0 = DEBIT
            // user, capped). Funding's `payment` is the opposite (> 0 =
            // user PAYS). Pass `-payment` so a paying user is debited and
            // a receiving user is credited; the returned `moved` is the
            // signed USER delta in the PnL convention — negate it back to
            // the FundingSettled convention (positive = user paid).
            self.ledger.get_or_create_account(user_id);
            let key = format!("funding_{period}_{}_{}", user_id.value(), symbol.value());
            let moved = self
                .ledger
                .settle_realized_pnl_capped(user_id, -payment, &key, ts)
                .expect("capped funding settle is infallible for a known account");
            if moved.is_zero() {
                continue;
            }
            let fact_amount = -moved; // back to: positive = user paid
            settled_count += 1;
            total_abs = total_abs + fact_amount.abs();
            out.push(Event::FundingSettled {
                user_id,
                symbol,
                funding_period_id: period,
                amount: fact_amount,
                timestamp: ts,
            });
        }
        // CEO review C4: settlement audit line before the invariant gate.
        tracing::info!(
            target: "post_trade",
            period, settled_count, %total_abs,
            "funding batch"
        );
        self.ledger
            .verify_all_invariants()
            .expect("ledger invariants after funding batch (spec invariant 32)");
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
    fn mark_price_update_tracked_for_notional() {
        let mut pt = PostTradeProcessor::new();
        pt.consume(
            &[Event::MarkPriceUpdate {
                symbol: SymbolId::new(1),
                mark_price: dec("60000"),
                index_price: dec("60000"),
                timestamp: ts(),
            }],
            ts(),
        );
        // No position → funding tick is a no-op but mark must be stored.
        let out = pt.consume(
            &[Event::FundingRateUpdate {
                symbol: SymbolId::new(1),
                funding_rate: dec("0.0001"),
                timestamp: ts(),
            }],
            ts(),
        );
        assert!(
            !out.iter()
                .any(|e| matches!(e, Event::FundingSettled { .. }))
        );
    }

    #[test]
    fn funding_long_pays_short_receives() {
        let mut pt = PostTradeProcessor::new();
        pt.handle_admin_credit(UserId::new(1), dec("100000"), "c1", ts());
        pt.handle_admin_credit(UserId::new(2), dec("100000"), "c2", ts());
        // user1 long 1 @60000, user2 short 1 @60000 (they cross)
        pt.consume(&[filled(1, Side::Buy, "1", "60000")], ts());
        pt.consume(&[filled(2, Side::Sell, "1", "60000")], ts());
        pt.consume(
            &[Event::MarkPriceUpdate {
                symbol: SymbolId::new(1),
                mark_price: dec("60000"),
                index_price: dec("60000"),
                timestamp: ts(),
            }],
            ts(),
        );
        // rate 0.01 → long pays 60000*1*0.01 = 600 ; short receives 600
        let out = pt.consume(
            &[Event::FundingRateUpdate {
                symbol: SymbolId::new(1),
                funding_rate: dec("0.01"),
                timestamp: ts(),
            }],
            ts(),
        );
        let settled: Vec<_> = out
            .iter()
            .filter_map(|e| match e {
                Event::FundingSettled {
                    user_id,
                    amount,
                    funding_period_id,
                    ..
                } => Some((user_id.value(), *amount, *funding_period_id)),
                _ => None,
            })
            .collect();
        assert_eq!(settled.len(), 2);
        // long (user1) pays +600 ; short (user2) receives -600
        assert!(
            settled
                .iter()
                .any(|(u, a, p)| *u == 1 && *a == dec("600") && *p == 1)
        );
        assert!(
            settled
                .iter()
                .any(|(u, a, p)| *u == 2 && *a == dec("-600") && *p == 1)
        );
        let b1 = pt
            .ledger()
            .get_balance(UserId::new(1), exg_ledger::WalletType::Funding)
            .unwrap();
        let b2 = pt
            .ledger()
            .get_balance(UserId::new(2), exg_ledger::WalletType::Funding)
            .unwrap();
        assert_eq!(b1.available, dec("99400")); // 100000 - 600
        assert_eq!(b2.available, dec("100600")); // 100000 + 600
        pt.ledger().verify_all_invariants().unwrap();
    }

    #[test]
    fn funding_period_id_increments_per_tick() {
        let mut pt = PostTradeProcessor::new();
        pt.handle_admin_credit(UserId::new(1), dec("100000"), "c1", ts());
        pt.consume(&[filled(1, Side::Buy, "1", "60000")], ts());
        pt.consume(
            &[Event::MarkPriceUpdate {
                symbol: SymbolId::new(1),
                mark_price: dec("60000"),
                index_price: dec("60000"),
                timestamp: ts(),
            }],
            ts(),
        );
        let o1 = pt.consume(
            &[Event::FundingRateUpdate {
                symbol: SymbolId::new(1),
                funding_rate: dec("0.001"),
                timestamp: ts(),
            }],
            ts(),
        );
        let o2 = pt.consume(
            &[Event::FundingRateUpdate {
                symbol: SymbolId::new(1),
                funding_rate: dec("0.001"),
                timestamp: ts(),
            }],
            ts(),
        );
        let p1 = o1
            .iter()
            .find_map(|e| {
                if let Event::FundingSettled {
                    funding_period_id, ..
                } = e
                {
                    Some(*funding_period_id)
                } else {
                    None
                }
            })
            .unwrap();
        let p2 = o2
            .iter()
            .find_map(|e| {
                if let Event::FundingSettled {
                    funding_period_id, ..
                } = e
                {
                    Some(*funding_period_id)
                } else {
                    None
                }
            })
            .unwrap();
        assert_eq!((p1, p2), (1, 2));
    }

    #[test]
    fn funding_imbalanced_book_invariants_hold() {
        let mut pt = PostTradeProcessor::new();
        pt.handle_admin_credit(UserId::new(1), dec("100000"), "c1", ts());
        // only a long, no offsetting short → Funding pool nets negative,
        // which verify_all_invariants explicitly permits.
        pt.consume(&[filled(1, Side::Buy, "2", "60000")], ts());
        pt.consume(
            &[Event::MarkPriceUpdate {
                symbol: SymbolId::new(1),
                mark_price: dec("60000"),
                index_price: dec("60000"),
                timestamp: ts(),
            }],
            ts(),
        );
        pt.consume(
            &[Event::FundingRateUpdate {
                symbol: SymbolId::new(1),
                funding_rate: dec("0.01"),
                timestamp: ts(),
            }],
            ts(),
        );
        pt.ledger().verify_all_invariants().unwrap();
    }

    #[test]
    fn funding_tick_zero_mark_with_open_positions_warns_skips() {
        // Spec C3: an open position present but mark_price never set must
        // NOT silently charge 0 and look like success. The tick is skipped:
        // no FundingSettled, no period bump.
        let mut pt = PostTradeProcessor::new();
        pt.handle_admin_credit(UserId::new(1), dec("100000"), "c1", ts());
        pt.consume(&[filled(1, Side::Buy, "1", "60000")], ts());
        // NOTE: no MarkPriceUpdate consumed → mark_price stays ZERO.
        let out = pt.consume(
            &[Event::FundingRateUpdate {
                symbol: SymbolId::new(1),
                funding_rate: dec("0.01"),
                timestamp: ts(),
            }],
            ts(),
        );
        assert!(
            !out.iter()
                .any(|e| matches!(e, Event::FundingSettled { .. })),
            "zero-mark funding tick with open positions emits no FundingSettled"
        );
        // funding_period_id must NOT have advanced: a subsequent valid tick
        // (after a mark is set) must be period 1, proving the skipped tick
        // did not bump the counter.
        pt.consume(
            &[Event::MarkPriceUpdate {
                symbol: SymbolId::new(1),
                mark_price: dec("60000"),
                index_price: dec("60000"),
                timestamp: ts(),
            }],
            ts(),
        );
        let out2 = pt.consume(
            &[Event::FundingRateUpdate {
                symbol: SymbolId::new(1),
                funding_rate: dec("0.01"),
                timestamp: ts(),
            }],
            ts(),
        );
        let period = out2
            .iter()
            .find_map(|e| {
                if let Event::FundingSettled {
                    funding_period_id, ..
                } = e
                {
                    Some(*funding_period_id)
                } else {
                    None
                }
            })
            .expect("FundingSettled emitted on the valid post-mark tick");
        assert_eq!(
            period, 1,
            "skipped zero-mark tick did not advance funding_period_id"
        );
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
