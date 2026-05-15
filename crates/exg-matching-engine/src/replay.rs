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
                let book = self.orderbook_mut();
                let existing = match book.get_order(*order_id) {
                    Some(o) => o,
                    None => return Err(ApplyError::UnknownOrder(*order_id)),
                };
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
                Ok(())
            }
            Event::TradeExecuted { .. } => Ok(()),
            Event::MarkPriceUpdate { .. } => Err(ApplyError::UnexpectedVariant {
                variant: "MarkPriceUpdate",
            }),
            Event::FundingRateUpdate { .. } => Err(ApplyError::UnexpectedVariant {
                variant: "FundingRateUpdate",
            }),
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
        MatchingEngine::new(cfg, 1)
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
    fn apply_mark_price_update_returns_unexpected_variant() {
        let mut engine = test_engine();
        let err = engine
            .apply_event(&Event::MarkPriceUpdate {
                symbol: SymbolId::new(1),
                mark_price: dec("50000"),
                index_price: dec("50000"),
                timestamp: ts(),
            })
            .unwrap_err();
        assert!(matches!(
            err,
            ApplyError::UnexpectedVariant {
                variant: "MarkPriceUpdate"
            }
        ));
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
    // Eng review B13: UnexpectedVariant arms for Funding / Liquidation
    // ────────────────────────────────────────────────────────────────────

    #[test]
    fn apply_funding_rate_update_returns_unexpected_variant() {
        let mut engine = test_engine();
        let err = engine
            .apply_event(&Event::FundingRateUpdate {
                symbol: SymbolId::new(1),
                funding_rate: dec("0.0001"),
                timestamp: ts(),
            })
            .unwrap_err();
        assert!(matches!(
            err,
            ApplyError::UnexpectedVariant {
                variant: "FundingRateUpdate"
            }
        ));
    }

    #[test]
    fn apply_liquidation_order_returns_unexpected_variant() {
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
}
