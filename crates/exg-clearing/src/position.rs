use exg_common::{Decimal128, ExgError, ExgResult, MarginMode, PositionSide, SymbolId, UserId};
use exg_risk_engine::Position;
use rustc_hash::FxHashMap;

/// Position manager tracks all open positions keyed by (user_id, symbol_id).
pub struct PositionManager {
    positions: FxHashMap<(UserId, SymbolId), Position>,
}

impl PositionManager {
    pub fn new() -> Self {
        Self {
            positions: FxHashMap::default(),
        }
    }

    /// Get a position.
    pub fn get_position(&self, user_id: UserId, symbol: SymbolId) -> Option<&Position> {
        self.positions.get(&(user_id, symbol))
    }

    /// Get all positions for a user.
    pub fn get_user_positions(&self, user_id: UserId) -> Vec<&Position> {
        self.positions
            .iter()
            .filter(|((uid, _), _)| *uid == user_id)
            .map(|(_, p)| p)
            .collect()
    }

    /// Open or increase a position. Returns the updated position.
    ///
    /// If no existing position, creates one.
    /// If same direction, increases size with averaged entry price:
    ///   new_avg = (old_size * old_entry + add_size * add_price) / (old_size + add_size)
    #[allow(clippy::too_many_arguments)]
    pub fn open_or_increase(
        &mut self,
        user_id: UserId,
        symbol: SymbolId,
        side: PositionSide,
        qty: Decimal128,
        price: Decimal128,
        leverage: Decimal128,
        margin_mode: MarginMode,
    ) -> &Position {
        let key = (user_id, symbol);
        let position = self.positions.entry(key).or_insert_with(|| Position {
            user_id,
            symbol,
            side,
            size: Decimal128::ZERO,
            entry_price: Decimal128::ZERO,
            leverage,
            margin: Decimal128::ZERO,
            unrealized_pnl: Decimal128::ZERO,
            accumulated_funding: Decimal128::ZERO,
            margin_mode,
        });

        // Weighted average entry price.
        let old_notional = position.size * position.entry_price;
        let new_notional = qty * price;
        let new_size = position.size + qty;

        if new_size.is_positive() {
            position.entry_price = (old_notional + new_notional) / new_size;
        }
        position.size = new_size;
        position.side = side;
        position.leverage = leverage;

        // Calculate and add initial margin for the new quantity.
        let add_margin = (qty * price) / leverage;
        position.margin = position.margin + add_margin;

        position
    }

    /// Reduce or close a position. Returns (realized_pnl, remaining_position).
    ///
    /// PnL = (exit_price - entry_price) * qty for long
    /// PnL = (entry_price - exit_price) * qty for short
    pub fn reduce_or_close(
        &mut self,
        user_id: UserId,
        symbol: SymbolId,
        qty: Decimal128,
        exit_price: Decimal128,
    ) -> ExgResult<(Decimal128, Option<&Position>)> {
        let key = (user_id, symbol);
        let position = self.positions.get(&key).ok_or_else(|| {
            ExgError::Internal(format!(
                "no position found for user {user_id} symbol {symbol}"
            ))
        })?;

        if qty > position.size {
            return Err(ExgError::Internal(format!(
                "reduce qty {} exceeds position size {}",
                qty, position.size
            )));
        }

        let realized_pnl = match position.side {
            PositionSide::Long | PositionSide::Both => (exit_price - position.entry_price) * qty,
            PositionSide::Short => (position.entry_price - exit_price) * qty,
        };

        // Proportional margin release.
        let margin_released = position.margin * qty / position.size;
        let new_size = position.size - qty;

        if new_size.is_zero() {
            self.positions.remove(&key);
            Ok((realized_pnl, None))
        } else {
            let pos = self.positions.get_mut(&key).unwrap();
            pos.size = new_size;
            pos.margin = pos.margin - margin_released;
            Ok((realized_pnl, self.positions.get(&key)))
        }
    }

    /// Force close a position (liquidation). Returns the full position that was closed.
    pub fn force_close(&mut self, user_id: UserId, symbol: SymbolId) -> Option<Position> {
        self.positions.remove(&(user_id, symbol))
    }

    /// Update unrealized PnL for all positions given current mark prices.
    pub fn update_mark_prices(&mut self, mark_prices: &FxHashMap<SymbolId, Decimal128>) {
        for ((_, symbol), position) in self.positions.iter_mut() {
            if let Some(&mark_price) = mark_prices.get(symbol) {
                position.unrealized_pnl = exg_risk_engine::margin::calc_unrealized_pnl(
                    position.entry_price,
                    mark_price,
                    position.size,
                    position.side,
                );
            }
        }
    }

    /// Get all positions (for funding settlement iteration).
    pub fn all_positions(&self) -> impl Iterator<Item = &Position> {
        self.positions.values()
    }

    pub fn all_positions_mut(&mut self) -> impl Iterator<Item = &mut Position> {
        self.positions.values_mut()
    }

    /// Snapshot.
    pub fn take_snapshot(&self) -> Vec<Position> {
        self.positions.values().cloned().collect()
    }

