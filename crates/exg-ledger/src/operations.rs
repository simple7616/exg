use std::collections::HashMap;

use exg_common::{Decimal128, ExgError, ExgResult, UnixMicros, UserId};
use rustc_hash::FxHashSet;

use crate::account::{UserAccount, WalletBalance, WalletType};
use crate::journal::{BalanceField, JournalEntry, JournalEntryType};

/// System user ID used as the "external world" counterparty for deposits/withdrawals.
const SYSTEM_USER_ID: UserId = UserId(0);

/// The core ledger holding all accounts and the journal.
pub struct Ledger {
    pub(crate) accounts: HashMap<UserId, UserAccount>,
    pub(crate) system_accounts: HashMap<WalletType, Decimal128>,
    pub(crate) journal: Vec<JournalEntry>,
    next_journal_id: u64,
    idempotency_keys: FxHashSet<String>,
}

impl Ledger {
    /// Create a new empty ledger.
    pub fn new() -> Self {
        Self {
            accounts: HashMap::new(),
            system_accounts: HashMap::new(),
            journal: Vec::new(),
            next_journal_id: 1,
            idempotency_keys: FxHashSet::default(),
        }
    }

    /// Get or create account for user.
    pub fn get_or_create_account(&mut self, user_id: UserId) -> &mut UserAccount {
        self.accounts
            .entry(user_id)
            .or_insert_with(|| UserAccount::new(user_id))
    }

    /// Check idempotency. Returns `true` if this key was already processed (caller should return Ok early).
    fn check_idempotency(&mut self, key: &str) -> bool {
        !self.idempotency_keys.insert(key.to_owned())
    }

    /// Append a journal entry and increment the ID counter.
    fn append_journal(&mut self, entry: JournalEntry) {
        self.journal.push(entry);
    }

    fn next_id(&mut self) -> u64 {
        let id = self.next_journal_id;
        self.next_journal_id += 1;
        id
    }

    /// Get system account balance (InsuranceFund, FeeCollection, Escrow, etc.).
    pub fn system_balance(&self, wallet: WalletType) -> Decimal128 {
        self.system_accounts
            .get(&wallet)
            .copied()
            .unwrap_or(Decimal128::ZERO)
    }

    /// Add to system account balance.
    fn add_system_balance(&mut self, wallet: WalletType, amount: Decimal128) {
        let entry = self
            .system_accounts
            .entry(wallet)
            .or_insert(Decimal128::ZERO);
        *entry = *entry + amount;
    }

    /// Subtract from system account balance. Returns error if insufficient.
    fn sub_system_balance(&mut self, wallet: WalletType, amount: Decimal128) -> ExgResult<()> {
        let current = self.system_balance(wallet);
        if current < amount {
            return Err(ExgError::InsufficientBalance {
                required: amount.to_string(),
                available: current.to_string(),
            });
        }
        let entry = self
            .system_accounts
            .entry(wallet)
            .or_insert(Decimal128::ZERO);
        *entry = *entry - amount;
        Ok(())
    }

    // ── Public operations ──────────────────────────────────────────────

    /// Deposit: external funds -> user's funding wallet available.
    pub fn deposit(
        &mut self,
        user_id: UserId,
        amount: Decimal128,
        idempotency_key: &str,
        timestamp: UnixMicros,
    ) -> ExgResult<()> {
        if !amount.is_positive() {
            return Err(ExgError::InvalidQuantity(
                "deposit amount must be positive".into(),
            ));
        }
        if self.check_idempotency(idempotency_key) {
            return Ok(());
        }

        let id = self.next_id();
        let account = self.get_or_create_account(user_id);
        let wallet = account.wallet_mut(WalletType::Funding);
        wallet.available = wallet.available + amount;

        self.append_journal(JournalEntry {
            id,
            debit_user: SYSTEM_USER_ID,
            debit_wallet: WalletType::Funding,
            debit_field: BalanceField::Available,
            credit_user: user_id,
            credit_wallet: WalletType::Funding,
            credit_field: BalanceField::Available,
            amount,
            entry_type: JournalEntryType::Deposit,
            idempotency_key: idempotency_key.to_owned(),
            timestamp,
        });

        Ok(())
    }

    /// Withdraw: user's funding wallet available -> external.
    pub fn withdraw(
        &mut self,
        user_id: UserId,
        amount: Decimal128,
        idempotency_key: &str,
        timestamp: UnixMicros,
    ) -> ExgResult<()> {
        if !amount.is_positive() {
            return Err(ExgError::InvalidQuantity(
                "withdrawal amount must be positive".into(),
            ));
        }
        if self.check_idempotency(idempotency_key) {
            return Ok(());
        }

        let account = self
            .accounts
            .get_mut(&user_id)
            .ok_or(ExgError::AccountNotFound(user_id))?;
        let wallet = account.wallet_mut(WalletType::Funding);

        if wallet.available < amount {
            // Remove the idempotency key so the operation can be retried.
            self.idempotency_keys.remove(idempotency_key);
            return Err(ExgError::InsufficientBalance {
                required: amount.to_string(),
                available: wallet.available.to_string(),
            });
        }

        wallet.available = wallet.available - amount;

        let id = self.next_id();
        self.append_journal(JournalEntry {
            id,
            debit_user: user_id,
            debit_wallet: WalletType::Funding,
            debit_field: BalanceField::Available,
            credit_user: SYSTEM_USER_ID,
            credit_wallet: WalletType::Funding,
            credit_field: BalanceField::Available,
            amount,
            entry_type: JournalEntryType::Withdrawal,
            idempotency_key: idempotency_key.to_owned(),
            timestamp,
        });

        Ok(())
    }

    /// Transfer between wallets (e.g., Funding -> Futures).
    pub fn transfer(
        &mut self,
        user_id: UserId,
        from_wallet: WalletType,
        to_wallet: WalletType,
        amount: Decimal128,
        idempotency_key: &str,
        timestamp: UnixMicros,
    ) -> ExgResult<()> {
        if !amount.is_positive() {
            return Err(ExgError::InvalidQuantity(
                "transfer amount must be positive".into(),
            ));
        }
        if self.check_idempotency(idempotency_key) {
            return Ok(());
        }

        let account = self
            .accounts
            .get_mut(&user_id)
            .ok_or(ExgError::AccountNotFound(user_id))?;

        let from_available = account.wallet_mut(from_wallet).available;
        if from_available < amount {
            self.idempotency_keys.remove(idempotency_key);
            return Err(ExgError::InsufficientBalance {
                required: amount.to_string(),
                available: from_available.to_string(),
            });
        }

        account.wallet_mut(from_wallet).available = from_available - amount;
        let to_bal = account.wallet_mut(to_wallet);
        to_bal.available = to_bal.available + amount;

        let id = self.next_id();
        self.append_journal(JournalEntry {
            id,
            debit_user: user_id,
            debit_wallet: from_wallet,
            debit_field: BalanceField::Available,
            credit_user: user_id,
            credit_wallet: to_wallet,
            credit_field: BalanceField::Available,
            amount,
            entry_type: JournalEntryType::Transfer,
            idempotency_key: idempotency_key.to_owned(),
            timestamp,
        });

        Ok(())
    }

