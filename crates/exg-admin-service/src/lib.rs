pub mod error;
pub mod reports;
pub mod risk_monitor;
pub mod symbol_management;
pub mod user_management;

pub use error::AdminError;
pub use reports::{AssetReport, FeeReport, ReportGenerator};
pub use risk_monitor::{PositionSummary, RiskMonitor, RiskSnapshot};
pub use symbol_management::{SymbolEntry, SymbolManager};
pub use user_management::{UserManagement, UserSummary};

#[cfg(test)]
mod tests {
    use exg_common::*;

    use super::*;

    fn dec(s: &str) -> Decimal128 {
        s.parse().unwrap()
    }

    // ── 1. Freeze / unfreeze user ──────────────────────────────────────

    #[test]
    fn test_freeze_unfreeze_user() {
        let mut mgr = UserManagement::new();
        let uid = UserId::new(1);

        assert!(!mgr.is_frozen(uid));
        assert!(mgr.freeze_user(uid)); // newly frozen
        assert!(!mgr.freeze_user(uid)); // already frozen
        assert!(mgr.is_frozen(uid));

        assert!(mgr.unfreeze_user(uid)); // was frozen
        assert!(!mgr.is_frozen(uid));
        assert!(!mgr.unfreeze_user(uid)); // not frozen
    }

    #[test]
    fn test_frozen_users_list() {
        let mut mgr = UserManagement::new();
        mgr.freeze_user(UserId::new(10));
        mgr.freeze_user(UserId::new(20));

        let mut frozen = mgr.frozen_users();
        frozen.sort();
        assert_eq!(frozen, vec![UserId::new(10), UserId::new(20)]);
    }

    // ── 2. Add symbol, list symbols ────────────────────────────────────

    #[test]
    fn test_add_and_list_symbols() {
        let mut sm = SymbolManager::new();
        sm.add_symbol(make_symbol(1, "BTCUSDT", SymbolStatus::Trading))
            .unwrap();
        sm.add_symbol(make_symbol(2, "ETHUSDT", SymbolStatus::Trading))
            .unwrap();

        assert_eq!(sm.list_symbols().len(), 2);
    }

    // ── 3. Update symbol status ────────────────────────────────────────

    #[test]
    fn test_update_symbol_status() {
        let mut sm = SymbolManager::new();
        sm.add_symbol(make_symbol(1, "BTCUSDT", SymbolStatus::Trading))
            .unwrap();

        sm.update_status(SymbolId::new(1), SymbolStatus::Halted)
            .unwrap();

        let entry = sm.get_symbol(SymbolId::new(1)).unwrap();
        assert_eq!(entry.status, SymbolStatus::Halted);
    }

    // ── 4. Update symbol fees ──────────────────────────────────────────

    #[test]
    fn test_update_symbol_fees() {
        let mut sm = SymbolManager::new();
        sm.add_symbol(make_symbol(1, "BTCUSDT", SymbolStatus::Trading))
            .unwrap();

        sm.update_fees(SymbolId::new(1), dec("0.0001"), dec("0.0005"))
            .unwrap();

        let entry = sm.get_symbol(SymbolId::new(1)).unwrap();
        assert_eq!(entry.maker_fee, dec("0.0001"));
        assert_eq!(entry.taker_fee, dec("0.0005"));
    }

    // ── 5. Duplicate symbol name → error ───────────────────────────────

