use serde::{Deserialize, Serialize};

use exg_common::{Decimal128, OrderId, SymbolId};

use crate::orderbook::BookOrder;

/// Serializable snapshot of the matching engine state for crash recovery.
#[derive(Debug, Serialize, Deserialize)]
pub struct EngineSnapshot {
    pub symbol: SymbolId,
    pub orders: Vec<BookOrder>,
    pub stop_orders: Vec<BookOrder>,
    pub mark_price: Decimal128,
    pub index_price: Decimal128,
    pub sequence: u64,
    pub trade_id_counter: u64,
    /// GTD expiry entries: (expire_time_micros, order_id).
    pub expiry_entries: Vec<(u64, OrderId)>,
}

impl crate::engine::MatchingEngine {
    /// Serialize engine state to JSON bytes.
    pub fn serialize_snapshot(&self) -> Vec<u8> {
        let snapshot = self.take_snapshot();
        serde_json::to_vec(&snapshot).expect("snapshot serialization failed")
    }

    /// Deserialize and restore engine from JSON bytes.
    pub fn deserialize_snapshot(
        data: &[u8],
        config: exg_risk_engine::SymbolConfig,
        node_id: u16,
    ) -> Self {
        let snapshot: EngineSnapshot =
            serde_json::from_slice(data).expect("snapshot deserialization failed");
        Self::restore_from_snapshot(snapshot, config, node_id)
    }
}