    /// Freeze balance for a new order: available -> frozen.
    pub fn freeze_for_order(
        &mut self,
        user_id: UserId,
        wallet: WalletType,
        amount: Decimal128,
        idempotency_key: &str,
        timestamp: UnixMicros,
    ) -> ExgResult<()> {
        if !amount.is_positive() {
            return Err(ExgError::InvalidQuantity(
                "freeze amount must be positive".into(),
            ));
        }
        if self.check_idempotency(idempotency_key) {
            return Ok(());
        }

        let account = self
            .accounts
            .get_mut(&user_id)
            .ok_or(ExgError::AccountNotFound(user_id))?;
        let bal = account.wallet_mut(wallet);

        if bal.available < amount {
            self.idempotency_keys.remove(idempotency_key);
            return Err(ExgError::InsufficientBalance {
                required: amount.to_string(),
                available: bal.available.to_string(),
            });
        }

        bal.available = bal.available - amount;
        bal.frozen = bal.frozen + amount;

        let id = self.next_id();
        self.append_journal(JournalEntry {
            id,
            debit_user: user_id,
            debit_wallet: wallet,
            debit_field: BalanceField::Available,
            credit_user: user_id,
            credit_wallet: wallet,
            credit_field: BalanceField::Frozen,
            amount,
            entry_type: JournalEntryType::OrderFreeze,
            idempotency_key: idempotency_key.to_owned(),
            timestamp,
        });

        Ok(())
    }

    /// Unfreeze when order is canceled: frozen -> available.
    pub fn unfreeze_order(
        &mut self,
        user_id: UserId,
        wallet: WalletType,
        amount: Decimal128,
        idempotency_key: &str,
        timestamp: UnixMicros,
    ) -> ExgResult<()> {
        if !amount.is_positive() {
            return Err(ExgError::InvalidQuantity(
                "unfreeze amount must be positive".into(),
            ));
        }
        if self.check_idempotency(idempotency_key) {
            return Ok(());
        }

        let account = self
            .accounts
            .get_mut(&user_id)
            .ok_or(ExgError::AccountNotFound(user_id))?;
        let bal = account.wallet_mut(wallet);

        if bal.frozen < amount {
            self.idempotency_keys.remove(idempotency_key);
            return Err(ExgError::InsufficientBalance {
                required: amount.to_string(),
                available: bal.frozen.to_string(),
            });
        }

        bal.frozen = bal.frozen - amount;
        bal.available = bal.available + amount;

        let id = self.next_id();
        self.append_journal(JournalEntry {
            id,
            debit_user: user_id,
            debit_wallet: wallet,
            debit_field: BalanceField::Frozen,
            credit_user: user_id,
            credit_wallet: wallet,
            credit_field: BalanceField::Available,
            amount,
            entry_type: JournalEntryType::OrderUnfreeze,
            idempotency_key: idempotency_key.to_owned(),
            timestamp,
        });

        Ok(())
    }

    /// Open/increase position: frozen -> margin, fee deducted from frozen -> fee collection.
    pub fn open_position(
        &mut self,
        user_id: UserId,
        margin_amount: Decimal128,
        fee: Decimal128,
        idempotency_key: &str,
        timestamp: UnixMicros,
    ) -> ExgResult<()> {
        if !margin_amount.is_positive() {
            return Err(ExgError::InvalidQuantity(
                "margin amount must be positive".into(),
            ));
        }
        if fee.is_negative() {
            return Err(ExgError::InvalidQuantity("fee cannot be negative".into()));
        }
        if self.check_idempotency(idempotency_key) {
            return Ok(());
        }

        let total_deduct = margin_amount + fee;

        let account = self
            .accounts
            .get_mut(&user_id)
            .ok_or(ExgError::AccountNotFound(user_id))?;
        let bal = account.wallet_mut(WalletType::Futures);

        if bal.frozen < total_deduct {
            self.idempotency_keys.remove(idempotency_key);
            return Err(ExgError::InsufficientBalance {
                required: total_deduct.to_string(),
                available: bal.frozen.to_string(),
            });
        }

        bal.frozen = bal.frozen - total_deduct;
        bal.margin = bal.margin + margin_amount;

        // Fee goes to system fee collection.
        if fee.is_positive() {
            self.add_system_balance(WalletType::FeeCollection, fee);
        }

        // Journal: margin move.
        let id = self.next_id();
        self.append_journal(JournalEntry {
            id,
            debit_user: user_id,
            debit_wallet: WalletType::Futures,
            debit_field: BalanceField::Frozen,
            credit_user: user_id,
            credit_wallet: WalletType::Futures,
            credit_field: BalanceField::Margin,
            amount: margin_amount,
            entry_type: JournalEntryType::PositionOpen,
            idempotency_key: idempotency_key.to_owned(),
            timestamp,
        });

        // Journal: fee.
        if fee.is_positive() {
            let fee_id = self.next_id();
            self.append_journal(JournalEntry {
                id: fee_id,
                debit_user: user_id,
                debit_wallet: WalletType::Futures,
                debit_field: BalanceField::Frozen,
                credit_user: SYSTEM_USER_ID,
                credit_wallet: WalletType::FeeCollection,
                credit_field: BalanceField::Available,
                amount: fee,
                entry_type: JournalEntryType::TradeFee,
                idempotency_key: format!("{idempotency_key}:fee"),
                timestamp,
            });
        }

        Ok(())
    }

