pub mod command;
pub mod event;

pub use command::Command;
pub use event::{Event, RejectReason};

#[cfg(test)]
mod tests {
    use super::*;
    use exg_common::{
        Decimal128, MarginMode, OrderId, OrderType, Side, SymbolId, TimeInForce, TradeId,
        UnixMicros, UserId,
    };

    fn dec(s: &str) -> Decimal128 {
        s.parse().unwrap()
    }

    fn sample_timestamp() -> UnixMicros {
        UnixMicros::from_micros(1_700_000_000_000_000)
    }

    // ── Helper: all Command variants ──────────────────────────────────

    fn all_commands() -> Vec<Command> {
        vec![
            Command::NewOrder {
                order_id: OrderId::new(1001),
                user_id: UserId::new(42),
                symbol: SymbolId::new(1),
                side: Side::Buy,
                order_type: OrderType::Limit,
                time_in_force: TimeInForce::Gtc,
                price: Some(dec("50000.5")),
                quantity: dec("1.25"),
                stop_price: None,
                trailing_delta: None,
                visible_quantity: None,
                reduce_only: false,
                margin_mode: MarginMode::Cross,
                leverage: Some(dec("10")),
                client_order_id: Some(9999),
                timestamp: sample_timestamp(),
            },
            Command::NewOrder {
                order_id: OrderId::new(1002),
                user_id: UserId::new(43),
                symbol: SymbolId::new(2),
                side: Side::Sell,
                order_type: OrderType::Market,
                time_in_force: TimeInForce::Ioc,
                price: None,
                quantity: dec("0.5"),
                stop_price: None,
                trailing_delta: None,
                visible_quantity: None,
                reduce_only: true,
                margin_mode: MarginMode::Isolated,
                leverage: None,
                client_order_id: None,
                timestamp: sample_timestamp(),
            },
            Command::NewOrder {
                order_id: OrderId::new(1003),
                user_id: UserId::new(44),
                symbol: SymbolId::new(3),
                side: Side::Buy,
                order_type: OrderType::StopLimit,
                time_in_force: TimeInForce::Gtd,
                price: Some(dec("49000")),
                quantity: dec("2"),
                stop_price: Some(dec("49500")),
                trailing_delta: None,
                visible_quantity: None,
                reduce_only: false,
                margin_mode: MarginMode::Cross,
                leverage: Some(dec("20")),
                client_order_id: Some(8888),
                timestamp: sample_timestamp(),
            },
            Command::NewOrder {
                order_id: OrderId::new(1004),
                user_id: UserId::new(45),
                symbol: SymbolId::new(1),
                side: Side::Sell,
                order_type: OrderType::TrailingStop,
                time_in_force: TimeInForce::Gtc,
                price: None,
                quantity: dec("3"),
                stop_price: None,
                trailing_delta: Some(dec("100")),
                visible_quantity: None,
                reduce_only: true,
                margin_mode: MarginMode::Isolated,
                leverage: None,
                client_order_id: None,
                timestamp: sample_timestamp(),
            },
            Command::NewOrder {
                order_id: OrderId::new(1005),
                user_id: UserId::new(46),
                symbol: SymbolId::new(1),
                side: Side::Buy,
                order_type: OrderType::Iceberg,
                time_in_force: TimeInForce::Gtc,
                price: Some(dec("50000")),
                quantity: dec("100"),
                stop_price: None,
                trailing_delta: None,
                visible_quantity: Some(dec("10")),
                reduce_only: false,
                margin_mode: MarginMode::Cross,
                leverage: Some(dec("5")),
                client_order_id: Some(7777),
                timestamp: sample_timestamp(),
            },
            Command::CancelOrder {
                order_id: OrderId::new(1001),
                user_id: UserId::new(42),
                symbol: SymbolId::new(1),
                timestamp: sample_timestamp(),
            },
            Command::AmendOrder {
                order_id: OrderId::new(1001),
                user_id: UserId::new(42),
                symbol: SymbolId::new(1),
                new_price: Some(dec("51000")),
                new_quantity: Some(dec("2.0")),
                timestamp: sample_timestamp(),
            },
            Command::AmendOrder {
                order_id: OrderId::new(1002),
                user_id: UserId::new(43),
                symbol: SymbolId::new(2),
                new_price: None,
                new_quantity: Some(dec("0.75")),
                timestamp: sample_timestamp(),
            },
            Command::CancelAllOrders {
                user_id: UserId::new(42),
                symbol: SymbolId::new(1),
                timestamp: sample_timestamp(),
            },
            Command::UpdateMarkPrice {
                symbol: SymbolId::new(1),
                mark_price: dec("60000"),
                index_price: dec("59950"),
                timestamp: sample_timestamp(),
            },
            Command::ComputeFunding {
                symbol: SymbolId::new(1),
                timestamp: sample_timestamp(),
            },
        ]
    }

