use std::sync::Arc;

use exg_common::SnowflakeGen;
use exg_config::ExgConfig;
use exg_ringbuffer::Producer;
use parking_lot::Mutex;

/// Shared state injected into every Actix handler.
///
/// `producer` is wrapped in `Mutex` because the underlying SPSC ring buffer
/// admits a single producer; multiple Actix worker threads serialize through
/// the lock to preserve that invariant. Throughput optimization is deferred
/// to Stage 7 (per spec §4.3).
#[derive(Clone)]
pub struct AppState {
    pub producer: Arc<Mutex<Producer>>,
    pub snowflake: Arc<SnowflakeGen>,
    pub cfg: Arc<ExgConfig>,
}
