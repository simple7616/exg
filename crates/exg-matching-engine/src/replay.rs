//! Stage 1b — WAL replay. Apply historical events to a freshly-built engine.
//!
//! Each WAL record is one `Event`. `apply_event` reverse-maps the event onto
//! the matching engine's mutable state without re-running matching: the
//! matching that produced these events has already been recorded as
//! subsequent `OrderFilled` events. Re-running matching during replay would
//! double-count fills.
//!
//! Replay is **not** idempotent. The caller must apply events in WAL order.

use exg_common::{Decimal128, OrderId};
use exg_protocol::Event;

use crate::engine::MatchingEngine;
use crate::orderbook::BookOrder;

#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    #[error("event references unknown order_id={0:?}")]
    UnknownOrder(OrderId),
    #[error("OrderAccepted for order_id={0:?} already present in book")]
    DuplicateOrder(OrderId),
    #[error("OrderFilled fill_qty {got} exceeds existing remaining {have}")]
    OverFill { got: Decimal128, have: Decimal128 },
    #[error("event variant {variant} unexpected during replay")]
    UnexpectedVariant { variant: &'static str },
}

impl MatchingEngine {
    /// Apply a historical event to the engine state during WAL replay.
    ///
    /// **Replay-only.** Must NOT be called on a live engine (no concurrency
    /// protection; will produce nonsense state if interleaved with
    /// `process_command`).
    pub fn apply_event(&mut self, event: &Event) -> Result<(), ApplyError> {
        match event {
            Event::OrderAccepted {
                order_id,
                user_id,
                symbol,
                client_order_id,
                timestamp,
                side,
                order_type,
                time_in_force,
                price,
                quantity,
                stop_price,
                reduce_only,
                visible_quantity,
                trailing_delta,
                trailing_peak_price,
            } => {
                if self.orderbook_mut().get_order(*order_id).is_some() {
                    return Err(ApplyError::DuplicateOrder(*order_id));
                }
                // Derive expire_time for GTD orders (matches engine.rs:215-224 logic).
                let expire_time = if *time_in_force == exg_common::TimeInForce::Gtd {
                    let twenty_four_hours_micros: u64 = 24 * 3600 * 1_000_000;
                    Some(exg_common::UnixMicros::from_micros(
                        timestamp.as_micros() + twenty_four_hours_micros,
                    ))
                } else {
                    None
                };
                // Derive hidden_qty for iceberg orders (matches engine.rs:206-213 logic).
                let hidden_qty = match visible_quantity {
                    Some(vis) => *quantity - *vis,
                    None => Decimal128::ZERO,
                };
                let remaining_qty = visible_quantity.unwrap_or(*quantity);
                let book_order = BookOrder {
                    order_id: *order_id,
                    user_id: *user_id,
                    symbol: *symbol,
                    side: *side,
                    price: *price,
                    remaining_qty,
                    original_qty: *quantity,
                    order_type: *order_type,
                    time_in_force: *time_in_force,
                    is_reduce_only: *reduce_only,
                    timestamp: *timestamp,
                    visible_qty: *visible_quantity,
                    hidden_qty,
                    trailing_delta: *trailing_delta,
                    trailing_peak_price: *trailing_peak_price,
                    expire_time,
                    client_order_id: *client_order_id,
                    stop_price: *stop_price,
                };
                // Task 9: record the id as observed this replay so a later
                // terminal duplicate taker leg (constant FINAL remaining_qty
                // across a multi-fill sweep) is distinguishable from a corrupt
                // orphan fill referencing a never-accepted id.
                self.replay_seen_order_ids_mut().insert(*order_id);
                // Conditional orders sit in stop_orders; everything else on the book.
                if order_type.is_conditional() {
                    self.stop_orders_mut().push(book_order);
                } else {
                    self.orderbook_mut().insert_order(book_order);
                }
                // GTD orders must also be tracked by the expiry heap so the GTD
                // sweeper can find them after replay (matches engine.rs:414).
                if *time_in_force == exg_common::TimeInForce::Gtd
                    && let Some(expire) = expire_time
                {
                    self.expiry_heap_mut()
                        .push(std::cmp::Reverse((expire, *order_id)));
                }
                Ok(())
            }
            Event::OrderRejected { .. } => Ok(()),
            Event::OrderCanceled { order_id, .. } => {
                if self.orderbook_mut().remove_order(*order_id).is_none() {
                    // Also try stop_orders (conditional orders).
                    let stop_orders = self.stop_orders_mut();
                    let pos = stop_orders.iter().position(|o| o.order_id == *order_id);
                    match pos {
                        Some(i) => {
                            stop_orders.remove(i);
                        }
                        None => return Err(ApplyError::UnknownOrder(*order_id)),
                    }
                }
                Ok(())
            }
            Event::OrderFilled {
                order_id,
                fill_qty,
                remaining_qty,
                ..
            } => {
                // Path 1 — order is resting in the orderbook (maker, an
                // already-promoted resting Limit conditional, or a
                // non-conditional taker still resting before its terminal
                // event). The taker OrderFilled.remaining_qty is the FINAL
                // post-match value (engine.rs emit_fill_events takes
                // taker.remaining_qty AFTER the matcher ran), so the first
                // OrderFilled the order receives carries its terminal state.
                if self.orderbook_mut().get_order(*order_id).is_some() {
                    let book = self.orderbook_mut();
                    let existing = book.get_order(*order_id).expect("checked above");
                    if *fill_qty > existing.remaining_qty {
                        return Err(ApplyError::OverFill {
                            got: *fill_qty,
                            have: existing.remaining_qty,
                        });
                    }
                    if remaining_qty.is_zero() {
                        book.remove_order(*order_id);
                    } else {
                        book.update_qty(*order_id, *remaining_qty);
                    }
                    return Ok(());
                }

                // Path 2 — order is NOT in the orderbook but IS in
                // stop_orders: a conditional triggered by a (passively
                // replayed) MarkPriceUpdate. Replay never re-triggers
                // (invariant 27), so it was never promoted. Perform the
                // deterministic, matcher-free analogue of the live
                // trigger_and_match_stops promotion (no matcher, no
                // re-trigger): remove from stop_orders, apply the EXACT
                // conditional->Market/Limit conversion, then drive the order
                // to the live-equivalent terminal state from the WAL event.
                // Mirrors the OrderCanceled arm's stop_orders fallback.
                {
                    let stop_orders = self.stop_orders_mut();
                    if let Some(i) = stop_orders.iter().position(|o| o.order_id == *order_id) {
                        let mut order = stop_orders.swap_remove(i);
                        if *fill_qty > order.remaining_qty {
                            return Err(ApplyError::OverFill {
                                got: *fill_qty,
                                have: order.remaining_qty,
                            });
                        }
                        // Exact conversion from trigger_and_match_stops
                        // (engine.rs): market-type conditionals become Market
                        // with a crossing price by side; limit-type keep
                        // their limit price as Limit.
                        match order.order_type {
                            exg_common::OrderType::StopMarket
                            | exg_common::OrderType::TakeProfitMarket
                            | exg_common::OrderType::TrailingStop => {
                                order.order_type = exg_common::OrderType::Market;
                                order.price = match order.side {
                                    exg_common::Side::Buy => Decimal128::MAX,
                                    exg_common::Side::Sell => Decimal128::ZERO,
                                };
                            }
                            exg_common::OrderType::StopLimit
                            | exg_common::OrderType::TakeProfitLimit => {
                                order.order_type = exg_common::OrderType::Limit;
                            }
                            _ => {}
                        }
                        // Live reinserts leftover only if
                        // !remaining_qty.is_zero() && order_type.is_limit()
                        // (a converted Market never rests — leftover dropped).
                        if !remaining_qty.is_zero() && order.order_type.is_limit() {
                            order.remaining_qty = *remaining_qty;
                            self.orderbook_mut().insert_order(order);
                        }
                        // else: fully filled OR Market-with-leftover -> order
                        // ends removed (already swap_removed from stop_orders,
                        // not inserted into the book).
                        return Ok(());
                    }
                }

                // Path 3 — not in the orderbook and not in stop_orders.
                //
                // (a) The id WAS observed earlier this replay (accepted, then
                //     removed by its FIRST terminal OrderFilled): this is a
                //     subsequent per-fill leg of a multi-fill taker sweep.
                //     Every taker OrderFilled for a sweep carries the same
                //     constant FINAL remaining_qty (engine.rs reads
                //     taker.remaining_qty AFTER the matcher ran), so once the
                //     order is gone the remaining legs are state-preserving
                //     no-ops — the matcher-free analogue of live's single
                //     post-match settlement.
                //
                // (b) The id was NEVER observed: a genuinely corrupt /
                //     truncated / reordered WAL referencing a never-accepted
                //     order. This MUST still fail-fast so boot aborts rather
                //     than replaying into wrong state (state-safety; preserves
                //     the boot_panics guarantee and mirrors the OrderCanceled
                //     arm's UnknownOrder rejection for a truly unknown id).
                if self.replay_seen_order_ids_mut().contains(order_id) {
                    Ok(())
                } else {
                    Err(ApplyError::UnknownOrder(*order_id))
                }
            }
            Event::TradeExecuted { .. } => Ok(()),
            Event::MarkPriceUpdate {
                mark_price,
                index_price,
                ..
            } => {
                // Passive only — triggered OrderFilled/TradeExecuted events
                // are separate WAL records replayed via their own arms.
                // Re-triggering here would double-count fills (same principle
                // as OrderAccepted not re-matching). Invariant 27.
                self.apply_mark_index_passive(*mark_price, *index_price);
                Ok(())
            }
            Event::FundingRateUpdate { funding_rate, .. } => {
                self.set_last_funding_rate(*funding_rate);
                Ok(())
            }
            Event::LiquidationOrder { .. } => Err(ApplyError::UnexpectedVariant {
                variant: "LiquidationOrder",
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use exg_common::{
        Decimal128, MarginMode, OrderId, OrderType, Side, SymbolId, TimeInForce, TradeId,
        UnixMicros, UserId,
    };
    use exg_protocol::{Event, RejectReason};
    use exg_risk_engine::{MarginTier, SymbolConfig};

    fn dec(s: &str) -> Decimal128 {
        s.parse().unwrap()
    }

    fn ts() -> UnixMicros {
        UnixMicros::from_micros(1_700_000_000_000_000)
    }

    fn test_engine() -> MatchingEngine {
        let cfg = SymbolConfig {
            symbol: SymbolId::new(1),
            tick_size: dec("0.01"),
            lot_size: dec("0.001"),
            min_notional: dec("10"),
            max_leverage: dec("125"),
            maker_fee: dec("0.0002"),
            taker_fee: dec("0.0005"),
            margin_tiers: vec![MarginTier {
                notional_floor: dec("0"),
                notional_cap: dec("50000"),
                maintenance_margin_rate: dec("0.004"),
                maintenance_amount: dec("0"),
            }],
        };
        MatchingEngine::new(cfg, 1, dec("0.0001"))
    }

    fn accept_event(order_id: u64, qty: &str, price: &str) -> Event {
        Event::OrderAccepted {
            order_id: OrderId::new(order_id),
            user_id: UserId::new(42),
            symbol: SymbolId::new(1),
            client_order_id: None,
            timestamp: ts(),
            side: Side::Buy,
            order_type: OrderType::Limit,
            time_in_force: TimeInForce::Gtc,
            price: dec(price),
            quantity: dec(qty),
            stop_price: None,
            reduce_only: false,
            visible_quantity: None,
            trailing_delta: None,
            trailing_peak_price: None,
        }
    }

    /// Builder for OrderAccepted with custom fields (B6 schema-field tests).
    #[allow(clippy::too_many_arguments)]
    fn accept_event_full(
        order_id: u64,
        order_type: OrderType,
        time_in_force: TimeInForce,
        qty: &str,
        price: &str,
        visible_quantity: Option<Decimal128>,
        trailing_delta: Option<Decimal128>,
        trailing_peak_price: Option<Decimal128>,
        stop_price: Option<Decimal128>,
    ) -> Event {
        Event::OrderAccepted {
            order_id: OrderId::new(order_id),
            user_id: UserId::new(42),
            symbol: SymbolId::new(1),
            client_order_id: None,
            timestamp: ts(),
            side: Side::Buy,
            order_type,
            time_in_force,
            price: dec(price),
            quantity: dec(qty),
            stop_price,
            reduce_only: false,
            visible_quantity,
            trailing_delta,
            trailing_peak_price,
        }
    }

    #[test]
    fn apply_order_accepted_inserts_book_order() {
        let mut engine = test_engine();
        engine
            .apply_event(&accept_event(1, "1.0", "50000"))
            .unwrap();
        let order = engine.orderbook().get_order(OrderId::new(1)).unwrap();
        assert_eq!(order.remaining_qty, dec("1.0"));
        assert_eq!(order.price, dec("50000"));
    }

    #[test]
    fn apply_order_canceled_removes_book_order() {
        let mut engine = test_engine();
        engine
            .apply_event(&accept_event(1, "1.0", "50000"))
            .unwrap();
        engine
            .apply_event(&Event::OrderCanceled {
                order_id: OrderId::new(1),
                user_id: UserId::new(42),
                symbol: SymbolId::new(1),
                remaining_qty: dec("1.0"),
                timestamp: ts(),
            })
            .unwrap();
        assert!(engine.orderbook().get_order(OrderId::new(1)).is_none());
    }

    #[test]
    fn apply_order_filled_decrements_remaining_qty() {
        let mut engine = test_engine();
        engine
            .apply_event(&accept_event(1, "1.0", "50000"))
            .unwrap();
        engine
            .apply_event(&Event::OrderFilled {
                order_id: OrderId::new(1),
                trade_id: TradeId::new(100),
                user_id: UserId::new(42),
                symbol: SymbolId::new(1),
                side: Side::Buy,
                fill_price: dec("50000"),
                fill_qty: dec("0.4"),
                is_maker: true,
                remaining_qty: dec("0.6"),
                timestamp: ts(),
            })
            .unwrap();
        let order = engine.orderbook().get_order(OrderId::new(1)).unwrap();
        assert_eq!(order.remaining_qty, dec("0.6"));
    }

    #[test]
    fn apply_order_filled_zero_removes_book_order() {
        let mut engine = test_engine();
        engine
            .apply_event(&accept_event(1, "1.0", "50000"))
            .unwrap();
        engine
            .apply_event(&Event::OrderFilled {
                order_id: OrderId::new(1),
                trade_id: TradeId::new(100),
                user_id: UserId::new(42),
                symbol: SymbolId::new(1),
                side: Side::Buy,
                fill_price: dec("50000"),
                fill_qty: dec("1.0"),
                is_maker: false,
                remaining_qty: Decimal128::ZERO,
                timestamp: ts(),
            })
            .unwrap();
        assert!(engine.orderbook().get_order(OrderId::new(1)).is_none());
    }

    #[test]
    fn apply_trade_executed_is_noop_on_book() {
        let mut engine = test_engine();
        engine
            .apply_event(&accept_event(1, "1.0", "50000"))
            .unwrap();
        engine
            .apply_event(&Event::TradeExecuted {
                trade_id: TradeId::new(100),
                symbol: SymbolId::new(1),
                price: dec("50000"),
                qty: dec("0.5"),
                buyer_order_id: OrderId::new(1),
                seller_order_id: OrderId::new(2),
                buyer_user_id: UserId::new(42),
                seller_user_id: UserId::new(43),
                buyer_fee: dec("0.005"),
                seller_fee: dec("0.0125"),
                timestamp: ts(),
            })
            .unwrap();
        assert_eq!(
            engine
                .orderbook()
                .get_order(OrderId::new(1))
                .unwrap()
                .remaining_qty,
            dec("1.0")
        );
    }

    #[test]
    fn apply_order_rejected_is_noop() {
        let mut engine = test_engine();
        engine
            .apply_event(&Event::OrderRejected {
                order_id: OrderId::new(9),
                user_id: UserId::new(42),
                reason: RejectReason::InsufficientMargin,
                timestamp: ts(),
            })
            .unwrap();
        assert_eq!(engine.orderbook().order_count(), 0);
    }

    #[test]
    fn apply_duplicate_order_accepted_returns_err() {
        let mut engine = test_engine();
        engine
            .apply_event(&accept_event(1, "1.0", "50000"))
            .unwrap();
        let err = engine
            .apply_event(&accept_event(1, "2.0", "50001"))
            .unwrap_err();
        assert!(matches!(err, ApplyError::DuplicateOrder(_)));
    }

    #[test]
    fn apply_unknown_order_canceled_returns_err() {
        let mut engine = test_engine();
        let err = engine
            .apply_event(&Event::OrderCanceled {
                order_id: OrderId::new(99),
                user_id: UserId::new(42),
                symbol: SymbolId::new(1),
                remaining_qty: dec("0"),
                timestamp: ts(),
            })
            .unwrap_err();
        assert!(matches!(err, ApplyError::UnknownOrder(_)));
    }

    // Task 9: a NEVER-observed order_id still fails-fast (state-safety: a
    // corrupt / truncated / reordered WAL must abort boot, not replay into
    // wrong state). The Task 9 terminal-duplicate no-op (Path 3 case a)
    // applies ONLY to ids seen earlier this replay — see
    // apply_terminal_duplicate_taker_leg_is_noop and the replay_equiv sweep
    // round-trip tests.
    #[test]
    fn apply_unknown_order_filled_returns_err() {
        let mut engine = test_engine();
        let err = engine
            .apply_event(&Event::OrderFilled {
                order_id: OrderId::new(999),
                trade_id: TradeId::new(100),
                user_id: UserId::new(42),
                symbol: SymbolId::new(1),
                side: Side::Buy,
                fill_price: dec("50000"),
                fill_qty: dec("0.1"),
                is_maker: false,
                remaining_qty: dec("0.9"),
                timestamp: ts(),
            })
            .unwrap_err();
        assert!(matches!(err, ApplyError::UnknownOrder(_)));
    }

    // Task 9: once an order has been observed and driven to its terminal
    // (removed) state by its FIRST OrderFilled, a subsequent OrderFilled for
    // that same id (a per-fill leg of a multi-fill taker sweep, carrying the
    // constant FINAL remaining_qty) is a state-preserving no-op — NOT an
    // UnknownOrder error. This is what makes live<->replay equivalence hold
    // for sweeps while still fail-fast'ing on genuinely orphan ids.
    #[test]
    fn apply_terminal_duplicate_taker_leg_is_noop() {
        let mut engine = test_engine();
        // Accept then fully fill (terminal) — order removed from the book.
        engine
            .apply_event(&accept_event(7, "1.0", "50000"))
            .unwrap();
        engine
            .apply_event(&Event::OrderFilled {
                order_id: OrderId::new(7),
                trade_id: TradeId::new(100),
                user_id: UserId::new(42),
                symbol: SymbolId::new(1),
                side: Side::Buy,
                fill_price: dec("50000"),
                fill_qty: dec("1.0"),
                is_maker: false,
                remaining_qty: dec("0"),
                timestamp: ts(),
            })
            .unwrap();
        assert_eq!(engine.orderbook().order_count(), 0);
        // A second (duplicate terminal) leg for the same id: benign no-op.
        engine
            .apply_event(&Event::OrderFilled {
                order_id: OrderId::new(7),
                trade_id: TradeId::new(101),
                user_id: UserId::new(42),
                symbol: SymbolId::new(1),
                side: Side::Buy,
                fill_price: dec("50000"),
                fill_qty: dec("0"),
                is_maker: false,
                remaining_qty: dec("0"),
                timestamp: ts(),
            })
            .expect("terminal duplicate taker leg is a state-preserving no-op");
        assert_eq!(engine.orderbook().order_count(), 0);
        assert_eq!(engine.stop_order_count(), 0);
    }

    #[test]
    fn apply_over_fill_returns_err() {
        let mut engine = test_engine();
        engine
            .apply_event(&accept_event(1, "1.0", "50000"))
            .unwrap();
        let err = engine
            .apply_event(&Event::OrderFilled {
                order_id: OrderId::new(1),
                trade_id: TradeId::new(100),
                user_id: UserId::new(42),
                symbol: SymbolId::new(1),
                side: Side::Buy,
                fill_price: dec("50000"),
                fill_qty: dec("2.0"),
                is_maker: true,
                remaining_qty: Decimal128::ZERO,
                timestamp: ts(),
            })
            .unwrap_err();
        assert!(matches!(err, ApplyError::OverFill { .. }));
    }

    #[test]
    fn apply_event_mark_price_update_passive_only() {
        let mut engine = test_engine();
        let accept = accept_event_full(
            7,
            OrderType::StopMarket,
            TimeInForce::Gtc,
            "1.0",
            "59000",
            None,
            None,
            None,
            Some(dec("59000")),
        );
        engine.apply_event(&accept).unwrap();
        engine
            .apply_event(&Event::MarkPriceUpdate {
                symbol: SymbolId::new(1),
                mark_price: dec("58000"),
                index_price: dec("58000"),
                timestamp: ts(),
            })
            .unwrap();
        assert_eq!(engine.mark_price(), dec("58000"));
        assert_eq!(
            engine.stop_orders_mut().len(),
            1,
            "stop must NOT trigger on replay"
        );
    }

    #[test]
    fn apply_event_funding_rate_update_sets_last_rate() {
        let mut engine = test_engine();
        engine
            .apply_event(&Event::FundingRateUpdate {
                symbol: SymbolId::new(1),
                funding_rate: dec("0.0042"),
                timestamp: ts(),
            })
            .unwrap();
        assert_eq!(engine.last_funding_rate(), dec("0.0042"));
    }

    #[test]
    fn apply_event_mark_price_replay_preserves_trailing_peak() {
        // Eng/CEO review C8: guards the Stage 1b silent-corruption-on-replay
        // class for trailing peaks. STRICTLY ASCENDING marks so neither
        // engine triggers the trailing sell — isolates peak-fidelity.
        //
        // accept_event_full hardcodes side = Buy; a Buy trailing stop tracks
        // the downward trough and triggers on rising price, which would make
        // the ascending-marks premise false. The test intent requires a SELL
        // trailing stop (peak tracks the upward high, no trigger on rising
        // mark), so this one OrderAccepted is built inline with Side::Sell.
        // All other fields mirror accept_event_full(9, TrailingStop, Gtc,
        // "1.0", "60000", None, Some(100), Some(60000), Some(59900)).
        let accepted = Event::OrderAccepted {
            order_id: OrderId::new(9),
            user_id: UserId::new(42),
            symbol: SymbolId::new(1),
            client_order_id: None,
            timestamp: ts(),
            side: Side::Sell,
            order_type: OrderType::TrailingStop,
            time_in_force: TimeInForce::Gtc,
            price: dec("60000"),
            quantity: dec("1.0"),
            stop_price: Some(dec("59900")),
            reduce_only: false,
            visible_quantity: None,
            trailing_delta: Some(dec("100")),
            trailing_peak_price: Some(dec("60000")),
        };
        let ascending = ["60500", "61000", "61500"];

        let mut live = test_engine();
        live.apply_event(&accepted).unwrap();
        for px in ascending {
            let _ = live.update_mark_price(SymbolId::new(1), dec(px), dec(px), ts());
        }
        assert_eq!(
            live.stop_orders_mut().len(),
            1,
            "ascending marks must not trigger a trailing sell"
        );
        let live_peak = live.stop_orders_mut()[0].trailing_peak_price;

        let mut replayed = test_engine();
        replayed.apply_event(&accepted).unwrap();
        for px in ascending {
            replayed
                .apply_event(&Event::MarkPriceUpdate {
                    symbol: SymbolId::new(1),
                    mark_price: dec(px),
                    index_price: dec(px),
                    timestamp: ts(),
                })
                .unwrap();
        }
        let replayed_peak = replayed.stop_orders_mut()[0].trailing_peak_price;

        assert_eq!(
            live_peak,
            Some(dec("61500")),
            "live peak should track the 61500 high"
        );
        assert_eq!(
            replayed_peak, live_peak,
            "replayed trailing_peak_price must equal live — no silent drift"
        );
    }

    #[test]
    fn replay_then_take_snapshot_round_trip() {
        use exg_common::SnowflakeGen;
        use exg_protocol::Command;
        let mut live = test_engine();
        live.set_mark_price(dec("60000"));
        let sf = SnowflakeGen::new(1);
        let mut events = Vec::new();
        for i in 0..5 {
            let cmd = Command::NewOrder {
                order_id: OrderId::new(sf.next_id()),
                user_id: UserId::new(42),
                symbol: SymbolId::new(1),
                side: Side::Buy,
                order_type: OrderType::Limit,
                time_in_force: TimeInForce::Gtc,
                price: Some(dec(&format!("{}", 55000 + i))),
                quantity: dec("0.001"),
                stop_price: None,
                trailing_delta: None,
                visible_quantity: None,
                reduce_only: false,
                margin_mode: MarginMode::Cross,
                leverage: Some(dec("10")),
                timestamp: ts(),
                client_order_id: None,
            };
            events.extend(live.process_command(&cmd));
        }
        let mut replayed = test_engine();
        for evt in &events {
            replayed.apply_event(evt).unwrap();
        }
        assert_eq!(
            live.orderbook().order_count(),
            replayed.orderbook().order_count(),
            "replayed engine must have same order count as live engine"
        );
    }

    // ────────────────────────────────────────────────────────────────────
    // Eng review B12: conditional / stop-order replay paths
    // ────────────────────────────────────────────────────────────────────

    #[test]
    fn apply_order_accepted_conditional_pushes_to_stop_orders() {
        let mut engine = test_engine();
        let evt = accept_event_full(
            42,
            OrderType::StopLimit,
            TimeInForce::Gtc,
            "1.0",
            "50000",
            None,
            None,
            None,
            Some(dec("49000")),
        );
        engine.apply_event(&evt).unwrap();
        assert!(engine.orderbook().get_order(OrderId::new(42)).is_none());
        assert_eq!(engine.stop_orders_mut().len(), 1);
        assert_eq!(engine.stop_orders_mut()[0].order_id, OrderId::new(42));
    }

    #[test]
    fn apply_order_canceled_removes_from_stop_orders() {
        let mut engine = test_engine();
        let accept = accept_event_full(
            43,
            OrderType::StopMarket,
            TimeInForce::Gtc,
            "1.0",
            "50000",
            None,
            None,
            None,
            Some(dec("49500")),
        );
        engine.apply_event(&accept).unwrap();
        assert_eq!(engine.stop_orders_mut().len(), 1);
        engine
            .apply_event(&Event::OrderCanceled {
                order_id: OrderId::new(43),
                user_id: UserId::new(42),
                symbol: SymbolId::new(1),
                remaining_qty: dec("1.0"),
                timestamp: ts(),
            })
            .unwrap();
        assert_eq!(engine.stop_orders_mut().len(), 0);
    }

    // ────────────────────────────────────────────────────────────────────
    // Eng review B6: schema-field replay paths (iceberg / GTD / trailing)
    // ────────────────────────────────────────────────────────────────────

    #[test]
    fn apply_order_accepted_iceberg_preserves_visible_quantity() {
        let mut engine = test_engine();
        let evt = accept_event_full(
            100,
            OrderType::Iceberg,
            TimeInForce::Gtc,
            "10.0",
            "50000",
            Some(dec("1.0")),
            None,
            None,
            None,
        );
        engine.apply_event(&evt).unwrap();
        let order = engine.orderbook().get_order(OrderId::new(100)).unwrap();
        assert_eq!(order.visible_qty, Some(dec("1.0")));
        assert_eq!(order.hidden_qty, dec("9.0"));
        assert_eq!(order.remaining_qty, dec("1.0"));
        assert_eq!(order.original_qty, dec("10.0"));
    }

    #[test]
    fn apply_order_accepted_gtd_pushes_to_expiry_heap() {
        let mut engine = test_engine();
        let evt = accept_event_full(
            200,
            OrderType::Limit,
            TimeInForce::Gtd,
            "1.0",
            "50000",
            None,
            None,
            None,
            None,
        );
        engine.apply_event(&evt).unwrap();
        let heap = engine.expiry_heap_mut();
        assert_eq!(heap.len(), 1);
        let std::cmp::Reverse((expire_time, order_id)) = heap.peek().unwrap();
        assert_eq!(*order_id, OrderId::new(200));
        let expected = ts().as_micros() + 24 * 3600 * 1_000_000;
        assert_eq!(expire_time.as_micros(), expected);
        let order = engine.orderbook().get_order(OrderId::new(200)).unwrap();
        assert!(order.expire_time.is_some());
    }

    #[test]
    fn apply_order_accepted_trailing_preserves_peak_price() {
        let mut engine = test_engine();
        let evt = accept_event_full(
            300,
            OrderType::TrailingStop,
            TimeInForce::Gtc,
            "1.0",
            "50000",
            None,
            Some(dec("100")),
            Some(dec("60000")),
            Some(dec("59900")),
        );
        engine.apply_event(&evt).unwrap();
        assert_eq!(engine.stop_orders_mut().len(), 1);
        let order = &engine.stop_orders_mut()[0];
        assert_eq!(order.trailing_delta, Some(dec("100")));
        assert_eq!(order.trailing_peak_price, Some(dec("60000")));
    }

    // ────────────────────────────────────────────────────────────────────
    // Stage 2 Task 5: LiquidationOrder still rejected (Stage 3+); full
    // mark-price + funding replay round-trip.
    // ────────────────────────────────────────────────────────────────────

    #[test]
    fn apply_event_liquidation_order_still_unexpected_variant() {
        let mut engine = test_engine();
        let err = engine
            .apply_event(&Event::LiquidationOrder {
                user_id: UserId::new(42),
                symbol: SymbolId::new(1),
                side: Side::Buy,
                quantity: dec("1.0"),
                timestamp: ts(),
            })
            .unwrap_err();
        assert!(matches!(
            err,
            ApplyError::UnexpectedVariant {
                variant: "LiquidationOrder"
            }
        ));
    }

    #[test]
    fn replay_round_trip_with_mark_price_and_funding() {
        use exg_protocol::Command;
        let mut live2 = test_engine();
        let mut all_events = Vec::new();
        for cmd in [
            Command::NewOrder {
                order_id: OrderId::new(1),
                user_id: UserId::new(42),
                symbol: SymbolId::new(1),
                side: Side::Buy,
                order_type: OrderType::Limit,
                time_in_force: TimeInForce::Gtc,
                price: Some(dec("59000")),
                quantity: dec("0.001"),
                stop_price: None,
                trailing_delta: None,
                visible_quantity: None,
                reduce_only: false,
                margin_mode: MarginMode::Cross,
                leverage: Some(dec("10")),
                client_order_id: None,
                timestamp: ts(),
            },
            Command::UpdateMarkPrice {
                symbol: SymbolId::new(1),
                mark_price: dec("60600"),
                index_price: dec("60000"),
                timestamp: ts(),
            },
            Command::ComputeFunding {
                symbol: SymbolId::new(1),
                timestamp: ts(),
            },
        ] {
            all_events.extend(live2.process_command(&cmd));
        }
        let mut replayed = test_engine();
        for evt in &all_events {
            replayed.apply_event(evt).unwrap();
        }
        assert_eq!(
            live2.orderbook().order_count(),
            replayed.orderbook().order_count()
        );
        assert_eq!(live2.mark_price(), replayed.mark_price());
        assert_eq!(live2.last_funding_rate(), replayed.last_funding_rate());
    }

    // ---- Task 9: live<->replay state-equivalence round-trip tests ----
    //
    // Correctness of the triggered-conditional fill replay reconciliation is
    // defined by these tests, NOT by prose: build a live engine, run a Command
    // stream, collect ALL emitted events, replay them into a FRESH engine via
    // apply_event, and assert identical final state (orderbook order count,
    // stop-order count, mark price, last funding rate, and per-resting-order
    // remaining_qty). Replay must reach the same state without running the
    // matcher and without re-triggering stops (invariant 27).

    fn assert_live_replay_equivalent(commands: &[exg_protocol::Command]) {
        let mut live = test_engine();
        let mut all_events = Vec::new();
        for c in commands {
            all_events.extend(live.process_command(c));
        }
        let mut replayed = test_engine();
        for e in &all_events {
            replayed
                .apply_event(e)
                .unwrap_or_else(|err| panic!("replay failed on {e:?}: {err:?}"));
        }
        assert_eq!(
            live.orderbook().order_count(),
            replayed.orderbook().order_count(),
            "order_count"
        );
        assert_eq!(
            live.stop_order_count(),
            replayed.stop_order_count(),
            "stop_order_count"
        );
        assert_eq!(live.mark_price(), replayed.mark_price(), "mark_price");
        assert_eq!(
            live.last_funding_rate(),
            replayed.last_funding_rate(),
            "last_funding_rate"
        );
        // Per-order remaining-qty equivalence for every resting order seen live.
        for o in live.orderbook().all_orders() {
            let l = Some(o.remaining_qty);
            let r = replayed
                .orderbook()
                .get_order(o.order_id)
                .map(|ro| ro.remaining_qty);
            assert_eq!(l, r, "remaining_qty mismatch for order {:?}", o.order_id);
        }
        // And the converse: no order resting in replay that is absent live.
        for o in replayed.orderbook().all_orders() {
            assert!(
                live.orderbook().get_order(o.order_id).is_some(),
                "replay has resting order {:?} absent from live",
                o.order_id
            );
        }
    }

    fn new_order(
        id: u64,
        side: Side,
        order_type: OrderType,
        price: Option<&str>,
        qty: &str,
        stop_price: Option<&str>,
        trailing_delta: Option<&str>,
    ) -> exg_protocol::Command {
        exg_protocol::Command::NewOrder {
            order_id: OrderId::new(id),
            user_id: UserId::new(id),
            symbol: SymbolId::new(1),
            side,
            order_type,
            time_in_force: TimeInForce::Gtc,
            price: price.map(dec),
            quantity: dec(qty),
            stop_price: stop_price.map(dec),
            trailing_delta: trailing_delta.map(dec),
            visible_quantity: None,
            reduce_only: false,
            margin_mode: MarginMode::Cross,
            leverage: Some(dec("10")),
            client_order_id: None,
            timestamp: UnixMicros::from_micros(1_000_000 + id),
        }
    }

    fn mark(mark_price: &str) -> exg_protocol::Command {
        exg_protocol::Command::UpdateMarkPrice {
            symbol: SymbolId::new(1),
            mark_price: dec(mark_price),
            index_price: dec(mark_price),
            timestamp: ts(),
        }
    }

    // Regression guard (Task 9 Step 3 note): a NON-conditional market taker
    // sweeping >=2 makers also emits >=2 taker OrderFilled events all carrying
    // the same constant FINAL remaining_qty. The pre-fix orderbook-only arm
    // removed the taker on the first such event then errored UnknownOrder on
    // the second. Round-trip equivalence must hold for this normal path too.
    #[test]
    fn replay_equiv_market_taker_sweeps_two_makers() {
        let cmds = vec![
            new_order(
                1,
                Side::Sell,
                OrderType::Limit,
                Some("51000"),
                "3",
                None,
                None,
            ),
            new_order(
                2,
                Side::Sell,
                OrderType::Limit,
                Some("51500"),
                "4",
                None,
                None,
            ),
            // Plain market buy 7 sweeps both resting asks (3 + 4), no rest.
            new_order(3, Side::Buy, OrderType::Market, None, "7", None, None),
        ];
        assert_live_replay_equivalent(&cmds);
    }

    // Scenario 1: StopMarket buy, single resting maker, full fill.
    #[test]
    fn replay_equiv_stop_market_single_maker_full_fill() {
        let cmds = vec![
            // Resting ask, 5 @ 51000.
            new_order(
                1,
                Side::Sell,
                OrderType::Limit,
                Some("51000"),
                "5",
                None,
                None,
            ),
            // Stop-market buy 5, stop_price 51000.
            new_order(
                2,
                Side::Buy,
                OrderType::StopMarket,
                None,
                "5",
                Some("51000"),
                None,
            ),
            // Mark crosses the stop -> trigger + full fill.
            mark("51000"),
        ];
        assert_live_replay_equivalent(&cmds);
    }

    // Scenario 2: StopMarket buy sweeps TWO makers at different prices.
    #[test]
    fn replay_equiv_stop_market_sweeps_two_makers() {
        let cmds = vec![
            new_order(
                1,
                Side::Sell,
                OrderType::Limit,
                Some("51000"),
                "3",
                None,
                None,
            ),
            new_order(
                2,
                Side::Sell,
                OrderType::Limit,
                Some("51500"),
                "4",
                None,
                None,
            ),
            // Stop-market buy 7 spans both resting asks (3 + 4).
            new_order(
                3,
                Side::Buy,
                OrderType::StopMarket,
                None,
                "7",
                Some("51000"),
                None,
            ),
            mark("51000"),
        ];
        assert_live_replay_equivalent(&cmds);
    }

    // Scenario 3: StopLimit triggers, partial fill, remainder rests as Limit,
    // a later crossing NewOrder fills the remainder.
    #[test]
    fn replay_equiv_stop_limit_partial_then_rests_then_fills() {
        let cmds = vec![
            // Resting ask 2 @ 51000 (only partially covers the stop's 5).
            new_order(
                1,
                Side::Sell,
                OrderType::Limit,
                Some("51000"),
                "2",
                None,
                None,
            ),
            // Stop-LIMIT buy 5 @ limit 52000, stop_price 51000.
            new_order(
                2,
                Side::Buy,
                OrderType::StopLimit,
                Some("52000"),
                "5",
                Some("51000"),
                None,
            ),
            // Mark crosses -> triggers, fills 2, remainder 3 rests as Limit @ 52000.
            mark("51000"),
            // Later: a sell crosses the resting 3 @ 52000 and fills it.
            new_order(
                3,
                Side::Sell,
                OrderType::Limit,
                Some("52000"),
                "3",
                None,
                None,
            ),
        ];
        assert_live_replay_equivalent(&cmds);
    }

    // Scenario 4: StopMarket insufficient liquidity -> partial fill, Market
    // leftover dropped (live order gone, never rests).
    #[test]
    fn replay_equiv_stop_market_insufficient_liquidity_partial() {
        let cmds = vec![
            // Only 2 resting; stop wants 5.
            new_order(
                1,
                Side::Sell,
                OrderType::Limit,
                Some("51000"),
                "2",
                None,
                None,
            ),
            new_order(
                2,
                Side::Buy,
                OrderType::StopMarket,
                None,
                "5",
                Some("51000"),
                None,
            ),
            mark("51000"),
        ];
        assert_live_replay_equivalent(&cmds);
    }

    // Scenario 5: TrailingStop sell — peak tracked, reversal triggers + fills.
    #[test]
    fn replay_equiv_trailing_stop_triggered() {
        let cmds = vec![
            // Seed the mark so the trailing peak has a baseline.
            mark("50000"),
            // Resting bid for the triggered sell to match against.
            new_order(
                1,
                Side::Buy,
                OrderType::Limit,
                Some("49000"),
                "5",
                None,
                None,
            ),
            // Trailing stop sell qty 5, delta 1000.
            new_order(
                2,
                Side::Sell,
                OrderType::TrailingStop,
                None,
                "5",
                None,
                Some("1000"),
            ),
            // Price rises -> peak tracks to 52000.
            mark("52000"),
            // Drops but not enough (still above 52000-1000=51000).
            mark("51500"),
            // Drops to trigger -> fills against the resting bid.
            mark("51000"),
        ];
        assert_live_replay_equivalent(&cmds);
    }
}
