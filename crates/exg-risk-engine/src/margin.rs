use exg_common::{Decimal128, ExgError, ExgResult, PositionSide};

use crate::{MarginTier, Position};

/// Initial margin = notional / leverage.
pub fn calc_initial_margin(notional: Decimal128, leverage: Decimal128) -> Decimal128 {
    notional / leverage
}

/// Maintenance margin using tiered rate schedule.
///
/// For each tier: maintenance_margin = notional * mmr - maintenance_amount.
/// The tier is selected by finding the one where notional_floor <= notional < notional_cap.
pub fn calc_maintenance_margin(notional: Decimal128, tiers: &[MarginTier]) -> Decimal128 {
    for tier in tiers {
        if notional >= tier.notional_floor && notional < tier.notional_cap {
            return notional * tier.maintenance_margin_rate - tier.maintenance_amount;
        }
    }
    // If notional exceeds all tiers, use the last tier.
    if let Some(tier) = tiers.last()
        && notional >= tier.notional_floor
    {
        return notional * tier.maintenance_margin_rate - tier.maintenance_amount;
    }
    Decimal128::ZERO
}

/// Liquidation price calculation.
///
/// For long:  liq_price = entry_price * (1 - 1/leverage + mmr) - accumulated_funding / size
/// For short: liq_price = entry_price * (1 + 1/leverage - mmr) + accumulated_funding / size
pub fn calc_liquidation_price(
    entry_price: Decimal128,
    leverage: Decimal128,
    mmr: Decimal128,
    side: PositionSide,
    size: Decimal128,
    accumulated_funding: Decimal128,
) -> Decimal128 {
    let one = Decimal128::ONE;
    let inv_leverage = one / leverage;
    let funding_per_unit = accumulated_funding / size;

    match side {
        PositionSide::Long => {
            entry_price * (one - inv_leverage + mmr) - funding_per_unit
        }
        PositionSide::Short => {
            entry_price * (one + inv_leverage - mmr) + funding_per_unit
        }
        PositionSide::Both => {
            // For one-way mode, treat as long if positive. Caller should resolve.
            entry_price * (one - inv_leverage + mmr) - funding_per_unit
        }
    }
}

/// Margin ratio = total_maintenance_margin / (wallet_balance + total_unrealized_pnl).
///
/// Returns Decimal128::MAX if equity is zero or negative (effectively infinite risk).
pub fn calc_margin_ratio(
    positions: &[Position],
    wallet_balance: Decimal128,
    tiers: &[MarginTier],
) -> Decimal128 {
    let total_unrealized_pnl: Decimal128 = positions.iter().map(|p| p.unrealized_pnl).sum();
    let equity = wallet_balance + total_unrealized_pnl;

    if !equity.is_positive() {
        return Decimal128::MAX;
    }

    let total_maintenance_margin: Decimal128 = positions
        .iter()
        .map(|p| {
            let notional = p.size * p.entry_price;
            calc_maintenance_margin(notional, tiers)
        })
        .sum();

    total_maintenance_margin / equity
}

/// Unrealized PnL for a position given current mark price.
///
/// Long:  (mark_price - entry_price) * size
/// Short: (entry_price - mark_price) * size
pub fn calc_unrealized_pnl(
    entry_price: Decimal128,
    mark_price: Decimal128,
    size: Decimal128,
    side: PositionSide,
) -> Decimal128 {
    match side {
        PositionSide::Long | PositionSide::Both => (mark_price - entry_price) * size,
        PositionSide::Short => (entry_price - mark_price) * size,
    }
}

