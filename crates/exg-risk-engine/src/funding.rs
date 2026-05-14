use exg_common::{Decimal128, PositionSide};

/// Impact mid price from orderbook snapshot.
///
/// Calculates the VWAP for both bid and ask sides up to `impact_notional`,
/// then returns their average. Returns `None` if either side has insufficient
/// liquidity to fill `impact_notional`.
pub fn calc_impact_mid_price(
    bids: &[(Decimal128, Decimal128)],
    asks: &[(Decimal128, Decimal128)],
    impact_notional: Decimal128,
) -> Option<Decimal128> {
    let impact_bid = calc_impact_price(bids, impact_notional)?;
    let impact_ask = calc_impact_price(asks, impact_notional)?;
    let two = Decimal128::from(2i64);
    Some((impact_bid + impact_ask) / two)
}

/// VWAP price to fill `target_notional` worth of orders from a sorted price level list.
///
/// Uses weighted-sum approach to avoid large intermediate divisions.
fn calc_impact_price(
    levels: &[(Decimal128, Decimal128)],
    target_notional: Decimal128,
) -> Option<Decimal128> {
    let mut remaining_notional = target_notional;
    let mut weighted_sum = Decimal128::ZERO;
    let mut total_qty = Decimal128::ZERO;

    for &(price, qty) in levels {
        let level_notional = price * qty;
        if level_notional >= remaining_notional {
            // This level has enough to fill the rest.
            let fill_qty = remaining_notional / price;
            weighted_sum = weighted_sum + fill_qty * price;
            total_qty = total_qty + fill_qty;
            // VWAP = weighted_sum / total_qty
            // But weighted_sum == target_notional by construction,
            // so VWAP = target_notional / total_qty.
            // Use weighted_sum / total_qty to keep values smaller.
            return Some(weighted_sum / total_qty);
        }
        remaining_notional = remaining_notional - level_notional;
        weighted_sum = weighted_sum + qty * price;
        total_qty = total_qty + qty;
    }
    // Insufficient liquidity.
    None
}

/// Premium index = (impact_mid - index_price) / index_price.
pub fn calc_premium_index(impact_mid: Decimal128, index_price: Decimal128) -> Decimal128 {
    (impact_mid - index_price) / index_price
}

/// Funding rate = clamp(premium_index + interest_rate, -0.75%, +0.75%).
///
/// Default interest_rate is 0.01% (0.0001).
pub fn calc_funding_rate(premium_index: Decimal128, interest_rate: Decimal128) -> Decimal128 {
    let raw_rate = premium_index + interest_rate;
    let lower: Decimal128 = "-0.0075".parse().unwrap();
    let upper: Decimal128 = "0.0075".parse().unwrap();
    raw_rate.max(lower).min(upper)
}

