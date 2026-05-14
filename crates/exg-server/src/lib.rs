//! `exg-server` library crate.
//!
//! The public entry point is [`run_with_config`], which starts the exchange
//! server in-process and returns a [`ServerHandle`] that tests (and `main.rs`)
//! can use to drive requests and perform deterministic shutdown.
//!
//! Shutdown follows the 5-step sequence from spec §4.6:
//!   1. Caller awaits ctrl-c (done by binary in main.rs; tests use explicit shutdown).
//!   2. Stop the HTTP listener (`actix_handle.stop(true).await`).
//!   3. Signal the matching thread (`shutdown_flag.store(true)`).
//!   4. Join the matching thread.
//!   5. Final WAL flush is performed by the matching thread before exit.

use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use actix_web::dev::ServerHandle as ActixServerHandle;
use anyhow::{Context, bail};
use exg_api_gateway::app_factory::build_app;
use exg_api_gateway::state::AppState;
use exg_common::{Decimal128, SnowflakeGen, SymbolId};
use exg_config::ExgConfig;
use exg_matching_engine::MatchingEngine;
use exg_protocol::{Command, Event};
use exg_ringbuffer::RingBuffer;
use exg_risk_engine::{MarginTier, SymbolConfig};
use exg_wal::{WalConfig, WalWriter};
use parking_lot::Mutex;
use tracing::{error, info, warn};

// ── Public handle returned by run_with_config ─────────────────────────────

/// Handle to a running server instance.
///
/// Dropping this without calling `shutdown()` is a logic error — the matching
/// thread will keep running. Call `shutdown().await` for deterministic teardown.
pub struct ServerHandle {
    /// The TCP port the HTTP server is bound to. Useful in tests with port 0.
    pub bound_port: u16,
    pub actix_handle: ActixServerHandle,
    /// `Some` until `shutdown()` consumes it.
    matching_thread: Option<JoinHandle<()>>,
    shutdown_flag: Arc<AtomicBool>,
}

impl ServerHandle {
    /// Perform the 5-step graceful shutdown (spec §4.6).
    ///
    /// Step 1 (await ctrl-c) is the caller's responsibility — the binary does
    /// it in `main.rs`; integration tests call this directly.
    pub async fn shutdown(mut self) -> anyhow::Result<()> {
        // Step 2: gracefully drain in-flight HTTP requests.
        self.actix_handle.stop(true).await;

        // Step 3: signal the matching thread to flush and exit.
        self.shutdown_flag.store(true, Ordering::Release);

        // Step 4: join the matching thread.
        if let Some(jh) = self.matching_thread.take() {
            jh.join()
                .map_err(|_| anyhow::anyhow!("matching thread panicked"))?;
        }

        // Step 5: final WAL flush is performed by the matching thread before exit.

        Ok(())
    }
}

// ── Startup invariant validation (spec §4.5) ──────────────────────────────

fn validate_invariants(cfg: &ExgConfig) -> anyhow::Result<()> {
    // Invariant 0: structural config validation (decimal parses, ranges, etc.)
    // ExgConfig::load runs this automatically, but in-process callers
    // (notably integration tests and Stage 1 reload paths) construct
    // ExgConfig values programmatically and bypass load. Run it here so the
    // invariant holds regardless of the construction path.
    cfg.validate().with_context(|| "config validation failed")?;

    // Invariant 1: host must be loopback (Stage 0 — no external exposure).
    let host = &cfg.server.host;
    match host.as_str() {
        "127.0.0.1" | "::1" | "localhost" => {}
        other => bail!("server.host must be 127.0.0.1 / ::1 / localhost for Stage 0, got: {other}"),
    }

    // Invariant 2: exactly one symbol (Stage 0 single-symbol constraint).
    if cfg.trading.symbols.len() != 1 {
        bail!(
            "Stage 0 requires exactly 1 trading symbol, got {}",
            cfg.trading.symbols.len()
        );
    }

    // Invariant 3: WAL directory must be empty or not yet created.
    // Stage 0 has no replay/snapshot; an existing WAL would silently splice
    // a fresh in-memory state onto an old event timeline. Reject anything
    // resembling prior data: WAL segments are `wal-N.log`, snapshots are
    // `snapshot-*`. We treat ANY file in the dir as evidence of prior use
    // (including stray `.DS_Store`, sentinels from tests, etc.) — the
    // operator must clear the dir to confirm intent.
    let wal_dir = PathBuf::from(&cfg.wal.dir);
    if wal_dir.exists() {
        let entry_count = std::fs::read_dir(&wal_dir)
            .with_context(|| format!("cannot read WAL dir: {}", wal_dir.display()))?
            .filter_map(|e| e.ok())
            .count();
        if entry_count > 0 {
            bail!(
                "WAL directory {} is non-empty ({} entries); Stage 0 requires a fresh WAL. Clear the dir or pick a new path. (Spec §4.5 step 3b.)",
                wal_dir.display(),
                entry_count
            );
        }
    }

    Ok(())
}

