use exg_common::{Decimal128, PositionSide, Side, SymbolId, UserId};
use exg_risk_engine::{MarginTier, Position, margin::calc_margin_ratio};

/// Result of a risk scan containing actions to take.
pub struct RiskMonitorResult {
    /// Users whose orders should be canceled (margin call).
    pub cancel_all_users: Vec<(UserId, SymbolId)>,
    /// Users whose positions should be liquidated.
    pub liquidation_orders: Vec<LiquidationRequest>,
}

/// A request to liquidate a user's position.
pub struct LiquidationRequest {
    pub user_id: UserId,
    pub symbol: SymbolId,
    pub side: Side,
    pub quantity: Decimal128,
}

/// Pure-logic risk monitor — no async runtime, driven externally.
///
/// Scans positions against mark prices and wallet balances to determine
/// which users need margin calls or liquidations.
pub struct RiskMonitor {
    margin_call_threshold: Decimal128,
    liquidation_threshold: Decimal128,
}

impl RiskMonitor {
    pub fn new(margin_call_threshold: Decimal128, liquidation_threshold: Decimal128) -> Self {
        Self {
            margin_call_threshold,
            liquidation_threshold,
        }
    }

    /// Scan all positions against current mark prices and account balances.
    ///
    /// For each user, computes margin ratio. If ratio >= margin_call_threshold
    /// but < liquidation_threshold, the user's orders should be canceled.
    /// If ratio >= liquidation_threshold, the user's position should be liquidated.
    pub fn scan_positions(
        &self,
        positions: &[&Position],
        wallet_balances: &[(UserId, Decimal128)],
        mark_prices: &[(SymbolId, Decimal128)],
        tiers: &[MarginTier],
    ) -> RiskMonitorResult {
        use rustc_hash::FxHashMap;

        // Build lookup maps.
        let balance_map: FxHashMap<UserId, Decimal128> = wallet_balances.iter().copied().collect();
        let price_map: FxHashMap<SymbolId, Decimal128> = mark_prices.iter().copied().collect();

        // Group positions by user, updating unrealized PnL with current mark prices.
        let mut user_positions: FxHashMap<UserId, Vec<Position>> = FxHashMap::default();
        for &pos in positions {
            let mut p = pos.clone();
            if let Some(&mark) = price_map.get(&p.symbol) {
                p.unrealized_pnl = exg_risk_engine::margin::calc_unrealized_pnl(
                    p.entry_price,
                    mark,
                    p.size,
                    p.side,
                );
            }
            user_positions.entry(p.user_id).or_default().push(p);
        }

        let mut cancel_all_users = Vec::new();
        let mut liquidation_orders = Vec::new();

        for (user_id, user_pos) in &user_positions {
            let wallet_balance = balance_map
                .get(user_id)
                .copied()
                .unwrap_or(Decimal128::ZERO);
            let ratio = calc_margin_ratio(user_pos, wallet_balance, tiers);

            if ratio >= self.liquidation_threshold {
                // Liquidation: cancel orders + force-close positions.
                for p in user_pos {
                    cancel_all_users.push((*user_id, p.symbol));
                    let side = match p.side {
                        PositionSide::Long | PositionSide::Both => Side::Sell,
                        PositionSide::Short => Side::Buy,
                    };
                    liquidation_orders.push(LiquidationRequest {
                        user_id: *user_id,
                        symbol: p.symbol,
                        side,
                        quantity: p.size,
                    });
                }
            } else if ratio >= self.margin_call_threshold {
                // Margin call: cancel open orders only.
                for p in user_pos {
                    cancel_all_users.push((*user_id, p.symbol));
                }
            }
        }

        RiskMonitorResult {
            cancel_all_users,
            liquidation_orders,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use exg_common::{Decimal128, MarginMode, PositionSide, SymbolId, UserId};
    use exg_risk_engine::{MarginTier, Position};

    fn dec(s: &str) -> Decimal128 {
        s.parse().unwrap()
    }

    fn make_tier() -> Vec<MarginTier> {
        vec![MarginTier {
            notional_floor: dec("0"),
            notional_cap: dec("100000000"),
            maintenance_margin_rate: dec("0.004"),
            maintenance_amount: dec("0"),
        }]
    }

    fn make_position(
        user_id: u64,
        symbol: u16,
        side: PositionSide,
        size: &str,
        entry: &str,
        margin: &str,
    ) -> Position {
        Position {
            user_id: UserId::new(user_id),
            symbol: SymbolId::new(symbol),
            side,
            size: dec(size),
            entry_price: dec(entry),
            leverage: dec("10"),
            margin: dec(margin),
            unrealized_pnl: Decimal128::ZERO,
            accumulated_funding: Decimal128::ZERO,
            margin_mode: MarginMode::Cross,
        }
    }

    #[test]
    fn test_healthy_position() {
        let monitor = RiskMonitor::new(dec("0.8"), dec("1.0"));
        let tiers = make_tier();

        // notional = 1 * 50000 = 50000, mm = 50000 * 0.004 = 200
        // Mark price same as entry => upnl = 0, equity = 10000 + 0 = 10000
        // ratio = 200 / 10000 = 0.02 — well below 0.8
        let pos = make_position(1, 1, PositionSide::Long, "1", "50000", "5000");
        let result = monitor.scan_positions(
            &[&pos],
            &[(UserId::new(1), dec("10000"))],
            &[(SymbolId::new(1), dec("50000"))],
            &tiers,
        );

        assert!(result.cancel_all_users.is_empty());
        assert!(result.liquidation_orders.is_empty());
    }

    #[test]
    fn test_margin_call() {
        let monitor = RiskMonitor::new(dec("0.8"), dec("1.0"));
        let tiers = make_tier();

        // notional = 1 * 50000 = 50000, mm = 200
        // Mark at 41000 => upnl = (41000-50000)*1 = -9000
        // equity = 260 + (-9000) = -8740 => negative => ratio = MAX
        // That would be liquidation. Let's adjust.
        //
        // We need ratio between 0.8 and 1.0.
        // ratio = mm / equity. mm = 200.
        // 0.8 <= 200/equity < 1.0 => 200 < equity <= 250.
        // equity = wallet + upnl. wallet = 10000.
        // upnl = (mark - 50000) * 1. equity = 10000 + mark - 50000 = mark - 40000.
        // Need 200 < mark - 40000 <= 250 => 40200 < mark <= 40250.
        let pos = make_position(1, 1, PositionSide::Long, "1", "50000", "5000");
        let result = monitor.scan_positions(
            &[&pos],
            &[(UserId::new(1), dec("10000"))],
            &[(SymbolId::new(1), dec("40220"))],
            &tiers,
        );

        // upnl = (40220 - 50000) = -9780, equity = 10000 - 9780 = 220
        // ratio = 200 / 220 ≈ 0.909 => >= 0.8 and < 1.0 => margin call
        assert_eq!(result.cancel_all_users.len(), 1);
        assert_eq!(result.cancel_all_users[0].0, UserId::new(1));
        assert!(result.liquidation_orders.is_empty());
    }

    #[test]
    fn test_liquidation_trigger() {
        let monitor = RiskMonitor::new(dec("0.8"), dec("1.0"));
        let tiers = make_tier();

        // ratio >= 1.0 means mm / equity >= 1.0 => equity <= mm = 200.
        // equity = 10000 + (mark - 50000). Need equity <= 200 => mark <= 40200.
        let pos = make_position(1, 1, PositionSide::Long, "1", "50000", "5000");
        let result = monitor.scan_positions(
            &[&pos],
            &[(UserId::new(1), dec("10000"))],
            &[(SymbolId::new(1), dec("40100"))],
            &tiers,
        );

        // upnl = -9900, equity = 100, ratio = 200/100 = 2.0 >= 1.0
        assert_eq!(result.cancel_all_users.len(), 1);
        assert_eq!(result.liquidation_orders.len(), 1);
        assert_eq!(result.liquidation_orders[0].user_id, UserId::new(1));
        assert_eq!(result.liquidation_orders[0].side, Side::Sell);
        assert_eq!(result.liquidation_orders[0].quantity, dec("1"));
    }
}