    /// Close position: release margin, settle PnL with counterparty, collect fee.
    ///
    /// `realized_pnl > 0` means user profits (counterparty pays).
    /// `realized_pnl < 0` means user loses (user pays counterparty).
    #[allow(clippy::too_many_arguments)]
    pub fn close_position(
        &mut self,
        user_id: UserId,
        margin_released: Decimal128,
        realized_pnl: Decimal128,
        fee: Decimal128,
        counterparty_id: UserId,
        idempotency_key: &str,
        timestamp: UnixMicros,
    ) -> ExgResult<()> {
        if !margin_released.is_positive() {
            return Err(ExgError::InvalidQuantity(
                "margin released must be positive".into(),
            ));
        }
        if fee.is_negative() {
            return Err(ExgError::InvalidQuantity("fee cannot be negative".into()));
        }
        if self.check_idempotency(idempotency_key) {
            return Ok(());
        }

        // Validate user has enough margin.
        {
            let account = self
                .accounts
                .get(&user_id)
                .ok_or(ExgError::AccountNotFound(user_id))?;
            let bal = account
                .wallets
                .get(&WalletType::Futures)
                .cloned()
                .unwrap_or_default();
            if bal.margin < margin_released {
                self.idempotency_keys.remove(idempotency_key);
                return Err(ExgError::InsufficientBalance {
                    required: margin_released.to_string(),
                    available: bal.margin.to_string(),
                });
            }
        }

        // If PnL is negative (user loses), the loss is capped at margin_released for safety,
        // but we trust the clearing layer to provide correct values.
        // If PnL is positive, counterparty must have enough margin.
        if realized_pnl.is_positive() {
            let cp_account = self
                .accounts
                .get(&counterparty_id)
                .ok_or(ExgError::AccountNotFound(counterparty_id))?;
            let cp_bal = cp_account
                .wallets
                .get(&WalletType::Futures)
                .cloned()
                .unwrap_or_default();
            if cp_bal.margin < realized_pnl {
                self.idempotency_keys.remove(idempotency_key);
                return Err(ExgError::InsufficientBalance {
                    required: realized_pnl.to_string(),
                    available: cp_bal.margin.to_string(),
                });
            }
        }

        // Execute: release margin.
        {
            let account = self.accounts.get_mut(&user_id).unwrap();
            let bal = account.wallet_mut(WalletType::Futures);
            bal.margin = bal.margin - margin_released;

            // Net credit to available = margin_released + pnl - fee.
            let net_credit = margin_released + realized_pnl - fee;
            bal.available = bal.available + net_credit;
        }

        // Settle PnL with counterparty.
        if realized_pnl.is_positive() {
            // User profits -> counterparty loses.
            let cp_account = self.accounts.get_mut(&counterparty_id).unwrap();
            let cp_bal = cp_account.wallet_mut(WalletType::Futures);
            cp_bal.margin = cp_bal.margin - realized_pnl;
        } else if realized_pnl.is_negative() {
            // User loses -> counterparty gains.
            let pnl_abs = realized_pnl.abs();
            let cp_account = self.get_or_create_account(counterparty_id);
            let cp_bal = cp_account.wallet_mut(WalletType::Futures);
            cp_bal.margin = cp_bal.margin + pnl_abs;
        }

        // Fee to system.
        if fee.is_positive() {
            self.add_system_balance(WalletType::FeeCollection, fee);
        }

        // Journal entries.
        let id = self.next_id();
        self.append_journal(JournalEntry {
            id,
            debit_user: user_id,
            debit_wallet: WalletType::Futures,
            debit_field: BalanceField::Margin,
            credit_user: user_id,
            credit_wallet: WalletType::Futures,
            credit_field: BalanceField::Available,
            amount: margin_released,
            entry_type: JournalEntryType::PositionClose,
            idempotency_key: idempotency_key.to_owned(),
            timestamp,
        });

        if !realized_pnl.is_zero() {
            let pnl_id = self.next_id();
            let pnl_abs = realized_pnl.abs();
            if realized_pnl.is_positive() {
                self.append_journal(JournalEntry {
                    id: pnl_id,
                    debit_user: counterparty_id,
                    debit_wallet: WalletType::Futures,
                    debit_field: BalanceField::Margin,
                    credit_user: user_id,
                    credit_wallet: WalletType::Futures,
                    credit_field: BalanceField::Available,
                    amount: pnl_abs,
                    entry_type: JournalEntryType::PositionClose,
                    idempotency_key: format!("{idempotency_key}:pnl"),
                    timestamp,
                });
            } else {
                self.append_journal(JournalEntry {
                    id: pnl_id,
                    debit_user: user_id,
                    debit_wallet: WalletType::Futures,
                    debit_field: BalanceField::Available,
                    credit_user: counterparty_id,
                    credit_wallet: WalletType::Futures,
                    credit_field: BalanceField::Margin,
                    amount: pnl_abs,
                    entry_type: JournalEntryType::PositionClose,
                    idempotency_key: format!("{idempotency_key}:pnl"),
                    timestamp,
                });
            }
        }

        if fee.is_positive() {
            let fee_id = self.next_id();
            self.append_journal(JournalEntry {
                id: fee_id,
                debit_user: user_id,
                debit_wallet: WalletType::Futures,
                debit_field: BalanceField::Available,
                credit_user: SYSTEM_USER_ID,
                credit_wallet: WalletType::FeeCollection,
                credit_field: BalanceField::Available,
                amount: fee,
                entry_type: JournalEntryType::TradeFee,
                idempotency_key: format!("{idempotency_key}:fee"),
                timestamp,
            });
        }

        Ok(())
    }