    // ── Helper: all Event variants ────────────────────────────────────

    fn all_events() -> Vec<Event> {
        vec![
            Event::OrderAccepted {
                order_id: OrderId::new(1001),
                user_id: UserId::new(42),
                symbol: SymbolId::new(1),
                client_order_id: Some(9999),
                timestamp: sample_timestamp(),
                side: Side::Buy,
                order_type: OrderType::Limit,
                time_in_force: TimeInForce::Gtc,
                price: dec("50000.5"),
                quantity: dec("1.0"),
                stop_price: None,
                reduce_only: false,
                visible_quantity: None,
                trailing_delta: None,
                trailing_peak_price: None,
            },
            Event::OrderAccepted {
                order_id: OrderId::new(1002),
                user_id: UserId::new(43),
                symbol: SymbolId::new(2),
                client_order_id: None,
                timestamp: sample_timestamp(),
                side: Side::Sell,
                order_type: OrderType::Market,
                time_in_force: TimeInForce::Ioc,
                price: Decimal128::ZERO,
                quantity: dec("0.5"),
                stop_price: None,
                reduce_only: true,
                visible_quantity: None,
                trailing_delta: None,
                trailing_peak_price: None,
            },
            Event::OrderRejected {
                order_id: OrderId::new(2001),
                user_id: UserId::new(50),
                reason: RejectReason::InsufficientMargin,
                timestamp: sample_timestamp(),
            },
            Event::OrderRejected {
                order_id: OrderId::new(2002),
                user_id: UserId::new(51),
                reason: RejectReason::PostOnlyWouldTake,
                timestamp: sample_timestamp(),
            },
            Event::OrderRejected {
                order_id: OrderId::new(2003),
                user_id: UserId::new(52),
                reason: RejectReason::FokNotFillable,
                timestamp: sample_timestamp(),
            },
            Event::OrderCanceled {
                order_id: OrderId::new(1001),
                user_id: UserId::new(42),
                symbol: SymbolId::new(1),
                remaining_qty: dec("0.5"),
                timestamp: sample_timestamp(),
            },
            Event::OrderFilled {
                order_id: OrderId::new(1001),
                trade_id: TradeId::new(5001),
                user_id: UserId::new(42),
                symbol: SymbolId::new(1),
                side: Side::Buy,
                fill_price: dec("50000.5"),
                fill_qty: dec("0.75"),
                is_maker: true,
                remaining_qty: dec("0.5"),
                timestamp: sample_timestamp(),
            },
            Event::OrderFilled {
                order_id: OrderId::new(1002),
                trade_id: TradeId::new(5002),
                user_id: UserId::new(43),
                symbol: SymbolId::new(1),
                side: Side::Sell,
                fill_price: dec("50000.5"),
                fill_qty: dec("0.75"),
                is_maker: false,
                remaining_qty: Decimal128::ZERO,
                timestamp: sample_timestamp(),
            },
            Event::TradeExecuted {
                trade_id: TradeId::new(5001),
                symbol: SymbolId::new(1),
                price: dec("50000.5"),
                qty: dec("0.75"),
                buyer_order_id: OrderId::new(1001),
                seller_order_id: OrderId::new(1002),
                buyer_user_id: UserId::new(42),
                seller_user_id: UserId::new(43),
                buyer_fee: dec("0.0075"),
                seller_fee: dec("0.015"),
                timestamp: sample_timestamp(),
            },
            Event::MarkPriceUpdate {
                symbol: SymbolId::new(1),
                mark_price: dec("50001.23"),
                index_price: dec("50000.89"),
                timestamp: sample_timestamp(),
            },
            Event::FundingRateUpdate {
                symbol: SymbolId::new(1),
                funding_rate: dec("0.0001"),
                timestamp: sample_timestamp(),
            },
            Event::LiquidationOrder {
                user_id: UserId::new(99),
                symbol: SymbolId::new(1),
                side: Side::Sell,
                quantity: dec("5.0"),
                timestamp: sample_timestamp(),
            },
        ]
    }

