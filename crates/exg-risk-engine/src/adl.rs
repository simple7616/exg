use exg_common::{Decimal128, UserId};

use crate::Position;

/// Calculate ADL priority ranking.
///
/// ADL score = pnl_pct * leverage_factor
///   pnl_pct = unrealized_pnl / margin
///   leverage_factor = abs(notional) / margin
///
/// Returns a sorted vec of (user_id, adl_score) in descending order of score.
/// Higher score = higher priority for auto-deleveraging.
pub fn calc_adl_ranking(
    positions: &[Position],
    mark_price: Decimal128,
) -> Vec<(UserId, Decimal128)> {
    let mut scores: Vec<(UserId, Decimal128)> = positions
        .iter()
        .filter(|p| !p.margin.is_zero())
        .map(|p| {
            let unrealized_pnl =
                crate::margin::calc_unrealized_pnl(p.entry_price, mark_price, p.size, p.side);
            let notional = p.size * mark_price;
            // ADL score = (upnl / margin) * (notional / margin)
            //           = (upnl * notional) / (margin * margin)
            // Restructure to avoid overflow: compute in two steps with smaller intermediates.
            let margin_sq = p.margin * p.margin;
            let score = unrealized_pnl * notional / margin_sq;
            (p.user_id, score)
        })
        .collect();

    // Sort descending by score.
    scores.sort_by(|a, b| b.1.cmp(&a.1));
    scores
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Position;
    use exg_common::{Decimal128, MarginMode, PositionSide, SymbolId, UserId};

    fn dec(s: &str) -> Decimal128 {
        s.parse().unwrap()
    }

    #[test]
    fn test_decimal_division_sanity() {
        // Verify that divisions that trigger the wide path work correctly.
        let a = dec("55000");
        let b = dec("5000");
        let result = a / b;
        assert_eq!(result, dec("11"), "55000/5000 should be 11, got {result}");

        let c = dec("45000");
        let result2 = c / b;
        assert_eq!(result2, dec("9"), "45000/5000 should be 9, got {result2}");
    }

    #[test]
    fn test_adl_ranking_basic() {
        let mark_price = dec("55000");
        let positions = vec![
            // User 1: Long, entry=50000, size=1, margin=5000
            // upnl = (55000-50000)*1 = 5000
            // pnl% = 5000/5000 = 1.0
            // leverage_factor = 55000/5000 = 11
            // score = 1.0 * 11 = 11
            Position {
                user_id: UserId::new(1),
                symbol: SymbolId::new(1),
                side: PositionSide::Long,
                size: dec("1"),
                entry_price: dec("50000"),
                leverage: dec("10"),
                margin: dec("5000"),
                unrealized_pnl: dec("5000"),
                accumulated_funding: dec("0"),
                margin_mode: MarginMode::Cross,
            },
            // User 2: Long, entry=54000, size=1, margin=5400
            // upnl = (55000-54000)*1 = 1000
            // pnl% = 1000/5400 ≈ 0.18518...
            // leverage_factor = 55000/5400 ≈ 10.18518...
            // score ≈ 0.18518 * 10.18518 ≈ 1.88614...
            Position {
                user_id: UserId::new(2),
                symbol: SymbolId::new(1),
                side: PositionSide::Long,
                size: dec("1"),
                entry_price: dec("54000"),
                leverage: dec("10"),
                margin: dec("5400"),
                unrealized_pnl: dec("1000"),
                accumulated_funding: dec("0"),
                margin_mode: MarginMode::Cross,
            },
            // User 3: Short, entry=60000, size=1, margin=6000
            // upnl = (60000-55000)*1 = 5000
            // pnl% = 5000/6000 ≈ 0.833...
            // leverage_factor = 55000/6000 ≈ 9.1666...
            // score ≈ 0.833 * 9.1666 ≈ 7.638...
            Position {
                user_id: UserId::new(3),
                symbol: SymbolId::new(1),
                side: PositionSide::Short,
                size: dec("1"),
                entry_price: dec("60000"),
                leverage: dec("10"),
                margin: dec("6000"),
                unrealized_pnl: dec("5000"),
                accumulated_funding: dec("0"),
                margin_mode: MarginMode::Cross,
            },
        ];

        let ranking = calc_adl_ranking(&positions, mark_price);

        assert_eq!(ranking.len(), 3);
        // User 1 should be first (highest score = 11)
        assert_eq!(ranking[0].0, UserId::new(1));
        // User 3 should be second (score ≈ 7.638)
        assert_eq!(ranking[1].0, UserId::new(3));
        // User 2 should be last (score ≈ 1.886)
        assert_eq!(ranking[2].0, UserId::new(2));

        // Verify User 1's exact score: pnl%=1, lev=11 => 11
        assert_eq!(ranking[0].1, dec("11"));
    }

    #[test]
    fn test_adl_ranking_negative_pnl() {
        let mark_price = dec("45000");
        let positions = vec![
            // User 1: Long losing money
            // upnl = (45000-50000)*1 = -5000
            // pnl% = -5000/5000 = -1
            // leverage_factor = 45000/5000 = 9
            // score = -9
            Position {
                user_id: UserId::new(1),
                symbol: SymbolId::new(1),
                side: PositionSide::Long,
                size: dec("1"),
                entry_price: dec("50000"),
                leverage: dec("10"),
                margin: dec("5000"),
                unrealized_pnl: dec("-5000"),
                accumulated_funding: dec("0"),
                margin_mode: MarginMode::Cross,
            },
            // User 2: Short making money
            // upnl = (50000-45000)*1 = 5000
            // pnl% = 5000/5000 = 1
            // leverage_factor = 45000/5000 = 9
            // score = 9
            Position {
                user_id: UserId::new(2),
                symbol: SymbolId::new(1),
                side: PositionSide::Short,
                size: dec("1"),
                entry_price: dec("50000"),
                leverage: dec("10"),
                margin: dec("5000"),
                unrealized_pnl: dec("5000"),
                accumulated_funding: dec("0"),
                margin_mode: MarginMode::Cross,
            },
        ];

        let ranking = calc_adl_ranking(&positions, mark_price);

        // User 2 (score=9) first, User 1 (score=-9) second
        assert_eq!(ranking[0].0, UserId::new(2));
        assert_eq!(ranking[0].1, dec("9"));
        assert_eq!(ranking[1].0, UserId::new(1));
        assert_eq!(ranking[1].1, dec("-9"));
    }
}