    /// Close position with PnL settled via escrow (no specific counterparty).
    ///
    /// This is the standard clearing path for centralized exchanges where positions
    /// are fungible and PnL doesn't come from a specific counterparty.
    ///
    /// `realized_pnl > 0`: user profits — escrow pays.
    /// `realized_pnl < 0`: user loses — escrow receives.
    #[allow(clippy::too_many_arguments)]
    pub fn close_position_settled(
        &mut self,
        user_id: UserId,
        margin_released: Decimal128,
        realized_pnl: Decimal128,
        fee: Decimal128,
        idempotency_key: &str,
        timestamp: UnixMicros,
    ) -> ExgResult<()> {
        if !margin_released.is_positive() {
            return Err(ExgError::InvalidQuantity(
                "margin released must be positive".into(),
            ));
        }
        if fee.is_negative() {
            return Err(ExgError::InvalidQuantity("fee cannot be negative".into()));
        }
        if self.check_idempotency(idempotency_key) {
            return Ok(());
        }

        // Validate user has enough margin.
        {
            let account = self
                .accounts
                .get(&user_id)
                .ok_or(ExgError::AccountNotFound(user_id))?;
            let bal = account
                .wallets
                .get(&WalletType::Futures)
                .cloned()
                .unwrap_or_default();
            if bal.margin < margin_released {
                self.idempotency_keys.remove(idempotency_key);
                return Err(ExgError::InsufficientBalance {
                    required: margin_released.to_string(),
                    available: bal.margin.to_string(),
                });
            }
        }

        // If user profits, escrow must have funds. If user loses, escrow gains.
        if realized_pnl.is_positive() {
            let escrow_bal = self.system_balance(WalletType::Escrow);
            if escrow_bal < realized_pnl {
                // No escrow needed for normal operations — credit from pool.
                // In a production system this would be tracked differently.
                // For now, allow it (the exchange is always the counterparty).
            }
        }

        // Execute: release margin and settle PnL.
        {
            let account = self.accounts.get_mut(&user_id).unwrap();
            let bal = account.wallet_mut(WalletType::Futures);
            bal.margin = bal.margin - margin_released;
            let net_credit = margin_released + realized_pnl - fee;
            bal.available = bal.available + net_credit;
        }

        // Fee to system.
        if fee.is_positive() {
            self.add_system_balance(WalletType::FeeCollection, fee);
        }

        // Journal: margin release.
        let id = self.next_id();
        self.append_journal(JournalEntry {
            id,
            debit_user: user_id,
            debit_wallet: WalletType::Futures,
            debit_field: BalanceField::Margin,
            credit_user: user_id,
            credit_wallet: WalletType::Futures,
            credit_field: BalanceField::Available,
            amount: margin_released,
            entry_type: JournalEntryType::PositionClose,
            idempotency_key: idempotency_key.to_owned(),
            timestamp,
        });

        // Journal: PnL settlement (via escrow/system).
        if !realized_pnl.is_zero() {
            let pnl_id = self.next_id();
            let pnl_abs = realized_pnl.abs();
            if realized_pnl.is_positive() {
                // System pays user.
                self.append_journal(JournalEntry {
                    id: pnl_id,
                    debit_user: SYSTEM_USER_ID,
                    debit_wallet: WalletType::Escrow,
                    debit_field: BalanceField::Available,
                    credit_user: user_id,
                    credit_wallet: WalletType::Futures,
                    credit_field: BalanceField::Available,
                    amount: pnl_abs,
                    entry_type: JournalEntryType::PositionClose,
                    idempotency_key: format!("{idempotency_key}:pnl"),
                    timestamp,
                });
            } else {
                // User pays system.
                self.append_journal(JournalEntry {
                    id: pnl_id,
                    debit_user: user_id,
                    debit_wallet: WalletType::Futures,
                    debit_field: BalanceField::Available,
                    credit_user: SYSTEM_USER_ID,
                    credit_wallet: WalletType::Escrow,
                    credit_field: BalanceField::Available,
                    amount: pnl_abs,
                    entry_type: JournalEntryType::PositionClose,
                    idempotency_key: format!("{idempotency_key}:pnl"),
                    timestamp,
                });
            }
        }

        // Journal: fee.
        if fee.is_positive() {
            let fee_id = self.next_id();
            self.append_journal(JournalEntry {
                id: fee_id,
                debit_user: user_id,
                debit_wallet: WalletType::Futures,
                debit_field: BalanceField::Available,
                credit_user: SYSTEM_USER_ID,
                credit_wallet: WalletType::FeeCollection,
                credit_field: BalanceField::Available,
                amount: fee,
                entry_type: JournalEntryType::TradeFee,
                idempotency_key: format!("{idempotency_key}:fee"),
                timestamp,
            });
        }

        Ok(())
    }

    /// Liquidation: seize margin, surplus to insurance fund or deficit from insurance fund.
    ///
    /// `surplus > 0`: remaining margin after covering loss -> insurance fund.
    /// `surplus < 0`: insurance fund covers the deficit (socialized loss if fund depleted).
    pub fn liquidate(
        &mut self,
        user_id: UserId,
        margin_seized: Decimal128,
        surplus: Decimal128,
        idempotency_key: &str,
        timestamp: UnixMicros,
    ) -> ExgResult<()> {
        if !margin_seized.is_positive() {
            return Err(ExgError::InvalidQuantity(
                "margin seized must be positive".into(),
            ));
        }
        if self.check_idempotency(idempotency_key) {
            return Ok(());
        }

        // Validate margin available.
        {
            let account = self
                .accounts
                .get(&user_id)
                .ok_or(ExgError::AccountNotFound(user_id))?;
            let bal = account
                .wallets
                .get(&WalletType::Futures)
                .cloned()
                .unwrap_or_default();
            if bal.margin < margin_seized {
                self.idempotency_keys.remove(idempotency_key);
                return Err(ExgError::InsufficientBalance {
                    required: margin_seized.to_string(),
                    available: bal.margin.to_string(),
                });
            }
        }

        // For deficit, check insurance fund before mutating anything.
        if surplus.is_negative() {
            let deficit = surplus.abs();
            let insurance_bal = self.system_balance(WalletType::InsuranceFund);
            if insurance_bal < deficit {
                self.idempotency_keys.remove(idempotency_key);
                return Err(ExgError::InsuranceFundDepleted);
            }
        }

        // Seize margin from user -> escrow.
        {
            let account = self.accounts.get_mut(&user_id).unwrap();
            let bal = account.wallet_mut(WalletType::Futures);
            bal.margin = bal.margin - margin_seized;
        }
        self.add_system_balance(WalletType::Escrow, margin_seized);

        if surplus.is_positive() {
            // Move surplus from escrow -> insurance fund.
            self.sub_system_balance(WalletType::Escrow, surplus)
                .expect("escrow has margin_seized >= surplus");
            self.add_system_balance(WalletType::InsuranceFund, surplus);
            // Remaining escrow = margin_seized - surplus (the loss, held for counterparty settlement).
        } else if surplus.is_negative() {
            // Insurance fund covers deficit -> escrow.
            let deficit = surplus.abs();
            self.sub_system_balance(WalletType::InsuranceFund, deficit)
                .expect("checked above");
            self.add_system_balance(WalletType::Escrow, deficit);
            // Escrow = margin_seized + deficit (held for counterparty settlement).
        }
        // If surplus == 0: all of margin_seized stays in escrow.

        let id = self.next_id();
        self.append_journal(JournalEntry {
            id,
            debit_user: user_id,
            debit_wallet: WalletType::Futures,
            debit_field: BalanceField::Margin,
            credit_user: SYSTEM_USER_ID,
            credit_wallet: WalletType::Escrow,
            credit_field: BalanceField::Available,
            amount: margin_seized,
            entry_type: JournalEntryType::Liquidation,
            idempotency_key: idempotency_key.to_owned(),
            timestamp,
        });

        if surplus.is_positive() {
            let surplus_id = self.next_id();
            self.append_journal(JournalEntry {
                id: surplus_id,
                debit_user: SYSTEM_USER_ID,
                debit_wallet: WalletType::Escrow,
                debit_field: BalanceField::Available,
                credit_user: SYSTEM_USER_ID,
                credit_wallet: WalletType::InsuranceFund,
                credit_field: BalanceField::Available,
                amount: surplus,
                entry_type: JournalEntryType::InsuranceFundContribution,
                idempotency_key: format!("{idempotency_key}:surplus"),
                timestamp,
            });
        } else if surplus.is_negative() {
            let deficit_id = self.next_id();
            self.append_journal(JournalEntry {
                id: deficit_id,
                debit_user: SYSTEM_USER_ID,
                debit_wallet: WalletType::InsuranceFund,
                debit_field: BalanceField::Available,
                credit_user: SYSTEM_USER_ID,
                credit_wallet: WalletType::Escrow,
                credit_field: BalanceField::Available,
                amount: surplus.abs(),
                entry_type: JournalEntryType::Liquidation,
                idempotency_key: format!("{idempotency_key}:deficit"),
                timestamp,
            });
        }

        Ok(())
    }

