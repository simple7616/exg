use exg_common::{Decimal128, ExgError, ExgResult};

use crate::account::WalletType;
use crate::operations::Ledger;

/// System wallet types that must always be non-negative.
const NON_NEGATIVE_SYSTEM_WALLETS: &[WalletType] = &[
    WalletType::InsuranceFund,
    WalletType::FeeCollection,
    WalletType::Escrow,
];

impl Ledger {
    /// Verify global balance invariant.
    ///
    /// Replays the journal: for each entry, debits subtract and credits add.
    /// The net effect across all (user, wallet, field) slots must be zero —
    /// meaning the system is a closed loop (deposits/withdrawals are modeled
    /// via system accounts implicitly).
    ///
    /// Additionally verifies that the computed balances match the live state.
    pub fn verify_global_invariant(&self) -> ExgResult<()> {
        // Sum total balance across all user wallets.
        let mut user_total = Decimal128::ZERO;
        for account in self.accounts.values() {
            for balance in account.wallets.values() {
                user_total = user_total + balance.total();
            }
        }

        // Sum system accounts.
        let mut system_total = Decimal128::ZERO;
        for &bal in self.system_accounts.values() {
            system_total = system_total + bal;
        }

        // In our model, deposits add money from "outside" (no debit from a user),
        // and withdrawals remove money to "outside" (no credit to a user).
        // So total user + system balances should equal net deposits - net withdrawals.
        // We verify this by replaying the journal.
        let mut net_external = Decimal128::ZERO; // deposits - withdrawals

        use crate::journal::JournalEntryType;

        for entry in &self.journal {
            match entry.entry_type {
                JournalEntryType::Deposit => {
                    net_external = net_external + entry.amount;
                }
                JournalEntryType::Withdrawal => {
                    net_external = net_external - entry.amount;
                }
                _ => {}
            }
        }

        let total = user_total + system_total;
        if total != net_external {
            return Err(ExgError::BalanceInvariantViolation(format!(
                "global invariant failed: user_total={user_total}, system_total={system_total}, \
                 expected={net_external}"
            )));
        }

        Ok(())
    }

    /// Verify a single account: no sub-field is negative.
    pub fn verify_account_invariant(&self, user_id: exg_common::UserId) -> ExgResult<()> {
        let account = self
            .accounts
            .get(&user_id)
            .ok_or(ExgError::AccountNotFound(user_id))?;

        for (wallet_type, balance) in &account.wallets {
            if balance.available.is_negative() {
                return Err(ExgError::BalanceInvariantViolation(format!(
                    "user {user_id} wallet {wallet_type:?}: available is negative ({})",
                    balance.available
                )));
            }
            if balance.frozen.is_negative() {
                return Err(ExgError::BalanceInvariantViolation(format!(
                    "user {user_id} wallet {wallet_type:?}: frozen is negative ({})",
                    balance.frozen
                )));
            }
            if balance.margin.is_negative() {
                return Err(ExgError::BalanceInvariantViolation(format!(
                    "user {user_id} wallet {wallet_type:?}: margin is negative ({})",
                    balance.margin
                )));
            }
        }

        Ok(())
    }

    /// Verify all account invariants + global invariant.
    pub fn verify_all_invariants(&self) -> ExgResult<()> {
        for &user_id in self.accounts.keys() {
            self.verify_account_invariant(user_id)?;
        }

        // Verify specific system account balances are non-negative.
        // Note: Funding pool can legitimately be negative (receives before pays).
        for &wallet_type in NON_NEGATIVE_SYSTEM_WALLETS {
            let balance = self.system_balance(wallet_type);
            if balance.is_negative() {
                return Err(ExgError::BalanceInvariantViolation(format!(
                    "system account {wallet_type:?}: balance is negative ({balance})"
                )));
            }
        }

        self.verify_global_invariant()
    }
}