    #[test]
    fn test_duplicate_symbol_name() {
        let mut sm = SymbolManager::new();
        sm.add_symbol(make_symbol(1, "BTCUSDT", SymbolStatus::Trading))
            .unwrap();

        let result = sm.add_symbol(make_symbol(2, "BTCUSDT", SymbolStatus::Trading));
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            AdminError::DuplicateSymbol(_)
        ));
    }

    // ── 6. Risk monitor: record liquidation, check count ───────────────

    #[test]
    fn test_risk_monitor_liquidation_count() {
        let mut rm = RiskMonitor::new(dec("0.8"));
        assert_eq!(rm.get_liquidation_count(), 0);

        rm.record_liquidation();
        rm.record_liquidation();
        rm.record_liquidation();
        assert_eq!(rm.get_liquidation_count(), 3);

        rm.record_adl();
        assert_eq!(rm.get_adl_count(), 1);

        rm.reset_24h_counters();
        assert_eq!(rm.get_liquidation_count(), 0);
        assert_eq!(rm.get_adl_count(), 0);
    }

    // ── 7. Near-liquidation detection ──────────────────────────────────

    #[test]
    fn test_near_liquidation_detection() {
        let rm = RiskMonitor::new(dec("0.8"));

        assert!(!rm.is_near_liquidation(dec("0.5")));
        assert!(!rm.is_near_liquidation(dec("0.79")));
        assert!(rm.is_near_liquidation(dec("0.8")));
        assert!(rm.is_near_liquidation(dec("0.95")));
    }

    // ── 8. Fee report generation with time range ───────────────────────

    #[test]
    fn test_fee_report_generation() {
        let mut rg = ReportGenerator::new();

        rg.record_fee(dec("1.5"), dec("3.0"), UnixMicros::from_micros(100));
        rg.record_fee(dec("2.0"), dec("4.0"), UnixMicros::from_micros(200));
        rg.record_fee(dec("10.0"), dec("20.0"), UnixMicros::from_micros(500));
        rg.record_funding_income(dec("0.5"), UnixMicros::from_micros(150));
        rg.record_funding_income(dec("1.0"), UnixMicros::from_micros(600));

        // Report for [100, 300) — includes first two fees and first funding.
        let report =
            rg.generate_fee_report(UnixMicros::from_micros(100), UnixMicros::from_micros(300));

        assert_eq!(report.total_maker_fees, dec("3.5"));
        assert_eq!(report.total_taker_fees, dec("7.0"));
        assert_eq!(report.total_funding_income, dec("0.5"));
    }

    // ── 9. Asset report with deposits/withdrawals ──────────────────────

    #[test]
    fn test_asset_report() {
        let mut rg = ReportGenerator::new();

        rg.record_deposit(dec("1000"));
        rg.record_deposit(dec("500"));
        rg.record_withdrawal(dec("200"));

        let report = rg.generate_asset_report();
        assert_eq!(report.total_deposits, dec("1500"));
        assert_eq!(report.total_withdrawals, dec("200"));
        assert_eq!(report.net_flow, dec("1300"));
        assert_eq!(report.deposit_count, 2);
        assert_eq!(report.withdrawal_count, 1);
    }

    // ── 10. Symbol name lookup ─────────────────────────────────────────

    #[test]
    fn test_symbol_name_lookup() {
        let mut sm = SymbolManager::new();
        sm.add_symbol(make_symbol(1, "BTCUSDT", SymbolStatus::Trading))
            .unwrap();
        sm.add_symbol(make_symbol(2, "ETHUSDT", SymbolStatus::Halted))
            .unwrap();

        let entry = sm.get_by_name("BTCUSDT").unwrap();
        assert_eq!(entry.symbol_id, SymbolId::new(1));

        assert!(sm.get_by_name("SOLUSDT").is_none());

        // active_symbols should only return Trading
        let active = sm.active_symbols();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].name, "BTCUSDT");
    }

    // ── Update non-existent symbol → error ─────────────────────────────

    #[test]
    fn test_update_nonexistent_symbol() {
        let mut sm = SymbolManager::new();
        let result = sm.update_status(SymbolId::new(99), SymbolStatus::Halted);
        assert!(matches!(result.unwrap_err(), AdminError::SymbolNotFound(_)));

        let result = sm.update_fees(SymbolId::new(99), dec("0.01"), dec("0.02"));
        assert!(matches!(result.unwrap_err(), AdminError::SymbolNotFound(_)));
    }

    // ── Helper ─────────────────────────────────────────────────────────

    fn make_symbol(id: u16, name: &str, status: SymbolStatus) -> SymbolEntry {
        SymbolEntry {
            symbol_id: SymbolId::new(id),
            name: name.to_string(),
            symbol_type: SymbolType::PerpetualLinear,
            status,
            tick_size: dec("0.01"),
            lot_size: dec("0.001"),
            min_notional: dec("10"),
            max_leverage: dec("100"),
            maker_fee: dec("0.0002"),
            taker_fee: dec("0.0004"),
            created_at: UnixMicros::from_micros(0),
            updated_at: UnixMicros::from_micros(0),
        }
    }
}
