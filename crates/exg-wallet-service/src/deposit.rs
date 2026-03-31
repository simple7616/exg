use std::collections::HashMap;

use exg_common::{Decimal128, UnixMicros, UserId};
use serde::{Deserialize, Serialize};

use crate::address::Chain;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Deposit {
    pub id: u64,
    pub user_id: UserId,
    pub chain: Chain,
    pub tx_hash: String,
    pub log_index: u32,
    pub from_address: String,
    pub to_address: String,
    pub amount: Decimal128,
    pub asset: String,
    pub confirmations: u32,
    pub required_confirmations: u32,
    pub status: DepositStatus,
    pub created_at: UnixMicros,
    pub updated_at: UnixMicros,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DepositStatus {
    /// Detected, waiting for confirmations
    Pending,
    /// Enough confirmations, ready to credit
    Confirmed,
    /// Balance updated in ledger
    Credited,
    /// Block reorg detected, reverted
    Reorged,
}

pub struct DepositTracker {
    deposits: Vec<Deposit>,
    /// Idempotency: (chain, tx_hash, log_index) -> deposit index
    idempotency: HashMap<(Chain, String, u32), usize>,
    next_id: u64,
}

impl DepositTracker {
    pub fn new() -> Self {
        Self {
            deposits: Vec::new(),
            idempotency: HashMap::new(),
            next_id: 1,
        }
    }

    /// Record a new deposit from chain scanner. Idempotent by (chain, tx_hash, log_index).
    #[allow(clippy::too_many_arguments)]
    pub fn record_deposit(
        &mut self,
        user_id: UserId,
        chain: Chain,
        tx_hash: &str,
        log_index: u32,
        from_address: &str,
        to_address: &str,
        amount: Decimal128,
        asset: &str,
        timestamp: UnixMicros,
    ) -> Option<&Deposit> {
        let key = (chain, tx_hash.to_string(), log_index);
        if self.idempotency.contains_key(&key) {
            return None;
        }

        let id = self.next_id;
        self.next_id += 1;

        let deposit = Deposit {
            id,
            user_id,
            chain,
            tx_hash: tx_hash.to_string(),
            log_index,
            from_address: from_address.to_string(),
            to_address: to_address.to_string(),
            amount,
            asset: asset.to_string(),
            confirmations: 0,
            required_confirmations: chain.required_confirmations(),
            status: DepositStatus::Pending,
            created_at: timestamp,
            updated_at: timestamp,
        };

        let index = self.deposits.len();
        self.deposits.push(deposit);
        self.idempotency.insert(key, index);

        Some(&self.deposits[index])
    }

    /// Update confirmation count. Returns `Some(true)` if status changed to Confirmed,
    /// `Some(false)` if updated but not yet confirmed, `None` if deposit not found.
    pub fn update_confirmations(
        &mut self,
        chain: Chain,
        tx_hash: &str,
        log_index: u32,
        confirmations: u32,
    ) -> Option<bool> {
        let key = (chain, tx_hash.to_string(), log_index);
        let index = *self.idempotency.get(&key)?;
        let deposit = &mut self.deposits[index];

        deposit.confirmations = confirmations;

        if deposit.status == DepositStatus::Pending
            && confirmations >= deposit.required_confirmations
        {
            deposit.status = DepositStatus::Confirmed;
            return Some(true);
        }

        Some(false)
    }

    /// Mark deposit as credited (after ledger update).
    pub fn mark_credited(&mut self, chain: Chain, tx_hash: &str, log_index: u32) -> bool {
        let key = (chain, tx_hash.to_string(), log_index);
        let Some(&index) = self.idempotency.get(&key) else {
            return false;
        };
        let deposit = &mut self.deposits[index];
        if deposit.status == DepositStatus::Confirmed {
            deposit.status = DepositStatus::Credited;
            true
        } else {
            false
        }
    }

    /// Mark deposit as reorged.
    pub fn mark_reorged(&mut self, chain: Chain, tx_hash: &str, log_index: u32) -> bool {
        let key = (chain, tx_hash.to_string(), log_index);
        let Some(&index) = self.idempotency.get(&key) else {
            return false;
        };
        let deposit = &mut self.deposits[index];
        if deposit.status == DepositStatus::Pending || deposit.status == DepositStatus::Confirmed {
            deposit.status = DepositStatus::Reorged;
            true
        } else {
            false
        }
    }

    /// Get all pending deposits (for confirmation monitoring).
    pub fn pending_deposits(&self) -> Vec<&Deposit> {
        self.deposits
            .iter()
            .filter(|d| d.status == DepositStatus::Pending)
            .collect()
    }

    /// Get confirmed but not yet credited deposits.
    pub fn confirmed_uncredited(&self) -> Vec<&Deposit> {
        self.deposits
            .iter()
            .filter(|d| d.status == DepositStatus::Confirmed)
            .collect()
    }

    /// Get deposits for a user.
    pub fn user_deposits(&self, user_id: UserId) -> Vec<&Deposit> {
        self.deposits
            .iter()
            .filter(|d| d.user_id == user_id)
            .collect()
    }
}

impl Default for DepositTracker {
    fn default() -> Self {
        Self::new()
    }
}
