use std::collections::HashMap;

use exg_common::UserId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Chain {
    Ethereum,
    BscMainnet,
    ArbitrumOne,
    Optimism,
    Tron,
}

impl Chain {
    pub const fn required_confirmations(&self) -> u32 {
        match self {
            Self::Ethereum => 12,
            Self::BscMainnet => 15,
            Self::ArbitrumOne | Self::Optimism => 12,
            Self::Tron => 19,
        }
    }

    pub const fn name(&self) -> &'static str {
        match self {
            Self::Ethereum => "ETH",
            Self::BscMainnet => "BSC",
            Self::ArbitrumOne => "ARB",
            Self::Optimism => "OP",
            Self::Tron => "TRX",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepositAddress {
    pub user_id: UserId,
    pub chain: Chain,
    pub address: String,
    pub derivation_index: u32,
}

/// Address manager — generates and tracks deposit addresses per user per chain.
pub struct AddressManager {
    /// (user_id, chain) -> DepositAddress
    addresses: HashMap<(UserId, Chain), DepositAddress>,
    /// Reverse lookup: address -> (user_id, chain)
    address_lookup: HashMap<String, (UserId, Chain)>,
    /// Next derivation index per chain
    next_index: HashMap<Chain, u32>,
}

impl AddressManager {
    pub fn new() -> Self {
        Self {
            addresses: HashMap::new(),
            address_lookup: HashMap::new(),
            next_index: HashMap::new(),
        }
    }

    /// Get or generate a deposit address for a user on a specific chain.
    /// In production this would derive from HD wallet. Here we generate a placeholder.
    pub fn get_or_create_address(&mut self, user_id: UserId, chain: Chain) -> &DepositAddress {
        let key = (user_id, chain);
        if !self.addresses.contains_key(&key) {
            let index = self.next_index.entry(chain).or_insert(0);
            let current_index = *index;
            *index += 1;

            let address = format!(
                "0x{:0>40}",
                format!("{}{}{}", chain.name(), user_id.value(), current_index)
            );

            let deposit_address = DepositAddress {
                user_id,
                chain,
                address: address.clone(),
                derivation_index: current_index,
            };

            self.address_lookup.insert(address, (user_id, chain));
            self.addresses.insert(key, deposit_address);
        }
        &self.addresses[&key]
    }

    /// Lookup which user owns an address.
    pub fn lookup_address(&self, address: &str) -> Option<(UserId, Chain)> {
        self.address_lookup.get(address).copied()
    }

    /// Get all addresses for a user.
    pub fn get_user_addresses(&self, user_id: UserId) -> Vec<&DepositAddress> {
        self.addresses
            .values()
            .filter(|addr| addr.user_id == user_id)
            .collect()
    }
}

impl Default for AddressManager {
    fn default() -> Self {
        Self::new()
    }
}
