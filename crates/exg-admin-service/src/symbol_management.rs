use exg_common::*;
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use crate::error::AdminError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolEntry {
    pub symbol_id: SymbolId,
    pub name: String,
    pub symbol_type: SymbolType,
    pub status: SymbolStatus,
    pub tick_size: Decimal128,
    pub lot_size: Decimal128,
    pub min_notional: Decimal128,
    pub max_leverage: Decimal128,
    pub maker_fee: Decimal128,
    pub taker_fee: Decimal128,
    pub created_at: UnixMicros,
    pub updated_at: UnixMicros,
}

pub struct SymbolManager {
    symbols: FxHashMap<SymbolId, SymbolEntry>,
    name_index: FxHashMap<String, SymbolId>,
}

impl SymbolManager {
    pub fn new() -> Self {
        Self {
            symbols: FxHashMap::default(),
            name_index: FxHashMap::default(),
        }
    }

    /// Add a new symbol. Fails if the name is already taken.
    pub fn add_symbol(&mut self, entry: SymbolEntry) -> Result<(), AdminError> {
        if self.name_index.contains_key(&entry.name) {
            return Err(AdminError::DuplicateSymbol(entry.name.clone()));
        }
        self.name_index.insert(entry.name.clone(), entry.symbol_id);
        self.symbols.insert(entry.symbol_id, entry);
        Ok(())
    }

    /// Update the trading status of a symbol.
    pub fn update_status(
        &mut self,
        symbol_id: SymbolId,
        status: SymbolStatus,
    ) -> Result<(), AdminError> {
        let entry = self
            .symbols
            .get_mut(&symbol_id)
            .ok_or(AdminError::SymbolNotFound(symbol_id))?;
        entry.status = status;
        entry.updated_at = UnixMicros::now();
        Ok(())
    }

    /// Update maker/taker fees for a symbol.
    pub fn update_fees(
        &mut self,
        symbol_id: SymbolId,
        maker: Decimal128,
        taker: Decimal128,
    ) -> Result<(), AdminError> {
        let entry = self
            .symbols
            .get_mut(&symbol_id)
            .ok_or(AdminError::SymbolNotFound(symbol_id))?;
        entry.maker_fee = maker;
        entry.taker_fee = taker;
        entry.updated_at = UnixMicros::now();
        Ok(())
    }

    /// Look up a symbol by ID.
    pub fn get_symbol(&self, symbol_id: SymbolId) -> Option<&SymbolEntry> {
        self.symbols.get(&symbol_id)
    }

    /// Look up a symbol by name.
    pub fn get_by_name(&self, name: &str) -> Option<&SymbolEntry> {
        let id = self.name_index.get(name)?;
        self.symbols.get(id)
    }

    /// List all symbols.
    pub fn list_symbols(&self) -> Vec<&SymbolEntry> {
        self.symbols.values().collect()
    }

    /// List only symbols with `Trading` status.
    pub fn active_symbols(&self) -> Vec<&SymbolEntry> {
        self.symbols
            .values()
            .filter(|e| e.status == SymbolStatus::Trading)
            .collect()
    }
}

impl Default for SymbolManager {
    fn default() -> Self {
        Self::new()
    }
}
