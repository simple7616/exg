use std::collections::HashMap;

use exg_common::{Decimal128, UserId};
use serde::{Deserialize, Serialize};

/// Wallet types in the exchange.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WalletType {
    Spot,
    Futures,
    Funding,
    InsuranceFund,
    FeeCollection,
    Escrow,
}

/// A single wallet balance with three sub-fields.
///
/// Invariant: `available >= 0 && frozen >= 0 && margin >= 0`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WalletBalance {
    pub available: Decimal128,
    pub frozen: Decimal128,
    pub margin: Decimal128,
}

impl WalletBalance {
    /// Total balance across all sub-fields.
    pub fn total(&self) -> Decimal128 {
        self.available + self.frozen + self.margin
    }
}

/// User account containing multiple wallets.
#[derive(Debug, Clone)]
pub struct UserAccount {
    pub user_id: UserId,
    pub wallets: HashMap<WalletType, WalletBalance>,
}

impl UserAccount {
    pub fn new(user_id: UserId) -> Self {
        Self {
            user_id,
            wallets: HashMap::new(),
        }
    }

    /// Get or create a wallet, returning a mutable reference.
    pub fn wallet_mut(&mut self, wallet_type: WalletType) -> &mut WalletBalance {
        self.wallets.entry(wallet_type).or_default()
    }

    /// Get a wallet balance (read-only).
    pub fn wallet(&self, wallet_type: WalletType) -> Option<&WalletBalance> {
        self.wallets.get(&wallet_type)
    }
}
