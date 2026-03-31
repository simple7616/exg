use exg_common::{Decimal128, ExgError, ExgResult, UserId};

use crate::{OrderInfo, RateLimitConfig, RateLimitState};

/// Check whether the new order's resulting position would exceed the max notional limit.
pub fn check_position_limit(
    positions: &[crate::Position],
    order: &OrderInfo,
    max_position_notional: Decimal128,
) -> ExgResult<()> {
    let order_notional = order.price * order.quantity;

    // Sum existing notional for the same symbol on the same side.
    let existing_notional: Decimal128 = positions
        .iter()
        .filter(|p| p.symbol == order.symbol)
        .map(|p| p.size * p.entry_price)
        .sum();

    let total = existing_notional + order_notional;
    if total > max_position_notional {
        return Err(ExgError::PositionLimitExceeded(order.symbol));
    }
    Ok(())
}

/// Check whether the order price is within the allowed band around mark price.
///
/// For buy: order_price <= mark_price * (1 + band_pct)
/// For sell: order_price >= mark_price * (1 - band_pct)
/// Simplified: |order_price - mark_price| / mark_price <= band_pct
pub fn check_price_band(
    order_price: Decimal128,
    mark_price: Decimal128,
    band_pct: Decimal128,
) -> ExgResult<()> {
    let diff = (order_price - mark_price).abs();
    let threshold = mark_price * band_pct;

    if diff > threshold {
        return Err(ExgError::PriceOutOfBand {
            order_price: order_price.to_string(),
            mark_price: mark_price.to_string(),
        });
    }
    Ok(())
}

/// Check whether the incoming order would self-trade with an existing order
/// from the same user on the opposite side for the same symbol.
pub fn check_self_trade(
    order: &OrderInfo,
    existing_orders: &[OrderInfo],
) -> ExgResult<()> {
    let opposite_side = order.side.opposite();
    for existing in existing_orders {
        if existing.user_id == order.user_id
            && existing.symbol == order.symbol
            && existing.side == opposite_side
        {
            return Err(ExgError::SelfTradePrevented(order.user_id));
        }
    }
    Ok(())
}