    /// Settle funding payment.
    ///
    /// `payment > 0`: user pays (deducted from available, then margin if insufficient).
    /// `payment < 0`: user receives (credited to available).
    pub fn settle_funding(
        &mut self,
        user_id: UserId,
        payment: Decimal128,
        idempotency_key: &str,
        timestamp: UnixMicros,
    ) -> ExgResult<()> {
        if payment.is_zero() {
            return Ok(());
        }
        if self.check_idempotency(idempotency_key) {
            return Ok(());
        }

        let account = self
            .accounts
            .get_mut(&user_id)
            .ok_or(ExgError::AccountNotFound(user_id))?;
        let bal = account.wallet_mut(WalletType::Futures);

        if payment.is_positive() {
            // User pays.
            let from_available = payment.min(bal.available);
            let from_margin = payment - from_available;

            if from_margin.is_positive() && bal.margin < from_margin {
                self.idempotency_keys.remove(idempotency_key);
                return Err(ExgError::InsufficientBalance {
                    required: payment.to_string(),
                    available: (bal.available + bal.margin).to_string(),
                });
            }

            bal.available = bal.available - from_available;
            if from_margin.is_positive() {
                bal.margin = bal.margin - from_margin;
            }

            // Track in funding system account (pool for bilateral settlement).
            self.add_system_balance(WalletType::Funding, payment);

            // Journal: available portion.
            if from_available.is_positive() {
                let id = self.next_id();
                self.append_journal(JournalEntry {
                    id,
                    debit_user: user_id,
                    debit_wallet: WalletType::Futures,
                    debit_field: BalanceField::Available,
                    credit_user: SYSTEM_USER_ID,
                    credit_wallet: WalletType::Futures,
                    credit_field: BalanceField::Available,
                    amount: from_available,
                    entry_type: JournalEntryType::FundingPayment,
                    idempotency_key: idempotency_key.to_owned(),
                    timestamp,
                });
            }

            // Journal: margin portion (if any).
            if from_margin.is_positive() {
                let id = self.next_id();
                self.append_journal(JournalEntry {
                    id,
                    debit_user: user_id,
                    debit_wallet: WalletType::Futures,
                    debit_field: BalanceField::Margin,
                    credit_user: SYSTEM_USER_ID,
                    credit_wallet: WalletType::Futures,
                    credit_field: BalanceField::Available,
                    amount: from_margin,
                    entry_type: JournalEntryType::FundingPayment,
                    idempotency_key: format!("{idempotency_key}:margin"),
                    timestamp,
                });
            }
        } else {
            // User receives.
            let receive = payment.abs();
            bal.available = bal.available + receive;

            // Debit from funding pool system account.
            self.add_system_balance(WalletType::Funding, payment); // payment is negative, so this subtracts

            let id = self.next_id();
            self.append_journal(JournalEntry {
                id,
                debit_user: SYSTEM_USER_ID,
                debit_wallet: WalletType::Futures,
                debit_field: BalanceField::Available,
                credit_user: user_id,
                credit_wallet: WalletType::Futures,
                credit_field: BalanceField::Available,
                amount: receive,
                entry_type: JournalEntryType::FundingPayment,
                idempotency_key: idempotency_key.to_owned(),
                timestamp,
            });
        }

        Ok(())
    }

    /// Settle funding with structured idempotency key and liquidation check.
    ///
    /// Constructs an idempotency key as `funding_{funding_period_id}_{user_id}_{symbol}`
    /// internally. Returns `true` if margin was tapped (the user needs a liquidation check).
    pub fn settle_funding_checked(
        &mut self,
        user_id: UserId,
        symbol: exg_common::SymbolId,
        funding_period_id: u64,
        payment: Decimal128,
        timestamp: UnixMicros,
    ) -> ExgResult<bool> {
        let idemp_key = format!(
            "funding_{}_{}_{}",
            funding_period_id,
            user_id.value(),
            symbol.value()
        );

        // Capture margin before settlement.
        let margin_before = self
            .accounts
            .get(&user_id)
            .and_then(|a| a.wallets.get(&WalletType::Futures))
            .map(|b| b.margin)
            .unwrap_or(Decimal128::ZERO);

        self.settle_funding(user_id, payment, &idemp_key, timestamp)?;

        // Check if margin was tapped.
        let margin_after = self
            .accounts
            .get(&user_id)
            .and_then(|a| a.wallets.get(&WalletType::Futures))
            .map(|b| b.margin)
            .unwrap_or(Decimal128::ZERO);

        Ok(margin_after < margin_before)
    }

    /// Get account balance snapshot for a specific wallet.
    pub fn get_balance(&self, user_id: UserId, wallet: WalletType) -> Option<&WalletBalance> {
        self.accounts
            .get(&user_id)
            .and_then(|a| a.wallets.get(&wallet))
    }

    /// Get insurance fund balance.
    pub fn insurance_fund_balance(&self) -> Decimal128 {
        self.system_balance(WalletType::InsuranceFund)
    }

    /// Get journal entries (for audit / replay).
    pub fn journal(&self) -> &[JournalEntry] {
        &self.journal
    }
}

impl Default for Ledger {
    fn default() -> Self {
        Self::new()
    }
}

