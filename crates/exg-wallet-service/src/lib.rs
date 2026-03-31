pub mod address;
pub mod deposit;
pub mod error;
pub mod hot_wallet;
pub mod withdrawal;

pub use address::{AddressManager, Chain, DepositAddress};
pub use deposit::{Deposit, DepositStatus, DepositTracker};
pub use error::WalletError;
pub use hot_wallet::HotWalletMonitor;
pub use withdrawal::{Withdrawal, WithdrawalProcessor, WithdrawalStatus};

#[cfg(test)]
mod tests {
    use exg_common::{Decimal128, UnixMicros, UserId};

    use crate::address::{AddressManager, Chain};
    use crate::deposit::DepositTracker;
    use crate::hot_wallet::HotWalletMonitor;
    use crate::withdrawal::{WithdrawalProcessor, WithdrawalStatus};

    fn dec(s: &str) -> Decimal128 {
        s.parse().unwrap()
    }

    fn ts(us: u64) -> UnixMicros {
        UnixMicros::from_micros(us)
    }

    // ── Address tests ──────────────────────────────────────────────────

    #[test]
    fn test_generate_address_same_user_chain_returns_same() {
        let mut mgr = AddressManager::new();
        let user = UserId::new(1);
        let addr1 = mgr.get_or_create_address(user, Chain::Ethereum).address.clone();
        let addr2 = mgr.get_or_create_address(user, Chain::Ethereum).address.clone();
        assert_eq!(addr1, addr2);
    }

    #[test]
    fn test_different_user_gets_different_address() {
        let mut mgr = AddressManager::new();
        let addr1 = mgr.get_or_create_address(UserId::new(1), Chain::Ethereum).address.clone();
        let addr2 = mgr.get_or_create_address(UserId::new(2), Chain::Ethereum).address.clone();
        assert_ne!(addr1, addr2);
    }

    #[test]
    fn test_address_lookup_returns_correct_user() {
        let mut mgr = AddressManager::new();
        let user = UserId::new(42);
        let addr = mgr.get_or_create_address(user, Chain::Tron).address.clone();
        let result = mgr.lookup_address(&addr);
        assert_eq!(result, Some((user, Chain::Tron)));
    }

    #[test]
    fn test_address_lookup_unknown_returns_none() {
        let mgr = AddressManager::new();
        assert_eq!(mgr.lookup_address("0xdeadbeef"), None);
    }

    #[test]
    fn test_get_user_addresses() {
        let mut mgr = AddressManager::new();
        let user = UserId::new(1);
        mgr.get_or_create_address(user, Chain::Ethereum);
        mgr.get_or_create_address(user, Chain::Tron);
        let addrs = mgr.get_user_addresses(user);
        assert_eq!(addrs.len(), 2);
    }

    // ── Deposit tests ──────────────────────────────────────────────────

    #[test]
    fn test_record_deposit_first_time_succeeds() {
        let mut tracker = DepositTracker::new();
        let result = tracker.record_deposit(
            UserId::new(1), Chain::Ethereum, "0xabc", 0,
            "0xfrom", "0xto", dec("100"), "USDT", ts(1000),
        );
        assert!(result.is_some());
        let deposit = result.unwrap();
        assert_eq!(deposit.amount, dec("100"));
        assert_eq!(deposit.asset, "USDT");
    }

    #[test]
    fn test_record_duplicate_deposit_returns_none() {
        let mut tracker = DepositTracker::new();
        tracker.record_deposit(
            UserId::new(1), Chain::Ethereum, "0xabc", 0,
            "0xfrom", "0xto", dec("100"), "USDT", ts(1000),
        );
        let dup = tracker.record_deposit(
            UserId::new(1), Chain::Ethereum, "0xabc", 0,
            "0xfrom", "0xto", dec("100"), "USDT", ts(2000),
        );
        assert!(dup.is_none());
    }

    #[test]
    fn test_update_confirmations_transitions_to_confirmed() {
        let mut tracker = DepositTracker::new();
        tracker.record_deposit(
            UserId::new(1), Chain::Ethereum, "0xabc", 0,
            "0xfrom", "0xto", dec("50"), "USDT", ts(1000),
        );
        // Not enough confirmations
        let changed = tracker.update_confirmations(Chain::Ethereum, "0xabc", 0, 5);
        assert_eq!(changed, Some(false));

        // Enough confirmations (Ethereum requires 12)
        let changed = tracker.update_confirmations(Chain::Ethereum, "0xabc", 0, 12);
        assert_eq!(changed, Some(true));
    }