    pub fn restore_from_snapshot(positions: Vec<Position>) -> Self {
        let mut map = FxHashMap::default();
        for pos in positions {
            map.insert((pos.user_id, pos.symbol), pos);
        }
        Self { positions: map }
    }

    /// Number of open positions.
    pub fn position_count(&self) -> usize {
        self.positions.len()
    }

    /// Get margin released for a given reduction qty (proportional).
    pub fn calc_margin_released(
        &self,
        user_id: UserId,
        symbol: SymbolId,
        qty: Decimal128,
    ) -> Option<Decimal128> {
        self.positions
            .get(&(user_id, symbol))
            .map(|p| p.margin * qty / p.size)
    }
}

impl Default for PositionManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dec(s: &str) -> Decimal128 {
        s.parse().unwrap()
    }

    #[test]
    fn test_open_position() {
        let mut pm = PositionManager::new();
        let uid = UserId::new(1);
        let sym = SymbolId::new(1);

        let pos = pm.open_or_increase(
            uid,
            sym,
            PositionSide::Long,
            dec("10"),
            dec("100"),
            dec("10"),
            MarginMode::Cross,
        );
        assert_eq!(pos.size, dec("10"));
        assert_eq!(pos.entry_price, dec("100"));
        assert_eq!(pos.margin, dec("100")); // 10 * 100 / 10
    }

    #[test]
    fn test_increase_position_averaged_entry() {
        let mut pm = PositionManager::new();
        let uid = UserId::new(1);
        let sym = SymbolId::new(1);

        pm.open_or_increase(
            uid,
            sym,
            PositionSide::Long,
            dec("10"),
            dec("100"),
            dec("10"),
            MarginMode::Cross,
        );
        let pos = pm.open_or_increase(
            uid,
            sym,
            PositionSide::Long,
            dec("10"),
            dec("120"),
            dec("10"),
            MarginMode::Cross,
        );

        assert_eq!(pos.size, dec("20"));
        // avg = (10*100 + 10*120) / 20 = 2200/20 = 110
        assert_eq!(pos.entry_price, dec("110"));
        // margin = 100 + 120 = 220
        assert_eq!(pos.margin, dec("220"));
    }

    #[test]
    fn test_reduce_position() {
        let mut pm = PositionManager::new();
        let uid = UserId::new(1);
        let sym = SymbolId::new(1);

        pm.open_or_increase(
            uid,
            sym,
            PositionSide::Long,
            dec("10"),
            dec("100"),
            dec("10"),
            MarginMode::Cross,
        );

        let (pnl, remaining) = pm.reduce_or_close(uid, sym, dec("3"), dec("110")).unwrap();
        // PnL = (110 - 100) * 3 = 30
        assert_eq!(pnl, dec("30"));
        assert!(remaining.is_some());
        assert_eq!(remaining.unwrap().size, dec("7"));
    }

    #[test]
    fn test_close_position() {
        let mut pm = PositionManager::new();
        let uid = UserId::new(1);
        let sym = SymbolId::new(1);

        pm.open_or_increase(
            uid,
            sym,
            PositionSide::Long,
            dec("10"),
            dec("100"),
            dec("10"),
            MarginMode::Cross,
        );

        let (pnl, remaining) = pm.reduce_or_close(uid, sym, dec("10"), dec("90")).unwrap();
        // PnL = (90 - 100) * 10 = -100
        assert_eq!(pnl, dec("-100"));
        assert!(remaining.is_none());
    }

    #[test]
    fn test_force_close() {
        let mut pm = PositionManager::new();
        let uid = UserId::new(1);
        let sym = SymbolId::new(1);

        pm.open_or_increase(
            uid,
            sym,
            PositionSide::Short,
            dec("5"),
            dec("200"),
            dec("20"),
            MarginMode::Isolated,
        );

        let closed = pm.force_close(uid, sym);
        assert!(closed.is_some());
        let p = closed.unwrap();
        assert_eq!(p.size, dec("5"));
        assert_eq!(p.side, PositionSide::Short);
        assert!(pm.get_position(uid, sym).is_none());
    }

    #[test]
    fn test_snapshot_roundtrip() {
        let mut pm = PositionManager::new();
        pm.open_or_increase(
            UserId::new(1),
            SymbolId::new(1),
            PositionSide::Long,
            dec("10"),
            dec("100"),
            dec("10"),
            MarginMode::Cross,
        );
        pm.open_or_increase(
            UserId::new(2),
            SymbolId::new(1),
            PositionSide::Short,
            dec("5"),
            dec("200"),
            dec("20"),
            MarginMode::Isolated,
        );

        let snap = pm.take_snapshot();
        let restored = PositionManager::restore_from_snapshot(snap);
        assert_eq!(restored.position_count(), 2);
        assert!(
            restored
                .get_position(UserId::new(1), SymbolId::new(1))
                .is_some()
        );
        assert!(
            restored
                .get_position(UserId::new(2), SymbolId::new(1))
                .is_some()
        );
    }
}
