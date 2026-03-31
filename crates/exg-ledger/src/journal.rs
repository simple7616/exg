use exg_common::{Decimal128, UnixMicros, UserId};
use serde::{Deserialize, Serialize};

use crate::account::WalletType;

/// Which sub-field of a wallet balance is affected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BalanceField {
    Available,
    Frozen,
    Margin,
}

/// Classification of the ledger operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum JournalEntryType {
    Deposit,
    Withdrawal,
    TradeFee,
    PositionOpen,
    PositionClose,
    FundingPayment,
    Liquidation,
    Transfer,
    OrderFreeze,
    OrderUnfreeze,
    InsuranceFundContribution,
    AdlSettlement,
}

/// A single double-entry bookkeeping record.
///
/// Invariant: `amount > 0`. The debit side loses `amount`, the credit side gains `amount`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalEntry {
    pub id: u64,
    pub debit_user: UserId,
    pub debit_wallet: WalletType,
    pub debit_field: BalanceField,
    pub credit_user: UserId,
    pub credit_wallet: WalletType,
    pub credit_field: BalanceField,
    pub amount: Decimal128,
    pub entry_type: JournalEntryType,
    pub idempotency_key: String,
    pub timestamp: UnixMicros,
}
