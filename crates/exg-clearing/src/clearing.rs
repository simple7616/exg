use exg_common::{
    Decimal128, ExgError, ExgResult, MarginMode, PositionSide, SymbolId, TradeId, UnixMicros,
    UserId,
};
use exg_ledger::{Ledger, WalletType};
use exg_risk_engine::{MarginTier, Position, SymbolConfig, funding::calc_funding_fee};
use serde::{Deserialize, Serialize};

use crate::position::PositionManager;

/// Helper struct for trade processing.
pub struct TradeInfo {
    pub trade_id: TradeId,
    pub symbol: SymbolId,
    pub price: Decimal128,
    pub qty: Decimal128,
    pub buyer_user_id: UserId,
    pub seller_user_id: UserId,
    pub buyer_fee: Decimal128,
    pub seller_fee: Decimal128,
    pub buyer_leverage: Decimal128,
    pub seller_leverage: Decimal128,
    pub buyer_margin_mode: MarginMode,
    pub seller_margin_mode: MarginMode,
    pub timestamp: UnixMicros,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FundingResult {
    pub total_long_payment: Decimal128,
    pub total_short_payment: Decimal128,
    pub users_needing_liquidation_check: Vec<UserId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClearingSnapshot {
    pub positions: Vec<Position>,
    // Ledger snapshot is opaque — we serialize the journal + accounts via ledger methods.
}

pub struct ClearingService {
    pub position_manager: PositionManager,
    pub ledger: Ledger,
}

impl ClearingService {
    pub fn new() -> Self {
        Self {
            position_manager: PositionManager::new(),
            ledger: Ledger::new(),
        }
    }

    /// Process a trade. For each side:
    /// 1. Determine if opening or closing
    /// 2. Update position
    /// 3. Execute ledger entries
    pub fn process_trade(
        &mut self,
        trade: &TradeInfo,
        _symbol_config: &SymbolConfig,
    ) -> ExgResult<()> {
        let trade_id_str = trade.trade_id.to_string();

        // Process buyer side (buyer is going long).
        self.process_trade_side(
            trade.buyer_user_id,
            trade.seller_user_id,
            trade.symbol,
            PositionSide::Long,
            trade.qty,
            trade.price,
            trade.buyer_fee,
            trade.buyer_leverage,
            trade.buyer_margin_mode,
            &format!("trade-{trade_id_str}-buyer"),
            trade.timestamp,
        )?;

        // Process seller side (seller is going short).
        self.process_trade_side(
            trade.seller_user_id,
            trade.buyer_user_id,
            trade.symbol,
            PositionSide::Short,
            trade.qty,
            trade.price,
            trade.seller_fee,
            trade.seller_leverage,
            trade.seller_margin_mode,
            &format!("trade-{trade_id_str}-seller"),
            trade.timestamp,
        )?;

        Ok(())
    }

    /// Process one side of a trade.
    ///
    /// `intended_side` is Long for buyer, Short for seller.
    /// If the user has an existing position in the opposite direction, reduce/close first,
    /// then open the remainder in the intended direction.
    #[allow(clippy::too_many_arguments)]
    fn process_trade_side(
        &mut self,
        user_id: UserId,
        _counterparty_id: UserId,
        symbol: SymbolId,
        intended_side: PositionSide,
        qty: Decimal128,
        price: Decimal128,
        fee: Decimal128,
        leverage: Decimal128,
        margin_mode: MarginMode,
        idempotency_prefix: &str,
        timestamp: UnixMicros,
    ) -> ExgResult<()> {
        let existing = self.position_manager.get_position(user_id, symbol);

        let opposite_side = match intended_side {
            PositionSide::Long => PositionSide::Short,
            PositionSide::Short => PositionSide::Long,
            PositionSide::Both => PositionSide::Both,
        };

        // Check if user has a position in the opposite direction (closing scenario).
        let (close_qty, open_qty) = if let Some(pos) = existing {
            if pos.side == opposite_side {
                // Closing / flipping.
                let close_qty = qty.min(pos.size);
                let open_qty = qty - close_qty;
                (close_qty, open_qty)
            } else {
                // Same direction — pure open/increase.
                (Decimal128::ZERO, qty)
            }
        } else {
            // No existing position — pure open.
            (Decimal128::ZERO, qty)
        };

        // Step 1: Close/reduce opposite position if needed.
        if close_qty.is_positive() {
            let margin_released = self
                .position_manager
                .calc_margin_released(user_id, symbol, close_qty)
                .unwrap_or(Decimal128::ZERO);

            let (realized_pnl, _remaining) = self
                .position_manager
                .reduce_or_close(user_id, symbol, close_qty, price)?;

            // Fee is proportionally split: close gets close_qty/qty share.
            let close_fee = if qty.is_positive() {
                fee * close_qty / qty
            } else {
                Decimal128::ZERO
            };

            self.ledger.close_position_settled(
                user_id,
                margin_released,
                realized_pnl,
                close_fee,
                &format!("{idempotency_prefix}-close"),
                timestamp,
            )?;
        }

        // Step 2: Open/increase position in intended direction if remainder.
        if open_qty.is_positive() {
            let margin_amount = (open_qty * price) / leverage;
            let open_fee = if close_qty.is_positive() && qty.is_positive() {
                fee * open_qty / qty
            } else {
                fee
            };

            // Ledger: frozen -> margin + fee.
            self.ledger.open_position(
                user_id,
                margin_amount,
                open_fee,
                &format!("{idempotency_prefix}-open"),
                timestamp,
            )?;

            self.position_manager.open_or_increase(
                user_id,
                symbol,
                intended_side,
                open_qty,
                price,
                leverage,
                margin_mode,
            );
        }

        Ok(())
    }

    /// Process a liquidation.
    ///
    /// 1. Force close the position
    /// 2. Calculate surplus/deficit
    /// 3. Execute ledger entries
    ///
    /// Returns surplus (positive) or deficit (negative).
    pub fn process_liquidation(
        &mut self,
        user_id: UserId,
        symbol: SymbolId,
        exit_price: Decimal128,
        _symbol_config: &SymbolConfig,
    ) -> ExgResult<Decimal128> {
        let position = self
            .position_manager
            .force_close(user_id, symbol)
            .ok_or_else(|| {
                ExgError::Internal(format!(
                    "no position to liquidate for user {user_id} symbol {symbol}"
                ))
            })?;

        // Calculate realized PnL.
        let realized_pnl = match position.side {
            PositionSide::Long | PositionSide::Both => {
                (exit_price - position.entry_price) * position.size
            }
            PositionSide::Short => (position.entry_price - exit_price) * position.size,
        };

        // surplus = margin + realized_pnl
        // If positive: leftover margin after covering loss goes to insurance fund.
        // If negative: loss exceeds margin, insurance fund covers deficit.
        let surplus = position.margin + realized_pnl;

        self.ledger.liquidate(
            user_id,
            position.margin,
            surplus,
            &format!("liq-{user_id}-{symbol}"),
            UnixMicros::now(),
        )?;

        Ok(surplus)
    }

    /// Settle funding payments for all positions of a given symbol.
    ///
    /// For each position:
    /// 1. Calculate funding fee via risk-engine
    /// 2. Settle via ledger (deduct from available, then margin if insufficient)
    /// 3. Track accumulated funding in position
    pub fn settle_funding(
        &mut self,
        symbol: SymbolId,
        funding_rate: Decimal128,
        mark_price: Decimal128,
        funding_period_id: u64,
        timestamp: UnixMicros,
    ) -> ExgResult<FundingResult> {
        let mut total_long_payment = Decimal128::ZERO;
        let mut total_short_payment = Decimal128::ZERO;
        let mut users_needing_liquidation_check = Vec::new();

        // Collect position data first to avoid borrowing conflicts.
        let position_data: Vec<(UserId, SymbolId, Decimal128, PositionSide, Decimal128)> = self
            .position_manager
            .all_positions()
            .filter(|p| p.symbol == symbol)
            .map(|p| (p.user_id, p.symbol, p.size, p.side, p.margin))
            .collect();

        for (user_id, sym, size, side, margin_before) in &position_data {
            let user_id = *user_id;
            let side = *side;

            let payment = calc_funding_fee(*size, mark_price, funding_rate, side);

            if payment.is_zero() {
                continue;
            }

            let idemp_key = format!("funding_{funding_period_id}_{user_id}_{sym}");

            // Settle in ledger.
            self.ledger
                .settle_funding(user_id, payment, &idemp_key, timestamp)?;

            // Check if margin was touched (user needs liquidation check).
            if payment.is_positive() {
                // Check if payment exceeded available balance — look at post-settlement balance.
                if let Some(bal) = self.ledger.get_balance(user_id, WalletType::Futures)
                    && bal.margin < *margin_before
                {
                    users_needing_liquidation_check.push(user_id);
                }
            }

            // Update accumulated funding on position.
            if let Some(pos) = self
                .position_manager
                .all_positions_mut()
                .find(|p| p.user_id == user_id && p.symbol == *sym)
            {
                pos.accumulated_funding = pos.accumulated_funding + payment;
                // If margin was deducted, reduce position margin accordingly.
                if payment.is_positive()
                    && let Some(bal) = self.ledger.get_balance(user_id, WalletType::Futures)
                    && bal.margin < pos.margin
                {
                    pos.margin = bal.margin;
                }
            }

            match side {
                PositionSide::Long | PositionSide::Both => {
                    total_long_payment = total_long_payment + payment;
                }
                PositionSide::Short => {
                    total_short_payment = total_short_payment + payment;
                }
            }
        }

        Ok(FundingResult {
            total_long_payment,
            total_short_payment,
            users_needing_liquidation_check,
        })
    }

    /// Execute ADL (Auto-Deleveraging) when insurance fund is depleted.
    ///
    /// Finds counterparties with highest ADL score and force-reduces their positions
    /// until the deficit is covered.
    ///
    /// Returns `(counterparty_user_id, qty_reduced)` pairs.
    #[allow(clippy::too_many_arguments)]
    pub fn execute_adl(
        &mut self,
        bankrupt_user: UserId,
        symbol: SymbolId,
        bankrupt_side: PositionSide,
        deficit: Decimal128,
        mark_price: Decimal128,
        tiers: &[MarginTier],
        timestamp: UnixMicros,
    ) -> ExgResult<Vec<(UserId, Decimal128)>> {
        let _ = tiers; // Reserved for future tier-aware ADL logic.
        // ADL targets are on the opposite side of the bankrupt user.
        let opposite_side = match bankrupt_side {
            PositionSide::Long => PositionSide::Short,
            PositionSide::Short => PositionSide::Long,
            PositionSide::Both => PositionSide::Long, // fallback
        };

        // Collect counterparty positions on the opposite side.
        let counterparty_positions: Vec<Position> = self
            .position_manager
            .all_positions()
            .filter(|p| p.symbol == symbol && p.side == opposite_side && p.user_id != bankrupt_user)
            .cloned()
            .collect();

        if counterparty_positions.is_empty() {
            return Err(ExgError::Internal(
                "no counterparty positions available for ADL".into(),
            ));
        }

        // Rank by ADL score.
        let ranking = exg_risk_engine::adl::calc_adl_ranking(&counterparty_positions, mark_price);

        let mut remaining_deficit = deficit;
        let mut results = Vec::new();

        for (cp_user_id, _score) in &ranking {
            if !remaining_deficit.is_positive() {
                break;
            }

            let cp_pos = match self.position_manager.get_position(*cp_user_id, symbol) {
                Some(p) => p,
                None => continue,
            };

            // Calculate how much notional we need to reduce.
            // deficit is in quote currency, so qty = deficit / mark_price.
            let needed_qty = remaining_deficit / mark_price;
            let reduce_qty = needed_qty.min(cp_pos.size);

            if reduce_qty.is_zero() {
                continue;
            }

            let margin_released = self
                .position_manager
                .calc_margin_released(*cp_user_id, symbol, reduce_qty)
                .unwrap_or(Decimal128::ZERO);

            let (realized_pnl, _remaining) = self.position_manager.reduce_or_close(
                *cp_user_id,
                symbol,
                reduce_qty,
                mark_price,
            )?;

            let idemp_key = format!("adl-{bankrupt_user}-{cp_user_id}-{symbol}-{timestamp}");

            self.ledger.close_position_settled(
                *cp_user_id,
                margin_released,
                realized_pnl,
                Decimal128::ZERO,
                &idemp_key,
                timestamp,
            )?;

            let covered = reduce_qty * mark_price;
            remaining_deficit = remaining_deficit - covered;
            results.push((*cp_user_id, reduce_qty));
        }

        Ok(results)
    }

    /// Get the insurance fund balance.
    pub fn insurance_fund_balance(&self) -> Decimal128 {
        self.ledger.insurance_fund_balance()
    }

    /// Snapshot entire clearing state.
    pub fn take_snapshot(&self) -> ClearingSnapshot {
        ClearingSnapshot {
            positions: self.position_manager.take_snapshot(),
        }
    }

    pub fn restore_from_snapshot(snapshot: ClearingSnapshot) -> Self {
        Self {
            position_manager: PositionManager::restore_from_snapshot(snapshot.positions),
            ledger: Ledger::new(), // Ledger state must be restored separately.
        }
    }
}

impl Default for ClearingService {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use exg_risk_engine::MarginTier;

    fn dec(s: &str) -> Decimal128 {
        s.parse().unwrap()
    }

    fn ts(us: u64) -> UnixMicros {
        UnixMicros::from_micros(us)
    }

    fn uid(id: u64) -> UserId {
        UserId::new(id)
    }

    fn default_symbol_config() -> SymbolConfig {
        SymbolConfig {
            symbol: SymbolId::new(1),
            tick_size: dec("0.01"),
            lot_size: dec("0.001"),
            min_notional: dec("10"),
            max_leverage: dec("10"),
            maker_fee: dec("0.0002"),
            taker_fee: dec("0.0004"),
            margin_tiers: vec![MarginTier {
                notional_floor: dec("0"),
                notional_cap: dec("100000000"),
                maintenance_margin_rate: dec("0.004"),
                maintenance_amount: dec("0"),
            }],
        }
    }

    /// Helper: deposit to funding, transfer to futures, freeze margin+fee for a user.
    fn setup_user(ledger: &mut Ledger, user_id: UserId, amount: Decimal128) {
        ledger
            .deposit(user_id, amount, &format!("dep-{user_id}"), ts(1))
            .unwrap();
        ledger
            .transfer(
                user_id,
                WalletType::Funding,
                WalletType::Futures,
                amount,
                &format!("xfer-{user_id}"),
                ts(2),
            )
            .unwrap();
    }

    /// Helper: freeze an amount in user's futures wallet.
    fn freeze(ledger: &mut Ledger, user_id: UserId, amount: Decimal128, key: &str) {
        ledger
            .freeze_for_order(user_id, WalletType::Futures, amount, key, ts(3))
            .unwrap();
    }

    fn make_trade(
        trade_id: u64,
        buyer: UserId,
        seller: UserId,
        price: &str,
        qty: &str,
        buyer_fee: &str,
        seller_fee: &str,
        buyer_leverage: &str,
        seller_leverage: &str,
    ) -> TradeInfo {
        TradeInfo {
            trade_id: TradeId::new(trade_id),
            symbol: SymbolId::new(1),
            price: dec(price),
            qty: dec(qty),
            buyer_user_id: buyer,
            seller_user_id: seller,
            buyer_fee: dec(buyer_fee),
            seller_fee: dec(seller_fee),
            buyer_leverage: dec(buyer_leverage),
            seller_leverage: dec(seller_leverage),
            buyer_margin_mode: MarginMode::Cross,
            seller_margin_mode: MarginMode::Cross,
            timestamp: ts(100),
        }
    }

    // ── 1. Open long position ──────────────────────────────────────────

    #[test]
    fn test_open_long_position() {
        let mut cs = ClearingService::new();
        let buyer = uid(1);
        let seller = uid(2);
        let config = default_symbol_config();

        setup_user(&mut cs.ledger, buyer, dec("10000"));
        setup_user(&mut cs.ledger, seller, dec("10000"));

        // Freeze for both sides: margin + fee.
        // Buyer: notional=100*10=1000, margin=1000/10=100, fee=0.4 => freeze 100.4
        // Seller: same
        freeze(&mut cs.ledger, buyer, dec("200"), "freeze-buyer");
        freeze(&mut cs.ledger, seller, dec("200"), "freeze-seller");

        let trade = make_trade(1, buyer, seller, "100", "10", "0.4", "0.4", "10", "10");
        cs.process_trade(&trade, &config).unwrap();

        let pos = cs
            .position_manager
            .get_position(buyer, SymbolId::new(1))
            .unwrap();
        assert_eq!(pos.side, PositionSide::Long);
        assert_eq!(pos.size, dec("10"));
        assert_eq!(pos.entry_price, dec("100"));
    }

    // ── 2. Increase long (averaged entry) ──────────────────────────────

    #[test]
    fn test_increase_long_averaged_entry() {
        let mut cs = ClearingService::new();
        let buyer = uid(1);
        let seller = uid(2);
        let config = default_symbol_config();

        setup_user(&mut cs.ledger, buyer, dec("10000"));
        setup_user(&mut cs.ledger, seller, dec("10000"));

        // First trade: buy 10 at 100
        freeze(&mut cs.ledger, buyer, dec("200"), "freeze-b1");
        freeze(&mut cs.ledger, seller, dec("200"), "freeze-s1");
        let trade1 = make_trade(1, buyer, seller, "100", "10", "0", "0", "10", "10");
        cs.process_trade(&trade1, &config).unwrap();

        // Second trade: buy 10 at 120
        freeze(&mut cs.ledger, buyer, dec("200"), "freeze-b2");
        freeze(&mut cs.ledger, seller, dec("200"), "freeze-s2");
        let trade2 = make_trade(2, buyer, seller, "120", "10", "0", "0", "10", "10");
        cs.process_trade(&trade2, &config).unwrap();

        let pos = cs
            .position_manager
            .get_position(buyer, SymbolId::new(1))
            .unwrap();
        assert_eq!(pos.size, dec("20"));
        // avg = (10*100 + 10*120) / 20 = 110
        assert_eq!(pos.entry_price, dec("110"));
    }

    // ── 3. Close long (profit) ─────────────────────────────────────────

    #[test]
    fn test_close_long_profit() {
        let mut cs = ClearingService::new();
        let buyer = uid(1);
        let seller = uid(2);
        let config = default_symbol_config();

        setup_user(&mut cs.ledger, buyer, dec("10000"));
        setup_user(&mut cs.ledger, seller, dec("10000"));

        // Open long at 100
        freeze(&mut cs.ledger, buyer, dec("200"), "freeze-b1");
        freeze(&mut cs.ledger, seller, dec("200"), "freeze-s1");
        let trade1 = make_trade(1, buyer, seller, "100", "10", "0", "0", "10", "10");
        cs.process_trade(&trade1, &config).unwrap();

        // Close long by selling at 110 — buyer becomes seller
        freeze(&mut cs.ledger, seller, dec("200"), "freeze-s2");
        freeze(&mut cs.ledger, buyer, dec("200"), "freeze-b2");
        let trade2 = make_trade(2, seller, buyer, "110", "10", "0", "0", "10", "10");
        cs.process_trade(&trade2, &config).unwrap();

        // Buyer's long should be closed.
        assert!(
            cs.position_manager
                .get_position(buyer, SymbolId::new(1))
                .is_none()
        );
        // PnL = (110 - 100) * 10 = 100 — reflected in available balance.
    }

    // ── 4. Close long (loss) ───────────────────────────────────────────

    #[test]
    fn test_close_long_loss() {
        let mut cs = ClearingService::new();
        let buyer = uid(1);
        let seller = uid(2);
        let config = default_symbol_config();

        setup_user(&mut cs.ledger, buyer, dec("10000"));
        setup_user(&mut cs.ledger, seller, dec("10000"));

        // Open long at 100
        freeze(&mut cs.ledger, buyer, dec("200"), "freeze-b1");
        freeze(&mut cs.ledger, seller, dec("200"), "freeze-s1");
        let trade1 = make_trade(1, buyer, seller, "100", "10", "0", "0", "10", "10");
        cs.process_trade(&trade1, &config).unwrap();

        // Close at 90 — buyer sells at loss
        freeze(&mut cs.ledger, seller, dec("200"), "freeze-s2");
        freeze(&mut cs.ledger, buyer, dec("200"), "freeze-b2");
        let trade2 = make_trade(2, seller, buyer, "90", "10", "0", "0", "10", "10");
        cs.process_trade(&trade2, &config).unwrap();

        assert!(
            cs.position_manager
                .get_position(buyer, SymbolId::new(1))
                .is_none()
        );
        // PnL = (90 - 100) * 10 = -100
    }

    // ── 5. Open short + close short ────────────────────────────────────

    #[test]
    fn test_short_open_and_close() {
        let mut cs = ClearingService::new();
        let buyer = uid(1);
        let seller = uid(2);
        let config = default_symbol_config();

        setup_user(&mut cs.ledger, buyer, dec("10000"));
        setup_user(&mut cs.ledger, seller, dec("10000"));

        // Seller opens short at 100
        freeze(&mut cs.ledger, buyer, dec("200"), "freeze-b1");
        freeze(&mut cs.ledger, seller, dec("200"), "freeze-s1");
        let trade1 = make_trade(1, buyer, seller, "100", "10", "0", "0", "10", "10");
        cs.process_trade(&trade1, &config).unwrap();

        let pos = cs
            .position_manager
            .get_position(seller, SymbolId::new(1))
            .unwrap();
        assert_eq!(pos.side, PositionSide::Short);
        assert_eq!(pos.size, dec("10"));

        // Close short by buying at 90 — seller profits 100
        freeze(&mut cs.ledger, buyer, dec("200"), "freeze-b2");
        freeze(&mut cs.ledger, seller, dec("200"), "freeze-s2");
        let trade2 = make_trade(2, seller, buyer, "90", "10", "0", "0", "10", "10");
        cs.process_trade(&trade2, &config).unwrap();

        // Seller's short is closed; PnL = (100-90)*10 = 100
        assert!(
            cs.position_manager
                .get_position(seller, SymbolId::new(1))
                .is_none()
        );
    }

    // ── 6. Partial close ───────────────────────────────────────────────

    #[test]
    fn test_partial_close() {
        let mut cs = ClearingService::new();
        let buyer = uid(1);
        let seller = uid(2);
        let config = default_symbol_config();

        setup_user(&mut cs.ledger, buyer, dec("10000"));
        setup_user(&mut cs.ledger, seller, dec("10000"));

        // Open long 10 at 100
        freeze(&mut cs.ledger, buyer, dec("200"), "freeze-b1");
        freeze(&mut cs.ledger, seller, dec("200"), "freeze-s1");
        let trade1 = make_trade(1, buyer, seller, "100", "10", "0", "0", "10", "10");
        cs.process_trade(&trade1, &config).unwrap();

        // Close 3 at 110 — buyer sells 3
        freeze(&mut cs.ledger, seller, dec("100"), "freeze-s2");
        freeze(&mut cs.ledger, buyer, dec("100"), "freeze-b2");
        let trade2 = make_trade(2, seller, buyer, "110", "3", "0", "0", "10", "10");
        cs.process_trade(&trade2, &config).unwrap();

        let pos = cs
            .position_manager
            .get_position(buyer, SymbolId::new(1))
            .unwrap();
        assert_eq!(pos.size, dec("7"));
        assert_eq!(pos.entry_price, dec("100")); // Entry unchanged.
    }

    // ── 7. Flip position ───────────────────────────────────────────────

    #[test]
    fn test_flip_position() {
        let mut cs = ClearingService::new();
        let buyer = uid(1);
        let seller = uid(2);
        let config = default_symbol_config();

        setup_user(&mut cs.ledger, buyer, dec("10000"));
        setup_user(&mut cs.ledger, seller, dec("10000"));

        // Open long 10 at 100
        freeze(&mut cs.ledger, buyer, dec("200"), "freeze-b1");
        freeze(&mut cs.ledger, seller, dec("200"), "freeze-s1");
        let trade1 = make_trade(1, buyer, seller, "100", "10", "0", "0", "10", "10");
        cs.process_trade(&trade1, &config).unwrap();

        assert!(
            cs.position_manager
                .get_position(buyer, SymbolId::new(1))
                .is_some()
        );

        // Sell 15 at 110 — close long 10, open short 5
        freeze(&mut cs.ledger, seller, dec("500"), "freeze-s2");
        freeze(&mut cs.ledger, buyer, dec("500"), "freeze-b2");
        let trade2 = make_trade(2, seller, buyer, "110", "15", "0", "0", "10", "10");
        cs.process_trade(&trade2, &config).unwrap();

        // Buyer (who was long, now sold 15) should have a short position of 5.
        let pos = cs
            .position_manager
            .get_position(buyer, SymbolId::new(1))
            .unwrap();
        assert_eq!(pos.side, PositionSide::Short);
        assert_eq!(pos.size, dec("5"));
        assert_eq!(pos.entry_price, dec("110"));
    }

    // ── 8. Fee collection ──────────────────────────────────────────────

    #[test]
    fn test_fee_collection() {
        let mut cs = ClearingService::new();
        let buyer = uid(1);
        let seller = uid(2);
        let config = default_symbol_config();

        setup_user(&mut cs.ledger, buyer, dec("10000"));
        setup_user(&mut cs.ledger, seller, dec("10000"));

        // Freeze enough for margin + fee.
        freeze(&mut cs.ledger, buyer, dec("200"), "freeze-b1");
        freeze(&mut cs.ledger, seller, dec("200"), "freeze-s1");

        let trade = make_trade(1, buyer, seller, "100", "10", "5", "3", "10", "10");
        cs.process_trade(&trade, &config).unwrap();

        // Total fees = 5 + 3 = 8 collected in fee system account.
        let fee_bal = cs.ledger.system_balance(WalletType::FeeCollection);
        assert_eq!(fee_bal, dec("8"));
    }

    // ── 9. Liquidation surplus ─────────────────────────────────────────

    #[test]
    fn test_liquidation_surplus() {
        let mut cs = ClearingService::new();
        let user = uid(1);
        let seller = uid(2);
        let config = default_symbol_config();

        setup_user(&mut cs.ledger, user, dec("10000"));
        setup_user(&mut cs.ledger, seller, dec("10000"));

        // Open long 10 at 100, margin = 100
        freeze(&mut cs.ledger, user, dec("200"), "freeze-b1");
        freeze(&mut cs.ledger, seller, dec("200"), "freeze-s1");
        let trade = make_trade(1, user, seller, "100", "10", "0", "0", "10", "10");
        cs.process_trade(&trade, &config).unwrap();

        // Liquidate at 95 (loss = 50). Margin was 100. Surplus = 100 + (-50) = 50.
        let surplus = cs
            .process_liquidation(user, SymbolId::new(1), dec("95"), &config)
            .unwrap();
        assert_eq!(surplus, dec("50"));
        assert!(cs.insurance_fund_balance().is_positive());
    }

    // ── 10. Liquidation deficit ────────────────────────────────────────

    #[test]
    fn test_liquidation_deficit() {
        let mut cs = ClearingService::new();
        let user = uid(1);
        let seller = uid(2);
        let config = default_symbol_config();

        // First seed the insurance fund.
        let seed_user = uid(99);
        setup_user(&mut cs.ledger, seed_user, dec("5000"));
        freeze(&mut cs.ledger, seed_user, dec("500"), "freeze-seed");
        cs.ledger
            .open_position(seed_user, dec("500"), dec("0"), "open-seed", ts(5))
            .unwrap();
        cs.ledger
            .liquidate(seed_user, dec("500"), dec("200"), "liq-seed", ts(6))
            .unwrap();
        assert_eq!(cs.insurance_fund_balance(), dec("200"));

        // Now main user.
        setup_user(&mut cs.ledger, user, dec("10000"));
        setup_user(&mut cs.ledger, seller, dec("10000"));

        freeze(&mut cs.ledger, user, dec("200"), "freeze-b1");
        freeze(&mut cs.ledger, seller, dec("200"), "freeze-s1");
        let trade = make_trade(1, user, seller, "100", "10", "0", "0", "10", "10");
        cs.process_trade(&trade, &config).unwrap();

        // Liquidate at 85 (loss = 150). Margin was 100. Deficit = 100 + (-150) = -50.
        let surplus = cs
            .process_liquidation(user, SymbolId::new(1), dec("85"), &config)
            .unwrap();
        assert_eq!(surplus, dec("-50"));
        // Insurance fund: 200 - 50 = 150
        assert_eq!(cs.insurance_fund_balance(), dec("150"));
    }

    // ── 11. Funding: long pays (positive rate) ─────────────────────────

    #[test]
    fn test_funding_long_pays() {
        let mut cs = ClearingService::new();
        let buyer = uid(1);
        let seller = uid(2);
        let config = default_symbol_config();

        setup_user(&mut cs.ledger, buyer, dec("10000"));
        setup_user(&mut cs.ledger, seller, dec("10000"));

        freeze(&mut cs.ledger, buyer, dec("200"), "freeze-b1");
        freeze(&mut cs.ledger, seller, dec("200"), "freeze-s1");
        let trade = make_trade(1, buyer, seller, "100", "10", "0", "0", "10", "10");
        cs.process_trade(&trade, &config).unwrap();

        let available_before = cs
            .ledger
            .get_balance(buyer, WalletType::Futures)
            .unwrap()
            .available;

        // Positive rate: longs pay. fee = 10 * 100 * 0.001 = 1
        let result = cs
            .settle_funding(SymbolId::new(1), dec("0.001"), dec("100"), 1, ts(200))
            .unwrap();

        assert!(result.total_long_payment.is_positive());

        let available_after = cs
            .ledger
            .get_balance(buyer, WalletType::Futures)
            .unwrap()
            .available;
        assert!(available_after < available_before);
    }

    // ── 12. Funding: short receives (positive rate) ────────────────────

    #[test]
    fn test_funding_short_receives() {
        let mut cs = ClearingService::new();
        let buyer = uid(1);
        let seller = uid(2);
        let config = default_symbol_config();

        setup_user(&mut cs.ledger, buyer, dec("10000"));
        setup_user(&mut cs.ledger, seller, dec("10000"));

        freeze(&mut cs.ledger, buyer, dec("200"), "freeze-b1");
        freeze(&mut cs.ledger, seller, dec("200"), "freeze-s1");
        let trade = make_trade(1, buyer, seller, "100", "10", "0", "0", "10", "10");
        cs.process_trade(&trade, &config).unwrap();

        let available_before = cs
            .ledger
            .get_balance(seller, WalletType::Futures)
            .unwrap()
            .available;

        // Positive rate: shorts receive. fee for short = -(10 * 100 * 0.001) = -1
        let result = cs
            .settle_funding(SymbolId::new(1), dec("0.001"), dec("100"), 1, ts(200))
            .unwrap();

        // Short payment is negative (receiving).
        assert!(result.total_short_payment.is_negative());

        let available_after = cs
            .ledger
            .get_balance(seller, WalletType::Futures)
            .unwrap()
            .available;
        assert!(available_after > available_before);
    }

    // ── 13. Funding from margin ────────────────────────────────────────

    #[test]
    fn test_funding_from_margin() {
        let mut cs = ClearingService::new();
        let buyer = uid(1);
        let seller = uid(2);
        let config = default_symbol_config();

        // Give buyer just enough for margin, very little available.
        setup_user(&mut cs.ledger, buyer, dec("200"));
        setup_user(&mut cs.ledger, seller, dec("10000"));

        // Buyer freezes 200 (all balance): margin = 100, leaves 0 available after open.
        freeze(&mut cs.ledger, buyer, dec("200"), "freeze-b1");
        freeze(&mut cs.ledger, seller, dec("200"), "freeze-s1");

        // Trade at 100, qty 10, leverage 10 => margin = 100. After open: 200 frozen -> 100 margin, 100 refund? No.
        // Actually: freeze 200, open_position(margin=100, fee=0) => frozen -= 100, margin += 100. Remaining frozen = 100.
        // We need to unfreeze the remainder or account for it properly.
        // Let's adjust: freeze exactly margin+fee.
        // Let's re-setup more carefully.
        let mut cs2 = ClearingService::new();
        setup_user(&mut cs2.ledger, buyer, dec("110"));
        setup_user(&mut cs2.ledger, seller, dec("10000"));

        // Freeze exactly 100 (margin) for buyer.
        freeze(&mut cs2.ledger, buyer, dec("100"), "freeze-b1");
        freeze(&mut cs2.ledger, seller, dec("200"), "freeze-s1");

        let trade = make_trade(1, buyer, seller, "100", "10", "0", "0", "10", "10");
        cs2.process_trade(&trade, &config).unwrap();

        // Buyer: available = 110 - 100 = 10, margin = 100
        let bal = cs2.ledger.get_balance(buyer, WalletType::Futures).unwrap();
        assert_eq!(bal.available, dec("10"));
        assert_eq!(bal.margin, dec("100"));

        // Funding = 10 * 100 * 0.01 = 10. Available (10) covers it exactly.
        // But let's use a bigger rate so margin is touched.
        // Funding = 10 * 100 * 0.02 = 20. Available = 10, so 10 from margin.
        let result = cs2
            .settle_funding(SymbolId::new(1), dec("0.02"), dec("100"), 1, ts(200))
            .unwrap();

        // User should be flagged for liquidation check.
        assert!(result.users_needing_liquidation_check.contains(&buyer));

        let bal = cs2.ledger.get_balance(buyer, WalletType::Futures).unwrap();
        assert_eq!(bal.available, Decimal128::ZERO);
        assert_eq!(bal.margin, dec("90")); // 100 - 10
    }

    // ── 14. Funding idempotency ────────────────────────────────────────

    #[test]
    fn test_funding_idempotency() {
        let mut cs = ClearingService::new();
        let buyer = uid(1);
        let seller = uid(2);
        let config = default_symbol_config();

        setup_user(&mut cs.ledger, buyer, dec("10000"));
        setup_user(&mut cs.ledger, seller, dec("10000"));

        freeze(&mut cs.ledger, buyer, dec("200"), "freeze-b1");
        freeze(&mut cs.ledger, seller, dec("200"), "freeze-s1");
        let trade = make_trade(1, buyer, seller, "100", "10", "0", "0", "10", "10");
        cs.process_trade(&trade, &config).unwrap();

        let available_before = cs
            .ledger
            .get_balance(buyer, WalletType::Futures)
            .unwrap()
            .available;

        // First settlement.
        cs.settle_funding(SymbolId::new(1), dec("0.001"), dec("100"), 1, ts(200))
            .unwrap();
        let available_after_first = cs
            .ledger
            .get_balance(buyer, WalletType::Futures)
            .unwrap()
            .available;

        // Same period_id again — should be idempotent.
        cs.settle_funding(SymbolId::new(1), dec("0.001"), dec("100"), 1, ts(201))
            .unwrap();
        let available_after_second = cs
            .ledger
            .get_balance(buyer, WalletType::Futures)
            .unwrap()
            .available;

        // Balance shouldn't change on second call.
        assert_eq!(available_after_first, available_after_second);
        assert!(available_after_first < available_before);
    }

    // ── 15. Snapshot/restore roundtrip ─────────────────────────────────

    #[test]
    fn test_snapshot_restore_roundtrip() {
        let mut cs = ClearingService::new();
        let buyer = uid(1);
        let seller = uid(2);
        let config = default_symbol_config();

        setup_user(&mut cs.ledger, buyer, dec("10000"));
        setup_user(&mut cs.ledger, seller, dec("10000"));

        freeze(&mut cs.ledger, buyer, dec("200"), "freeze-b1");
        freeze(&mut cs.ledger, seller, dec("200"), "freeze-s1");
        let trade = make_trade(1, buyer, seller, "100", "10", "0", "0", "10", "10");
        cs.process_trade(&trade, &config).unwrap();

        let snapshot = cs.take_snapshot();

        // Serialize/deserialize.
        let json = serde_json::to_string(&snapshot).unwrap();
        let restored_snap: ClearingSnapshot = serde_json::from_str(&json).unwrap();

        let restored = ClearingService::restore_from_snapshot(restored_snap);

        // Verify positions match.
        assert_eq!(
            restored.position_manager.position_count(),
            cs.position_manager.position_count()
        );

        let orig_pos = cs
            .position_manager
            .get_position(buyer, SymbolId::new(1))
            .unwrap();
        let rest_pos = restored
            .position_manager
            .get_position(buyer, SymbolId::new(1))
            .unwrap();
        assert_eq!(orig_pos.size, rest_pos.size);
        assert_eq!(orig_pos.entry_price, rest_pos.entry_price);
        assert_eq!(orig_pos.side, rest_pos.side);
    }

    // ── 16. ADL selects highest-ranked counterparty ───────────────────

    #[test]
    fn test_adl_selects_highest_ranked_counterparty() {
        let mut cs = ClearingService::new();
        let bankrupt = uid(1);
        let cp_high = uid(2); // high profit => high ADL score
        let cp_low = uid(3); // low profit => low ADL score
        let config = default_symbol_config();

        setup_user(&mut cs.ledger, bankrupt, dec("10000"));
        setup_user(&mut cs.ledger, cp_high, dec("10000"));
        setup_user(&mut cs.ledger, cp_low, dec("10000"));

        // Bankrupt user: long 10 at 100 (will be force-closed externally).
        freeze(&mut cs.ledger, bankrupt, dec("200"), "freeze-b1");
        freeze(&mut cs.ledger, cp_high, dec("200"), "freeze-s1");
        let trade1 = make_trade(1, bankrupt, cp_high, "100", "10", "0", "0", "10", "10");
        cs.process_trade(&trade1, &config).unwrap();

        // cp_low: short 5 at 100 with higher margin (worse profit ratio => lower ADL score).
        let another_buyer = uid(4);
        setup_user(&mut cs.ledger, another_buyer, dec("10000"));
        freeze(&mut cs.ledger, another_buyer, dec("500"), "freeze-b2");
        freeze(&mut cs.ledger, cp_low, dec("500"), "freeze-s2");
        // cp_low shorts 5 at 100, but with leverage 2 => margin = 5*100/2 = 250
        let trade2 = make_trade(2, another_buyer, cp_low, "100", "5", "0", "0", "10", "2");
        cs.process_trade(&trade2, &config).unwrap();

        // Force-close bankrupt user's position first.
        cs.position_manager.force_close(bankrupt, SymbolId::new(1));

        // Mark price dropped to 80, so shorts are in profit.
        // cp_high has short 10 at 100, margin 100:
        //   upnl = (100-80)*10 = 200, notional = 80*10 = 800
        //   score = (200 * 800) / (100 * 100) = 16
        // cp_low has short 5 at 100, margin 250:
        //   upnl = (100-80)*5 = 100, notional = 80*5 = 400
        //   score = (100 * 400) / (250 * 250) = 0.64
        //
        // cp_high has much higher score => selected first.
        let result = cs
            .execute_adl(
                bankrupt,
                SymbolId::new(1),
                PositionSide::Long, // bankrupt was long
                dec("50"),          // deficit
                dec("80"),          // mark price
                &config.margin_tiers,
                ts(500),
            )
            .unwrap();

        // Should select cp_high first (highest ADL score).
        assert!(!result.is_empty());
        assert_eq!(result[0].0, cp_high);
    }
}