// ── SymbolConfig conversion ───────────────────────────────────────────────

fn symbol_config_from_entry(entry: &exg_config::SymbolConfigEntry) -> anyhow::Result<SymbolConfig> {
    let parse = |s: &str| -> anyhow::Result<Decimal128> {
        s.parse::<Decimal128>()
            .with_context(|| format!("invalid decimal: {s}"))
    };

    let margin_tiers: anyhow::Result<Vec<MarginTier>> = entry
        .margin_tiers
        .iter()
        .map(|t| {
            Ok(MarginTier {
                notional_floor: parse(&t.notional_floor)?,
                notional_cap: parse(&t.notional_cap)?,
                maintenance_margin_rate: parse(&t.maintenance_margin_rate)?,
                maintenance_amount: parse(&t.maintenance_amount)?,
            })
        })
        .collect();

    Ok(SymbolConfig {
        symbol: SymbolId::new(entry.id),
        tick_size: parse(&entry.tick_size)?,
        lot_size: parse(&entry.lot_size)?,
        min_notional: parse(&entry.min_notional)?,
        max_leverage: parse(&entry.max_leverage)?,
        maker_fee: parse(&entry.maker_fee)?,
        taker_fee: parse(&entry.taker_fee)?,
        margin_tiers: margin_tiers?,
    })
}

// ── Entry point ───────────────────────────────────────────────────────────