    // ── Helper: all RejectReason variants ─────────────────────────────

    fn all_reject_reasons() -> Vec<RejectReason> {
        vec![
            RejectReason::InsufficientMargin,
            RejectReason::PositionLimitExceeded,
            RejectReason::PriceOutOfBand,
            RejectReason::SelfTradePrevented,
            RejectReason::RateLimitExceeded,
            RejectReason::PostOnlyWouldTake,
            RejectReason::FokNotFillable,
            RejectReason::SymbolSuspended,
            RejectReason::MarkPriceStale,
            RejectReason::InvalidOrder,
            RejectReason::DuplicateOrder,
            RejectReason::OrderNotFound,
        ]
    }

    // ── Serde JSON roundtrip ──────────────────────────────────────────

    #[test]
    fn test_command_serde_roundtrip() {
        for (i, cmd) in all_commands().into_iter().enumerate() {
            let json = serde_json::to_string(&cmd)
                .unwrap_or_else(|e| panic!("serialize Command[{i}] failed: {e}"));
            let deserialized: Command = serde_json::from_str(&json)
                .unwrap_or_else(|e| panic!("deserialize Command[{i}] failed: {e}\njson: {json}"));
            assert_eq!(cmd, deserialized, "Command[{i}] roundtrip mismatch");
        }
    }

    #[test]
    fn test_event_serde_roundtrip() {
        for (i, evt) in all_events().into_iter().enumerate() {
            let json = serde_json::to_string(&evt)
                .unwrap_or_else(|e| panic!("serialize Event[{i}] failed: {e}"));
            let deserialized: Event = serde_json::from_str(&json)
                .unwrap_or_else(|e| panic!("deserialize Event[{i}] failed: {e}\njson: {json}"));
            assert_eq!(evt, deserialized, "Event[{i}] roundtrip mismatch");
        }
    }

    #[test]
    fn test_reject_reason_serde_roundtrip() {
        for (i, reason) in all_reject_reasons().into_iter().enumerate() {
            let json = serde_json::to_string(&reason)
                .unwrap_or_else(|e| panic!("serialize RejectReason[{i}] failed: {e}"));
            let deserialized: RejectReason = serde_json::from_str(&json).unwrap_or_else(|e| {
                panic!("deserialize RejectReason[{i}] failed: {e}\njson: {json}")
            });
            assert_eq!(reason, deserialized, "RejectReason[{i}] roundtrip mismatch");
        }
    }

    // ── rkyv roundtrip ────────────────────────────────────────────────

