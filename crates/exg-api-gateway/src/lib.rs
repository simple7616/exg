pub mod conversion;
pub mod error;
pub mod middleware;
pub mod types;
pub mod ws;

pub use conversion::*;
pub use error::{ApiError, ERR_INSUFFICIENT_BALANCE, ERR_INVALID_PARAMETER, ERR_ORDER_NOT_FOUND, ERR_TOO_MANY_REQUESTS, ERR_UNAUTHORIZED, ERR_UNKNOWN};
pub use middleware::{ApiKeyAuth, RateLimiter, validate_timestamp};
pub use types::*;
pub use ws::{SubscriptionManager, WsRequest, WsResponse, parse_stream_name};

#[cfg(test)]
mod tests {
    use super::*;
    use exg_common::{OrderId, OrderType, Side, SymbolId, TimeInForce, UnixMicros, UserId};
    use exg_protocol::Command;

    // ── Conversion tests ─────────────────────────────────────────────────

    #[test]
    fn test_place_order_limit_buy_to_command() {
        let req = PlaceOrderRequest {
            symbol: "BTCUSDT".to_owned(),
            side: "BUY".to_owned(),
            order_type: "LIMIT".to_owned(),
            time_in_force: Some("GTC".to_owned()),
            quantity: "1.5".to_owned(),
            price: Some("50000".to_owned()),
            stop_price: None,
            reduce_only: None,
            client_order_id: Some("12345".to_owned()),
        };

        let user_id = UserId::new(42);
        let order_id = OrderId::new(1001);
        let ts = UnixMicros::from_micros(1_700_000_000_000_000);

        let cmd = to_new_order_command(&req, user_id, order_id, ts).unwrap();

        match cmd {
            Command::NewOrder {
                order_id: oid,
                user_id: uid,
                side,
                order_type,
                time_in_force,
                price,
                quantity,
                reduce_only,
                client_order_id,
                ..
            } => {
                assert_eq!(oid, OrderId::new(1001));
                assert_eq!(uid, UserId::new(42));
                assert_eq!(side, Side::Buy);
                assert_eq!(order_type, OrderType::Limit);
                assert_eq!(time_in_force, TimeInForce::Gtc);
                assert!(price.is_some());
                assert_eq!(quantity.to_string(), "1.5");
                assert!(!reduce_only);
                assert_eq!(client_order_id, Some(12345));
            }
            _ => panic!("Expected NewOrder command"),
        }
    }

    #[test]
    fn test_place_order_market_sell_to_command() {
        let req = PlaceOrderRequest {
            symbol: "ETHUSDT".to_owned(),
            side: "SELL".to_owned(),
            order_type: "MARKET".to_owned(),
            time_in_force: None,
            quantity: "10".to_owned(),
            price: None,
            stop_price: None,
            reduce_only: Some(true),
            client_order_id: None,
        };

        let cmd = to_new_order_command(
            &req,
            UserId::new(1),
            OrderId::new(2),
            UnixMicros::from_micros(0),
        )
        .unwrap();

        match cmd {
            Command::NewOrder {
                side,
                order_type,
                time_in_force,
                price,
                reduce_only,
                client_order_id,
                ..
            } => {
                assert_eq!(side, Side::Sell);
                assert_eq!(order_type, OrderType::Market);
                assert_eq!(time_in_force, TimeInForce::Ioc); // default for market
                assert!(price.is_none());
                assert!(reduce_only);
                assert_eq!(client_order_id, None);
            }
            _ => panic!("Expected NewOrder command"),
        }
    }

