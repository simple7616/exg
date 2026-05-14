use std::collections::HashMap;

use exg_common::Decimal128;

use crate::address::Chain;

/// Tracks hot wallet balances per chain for collection/rebalancing decisions.
pub struct HotWalletMonitor {
    /// (chain, asset) -> balance
    balances: HashMap<(Chain, String), Decimal128>,
    /// Trigger collection when deposit address balance exceeds this
    collection_threshold: Decimal128,
    cold_wallet_addresses: HashMap<Chain, String>,
}

impl HotWalletMonitor {
    pub fn new(collection_threshold: Decimal128) -> Self {
        Self {
            balances: HashMap::new(),
            collection_threshold,
            cold_wallet_addresses: HashMap::new(),
        }
    }

    pub fn update_balance(&mut self, chain: Chain, asset: &str, balance: Decimal128) {
        self.balances.insert((chain, asset.to_string()), balance);
    }

    pub fn get_balance(&self, chain: Chain, asset: &str) -> Decimal128 {
        self.balances
            .get(&(chain, asset.to_string()))
            .copied()
            .unwrap_or(Decimal128::ZERO)
    }

    pub fn set_cold_wallet(&mut self, chain: Chain, address: &str) {
        self.cold_wallet_addresses
            .insert(chain, address.to_string());
    }

    pub fn needs_collection(&self, chain: Chain, asset: &str) -> bool {
        let balance = self.get_balance(chain, asset);
        balance >= self.collection_threshold
    }
}