    #[test]
    fn test_command_rkyv_roundtrip() {
        for (i, cmd) in all_commands().into_iter().enumerate() {
            let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&cmd)
                .unwrap_or_else(|e| panic!("rkyv serialize Command[{i}] failed: {e}"));
            let deserialized: Command = rkyv::from_bytes::<Command, rkyv::rancor::Error>(&bytes)
                .unwrap_or_else(|e| panic!("rkyv deserialize Command[{i}] failed: {e}"));
            assert_eq!(cmd, deserialized, "Command[{i}] rkyv roundtrip mismatch");
        }
    }

    #[test]
    fn test_event_rkyv_roundtrip() {
        for (i, evt) in all_events().into_iter().enumerate() {
            let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&evt)
                .unwrap_or_else(|e| panic!("rkyv serialize Event[{i}] failed: {e}"));
            let deserialized: Event = rkyv::from_bytes::<Event, rkyv::rancor::Error>(&bytes)
                .unwrap_or_else(|e| panic!("rkyv deserialize Event[{i}] failed: {e}"));
            assert_eq!(evt, deserialized, "Event[{i}] rkyv roundtrip mismatch");
        }
    }

    #[test]
    fn test_reject_reason_rkyv_roundtrip() {
        for (i, reason) in all_reject_reasons().into_iter().enumerate() {
            let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&reason)
                .unwrap_or_else(|e| panic!("rkyv serialize RejectReason[{i}] failed: {e}"));
            let deserialized: RejectReason =
                rkyv::from_bytes::<RejectReason, rkyv::rancor::Error>(&bytes)
                    .unwrap_or_else(|e| panic!("rkyv deserialize RejectReason[{i}] failed: {e}"));
            assert_eq!(
                reason, deserialized,
                "RejectReason[{i}] rkyv roundtrip mismatch"
            );
        }
    }

    // ── Field-level verification ──────────────────────────────────────

    #[test]
    fn test_new_order_fields_survive_serde() {
        let cmd = Command::NewOrder {
            order_id: OrderId::new(777),
            user_id: UserId::new(88),
            symbol: SymbolId::new(5),
            side: Side::Sell,
            order_type: OrderType::StopMarket,
            time_in_force: TimeInForce::Fok,
            price: None,
            quantity: dec("99.99"),
            stop_price: Some(dec("48000")),
            trailing_delta: None,
            visible_quantity: None,
            reduce_only: true,
            margin_mode: MarginMode::Isolated,
            leverage: Some(dec("50")),
            client_order_id: Some(12345),
            timestamp: UnixMicros::from_micros(1_234_567_890),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        let rt: Command = serde_json::from_str(&json).unwrap();
        if let Command::NewOrder {
            order_id,
            user_id,
            symbol,
            side,
            order_type,
            time_in_force,
            price,
            quantity,
            stop_price,
            trailing_delta,
            visible_quantity,
            reduce_only,
            margin_mode,
            leverage,
            client_order_id,
            timestamp,
        } = rt
        {
            assert_eq!(order_id, OrderId::new(777));
            assert_eq!(user_id, UserId::new(88));
            assert_eq!(symbol, SymbolId::new(5));
            assert_eq!(side, Side::Sell);
            assert_eq!(order_type, OrderType::StopMarket);
            assert_eq!(time_in_force, TimeInForce::Fok);
            assert_eq!(price, None);
            assert_eq!(quantity, dec("99.99"));
            assert_eq!(stop_price, Some(dec("48000")));
            assert_eq!(trailing_delta, None);
            assert_eq!(visible_quantity, None);
            assert!(reduce_only);
            assert_eq!(margin_mode, MarginMode::Isolated);
            assert_eq!(leverage, Some(dec("50")));
            assert_eq!(client_order_id, Some(12345));
            assert_eq!(timestamp, UnixMicros::from_micros(1_234_567_890));
        } else {
            panic!("expected NewOrder variant");
        }
    }

    #[test]
    fn test_trade_executed_fields_survive_rkyv() {
        let evt = Event::TradeExecuted {
            trade_id: TradeId::new(9001),
            symbol: SymbolId::new(3),
            price: dec("61234.567"),
            qty: dec("0.001"),
            buyer_order_id: OrderId::new(100),
            seller_order_id: OrderId::new(200),
            buyer_user_id: UserId::new(10),
            seller_user_id: UserId::new(20),
            buyer_fee: dec("0.00001"),
            seller_fee: dec("0.00002"),
            timestamp: UnixMicros::from_micros(9_999_999),
        };
        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&evt).unwrap();
        let rt: Event = rkyv::from_bytes::<Event, rkyv::rancor::Error>(&bytes).unwrap();
        if let Event::TradeExecuted {
            trade_id,
            symbol,
            price,
            qty,
            buyer_order_id,
            seller_order_id,
            buyer_user_id,
            seller_user_id,
            buyer_fee,
            seller_fee,
            timestamp,
        } = rt
        {
            assert_eq!(trade_id, TradeId::new(9001));
            assert_eq!(symbol, SymbolId::new(3));
            assert_eq!(price, dec("61234.567"));
            assert_eq!(qty, dec("0.001"));
            assert_eq!(buyer_order_id, OrderId::new(100));
            assert_eq!(seller_order_id, OrderId::new(200));
            assert_eq!(buyer_user_id, UserId::new(10));
            assert_eq!(seller_user_id, UserId::new(20));
            assert_eq!(buyer_fee, dec("0.00001"));
            assert_eq!(seller_fee, dec("0.00002"));
            assert_eq!(timestamp, UnixMicros::from_micros(9_999_999));
        } else {
            panic!("expected TradeExecuted variant");
        }
    }

    #[test]
    fn test_order_filled_fields_survive_serde() {
        let evt = Event::OrderFilled {
            order_id: OrderId::new(555),
            trade_id: TradeId::new(6001),
            user_id: UserId::new(77),
            symbol: SymbolId::new(2),
            side: Side::Buy,
            fill_price: dec("3200.125"),
            fill_qty: dec("10"),
            is_maker: false,
            remaining_qty: dec("5"),
            timestamp: sample_timestamp(),
        };
        let json = serde_json::to_string(&evt).unwrap();
        let rt: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(evt, rt);
    }

    #[test]
    fn test_liquidation_order_roundtrip() {
        let evt = Event::LiquidationOrder {
            user_id: UserId::new(101),
            symbol: SymbolId::new(1),
            side: Side::Sell,
            quantity: dec("50"),
            timestamp: sample_timestamp(),
        };
        // serde
        let json = serde_json::to_string(&evt).unwrap();
        let rt_serde: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(evt, rt_serde);
        // rkyv
        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&evt).unwrap();
        let rt_rkyv: Event = rkyv::from_bytes::<Event, rkyv::rancor::Error>(&bytes).unwrap();
        assert_eq!(evt, rt_rkyv);
    }

    #[test]
    fn test_mark_price_update_roundtrip() {
        let evt = Event::MarkPriceUpdate {
            symbol: SymbolId::new(7),
            mark_price: dec("100000"),
            index_price: dec("99999.99"),
            timestamp: sample_timestamp(),
        };
        let json = serde_json::to_string(&evt).unwrap();
        let rt: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(evt, rt);
    }

    #[test]
    fn test_funding_rate_update_roundtrip() {
        let evt = Event::FundingRateUpdate {
            symbol: SymbolId::new(1),
            funding_rate: dec("-0.0005"),
            timestamp: sample_timestamp(),
        };
        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&evt).unwrap();
        let rt: Event = rkyv::from_bytes::<Event, rkyv::rancor::Error>(&bytes).unwrap();
        assert_eq!(evt, rt);
    }

    #[test]
    fn test_cancel_all_orders_roundtrip() {
        let cmd = Command::CancelAllOrders {
            user_id: UserId::new(42),
            symbol: SymbolId::new(1),
            timestamp: sample_timestamp(),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        let rt: Command = serde_json::from_str(&json).unwrap();
        assert_eq!(cmd, rt);
    }

    #[test]
    fn test_amend_order_partial_fields() {
        // Only new_price, no new_quantity
        let cmd = Command::AmendOrder {
            order_id: OrderId::new(300),
            user_id: UserId::new(60),
            symbol: SymbolId::new(1),
            new_price: Some(dec("55000")),
            new_quantity: None,
            timestamp: sample_timestamp(),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        let rt: Command = serde_json::from_str(&json).unwrap();
        assert_eq!(cmd, rt);
    }
}