    #[test]
    fn test_invalid_side_string() {
        let result = string_to_side("INVALID");
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_order_type_string() {
        let result = string_to_order_type("INVALID");
        assert!(result.is_err());
    }

    #[test]
    fn test_missing_price_for_limit_order() {
        let req = PlaceOrderRequest {
            symbol: "BTCUSDT".to_owned(),
            side: "BUY".to_owned(),
            order_type: "LIMIT".to_owned(),
            time_in_force: Some("GTC".to_owned()),
            quantity: "1".to_owned(),
            price: None, // missing!
            stop_price: None,
            reduce_only: None,
            client_order_id: None,
        };

        let result = to_new_order_command(
            &req,
            UserId::new(1),
            OrderId::new(1),
            UnixMicros::from_micros(0),
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.msg.contains("price"), "Error should mention price: {}", err.msg);
    }

    // ── Rate limiter tests ───────────────────────────────────────────────

    #[test]
    fn test_rate_limiter_under_limit_allowed() {
        let mut limiter = RateLimiter::new(10, 1.0);
        let now = UnixMicros::from_micros(1_000_000);
        // Fresh bucket has 10 tokens, consuming should succeed.
        for _ in 0..10 {
            assert!(limiter.consume("key1", now));
        }
    }

    #[test]
    fn test_rate_limiter_over_limit_rejected() {
        let mut limiter = RateLimiter::new(3, 1.0);
        let now = UnixMicros::from_micros(1_000_000);
        assert!(limiter.consume("key1", now));
        assert!(limiter.consume("key1", now));
        assert!(limiter.consume("key1", now));
        // 4th request should be rejected
        assert!(!limiter.consume("key1", now));
    }

    #[test]
    fn test_rate_limiter_tokens_refill_over_time() {
        let mut limiter = RateLimiter::new(2, 1.0); // 1 token/sec refill
        let t0 = UnixMicros::from_micros(1_000_000);

        // Drain all tokens
        assert!(limiter.consume("key1", t0));
        assert!(limiter.consume("key1", t0));
        assert!(!limiter.consume("key1", t0));

        // Advance 2 seconds => should refill 2 tokens
        let t1 = UnixMicros::from_micros(3_000_000);
        assert!(limiter.consume("key1", t1));
        assert!(limiter.consume("key1", t1));
        assert!(!limiter.consume("key1", t1));
    }

    #[test]
    fn test_rate_limiter_separate_buckets() {
        let mut limiter = RateLimiter::new(1, 1.0);
        let now = UnixMicros::from_micros(1_000_000);

        assert!(limiter.consume("key_a", now));
        assert!(!limiter.consume("key_a", now));

        // Different key should have its own bucket
        assert!(limiter.consume("key_b", now));
        assert!(!limiter.consume("key_b", now));
    }

    // ── Timestamp validation tests ───────────────────────────────────────

    #[test]
    fn test_timestamp_within_window() {
        assert!(validate_timestamp(1000, 1005, 10));
    }

    #[test]
    fn test_timestamp_outside_window() {
        assert!(!validate_timestamp(1000, 1020, 10));
    }

    #[test]
    fn test_timestamp_future_within_window() {
        // Request is 5ms ahead of server, within 10ms window
        assert!(validate_timestamp(1010, 1005, 10));
    }

    // ── WebSocket tests ──────────────────────────────────────────────────

    #[test]
    fn test_parse_subscribe_request() {
        let json = r#"{"method":"SUBSCRIBE","params":["btcusdt@depth20","ethusdt@trade"],"id":1}"#;
        let req: WsRequest = serde_json::from_str(json).unwrap();
        match req {
            WsRequest::Subscribe { params, id } => {
                assert_eq!(id, 1);
                assert_eq!(params.len(), 2);
                assert_eq!(params[0], "btcusdt@depth20");
                assert_eq!(params[1], "ethusdt@trade");
            }
            _ => panic!("Expected Subscribe"),
        }
    }

    #[test]
    fn test_parse_unsubscribe_request() {
        let json = r#"{"method":"UNSUBSCRIBE","params":["btcusdt@depth20"],"id":2}"#;
        let req: WsRequest = serde_json::from_str(json).unwrap();
        match req {
            WsRequest::Unsubscribe { params, id } => {
                assert_eq!(id, 2);
                assert_eq!(params.len(), 1);
                assert_eq!(params[0], "btcusdt@depth20");
            }
            _ => panic!("Expected Unsubscribe"),
        }
    }

    #[test]
    fn test_subscription_manager_subscribe_and_get_clients() {
        let mut mgr = SubscriptionManager::new();

        let newly = mgr.subscribe(1, &["btcusdt@depth20".to_owned(), "ethusdt@trade".to_owned()]);
        assert_eq!(newly.len(), 2);

        // Re-subscribing should not return duplicates
        let newly2 = mgr.subscribe(1, &["btcusdt@depth20".to_owned()]);
        assert!(newly2.is_empty());

        // Client 2 subscribes to same stream
        mgr.subscribe(2, &["btcusdt@depth20".to_owned()]);

        let clients = mgr.get_clients_for_stream("btcusdt@depth20").unwrap();
        assert!(clients.contains(&1));
        assert!(clients.contains(&2));

        let streams = mgr.get_client_streams(1).unwrap();
        assert!(streams.contains("btcusdt@depth20"));
        assert!(streams.contains("ethusdt@trade"));
    }

    #[test]
    fn test_subscription_manager_unsubscribe_and_remove_client() {
        let mut mgr = SubscriptionManager::new();
        mgr.subscribe(1, &["btcusdt@depth20".to_owned(), "ethusdt@trade".to_owned()]);
        mgr.subscribe(2, &["btcusdt@depth20".to_owned()]);

        // Unsubscribe client 1 from one stream
        let removed = mgr.unsubscribe(1, &["btcusdt@depth20".to_owned()]);
        assert_eq!(removed.len(), 1);

        // Client 2 should still be there
        let clients = mgr.get_clients_for_stream("btcusdt@depth20").unwrap();
        assert!(clients.contains(&2));
        assert!(!clients.contains(&1));

        // Remove client 2 entirely
        mgr.remove_client(2);
        assert!(mgr.get_clients_for_stream("btcusdt@depth20").is_none());
        assert!(mgr.get_client_streams(2).is_none());
    }

    #[test]
    fn test_parse_stream_name_valid() {
        let result = parse_stream_name("btcusdt@depth20");
        assert_eq!(result, Some(("btcusdt".to_owned(), "depth20".to_owned())));
    }

    #[test]
    fn test_parse_stream_name_invalid() {
        assert_eq!(parse_stream_name("invalid"), None);
        assert_eq!(parse_stream_name("@channel"), None);
        assert_eq!(parse_stream_name("symbol@"), None);
    }

    // ── Cancel / Amend conversion tests ─────────────────────────────────

    #[test]
    fn to_cancel_order_command_happy() {
        let req = CancelOrderRequest {
            order_id: 12345,
            symbol: "BTCUSDT".into(),
        };
        let cmd = to_cancel_order_command(
            &req,
            UserId::new(42),
            SymbolId::new(1),
            UnixMicros::from_micros(1),
        )
        .unwrap();
        match cmd {
            Command::CancelOrder { order_id, user_id, symbol, .. } => {
                assert_eq!(order_id, OrderId::new(12345));
                assert_eq!(user_id, UserId::new(42));
                assert_eq!(symbol, SymbolId::new(1));
            }
            _ => panic!("expected CancelOrder"),
        }
    }

    #[test]
    fn to_amend_order_command_happy_price_only() {
        let req = AmendOrderRequest {
            order_id: 99,
            symbol: "BTCUSDT".into(),
            new_price: Some("60500".into()),
            new_quantity: None,
        };
        let cmd = to_amend_order_command(
            &req,
            UserId::new(7),
            SymbolId::new(1),
            UnixMicros::from_micros(2),
        )
        .unwrap();
        match cmd {
            Command::AmendOrder { order_id, new_price, new_quantity, .. } => {
                assert_eq!(order_id, OrderId::new(99));
                assert!(new_price.is_some());
                assert!(new_quantity.is_none());
            }
            _ => panic!("expected AmendOrder"),
        }
    }

    #[test]
    fn to_amend_order_command_rejects_empty_amend() {
        let req = AmendOrderRequest {
            order_id: 99,
            symbol: "BTCUSDT".into(),
            new_price: None,
            new_quantity: None,
        };
        let err = to_amend_order_command(
            &req,
            UserId::new(7),
            SymbolId::new(1),
            UnixMicros::from_micros(2),
        )
        .unwrap_err();
        assert!(err.msg.contains("at least one of"), "msg: {}", err.msg);
    }

    // ── API error tests ──────────────────────────────────────────────────

    #[test]
    fn test_api_error_serialization() {
        let err = ApiError::bad_request("Invalid parameter");
        let json = serde_json::to_string(&err).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["code"], error::ERR_INVALID_PARAMETER);
        assert_eq!(value["msg"], "Invalid parameter");

        let err2 = ApiError::rate_limited();
        let json2 = serde_json::to_string(&err2).unwrap();
        let value2: serde_json::Value = serde_json::from_str(&json2).unwrap();
        assert_eq!(value2["code"], error::ERR_TOO_MANY_REQUESTS);
    }
}