/// Funding fee = position_notional * funding_rate.
///
/// For long positions: positive funding_rate means long pays (positive fee).
/// For short positions: positive funding_rate means short receives (negative fee).
///
/// Convention: positive return = user pays, negative = user receives.
pub fn calc_funding_fee(
    position_size: Decimal128,
    mark_price: Decimal128,
    funding_rate: Decimal128,
    side: PositionSide,
) -> Decimal128 {
    let notional = position_size * mark_price;
    let fee = notional * funding_rate;
    match side {
        PositionSide::Long => fee,
        PositionSide::Short => -fee,
        PositionSide::Both => fee, // caller should resolve direction
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dec(s: &str) -> Decimal128 {
        s.parse().unwrap()
    }

    // ── Funding Rate ────────────────────────────────────────────────

    #[test]
    fn test_funding_rate_positive_premium() {
        let premium = dec("0.001"); // 0.1%
        let interest = dec("0.0001"); // 0.01%
        let rate = calc_funding_rate(premium, interest);
        assert_eq!(rate, dec("0.0011"));
    }

    #[test]
    fn test_funding_rate_negative_premium() {
        let premium = dec("-0.002");
        let interest = dec("0.0001");
        let rate = calc_funding_rate(premium, interest);
        assert_eq!(rate, dec("-0.0019"));
    }

    #[test]
    fn test_funding_rate_clamped_upper() {
        let premium = dec("0.01"); // 1% — exceeds 0.75% cap
        let interest = dec("0.0001");
        let rate = calc_funding_rate(premium, interest);
        assert_eq!(rate, dec("0.0075"));
    }

    #[test]
    fn test_funding_rate_clamped_lower() {
        let premium = dec("-0.01");
        let interest = dec("0.0001");
        let rate = calc_funding_rate(premium, interest);
        assert_eq!(rate, dec("-0.0075"));
    }

    // ── Funding Fee ─────────────────────────────────────────────────

    #[test]
    fn test_funding_fee_long_pays() {
        // size=1, mark=50000, rate=0.001
        // notional=50000, fee=50
        // long pays positive
        let fee = calc_funding_fee(dec("1"), dec("50000"), dec("0.001"), PositionSide::Long);
        assert_eq!(fee, dec("50"));
    }

    #[test]
    fn test_funding_fee_short_receives() {
        // When rate is positive, short receives (negative fee for short).
        let fee = calc_funding_fee(dec("1"), dec("50000"), dec("0.001"), PositionSide::Short);
        assert_eq!(fee, dec("-50"));
    }

    #[test]
    fn test_funding_fee_negative_rate_long_receives() {
        // When rate is negative, long receives (negative fee).
        let fee = calc_funding_fee(dec("2"), dec("30000"), dec("-0.0005"), PositionSide::Long);
        // notional = 60000, fee = 60000 * -0.0005 = -30
        assert_eq!(fee, dec("-30"));
    }

    #[test]
    fn test_funding_fee_negative_rate_short_pays() {
        let fee = calc_funding_fee(dec("2"), dec("30000"), dec("-0.0005"), PositionSide::Short);
        // notional=60000, raw_fee=-30, short negates => 30
        assert_eq!(fee, dec("30"));
    }

    // ── Impact Mid Price ────────────────────────────────────────────

    #[test]
    fn test_impact_mid_price_normal() {
        // Use smaller values to avoid Decimal128 overflow in intermediate calculations.
        let bids = vec![
            (dec("100"), dec("10")), // 1000
            (dec("99"), dec("20")),  // 1980
        ];
        let asks = vec![
            (dec("101"), dec("10")), // 1010
            (dec("102"), dec("20")), // 2040
        ];
        let impact_notional = dec("1000");
        let result = calc_impact_mid_price(&bids, &asks, impact_notional);
        assert!(result.is_some());

        // Bid side: 1000 notional. First level: 100*10=1000, exactly fills.
        //   fill_qty = 1000/100 = 10, total=10, VWAP = 1000/10 = 100
        // Ask side: 1000 notional. First level: 101*10=1010 >= 1000.
        //   fill_qty = 1000/101 ≈ 9.900990.., total ≈ 9.900990..
        //   VWAP = 1000/9.900990.. = 101
        // mid = (100 + 101) / 2 = 100.5
        let mid = result.unwrap();
        assert_eq!(mid, dec("100.5"));
    }

    #[test]
    fn test_impact_mid_price_thin_book() {
        let bids = vec![(dec("100"), dec("0.001"))]; // only 0.1 notional
        let asks = vec![(dec("101"), dec("0.001"))];
        let impact_notional = dec("1000");
        // Insufficient liquidity
        assert!(calc_impact_mid_price(&bids, &asks, impact_notional).is_none());
    }

    #[test]
    fn test_impact_mid_price_partial_fill_across_levels() {
        let bids = vec![
            (dec("100"), dec("5")), // 500
            (dec("99"), dec("10")), // 990
        ];
        let asks = vec![
            (dec("101"), dec("5")),  // 505
            (dec("102"), dec("10")), // 1020
        ];
        let impact_notional = dec("1000");

        let result = calc_impact_mid_price(&bids, &asks, impact_notional);
        assert!(result.is_some());

        // Bid: level1=500 (qty=5), remaining=500.
        //   level2: fill_qty=500/99, weighted_sum = 500 + (500/99)*99 = 500+500 = 1000
        //   total_qty = 5 + 500/99 ≈ 10.0505..
        //   VWAP = 1000/10.0505.. ≈ 99.4975..
        // Ask: level1=505 (qty=5), remaining=495.
        //   level2: fill_qty=495/102, weighted_sum = 505 + (495/102)*102 = 505+495 = 1000
        //   total_qty = 5 + 495/102 ≈ 9.8529..
        //   VWAP = 1000/9.8529.. ≈ 101.4924..
        // mid ≈ (99.4975 + 101.4924) / 2 ≈ 100.495
        let mid = result.unwrap();
        assert!(mid > dec("100") && mid < dec("101"));
    }

    // ── Premium Index ───────────────────────────────────────────────

    #[test]
    fn test_premium_index() {
        let impact_mid = dec("50100");
        let index_price = dec("50000");
        // (50100 - 50000) / 50000 = 100/50000 = 0.002
        let result = calc_premium_index(impact_mid, index_price);
        assert_eq!(result, dec("0.002"));
    }
}