/// Check rate limits for orders.
pub fn check_rate_limit(
    state: &RateLimitState,
    config: &RateLimitConfig,
) -> ExgResult<()> {
    if state.orders_in_window >= config.max_orders_per_second {
        return Err(ExgError::RateLimitExceeded(UserId::new(0)));
    }
    if state.cancels_in_window >= config.max_cancels_per_second {
        return Err(ExgError::RateLimitExceeded(UserId::new(0)));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use exg_common::{Decimal128, MarginMode, OrderId, OrderType, PositionSide, Side, SymbolId, UserId};
    use crate::Position;

    fn dec(s: &str) -> Decimal128 {
        s.parse().unwrap()
    }

    // ── Position Limit ──────────────────────────────────────────────

    #[test]
    fn test_position_limit_pass() {
        let order = OrderInfo {
            order_id: OrderId::new(1),
            user_id: UserId::new(1),
            symbol: SymbolId::new(1),
            side: Side::Buy,
            price: dec("50000"),
            quantity: dec("1"),
            order_type: OrderType::Limit,
        };
        let result = check_position_limit(&[], &order, dec("1000000"));
        assert!(result.is_ok());
    }

    #[test]
    fn test_position_limit_fail() {
        let positions = vec![Position {
            user_id: UserId::new(1),
            symbol: SymbolId::new(1),
            side: PositionSide::Long,
            size: dec("10"),
            entry_price: dec("50000"),
            leverage: dec("10"),
            margin: dec("50000"),
            unrealized_pnl: dec("0"),
            accumulated_funding: dec("0"),
            margin_mode: MarginMode::Cross,
        }];
        let order = OrderInfo {
            order_id: OrderId::new(2),
            user_id: UserId::new(1),
            symbol: SymbolId::new(1),
            side: Side::Buy,
            price: dec("50000"),
            quantity: dec("10"),
            order_type: OrderType::Limit,
        };
        // existing=500000, new=500000, total=1000000 > 999999
        let result = check_position_limit(&positions, &order, dec("999999"));
        assert!(result.is_err());
        assert!(matches!(result, Err(ExgError::PositionLimitExceeded(_))));
    }

    // ── Price Band ──────────────────────────────────────────────────

    #[test]
    fn test_price_band_within() {
        // mark=50000, band=5%, order=52000 → diff=2000, threshold=2500 → ok
        let result = check_price_band(dec("52000"), dec("50000"), dec("0.05"));
        assert!(result.is_ok());
    }

    #[test]
    fn test_price_band_exactly_at_boundary() {
        // mark=50000, band=5%, order=52500 → diff=2500, threshold=2500 → ok (<=)
        let result = check_price_band(dec("52500"), dec("50000"), dec("0.05"));
        assert!(result.is_ok());
    }

    #[test]
    fn test_price_band_outside() {
        // mark=50000, band=5%, order=53000 → diff=3000, threshold=2500 → fail
        let result = check_price_band(dec("53000"), dec("50000"), dec("0.05"));
        assert!(result.is_err());
        assert!(matches!(result, Err(ExgError::PriceOutOfBand { .. })));
    }

    #[test]
    fn test_price_band_below() {
        // mark=50000, band=5%, order=47000 → diff=3000, threshold=2500 → fail
        let result = check_price_band(dec("47000"), dec("50000"), dec("0.05"));
        assert!(result.is_err());
    }

    // ── Self Trade ──────────────────────────────────────────────────

    #[test]
    fn test_self_trade_detected() {
        let order = OrderInfo {
            order_id: OrderId::new(2),
            user_id: UserId::new(1),
            symbol: SymbolId::new(1),
            side: Side::Buy,
            price: dec("50000"),
            quantity: dec("1"),
            order_type: OrderType::Limit,
        };
        let existing = vec![OrderInfo {
            order_id: OrderId::new(1),
            user_id: UserId::new(1),
            symbol: SymbolId::new(1),
            side: Side::Sell,
            price: dec("50000"),
            quantity: dec("1"),
            order_type: OrderType::Limit,
        }];
        let result = check_self_trade(&order, &existing);
        assert!(result.is_err());
        assert!(matches!(result, Err(ExgError::SelfTradePrevented(_))));
    }

    #[test]
    fn test_self_trade_different_user() {
        let order = OrderInfo {
            order_id: OrderId::new(2),
            user_id: UserId::new(1),
            symbol: SymbolId::new(1),
            side: Side::Buy,
            price: dec("50000"),
            quantity: dec("1"),
            order_type: OrderType::Limit,
        };
        let existing = vec![OrderInfo {
            order_id: OrderId::new(1),
            user_id: UserId::new(2),
            symbol: SymbolId::new(1),
            side: Side::Sell,
            price: dec("50000"),
            quantity: dec("1"),
            order_type: OrderType::Limit,
        }];
        assert!(check_self_trade(&order, &existing).is_ok());
    }

    #[test]
    fn test_self_trade_same_side() {
        let order = OrderInfo {
            order_id: OrderId::new(2),
            user_id: UserId::new(1),
            symbol: SymbolId::new(1),
            side: Side::Buy,
            price: dec("50000"),
            quantity: dec("1"),
            order_type: OrderType::Limit,
        };
        let existing = vec![OrderInfo {
            order_id: OrderId::new(1),
            user_id: UserId::new(1),
            symbol: SymbolId::new(1),
            side: Side::Buy,
            price: dec("49000"),
            quantity: dec("1"),
            order_type: OrderType::Limit,
        }];
        // Same side — not a self-trade
        assert!(check_self_trade(&order, &existing).is_ok());
    }

    // ── Rate Limit ──────────────────────────────────────────────────

    #[test]
    fn test_rate_limit_pass() {
        let state = RateLimitState {
            orders_in_window: 5,
            cancels_in_window: 3,
        };
        let config = RateLimitConfig {
            max_orders_per_second: 10,
            max_cancels_per_second: 10,
        };
        assert!(check_rate_limit(&state, &config).is_ok());
    }

    #[test]
    fn test_rate_limit_orders_exceeded() {
        let state = RateLimitState {
            orders_in_window: 10,
            cancels_in_window: 0,
        };
        let config = RateLimitConfig {
            max_orders_per_second: 10,
            max_cancels_per_second: 10,
        };
        let result = check_rate_limit(&state, &config);
        assert!(result.is_err());
        assert!(matches!(result, Err(ExgError::RateLimitExceeded(_))));
    }

    #[test]
    fn test_rate_limit_cancels_exceeded() {
        let state = RateLimitState {
            orders_in_window: 0,
            cancels_in_window: 11,
        };
        let config = RateLimitConfig {
            max_orders_per_second: 10,
            max_cancels_per_second: 10,
        };
        assert!(check_rate_limit(&state, &config).is_err());
    }
}
