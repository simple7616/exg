use std::collections::HashSet;

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

// ── WebSocket request/response types ─────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "method")]
pub enum WsRequest {
    #[serde(rename = "SUBSCRIBE")]
    Subscribe { params: Vec<String>, id: u64 },
    #[serde(rename = "UNSUBSCRIBE")]
    Unsubscribe { params: Vec<String>, id: u64 },
}

#[derive(Debug, Serialize)]
pub struct WsResponse {
    pub result: Option<serde_json::Value>,
    pub id: u64,
}

// ── Subscription manager ─────────────────────────────────────────────────

/// Manages bidirectional mapping between client IDs and subscribed streams.
pub struct SubscriptionManager {
    /// client_id -> set of subscribed streams
    subscriptions: FxHashMap<u64, HashSet<String>>,
    /// stream -> set of client_ids
    stream_clients: FxHashMap<String, HashSet<u64>>,
}

impl SubscriptionManager {
    pub fn new() -> Self {
        Self {
            subscriptions: FxHashMap::default(),
            stream_clients: FxHashMap::default(),
        }
    }

    /// Subscribe a client to the given streams. Returns the list of streams
    /// that were newly subscribed (i.e., not already subscribed).
    pub fn subscribe(&mut self, client_id: u64, streams: &[String]) -> Vec<String> {
        let client_streams = self.subscriptions.entry(client_id).or_default();
        let mut newly_subscribed = Vec::new();

        for stream in streams {
            if client_streams.insert(stream.clone()) {
                self.stream_clients
                    .entry(stream.clone())
                    .or_default()
                    .insert(client_id);
                newly_subscribed.push(stream.clone());
            }
        }

        newly_subscribed
    }

    /// Unsubscribe a client from the given streams. Returns the list of
    /// streams that were actually unsubscribed.
    pub fn unsubscribe(&mut self, client_id: u64, streams: &[String]) -> Vec<String> {
        let mut removed = Vec::new();

        if let Some(client_streams) = self.subscriptions.get_mut(&client_id) {
            for stream in streams {
                if client_streams.remove(stream) {
                    if let Some(clients) = self.stream_clients.get_mut(stream) {
                        clients.remove(&client_id);
                        if clients.is_empty() {
                            self.stream_clients.remove(stream);
                        }
                    }
                    removed.push(stream.clone());
                }
            }

            if client_streams.is_empty() {
                self.subscriptions.remove(&client_id);
            }
        }

        removed
    }

    /// Remove a client entirely, cleaning up all stream associations.
    pub fn remove_client(&mut self, client_id: u64) {
        if let Some(streams) = self.subscriptions.remove(&client_id) {
            for stream in &streams {
                if let Some(clients) = self.stream_clients.get_mut(stream) {
                    clients.remove(&client_id);
                    if clients.is_empty() {
                        self.stream_clients.remove(stream);
                    }
                }
            }
        }
    }

    /// Get all client IDs subscribed to a given stream.
    pub fn get_clients_for_stream(&self, stream: &str) -> Option<&HashSet<u64>> {
        self.stream_clients.get(stream)
    }

    /// Get all streams a given client is subscribed to.
    pub fn get_client_streams(&self, client_id: u64) -> Option<&HashSet<String>> {
        self.subscriptions.get(&client_id)
    }
}

impl Default for SubscriptionManager {
    fn default() -> Self {
        Self::new()
    }
}

// ── Stream name parsing ──────────────────────────────────────────────────

/// Parse a stream name into (symbol, channel) components.
///
/// Examples:
/// - `"btcusdt@depth20"` -> `Some(("btcusdt", "depth20"))`
/// - `"btcusdt@trade"` -> `Some(("btcusdt", "trade"))`
/// - `"btcusdt@kline_1m"` -> `Some(("btcusdt", "kline_1m"))`
/// - `"invalid"` -> `None`
pub fn parse_stream_name(stream: &str) -> Option<(String, String)> {
    let at_pos = stream.find('@')?;
    let symbol = &stream[..at_pos];
    let channel = &stream[at_pos + 1..];

    if symbol.is_empty() || channel.is_empty() {
        return None;
    }

    Some((symbol.to_owned(), channel.to_owned()))
}