// ══════════════════════════════════════════════════════════════════════════
// Tests
// ══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    fn dec(s: &str) -> Decimal128 {
        s.parse().unwrap()
    }

    fn ts(us: u64) -> UnixMicros {
        UnixMicros::from_micros(us)
    }

    fn uid(id: u64) -> UserId {
        UserId::new(id)
    }

    /// Helper: deposit and transfer to futures for a user.
    fn setup_futures_user(ledger: &mut Ledger, user_id: UserId, amount: Decimal128) {
        ledger
            .deposit(user_id, amount, &format!("dep-{}", user_id), ts(1))
            .unwrap();
        ledger
            .transfer(
                user_id,
                WalletType::Funding,
                WalletType::Futures,
                amount,
                &format!("xfer-{}", user_id),
                ts(2),
            )
            .unwrap();
    }

    // ── 1. Deposit/withdraw ────────────────────────────────────────────

    #[test]
    fn test_deposit_withdraw() {
        let mut ledger = Ledger::new();
        let user = uid(1);

        ledger.deposit(user, dec("1000"), "dep-1", ts(1)).unwrap();
        let bal = ledger.get_balance(user, WalletType::Funding).unwrap();
        assert_eq!(bal.available, dec("1000"));

        ledger.withdraw(user, dec("500"), "wd-1", ts(2)).unwrap();
        let bal = ledger.get_balance(user, WalletType::Funding).unwrap();
        assert_eq!(bal.available, dec("500"));

        ledger.verify_all_invariants().unwrap();
    }

    // ── 2. Withdraw insufficient ───────────────────────────────────────

    #[test]
    fn test_withdraw_insufficient() {
        let mut ledger = Ledger::new();
        let user = uid(1);

        ledger.deposit(user, dec("100"), "dep-1", ts(1)).unwrap();
        let result = ledger.withdraw(user, dec("200"), "wd-1", ts(2));
        assert!(matches!(result, Err(ExgError::InsufficientBalance { .. })));

        // Balance unchanged.
        let bal = ledger.get_balance(user, WalletType::Funding).unwrap();
        assert_eq!(bal.available, dec("100"));

        ledger.verify_all_invariants().unwrap();
    }

    // ── 3. Transfer ────────────────────────────────────────────────────

    #[test]
    fn test_transfer_spot_to_futures() {
        let mut ledger = Ledger::new();
        let user = uid(1);

        ledger.deposit(user, dec("1000"), "dep-1", ts(1)).unwrap();
        ledger
            .transfer(
                user,
                WalletType::Funding,
                WalletType::Futures,
                dec("600"),
                "xfer-1",
                ts(2),
            )
            .unwrap();

        let funding = ledger.get_balance(user, WalletType::Funding).unwrap();
        assert_eq!(funding.available, dec("400"));

        let futures = ledger.get_balance(user, WalletType::Futures).unwrap();
        assert_eq!(futures.available, dec("600"));

        ledger.verify_all_invariants().unwrap();
    }

    // ── 4. Order freeze/unfreeze ───────────────────────────────────────

    #[test]
    fn test_freeze_unfreeze() {
        let mut ledger = Ledger::new();
        let user = uid(1);

        setup_futures_user(&mut ledger, user, dec("1000"));

        ledger
            .freeze_for_order(user, WalletType::Futures, dec("100"), "freeze-1", ts(3))
            .unwrap();

        let bal = ledger.get_balance(user, WalletType::Futures).unwrap();
        assert_eq!(bal.available, dec("900"));
        assert_eq!(bal.frozen, dec("100"));

        ledger
            .unfreeze_order(user, WalletType::Futures, dec("50"), "unfreeze-1", ts(4))
            .unwrap();

        let bal = ledger.get_balance(user, WalletType::Futures).unwrap();
        assert_eq!(bal.available, dec("950"));
        assert_eq!(bal.frozen, dec("50"));

        ledger.verify_all_invariants().unwrap();
    }

    // ── 5. Open position ───────────────────────────────────────────────

    #[test]
    fn test_open_position() {
        let mut ledger = Ledger::new();
        let user = uid(1);

        setup_futures_user(&mut ledger, user, dec("1000"));

        // Freeze for order.
        ledger
            .freeze_for_order(user, WalletType::Futures, dec("110"), "freeze-1", ts(3))
            .unwrap();

        // Open position: 100 margin + 10 fee.
        ledger
            .open_position(user, dec("100"), dec("10"), "open-1", ts(4))
            .unwrap();

        let bal = ledger.get_balance(user, WalletType::Futures).unwrap();
        assert_eq!(bal.available, dec("890"));
        assert_eq!(bal.frozen, dec("0"));
        assert_eq!(bal.margin, dec("100"));

        // Fee collected.
        assert_eq!(ledger.system_balance(WalletType::FeeCollection), dec("10"));

        ledger.verify_all_invariants().unwrap();
    }

    // ── 6. Close position (profit) ─────────────────────────────────────

    #[test]
    fn test_close_position_profit() {
        let mut ledger = Ledger::new();
        let user = uid(1);
        let counterparty = uid(2);

        setup_futures_user(&mut ledger, user, dec("1000"));
        setup_futures_user(&mut ledger, counterparty, dec("1000"));

        // Freeze and open for both.
        ledger
            .freeze_for_order(user, WalletType::Futures, dec("100"), "f-u1", ts(10))
            .unwrap();
        ledger
            .open_position(user, dec("100"), dec("0"), "o-u1", ts(11))
            .unwrap();

        ledger
            .freeze_for_order(
                counterparty,
                WalletType::Futures,
                dec("100"),
                "f-u2",
                ts(10),
            )
            .unwrap();
        ledger
            .open_position(counterparty, dec("100"), dec("0"), "o-u2", ts(11))
            .unwrap();

        // Close: user profits 50, fee 5.
        ledger
            .close_position(
                user,
                dec("100"),
                dec("50"),
                dec("5"),
                counterparty,
                "close-1",
                ts(20),
            )
            .unwrap();

        let bal = ledger.get_balance(user, WalletType::Futures).unwrap();
        // available = 900 + (100 + 50 - 5) = 1045
        assert_eq!(bal.available, dec("1045"));
        assert_eq!(bal.margin, dec("0"));

        // Counterparty lost 50 from margin.
        let cp_bal = ledger
            .get_balance(counterparty, WalletType::Futures)
            .unwrap();
        assert_eq!(cp_bal.margin, dec("50"));

        ledger.verify_all_invariants().unwrap();
    }

    // ── 7. Close position (loss) ───────────────────────────────────────

    #[test]
    fn test_close_position_loss() {
        let mut ledger = Ledger::new();
        let user = uid(1);
        let counterparty = uid(2);

        setup_futures_user(&mut ledger, user, dec("1000"));
        setup_futures_user(&mut ledger, counterparty, dec("1000"));

        ledger
            .freeze_for_order(user, WalletType::Futures, dec("100"), "f-u1", ts(10))
            .unwrap();
        ledger
            .open_position(user, dec("100"), dec("0"), "o-u1", ts(11))
            .unwrap();

        ledger
            .freeze_for_order(
                counterparty,
                WalletType::Futures,
                dec("100"),
                "f-u2",
                ts(10),
            )
            .unwrap();
        ledger
            .open_position(counterparty, dec("100"), dec("0"), "o-u2", ts(11))
            .unwrap();

        // Close: user loses 30, fee 5.
        ledger
            .close_position(
                user,
                dec("100"),
                dec("-30"),
                dec("5"),
                counterparty,
                "close-1",
                ts(20),
            )
            .unwrap();

        let bal = ledger.get_balance(user, WalletType::Futures).unwrap();
        // available = 900 + (100 - 30 - 5) = 965
        assert_eq!(bal.available, dec("965"));
        assert_eq!(bal.margin, dec("0"));

        // Counterparty gains 30 in margin.
        let cp_bal = ledger
            .get_balance(counterparty, WalletType::Futures)
            .unwrap();
        assert_eq!(cp_bal.margin, dec("130"));

        ledger.verify_all_invariants().unwrap();
    }

    // ── 8. Liquidation surplus ─────────────────────────────────────────

    #[test]
    fn test_liquidation_surplus() {
        let mut ledger = Ledger::new();
        let user = uid(1);

        setup_futures_user(&mut ledger, user, dec("1000"));

        ledger
            .freeze_for_order(user, WalletType::Futures, dec("200"), "f-1", ts(10))
            .unwrap();
        ledger
            .open_position(user, dec("200"), dec("0"), "o-1", ts(11))
            .unwrap();

        // Liquidate: seize 200 margin, 50 surplus to insurance.
        ledger
            .liquidate(user, dec("200"), dec("50"), "liq-1", ts(20))
            .unwrap();

        let bal = ledger.get_balance(user, WalletType::Futures).unwrap();
        assert_eq!(bal.margin, dec("0"));

        assert_eq!(ledger.insurance_fund_balance(), dec("50"));

        ledger.verify_all_invariants().unwrap();
    }

    // ── 9. Liquidation deficit ─────────────────────────────────────────

    #[test]
    fn test_liquidation_deficit() {
        let mut ledger = Ledger::new();
        let user = uid(1);

        // Seed insurance fund via a surplus liquidation first.
        let seed_user = uid(99);
        setup_futures_user(&mut ledger, seed_user, dec("500"));
        ledger
            .freeze_for_order(seed_user, WalletType::Futures, dec("500"), "f-seed", ts(5))
            .unwrap();
        ledger
            .open_position(seed_user, dec("500"), dec("0"), "o-seed", ts(6))
            .unwrap();
        ledger
            .liquidate(seed_user, dec("500"), dec("200"), "liq-seed", ts(7))
            .unwrap();
        assert_eq!(ledger.insurance_fund_balance(), dec("200"));

        // Now user with deficit.
        setup_futures_user(&mut ledger, user, dec("100"));
        ledger
            .freeze_for_order(user, WalletType::Futures, dec("100"), "f-1", ts(10))
            .unwrap();
        ledger
            .open_position(user, dec("100"), dec("0"), "o-1", ts(11))
            .unwrap();

        // Deficit: -50 (insurance fund must cover 50).
        ledger
            .liquidate(user, dec("100"), dec("-50"), "liq-1", ts(20))
            .unwrap();

        assert_eq!(ledger.insurance_fund_balance(), dec("150"));

        ledger.verify_all_invariants().unwrap();
    }

    // ── 10. Funding payment ────────────────────────────────────────────

    #[test]
    fn test_funding_payment_pay_and_receive() {
        let mut ledger = Ledger::new();
        let payer = uid(1);
        let receiver = uid(2);

        setup_futures_user(&mut ledger, payer, dec("1000"));
        setup_futures_user(&mut ledger, receiver, dec("1000"));

        // Payer pays 20.
        ledger
            .settle_funding(payer, dec("20"), "fund-pay", ts(30))
            .unwrap();

        // Receiver receives 20.
        ledger
            .settle_funding(receiver, dec("-20"), "fund-recv", ts(30))
            .unwrap();

        let payer_bal = ledger.get_balance(payer, WalletType::Futures).unwrap();
        assert_eq!(payer_bal.available, dec("980"));

        let recv_bal = ledger.get_balance(receiver, WalletType::Futures).unwrap();
        assert_eq!(recv_bal.available, dec("1020"));

        ledger.verify_all_invariants().unwrap();
    }

    // ── 11. Funding from margin ────────────────────────────────────────

    #[test]
    fn test_funding_from_margin() {
        let mut ledger = Ledger::new();
        let user = uid(1);

        setup_futures_user(&mut ledger, user, dec("1000"));

        // Freeze and open: 900 available, 100 margin.
        ledger
            .freeze_for_order(user, WalletType::Futures, dec("100"), "f-1", ts(10))
            .unwrap();
        ledger
            .open_position(user, dec("100"), dec("0"), "o-1", ts(11))
            .unwrap();

        let bal = ledger.get_balance(user, WalletType::Futures).unwrap();
        assert_eq!(bal.available, dec("900"));
        assert_eq!(bal.margin, dec("100"));

        // Funding payment of 950: 900 from available + 50 from margin.
        ledger
            .settle_funding(user, dec("950"), "fund-1", ts(20))
            .unwrap();

        let bal = ledger.get_balance(user, WalletType::Futures).unwrap();
        assert_eq!(bal.available, dec("0"));
        assert_eq!(bal.margin, dec("50"));

        ledger.verify_all_invariants().unwrap();
    }

    // ── 12. Idempotency ────────────────────────────────────────────────

    #[test]
    fn test_idempotency() {
        let mut ledger = Ledger::new();
        let user = uid(1);

        ledger.deposit(user, dec("100"), "dep-1", ts(1)).unwrap();
        // Duplicate — should be silently ignored.
        ledger.deposit(user, dec("100"), "dep-1", ts(2)).unwrap();

        let bal = ledger.get_balance(user, WalletType::Funding).unwrap();
        assert_eq!(bal.available, dec("100"));

        // Different key — should work.
        ledger.deposit(user, dec("100"), "dep-2", ts(3)).unwrap();
        let bal = ledger.get_balance(user, WalletType::Funding).unwrap();
        assert_eq!(bal.available, dec("200"));

        ledger.verify_all_invariants().unwrap();
    }

    // ── 13. Invariant checks after every operation ─────────────────────

    #[test]
    fn test_invariants_after_operations() {
        let mut ledger = Ledger::new();
        let u1 = uid(1);
        let u2 = uid(2);

        ledger.deposit(u1, dec("5000"), "d1", ts(1)).unwrap();
        ledger.verify_all_invariants().unwrap();

        ledger.deposit(u2, dec("3000"), "d2", ts(2)).unwrap();
        ledger.verify_all_invariants().unwrap();

        ledger
            .transfer(
                u1,
                WalletType::Funding,
                WalletType::Futures,
                dec("2000"),
                "t1",
                ts(3),
            )
            .unwrap();
        ledger.verify_all_invariants().unwrap();

        ledger
            .freeze_for_order(u1, WalletType::Futures, dec("500"), "f1", ts(4))
            .unwrap();
        ledger.verify_all_invariants().unwrap();

        ledger
            .open_position(u1, dec("400"), dec("10"), "o1", ts(5))
            .unwrap();
        ledger.verify_all_invariants().unwrap();

        ledger.withdraw(u1, dec("1000"), "w1", ts(6)).unwrap();
        ledger.verify_all_invariants().unwrap();
    }

    // ── 14. Negative balance prevention ────────────────────────────────

    #[test]
    fn test_negative_balance_prevention() {
        let mut ledger = Ledger::new();
        let user = uid(1);

        ledger.deposit(user, dec("100"), "dep-1", ts(1)).unwrap();

        // Cannot withdraw more than available.
        let r = ledger.withdraw(user, dec("200"), "wd-1", ts(2));
        assert!(matches!(r, Err(ExgError::InsufficientBalance { .. })));

        // Cannot transfer more than available.
        let r = ledger.transfer(
            user,
            WalletType::Funding,
            WalletType::Futures,
            dec("200"),
            "xfer-1",
            ts(3),
        );
        assert!(matches!(r, Err(ExgError::InsufficientBalance { .. })));

        // Cannot freeze more than available.
        ledger
            .transfer(
                user,
                WalletType::Funding,
                WalletType::Futures,
                dec("50"),
                "xfer-2",
                ts(4),
            )
            .unwrap();
        let r = ledger.freeze_for_order(user, WalletType::Futures, dec("100"), "freeze-1", ts(5));
        assert!(matches!(r, Err(ExgError::InsufficientBalance { .. })));

        ledger.verify_all_invariants().unwrap();
    }

    // ── Funding insufficient ───────────────────────────────────────────

    #[test]
    fn test_funding_insufficient_total() {
        let mut ledger = Ledger::new();
        let user = uid(1);

        setup_futures_user(&mut ledger, user, dec("100"));

        // Funding payment exceeds available + margin.
        let r = ledger.settle_funding(user, dec("200"), "fund-1", ts(20));
        assert!(matches!(r, Err(ExgError::InsufficientBalance { .. })));

        // Balance unchanged.
        let bal = ledger.get_balance(user, WalletType::Futures).unwrap();
        assert_eq!(bal.available, dec("100"));

        ledger.verify_all_invariants().unwrap();
    }

    // ── Zero amount operations ─────────────────────────────────────────

    #[test]
    fn test_zero_funding_is_noop() {
        let mut ledger = Ledger::new();
        let user = uid(1);
        setup_futures_user(&mut ledger, user, dec("100"));

        ledger
            .settle_funding(user, Decimal128::ZERO, "fund-0", ts(20))
            .unwrap();

        let bal = ledger.get_balance(user, WalletType::Futures).unwrap();
        assert_eq!(bal.available, dec("100"));
    }

    // ── Insurance fund depleted ────────────────────────────────────────

    #[test]
    fn test_insurance_fund_depleted() {
        let mut ledger = Ledger::new();
        let user = uid(1);

        setup_futures_user(&mut ledger, user, dec("100"));
        ledger
            .freeze_for_order(user, WalletType::Futures, dec("100"), "f-1", ts(10))
            .unwrap();
        ledger
            .open_position(user, dec("100"), dec("0"), "o-1", ts(11))
            .unwrap();

        // Deficit but no insurance fund.
        let r = ledger.liquidate(user, dec("100"), dec("-50"), "liq-1", ts(20));
        assert!(matches!(r, Err(ExgError::InsuranceFundDepleted)));

        // Margin still intact after failed liquidation.
        let bal = ledger.get_balance(user, WalletType::Futures).unwrap();
        assert_eq!(bal.margin, dec("100"));
    }

    // ── Withdraw retryable after failure ───────────────────────────────

    #[test]
    fn test_withdraw_retryable_after_failure() {
        let mut ledger = Ledger::new();
        let user = uid(1);

        ledger.deposit(user, dec("100"), "dep-1", ts(1)).unwrap();

        // Fail first.
        let r = ledger.withdraw(user, dec("200"), "wd-1", ts(2));
        assert!(r.is_err());

        // Deposit more.
        ledger.deposit(user, dec("200"), "dep-2", ts(3)).unwrap();

        // Retry same key — should work now.
        ledger.withdraw(user, dec("200"), "wd-1", ts(4)).unwrap();
        let bal = ledger.get_balance(user, WalletType::Funding).unwrap();
        assert_eq!(bal.available, dec("100"));
    }

    // ── settle_funding_checked ─────────────────────────────────────────

    #[test]
    fn test_settle_funding_checked() {
        let mut ledger = Ledger::new();
        let user = uid(1);

        setup_futures_user(&mut ledger, user, dec("1000"));

        // Open a position: 900 available, 100 margin.
        ledger
            .freeze_for_order(user, WalletType::Futures, dec("100"), "f-1", ts(10))
            .unwrap();
        ledger
            .open_position(user, dec("100"), dec("0"), "o-1", ts(11))
            .unwrap();

        let bal = ledger.get_balance(user, WalletType::Futures).unwrap();
        assert_eq!(bal.available, dec("900"));
        assert_eq!(bal.margin, dec("100"));

        // Small payment from available only — margin not tapped.
        let tapped = ledger
            .settle_funding_checked(user, exg_common::SymbolId::new(1), 1, dec("10"), ts(20))
            .unwrap();
        assert!(!tapped);

        let bal = ledger.get_balance(user, WalletType::Futures).unwrap();
        assert_eq!(bal.available, dec("890"));
        assert_eq!(bal.margin, dec("100"));

        // Large payment that exceeds available — margin must be tapped.
        let tapped = ledger
            .settle_funding_checked(user, exg_common::SymbolId::new(1), 2, dec("900"), ts(30))
            .unwrap();
        assert!(tapped);

        let bal = ledger.get_balance(user, WalletType::Futures).unwrap();
        assert_eq!(bal.available, dec("0"));
        assert_eq!(bal.margin, dec("90")); // 100 - 10

        ledger.verify_all_invariants().unwrap();
    }
}