    #[test]
    fn test_mark_credited_after_confirmed() {
        let mut tracker = DepositTracker::new();
        tracker.record_deposit(
            UserId::new(1), Chain::Ethereum, "0xabc", 0,
            "0xfrom", "0xto", dec("50"), "USDT", ts(1000),
        );
        tracker.update_confirmations(Chain::Ethereum, "0xabc", 0, 12);
        let credited = tracker.mark_credited(Chain::Ethereum, "0xabc", 0);
        assert!(credited);
    }

    #[test]
    fn test_mark_credited_before_confirmed_fails() {
        let mut tracker = DepositTracker::new();
        tracker.record_deposit(
            UserId::new(1), Chain::Ethereum, "0xabc", 0,
            "0xfrom", "0xto", dec("50"), "USDT", ts(1000),
        );
        let credited = tracker.mark_credited(Chain::Ethereum, "0xabc", 0);
        assert!(!credited);
    }

    #[test]
    fn test_pending_deposits_filter() {
        let mut tracker = DepositTracker::new();
        tracker.record_deposit(
            UserId::new(1), Chain::Ethereum, "0xabc", 0,
            "0xfrom", "0xto", dec("50"), "USDT", ts(1000),
        );
        tracker.record_deposit(
            UserId::new(1), Chain::Ethereum, "0xdef", 0,
            "0xfrom", "0xto", dec("100"), "USDT", ts(2000),
        );
        // Confirm first deposit
        tracker.update_confirmations(Chain::Ethereum, "0xabc", 0, 12);

        let pending = tracker.pending_deposits();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].tx_hash, "0xdef");
    }

    #[test]
    fn test_confirmed_uncredited() {
        let mut tracker = DepositTracker::new();
        tracker.record_deposit(
            UserId::new(1), Chain::Ethereum, "0xabc", 0,
            "0xfrom", "0xto", dec("50"), "USDT", ts(1000),
        );
        tracker.update_confirmations(Chain::Ethereum, "0xabc", 0, 12);

        let uncredited = tracker.confirmed_uncredited();
        assert_eq!(uncredited.len(), 1);

        tracker.mark_credited(Chain::Ethereum, "0xabc", 0);
        let uncredited = tracker.confirmed_uncredited();
        assert_eq!(uncredited.len(), 0);
    }

    #[test]
    fn test_user_deposits() {
        let mut tracker = DepositTracker::new();
        tracker.record_deposit(
            UserId::new(1), Chain::Ethereum, "0xabc", 0,
            "0xfrom", "0xto", dec("50"), "USDT", ts(1000),
        );
        tracker.record_deposit(
            UserId::new(2), Chain::Ethereum, "0xdef", 0,
            "0xfrom", "0xto", dec("100"), "USDT", ts(2000),
        );
        let deposits = tracker.user_deposits(UserId::new(1));
        assert_eq!(deposits.len(), 1);
    }

    // ── Withdrawal tests ──────���────────────────────────────────────────

    #[test]
    fn test_submit_withdrawal_auto_approve_below_threshold() {
        let mut proc = WithdrawalProcessor::new(dec("1000"));
        let w = proc.submit(
            UserId::new(1), Chain::Ethereum, "0xdest",
            dec("500"), dec("1"), "USDT", ts(1000),
        );
        assert_eq!(w.status, WithdrawalStatus::Approved);
    }

    #[test]
    fn test_submit_withdrawal_pending_review_above_threshold() {
        let mut proc = WithdrawalProcessor::new(dec("1000"));
        let w = proc.submit(
            UserId::new(1), Chain::Ethereum, "0xdest",
            dec("5000"), dec("1"), "USDT", ts(1000),
        );
        assert_eq!(w.status, WithdrawalStatus::PendingReview);
    }

    #[test]
    fn test_withdrawal_approve_processing_completed_lifecycle() {
        let mut proc = WithdrawalProcessor::new(dec("100"));
        let w = proc.submit(
            UserId::new(1), Chain::Ethereum, "0xdest",
            dec("500"), dec("1"), "USDT", ts(1000),
        );
        let id = w.id;
        assert_eq!(w.status, WithdrawalStatus::PendingReview);

        proc.approve(id, UserId::new(99)).unwrap();
        proc.mark_processing(id, "0xtxhash").unwrap();
        proc.mark_completed(id).unwrap();

        let user_w = proc.user_withdrawals(UserId::new(1));
        assert_eq!(user_w[0].status, WithdrawalStatus::Completed);
        assert_eq!(user_w[0].tx_hash.as_deref(), Some("0xtxhash"));
    }

    #[test]
    fn test_reject_withdrawal() {
        let mut proc = WithdrawalProcessor::new(dec("100"));
        let w = proc.submit(
            UserId::new(1), Chain::Ethereum, "0xdest",
            dec("500"), dec("1"), "USDT", ts(1000),
        );
        let id = w.id;

        proc.reject(id, UserId::new(99)).unwrap();
        let user_w = proc.user_withdrawals(UserId::new(1));
        assert_eq!(user_w[0].status, WithdrawalStatus::Rejected);
        assert_eq!(user_w[0].reviewed_by, Some(UserId::new(99)));
    }

    #[test]
    fn test_invalid_status_transition() {
        let mut proc = WithdrawalProcessor::new(dec("100"));
        let w = proc.submit(
            UserId::new(1), Chain::Ethereum, "0xdest",
            dec("500"), dec("1"), "USDT", ts(1000),
        );
        let id = w.id;

        // Try to mark completed directly from PendingReview — should fail
        let result = proc.mark_completed(id);
        assert!(result.is_err());

        // Try to approve then mark completed without processing — should fail
        proc.approve(id, UserId::new(99)).unwrap();
        let result = proc.mark_completed(id);
        assert!(result.is_err());
    }

    #[test]
    fn test_withdrawal_not_found() {
        let mut proc = WithdrawalProcessor::new(dec("100"));
        let result = proc.approve(999, UserId::new(1));
        assert!(result.is_err());
    }

    #[test]
    fn test_pending_review_filter() {
        let mut proc = WithdrawalProcessor::new(dec("100"));
        proc.submit(
            UserId::new(1), Chain::Ethereum, "0xdest",
            dec("500"), dec("1"), "USDT", ts(1000),
        );
        proc.submit(
            UserId::new(2), Chain::Ethereum, "0xdest2",
            dec("50"), dec("1"), "USDT", ts(2000),
        );

        let pending = proc.pending_review();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].user_id, UserId::new(1));
    }

    #[test]
    fn test_approved_ready_filter() {
        let mut proc = WithdrawalProcessor::new(dec("100"));
        // Auto-approved
        proc.submit(
            UserId::new(1), Chain::Ethereum, "0xdest",
            dec("50"), dec("1"), "USDT", ts(1000),
        );

        let ready = proc.approved_ready();
        assert_eq!(ready.len(), 1);
    }

    // ── Hot wallet tests ───────────────────────────────────────────────

    #[test]
    fn test_hot_wallet_balance_tracking() {
        let mut monitor = HotWalletMonitor::new(dec("1000"));
        assert_eq!(monitor.get_balance(Chain::Ethereum, "USDT"), Decimal128::ZERO);

        monitor.update_balance(Chain::Ethereum, "USDT", dec("500"));
        assert_eq!(monitor.get_balance(Chain::Ethereum, "USDT"), dec("500"));

        monitor.update_balance(Chain::Ethereum, "USDT", dec("1500"));
        assert_eq!(monitor.get_balance(Chain::Ethereum, "USDT"), dec("1500"));
    }

    #[test]
    fn test_collection_threshold_check() {
        let mut monitor = HotWalletMonitor::new(dec("1000"));
        monitor.update_balance(Chain::Ethereum, "USDT", dec("500"));
        assert!(!monitor.needs_collection(Chain::Ethereum, "USDT"));

        monitor.update_balance(Chain::Ethereum, "USDT", dec("1000"));
        assert!(monitor.needs_collection(Chain::Ethereum, "USDT"));

        monitor.update_balance(Chain::Ethereum, "USDT", dec("2000"));
        assert!(monitor.needs_collection(Chain::Ethereum, "USDT"));
    }

    #[test]
    fn test_cold_wallet_address() {
        let mut monitor = HotWalletMonitor::new(dec("1000"));
        monitor.set_cold_wallet(Chain::Ethereum, "0xcold");
        // Just verify it doesn't panic — the address is stored for later use
    }
}
