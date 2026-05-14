use exg_common::{Decimal128, UnixMicros, UserId};
use serde::{Deserialize, Serialize};

use crate::address::Chain;
use crate::error::WalletError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Withdrawal {
    pub id: u64,
    pub user_id: UserId,
    pub chain: Chain,
    pub to_address: String,
    pub amount: Decimal128,
    pub fee: Decimal128,
    pub asset: String,
    pub tx_hash: Option<String>,
    pub status: WithdrawalStatus,
    pub reviewed_by: Option<UserId>,
    pub created_at: UnixMicros,
    pub updated_at: UnixMicros,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WithdrawalStatus {
    /// Needs human or auto approval
    PendingReview,
    /// Approved, ready for broadcast
    Approved,
    /// Transaction submitted to chain
    Processing,
    /// Transaction confirmed
    Completed,
    /// Rejected by reviewer
    Rejected,
    /// Transaction failed on chain
    Failed,
}

pub struct WithdrawalProcessor {
    withdrawals: Vec<Withdrawal>,
    next_id: u64,
    /// Auto-approve threshold (amounts below this don't need manual review)
    auto_approve_threshold: Decimal128,
}

impl WithdrawalProcessor {
    pub fn new(auto_approve_threshold: Decimal128) -> Self {
        Self {
            withdrawals: Vec::new(),
            next_id: 1,
            auto_approve_threshold,
        }
    }

    /// Submit a withdrawal request. Auto-approves if amount is below threshold.
    #[allow(clippy::too_many_arguments)]
    pub fn submit(
        &mut self,
        user_id: UserId,
        chain: Chain,
        to_address: &str,
        amount: Decimal128,
        fee: Decimal128,
        asset: &str,
        timestamp: UnixMicros,
    ) -> &Withdrawal {
        let id = self.next_id;
        self.next_id += 1;

        let status = if amount < self.auto_approve_threshold {
            WithdrawalStatus::Approved
        } else {
            WithdrawalStatus::PendingReview
        };

        let withdrawal = Withdrawal {
            id,
            user_id,
            chain,
            to_address: to_address.to_string(),
            amount,
            fee,
            asset: asset.to_string(),
            tx_hash: None,
            status,
            reviewed_by: None,
            created_at: timestamp,
            updated_at: timestamp,
        };

        self.withdrawals.push(withdrawal);
        self.withdrawals.last().unwrap()
    }

    fn find_mut(&mut self, withdrawal_id: u64) -> Result<&mut Withdrawal, WalletError> {
        self.withdrawals
            .iter_mut()
            .find(|w| w.id == withdrawal_id)
            .ok_or(WalletError::WithdrawalNotFound(withdrawal_id))
    }

    fn transition(
        withdrawal: &mut Withdrawal,
        expected: WithdrawalStatus,
        target: WithdrawalStatus,
    ) -> Result<(), WalletError> {
        if withdrawal.status != expected {
            return Err(WalletError::InvalidStatusTransition {
                from: withdrawal.status,
                to: target,
            });
        }
        withdrawal.status = target;
        Ok(())
    }

    /// Approve a pending withdrawal.
    pub fn approve(&mut self, withdrawal_id: u64, reviewer: UserId) -> Result<(), WalletError> {
        let withdrawal = self.find_mut(withdrawal_id)?;
        Self::transition(
            withdrawal,
            WithdrawalStatus::PendingReview,
            WithdrawalStatus::Approved,
        )?;
        withdrawal.reviewed_by = Some(reviewer);
        Ok(())
    }

    /// Reject a pending withdrawal.
    pub fn reject(&mut self, withdrawal_id: u64, reviewer: UserId) -> Result<(), WalletError> {
        let withdrawal = self.find_mut(withdrawal_id)?;
        Self::transition(
            withdrawal,
            WithdrawalStatus::PendingReview,
            WithdrawalStatus::Rejected,
        )?;
        withdrawal.reviewed_by = Some(reviewer);
        Ok(())
    }

    /// Mark as processing (tx submitted).
    pub fn mark_processing(
        &mut self,
        withdrawal_id: u64,
        tx_hash: &str,
    ) -> Result<(), WalletError> {
        let withdrawal = self.find_mut(withdrawal_id)?;
        Self::transition(
            withdrawal,
            WithdrawalStatus::Approved,
            WithdrawalStatus::Processing,
        )?;
        withdrawal.tx_hash = Some(tx_hash.to_string());
        Ok(())
    }

    /// Mark as completed (tx confirmed).
    pub fn mark_completed(&mut self, withdrawal_id: u64) -> Result<(), WalletError> {
        let withdrawal = self.find_mut(withdrawal_id)?;
        Self::transition(
            withdrawal,
            WithdrawalStatus::Processing,
            WithdrawalStatus::Completed,
        )
    }

    /// Mark as failed.
    pub fn mark_failed(&mut self, withdrawal_id: u64) -> Result<(), WalletError> {
        let withdrawal = self.find_mut(withdrawal_id)?;
        Self::transition(
            withdrawal,
            WithdrawalStatus::Processing,
            WithdrawalStatus::Failed,
        )
    }

    /// Get pending review withdrawals.
    pub fn pending_review(&self) -> Vec<&Withdrawal> {
        self.withdrawals
            .iter()
            .filter(|w| w.status == WithdrawalStatus::PendingReview)
            .collect()
    }

    /// Get approved withdrawals ready for processing.
    pub fn approved_ready(&self) -> Vec<&Withdrawal> {
        self.withdrawals
            .iter()
            .filter(|w| w.status == WithdrawalStatus::Approved)
            .collect()
    }

    /// Get withdrawals for a user.
    pub fn user_withdrawals(&self, user_id: UserId) -> Vec<&Withdrawal> {
        self.withdrawals
            .iter()
            .filter(|w| w.user_id == user_id)
            .collect()
    }
}