/// Start the exchange server from an in-memory config.
///
/// # Invariants enforced (spec §4.5)
/// - `cfg.server.host` ∈ {127.0.0.1, ::1, localhost}
/// - `cfg.trading.symbols.len() == 1`
/// - WAL directory is empty or does not exist
///
/// # Thread model
/// - HTTP: N Actix worker threads (default = logical CPU count)
/// - Matching engine: 1 dedicated OS thread, optionally pinned to `cfg.matching_core`
///   via `core_affinity`
///
/// # RingBuffer lifetime
/// The `RingBuffer` is `Box::leak`-ed to give it a `'static` lifetime so that
/// `Producer` (held in `AppState`) and `Consumer` (moved into the matching
/// thread) can both hold raw-pointer handles into the mmap without a borrow
/// conflict.  The leaked memory is reclaimed when the process exits.  Tests
/// that call this function multiple times will accumulate one leaked
/// `RingBuffer` per call — acceptable because test processes are short-lived.
pub async fn run_with_config(cfg: ExgConfig) -> anyhow::Result<ServerHandle> {
    // ── Step 0: validate startup invariants ──────────────────────────────
    validate_invariants(&cfg)?;

    let cfg = Arc::new(cfg);

    // ── Step 1: open WAL writer ───────────────────────────────────────────
    let wal_dir = PathBuf::from(&cfg.wal.dir);
    let wal_cfg = WalConfig {
        dir: wal_dir,
        segment_size: cfg.wal.segment_size_mb * 1024 * 1024,
        flush_interval_us: cfg.wal.flush_interval_us,
        flush_every_n: cfg.wal.flush_every_n,
    };
    let wal = Arc::new(Mutex::new(
        WalWriter::open(wal_cfg).context("failed to open WAL writer")?,
    ));

    // ── Step 2: allocate ring buffer (leaked for 'static lifetime) ────────
    let rb: &'static mut RingBuffer = Box::leak(Box::new(
        RingBuffer::new(cfg.ringbuffer.slot_count, cfg.ringbuffer.slot_size)
            .context("failed to create ring buffer")?,
    ));
    let (producer, consumer) = rb.split();

    // ── Step 3: build SymbolConfig + MatchingEngine ───────────────────────
    let sym_entry = &cfg.trading.symbols[0];
    let mark_price: Decimal128 = sym_entry
        .mark_price
        .parse()
        .with_context(|| format!("invalid mark_price: {}", sym_entry.mark_price))?;
    let symbol_config =
        symbol_config_from_entry(sym_entry).context("failed to parse symbol config")?;

    let mut engine = MatchingEngine::new(symbol_config, cfg.server.node_id);
    engine.set_mark_price(mark_price);

    // ── Step 4: build shared AppState ────────────────────────────────────
    let state = AppState {
        producer: Arc::new(Mutex::new(producer)),
        snowflake: Arc::new(SnowflakeGen::new(cfg.server.node_id)),
        cfg: Arc::clone(&cfg),
    };

    // ── Step 5: spawn matching engine OS thread ───────────────────────────
    let shutdown_flag = Arc::new(AtomicBool::new(false));
    let matching_shutdown = Arc::clone(&shutdown_flag);
    let matching_wal = Arc::clone(&wal);
    let slot_size = cfg.ringbuffer.slot_size;

    let matching_thread = std::thread::Builder::new()
        .name("matching-engine".into())
        .spawn(move || {
            // Best-effort CPU affinity — warn on failure (expected on macOS).
            let pinned = MatchingEngine::bind_to_core(0);
            if !pinned {
                warn!("matching thread: core affinity bind failed (non-fatal on macOS)");
            } else {
                info!("matching thread: bound to core 0");
            }

            let mut buf = vec![0u8; slot_size];

            // Inline closure that processes one popped command. Used both on
            // the steady-state path and during the post-shutdown drain so the
            // logic stays in one place.
            let mut process_one = |n: usize, buf: &[u8]| {
                let owned: Vec<u8> = buf[..n].to_vec();
                let cmd: Command = match rkyv::from_bytes::<Command, rkyv::rancor::Error>(&owned) {
                    Ok(c) => c,
                    Err(e) => panic!("matching thread: rkyv decode Command failed: {e}"),
                };
                let events: Vec<Event> = engine.process_command(&cmd);
                for evt in &events {
                    let bytes = match rkyv::to_bytes::<rkyv::rancor::Error>(evt) {
                        Ok(b) => b,
                        Err(e) => panic!("matching thread: rkyv encode Event failed: {e}"),
                    };
                    if let Err(e) = matching_wal.lock().append(&bytes) {
                        panic!("matching thread: WAL append failed: {e}");
                    }
                }
            };

            loop {
                match consumer.try_pop(&mut buf) {
                    Ok(n) => process_one(n, &buf),
                    Err(exg_ringbuffer::RingBufferError::Empty) => {
                        // Only consult shutdown_flag when ring buffer is empty.
                        // This guarantees the drain semantic: HTTP 200 means
                        // the command sits in the ring buffer, and the
                        // matching thread will not exit until every such
                        // command has been popped and WAL-appended. Checking
                        // the flag at the top of the loop would race with
                        // last-millisecond pushes (spec §4.6 step 4).
                        if matching_shutdown.load(Ordering::Acquire) {
                            // Final WAL flush before exit (spec §4.6 step 5).
                            if let Err(e) = matching_wal.lock().flush() {
                                error!("matching thread: final WAL flush failed: {e}");
                            }
                            info!("matching thread: shutdown complete");
                            return;
                        }
                        // No messages and no shutdown — spin hint and retry.
                        std::hint::spin_loop();
                    }
                    Err(e) => {
                        panic!("matching thread: unexpected ring buffer error: {e}");
                    }
                }
            }
        })
        .context("failed to spawn matching thread")?;

    // ── Step 6: bind HTTP server ──────────────────────────────────────────
    let host = cfg.server.host.clone();
    let port = cfg.server.port;

    // Bind a TcpListener first so we can read the actual port (supports port 0
    // in tests) before starting the Actix runtime.
    let listener = TcpListener::bind((host.as_str(), port))
        .with_context(|| format!("failed to bind {host}:{port}"))?;
    let bound_port = listener
        .local_addr()
        .context("failed to get local addr")?
        .port();

    let state_clone = state.clone();
    let server = actix_web::HttpServer::new(move || build_app(state_clone.clone()))
        .listen(listener)
        .context("actix HttpServer::listen failed")?
        .run();

    let actix_handle = server.handle();
    tokio::spawn(server);

    info!(
        host = %host,
        port = bound_port,
        symbol = %cfg.trading.symbols[0].name,
        node_id = cfg.server.node_id,
        "exg-server listening"
    );

    Ok(ServerHandle {
        bound_port,
        actix_handle,
        matching_thread: Some(matching_thread),
        shutdown_flag,
    })
}