/// Check margin sufficiency for a new order.
///
/// Required margin = order_notional / leverage (uses max_leverage from config).
/// The order is rejected if available_balance < required_margin.
pub fn check_margin_sufficient(
    account: &crate::Account,
    order: &crate::OrderInfo,
    _positions: &[Position],
    config: &crate::SymbolConfig,
) -> ExgResult<()> {
    let order_notional = order.price * order.quantity;
    let required_margin = calc_initial_margin(order_notional, config.max_leverage);

    if account.available_balance < required_margin {
        return Err(ExgError::InsufficientMargin {
            required: required_margin.to_string(),
            available: account.available_balance.to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use exg_common::{Decimal128, MarginMode, OrderId, OrderType, PositionSide, Side, SymbolId, UserId};
    use crate::{Account, MarginTier, OrderInfo, Position, SymbolConfig};

    fn dec(s: &str) -> Decimal128 {
        s.parse().unwrap()
    }

    fn binance_btc_tiers() -> Vec<MarginTier> {
        vec![
            MarginTier {
                notional_floor: dec("0"),
                notional_cap: dec("50000"),
                maintenance_margin_rate: dec("0.004"),
                maintenance_amount: dec("0"),
            },
            MarginTier {
                notional_floor: dec("50000"),
                notional_cap: dec("250000"),
                maintenance_margin_rate: dec("0.005"),
                maintenance_amount: dec("50"),
            },
            MarginTier {
                notional_floor: dec("250000"),
                notional_cap: dec("1000000"),
                maintenance_margin_rate: dec("0.01"),
                maintenance_amount: dec("1300"),
            },
            MarginTier {
                notional_floor: dec("1000000"),
                notional_cap: dec("10000000"),
                maintenance_margin_rate: dec("0.025"),
                maintenance_amount: dec("16300"),
            },
            MarginTier {
                notional_floor: dec("10000000"),
                notional_cap: dec("50000000"),
                maintenance_margin_rate: dec("0.05"),
                maintenance_amount: dec("266300"),
            },
        ]
    }

    // ── Initial Margin ──────────────────────────────────────────────

    #[test]
    fn test_initial_margin_basic() {
        let result = calc_initial_margin(dec("10000"), dec("10"));
        assert_eq!(result, dec("1000"));
    }

    #[test]
    fn test_initial_margin_high_leverage() {
        let result = calc_initial_margin(dec("50000"), dec("125"));
        assert_eq!(result, dec("400"));
    }

    // ── Maintenance Margin with Tiers ───────────────────────────────

    #[test]
    fn test_maintenance_margin_tier1() {
        let tiers = binance_btc_tiers();
        // 10000 * 0.004 - 0 = 40
        let result = calc_maintenance_margin(dec("10000"), &tiers);
        assert_eq!(result, dec("40"));
    }

    #[test]
    fn test_maintenance_margin_tier2() {
        let tiers = binance_btc_tiers();
        // 100000 * 0.005 - 50 = 450
        let result = calc_maintenance_margin(dec("100000"), &tiers);
        assert_eq!(result, dec("450"));
    }

    #[test]
    fn test_maintenance_margin_tier3() {
        let tiers = binance_btc_tiers();
        // 500000 * 0.01 - 1300 = 3700
        let result = calc_maintenance_margin(dec("500000"), &tiers);
        assert_eq!(result, dec("3700"));
    }

    #[test]
    fn test_maintenance_margin_last_tier_exceeded() {
        let tiers = binance_btc_tiers();
        // 60000000 exceeds last tier cap but should use last tier
        // 60000000 * 0.05 - 266300 = 2733700
        let result = calc_maintenance_margin(dec("60000000"), &tiers);
        assert_eq!(result, dec("2733700"));
    }

    // ── Liquidation Price ───────────────────────────────────────────

    #[test]
    fn test_liquidation_price_long() {
        // entry=50000, leverage=10, mmr=0.004, size=1, funding=0
        // liq = 50000 * (1 - 0.1 + 0.004) - 0 = 50000 * 0.904 = 45200
        let result = calc_liquidation_price(
            dec("50000"),
            dec("10"),
            dec("0.004"),
            PositionSide::Long,
            dec("1"),
            dec("0"),
        );
        assert_eq!(result, dec("45200"));
    }

    #[test]
    fn test_liquidation_price_short() {
        // entry=50000, leverage=10, mmr=0.004, size=1, funding=0
        // liq = 50000 * (1 + 0.1 - 0.004) + 0 = 50000 * 1.096 = 54800
        let result = calc_liquidation_price(
            dec("50000"),
            dec("10"),
            dec("0.004"),
            PositionSide::Short,
            dec("1"),
            dec("0"),
        );
        assert_eq!(result, dec("54800"));
    }

    #[test]
    fn test_liquidation_price_with_funding() {
        // entry=50000, leverage=20, mmr=0.004, size=2, funding=100
        // For long: liq = 50000 * (1 - 0.05 + 0.004) - 100/2
        //         = 50000 * 0.954 - 50
        //         = 47700 - 50 = 47650
        let result = calc_liquidation_price(
            dec("50000"),
            dec("20"),
            dec("0.004"),
            PositionSide::Long,
            dec("2"),
            dec("100"),
        );
        assert_eq!(result, dec("47650"));
    }

    // ── Margin Ratio ────────────────────────────────────────────────

    #[test]
    fn test_margin_ratio_normal() {
        let tiers = binance_btc_tiers();
        let positions = vec![Position {
            user_id: UserId::new(1),
            symbol: SymbolId::new(1),
            side: PositionSide::Long,
            size: dec("1"),
            entry_price: dec("30000"),
            leverage: dec("10"),
            margin: dec("3000"),
            unrealized_pnl: dec("500"),
            accumulated_funding: dec("0"),
            margin_mode: MarginMode::Cross,
        }];
        // notional = 1 * 30000 = 30000
        // mm = 30000 * 0.004 - 0 = 120
        // equity = 10000 + 500 = 10500
        // ratio = 120 / 10500 ≈ 0.011428...
        let result = calc_margin_ratio(&positions, dec("10000"), &tiers);
        // 120 / 10500 = 0.011428571428571428
        let expected = dec("120") / dec("10500");
        assert_eq!(result, expected);
    }

    #[test]
    fn test_margin_ratio_near_liquidation() {
        let tiers = binance_btc_tiers();
        let positions = vec![Position {
            user_id: UserId::new(1),
            symbol: SymbolId::new(1),
            side: PositionSide::Long,
            size: dec("1"),
            entry_price: dec("30000"),
            leverage: dec("10"),
            margin: dec("3000"),
            unrealized_pnl: dec("-2800"),
            accumulated_funding: dec("0"),
            margin_mode: MarginMode::Cross,
        }];
        // mm = 120, equity = 10000 + (-2800) = 7200
        // ratio = 120 / 7200 = 0.016666...
        let result = calc_margin_ratio(&positions, dec("10000"), &tiers);
        let expected = dec("120") / dec("7200");
        assert_eq!(result, expected);
    }

    #[test]
    fn test_margin_ratio_underwater() {
        let tiers = binance_btc_tiers();
        let positions = vec![Position {
            user_id: UserId::new(1),
            symbol: SymbolId::new(1),
            side: PositionSide::Long,
            size: dec("1"),
            entry_price: dec("30000"),
            leverage: dec("10"),
            margin: dec("3000"),
            unrealized_pnl: dec("-11000"),
            accumulated_funding: dec("0"),
            margin_mode: MarginMode::Cross,
        }];
        // equity = 10000 + (-11000) = -1000 (negative)
        let result = calc_margin_ratio(&positions, dec("10000"), &tiers);
        assert_eq!(result, Decimal128::MAX);
    }

    // ── Unrealized PnL ──────────────────────────────────────────────

    #[test]
    fn test_unrealized_pnl_long_profit() {
        let result = calc_unrealized_pnl(dec("50000"), dec("55000"), dec("2"), PositionSide::Long);
        // (55000 - 50000) * 2 = 10000
        assert_eq!(result, dec("10000"));
    }

    #[test]
    fn test_unrealized_pnl_long_loss() {
        let result = calc_unrealized_pnl(dec("50000"), dec("48000"), dec("2"), PositionSide::Long);
        // (48000 - 50000) * 2 = -4000
        assert_eq!(result, dec("-4000"));
    }

    #[test]
    fn test_unrealized_pnl_short_profit() {
        let result = calc_unrealized_pnl(dec("50000"), dec("45000"), dec("3"), PositionSide::Short);
        // (50000 - 45000) * 3 = 15000
        assert_eq!(result, dec("15000"));
    }

    #[test]
    fn test_unrealized_pnl_short_loss() {
        let result = calc_unrealized_pnl(dec("50000"), dec("52000"), dec("3"), PositionSide::Short);
        // (50000 - 52000) * 3 = -6000
        assert_eq!(result, dec("-6000"));
    }

    // ── Margin Sufficient Check ─────────────────────────────────────

    #[test]
    fn test_check_margin_sufficient_pass() {
        let account = Account {
            user_id: UserId::new(1),
            wallet_balance: dec("10000"),
            available_balance: dec("5000"),
            frozen_balance: dec("5000"),
        };
        let order = OrderInfo {
            order_id: OrderId::new(1),
            user_id: UserId::new(1),
            symbol: SymbolId::new(1),
            side: Side::Buy,
            price: dec("50000"),
            quantity: dec("0.1"),
            order_type: OrderType::Limit,
        };
        let config = SymbolConfig {
            symbol: SymbolId::new(1),
            tick_size: dec("0.01"),
            lot_size: dec("0.001"),
            min_notional: dec("10"),
            max_leverage: dec("10"),
            maker_fee: dec("0.0002"),
            taker_fee: dec("0.0004"),
            margin_tiers: vec![],
        };
        // notional = 50000 * 0.1 = 5000, required = 5000/10 = 500
        // available = 5000 >= 500
        assert!(check_margin_sufficient(&account, &order, &[], &config).is_ok());
    }

    #[test]
    fn test_check_margin_sufficient_fail() {
        let account = Account {
            user_id: UserId::new(1),
            wallet_balance: dec("100"),
            available_balance: dec("100"),
            frozen_balance: dec("0"),
        };
        let order = OrderInfo {
            order_id: OrderId::new(1),
            user_id: UserId::new(1),
            symbol: SymbolId::new(1),
            side: Side::Buy,
            price: dec("50000"),
            quantity: dec("1"),
            order_type: OrderType::Limit,
        };
        let config = SymbolConfig {
            symbol: SymbolId::new(1),
            tick_size: dec("0.01"),
            lot_size: dec("0.001"),
            min_notional: dec("10"),
            max_leverage: dec("10"),
            maker_fee: dec("0.0002"),
            taker_fee: dec("0.0004"),
            margin_tiers: vec![],
        };
        // notional = 50000, required = 5000, available = 100
        let result = check_margin_sufficient(&account, &order, &[], &config);
        assert!(result.is_err());
        assert!(matches!(result, Err(ExgError::InsufficientMargin { .. })));
    }
}
