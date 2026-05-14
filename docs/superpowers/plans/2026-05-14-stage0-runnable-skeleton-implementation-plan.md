# Stage 0 Runnable Skeleton Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Spec:** [`docs/superpowers/specs/2026-05-14-stage0-runnable-skeleton-design.md`](../specs/2026-05-14-stage0-runnable-skeleton-design.md)

**Goal:** Wire the LMAX command hot path end to end so a client can POST `/api/v1/order` / `cancel` / `amend` and the resulting events land in WAL on disk, verifiable via a new `exg-wal-dump` binary. No frontend, no DB, no WS, no auth — all deferred to Stages 1-7.

**Architecture:** Tokio multi-thread runtime hosts Actix HTTP; a dedicated CPU-pinned OS thread owns matching engine + WAL writes. HTTP workers serialize commands through `Arc<Mutex<Producer>>` into an mmap SPSC ring buffer; the matching thread pops, processes via `risk-engine` pre-trade + `MatchingEngine::process_command`, then `rkyv::to_bytes`-encodes each event and appends to `WalWriter`. Five-step deterministic shutdown preserves the "HTTP 200 = enqueued" semantic.

**Tech Stack:** Rust 2024 edition, Tokio, Actix-web 4, rkyv 0.8, parking_lot, core_affinity, Decimal128 (custom in `exg-common`), thiserror, tracing.

**Branch:** `feat/stage0-skeleton`. **Base commit:** `e179916`.

---

## Invariant Reminders (CLAUDE.md + Spec §9)

These hold throughout every task. Violations block merge:

1. Matching engine is the single writer. All orderbook/risk state mutations happen on the matching OS thread.
2. All financial values flow as `Decimal128`. **No `f64` introduced anywhere on the request path.**
3. WAL is the source of truth. No event reaches a consumer without WAL acknowledgement.
4. No fallback paths for WAL failure — process panics.
5. API errors use Binance-compatible codes only (see spec §7.1).
6. No swallowed errors, no `let _ = result;`.
7. Rust 2024: `gen` is reserved — use `id_gen` / `sf` / `rng`.
8. IDOR guard: cancel/amend with mismatched `user_id` → `OrderRejected { OrderNotFound }`.
9. Duplicate `client_order_id` is NOT deduplicated; two POSTs create two `order_id`s.
10. `engine.sequence` (command count) ≠ WAL sequence (event count); when in doubt, mean WAL sequence.

---

## File Structure

### Created

| File | Responsibility |
|---|---|
| `crates/exg-wal-dump/Cargo.toml` | New cargo bin crate manifest |
| `crates/exg-wal-dump/src/main.rs` | CLI entrypoint: parse args, call `dump`, print JSON |
| `crates/exg-wal-dump/src/lib.rs` | `pub fn dump(dir, from_seq, writer) -> Result` for testability |
| `crates/exg-wal-dump/tests/dump_tests.rs` | Happy / CRC fail / empty / --from-seq |
| `crates/exg-api-gateway/src/state.rs` | `AppState { producer, snowflake, cfg }` |
| `crates/exg-api-gateway/src/handlers.rs` | health / place_order / cancel_order / amend_order + unit tests |
| `crates/exg-api-gateway/src/app_factory.rs` | `build_app(state)` mounting routes |
| `crates/exg-server/src/lib.rs` | `pub fn run_with_config(cfg) -> ServerHandle` for testability |
| `crates/exg-server/tests/stage0_e2e.rs` | All end-to-end scenarios |
| `crates/exg-server/tests/boot_panics.rs` | Boot-time invariant guard tests |
| `crates/exg-protocol/tests/slot_size.rs` | Property test: every Command variant ≤ 4096 bytes when rkyv-encoded |
| `scripts/demo-stage0.sh` | Cold-boot demo: start server → 3 curls → SIGTERM → wal-dump |

### Modified

| File | Change |
|---|---|
| `Cargo.toml` (workspace) | Add `crates/exg-wal-dump` to `members` and `workspace.dependencies` |
| `crates/exg-config/src/lib.rs` | Add `pub mark_price: String` to `SymbolConfigEntry`; update `default_btcusdt()` |
| `crates/exg-config/src/validation.rs` | Add `mark_price` parse + > 0 check in `validate_symbol` |
| `crates/exg-config/src/tests.rs` | (if exists) or `src/lib.rs` cfg(test) — add mark_price tests |
| `config/default.toml` | Add `mark_price = "60000"` in `[[trading.symbols]]` |
| `crates/exg-api-gateway/Cargo.toml` | Add deps: `actix-web`, `exg-ringbuffer`, `exg-wal`, `exg-config`, `parking_lot`, `tokio`, `tracing` |
| `crates/exg-api-gateway/src/conversion.rs` | Add `to_cancel_order_command` + `to_amend_order_command` |
| `crates/exg-api-gateway/src/lib.rs` | Export new modules `state`, `handlers`, `app_factory` |
| `crates/exg-api-gateway/src/types.rs` | Add `CancelOrderRequest`, `AmendOrderRequest`, `PlaceOrderResponse`, `CancelOrderResponse`, `AmendOrderResponse`, `HealthResponse` |
| `crates/exg-matching-engine/src/engine.rs` | Add `pub fn set_mark_price(&mut self, price: Decimal128)` |
| `crates/exg-server/Cargo.toml` | Add deps: `exg-config`, `exg-protocol`, `exg-common`, `exg-ringbuffer`, `exg-wal`, `exg-matching-engine`, `exg-risk-engine`, `exg-api-gateway`, `parking_lot`, `core_affinity`, `actix-web`, `reqwest` (dev), `tempfile` (dev) |
| `crates/exg-server/src/main.rs` | Rewrite to call `exg_server::run_with_config` |

---

## Task 1 — Workspace setup: add `exg-wal-dump` member

**Files:**
- Modify: `Cargo.toml` (workspace root)
- Create: `crates/exg-wal-dump/Cargo.toml`
- Create: `crates/exg-wal-dump/src/main.rs` (placeholder)
- Create: `crates/exg-wal-dump/src/lib.rs` (placeholder)

- [ ] **Step 1: Create crate skeleton**

```bash
mkdir -p crates/exg-wal-dump/src crates/exg-wal-dump/tests
```

- [ ] **Step 2: Write `crates/exg-wal-dump/Cargo.toml`**

```toml
[package]
name = "exg-wal-dump"
edition.workspace = true
version.workspace = true

[[bin]]
name = "exg-wal-dump"
path = "src/main.rs"

[dependencies]
exg-wal = { workspace = true }
exg-protocol = { workspace = true }
exg-common = { workspace = true }
rkyv = { workspace = true }
serde_json = { workspace = true }
anyhow = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }
```

- [ ] **Step 3: Write placeholder `crates/exg-wal-dump/src/lib.rs`**

```rust
//! WAL event dumper. See `Stage 0` spec §5.3.

pub fn placeholder() {}
```

- [ ] **Step 4: Write placeholder `crates/exg-wal-dump/src/main.rs`**

```rust
fn main() {
    eprintln!("exg-wal-dump: not implemented yet");
    std::process::exit(2);
}
```

- [ ] **Step 5: Add to workspace `Cargo.toml`**

In the root `Cargo.toml`, add `"crates/exg-wal-dump"` to `[workspace] members`, and add `exg-wal-dump = { path = "crates/exg-wal-dump" }` to `[workspace.dependencies]` (preserve alphabetical-ish order with other workspace crates).

- [ ] **Step 6: Verify workspace builds**

Run: `cargo check --workspace`
Expected: clean, exg-wal-dump compiles as workspace member.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml crates/exg-wal-dump/
git commit -m "chore: add exg-wal-dump workspace member"
```

---

## Task 2 — `exg-config`: add `mark_price` field + validation + tests

**Files:**
- Modify: `crates/exg-config/src/lib.rs` (lines ~82-112 for `SymbolConfigEntry`; lines ~203-267 for `default_btcusdt`)
- Modify: `crates/exg-config/src/validation.rs:106` (`validate_symbol`)
- Modify: `config/default.toml` (line 41-53 `[[trading.symbols]]`)
- Modify: `crates/exg-config/src/tests.rs` (if exists) or inline tests

- [ ] **Step 1: Write the failing test first**

Append to `crates/exg-config/src/tests.rs` (or create cfg(test) module in `lib.rs`):

```rust
#[test]
fn test_symbol_mark_price_field_parses() {
    let mut cfg = ExgConfig::default_config();
    cfg.trading.symbols[0].mark_price = "60000".into();
    assert!(cfg.validate().is_ok());
}

#[test]
fn test_symbol_mark_price_must_be_positive() {
    let mut cfg = ExgConfig::default_config();
    cfg.trading.symbols[0].mark_price = "0".into();
    let err = cfg.validate().unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("mark_price"), "msg: {msg}");
}

#[test]
fn test_symbol_mark_price_must_parse_as_decimal() {
    let mut cfg = ExgConfig::default_config();
    cfg.trading.symbols[0].mark_price = "not-a-number".into();
    assert!(cfg.validate().is_err());
}
```

- [ ] **Step 2: Run tests — verify they fail**

Run: `cargo test -p exg-config test_symbol_mark_price`
Expected: compile errors (field `mark_price` doesn't exist on `SymbolConfigEntry`).

- [ ] **Step 3: Add `mark_price` field to `SymbolConfigEntry`**

In `crates/exg-config/src/lib.rs`, in the `SymbolConfigEntry` struct, append:

```rust
    /// Static mark price for Stage 0; replaced by oracle/mark service in Stage 2.
    /// Decimal as string, e.g. "60000".
    pub mark_price: String,
```

- [ ] **Step 4: Update `default_btcusdt()` and any other constructors**

In `crates/exg-config/src/lib.rs` `default_btcusdt()`, after `taker_fee`:

```rust
            mark_price: "60000".into(),
```

- [ ] **Step 5: Add validation**

In `crates/exg-config/src/validation.rs`, inside `validate_symbol`, add:

```rust
    let mark = sym
        .mark_price
        .parse::<exg_common::Decimal128>()
        .map_err(|e| ConfigError::Validation(format!(
            "symbol {} mark_price: {e}", sym.name
        )))?;
    if mark <= exg_common::Decimal128::ZERO {
        return Err(ConfigError::Validation(format!(
            "symbol {} mark_price must be positive, got {}", sym.name, mark
        )));
    }
```

(Adjust import path; if `exg_common` isn't directly available in this crate, add `exg-common` to `crates/exg-config/Cargo.toml` dependencies first. Check first with `grep "exg-common" crates/exg-config/Cargo.toml`.)

- [ ] **Step 6: Update `config/default.toml`**

In `config/default.toml`, in the existing `[[trading.symbols]]` block, after `taker_fee = "0.0005"`:

```toml
mark_price = "60000"
```

- [ ] **Step 7: Run tests — verify they pass**

Run: `cargo test -p exg-config`
Expected: all green (existing 14 + new 3 = 17).

- [ ] **Step 8: Commit**

```bash
git add crates/exg-config/ config/default.toml
git commit -m "feat(config): add per-symbol mark_price field for stage 0"
```

---

## Task 3 — `exg-protocol`: slot-size property test

**Files:**
- Create: `crates/exg-protocol/tests/slot_size.rs`

Background: spec §8.1 mandates a property test ensuring every `Command` variant fits in `cfg.ringbuffer.slot_size = 4096`. This catches future protocol field expansions that would silently start failing on `Producer::try_push` with `MessageTooLarge`.

- [ ] **Step 1: Create the test file**

```rust
// crates/exg-protocol/tests/slot_size.rs

use exg_common::{
    Decimal128, MarginMode, OrderId, OrderType, Side, SymbolId, TimeInForce, UnixMicros, UserId,
};
use exg_protocol::Command;

const SLOT_SIZE: usize = 4096;

fn dec(s: &str) -> Decimal128 {
    s.parse().unwrap()
}

fn ts() -> UnixMicros {
    UnixMicros::from_micros(1_700_000_000_000_000)
}

fn maximal_new_order() -> Command {
    Command::NewOrder {
        order_id: OrderId::new(u64::MAX),
        user_id: UserId::new(u64::MAX),
        symbol: SymbolId::new(u16::MAX),
        side: Side::Buy,
        order_type: OrderType::Iceberg,
        time_in_force: TimeInForce::Gtd,
        price: Some(dec("999999999999.999999999999999999")),
        quantity: dec("999999999999.999999999999999999"),
        stop_price: Some(dec("999999999999.999999999999999999")),
        trailing_delta: Some(dec("999999999.999999")),
        visible_quantity: Some(dec("100.0")),
        reduce_only: true,
        margin_mode: MarginMode::Cross,
        leverage: Some(dec("125")),
        client_order_id: Some(u64::MAX),
        timestamp: ts(),
    }
}

fn maximal_cancel() -> Command {
    Command::CancelOrder {
        order_id: OrderId::new(u64::MAX),
        user_id: UserId::new(u64::MAX),
        symbol: SymbolId::new(u16::MAX),
        timestamp: ts(),
    }
}

fn maximal_amend() -> Command {
    Command::AmendOrder {
        order_id: OrderId::new(u64::MAX),
        user_id: UserId::new(u64::MAX),
        symbol: SymbolId::new(u16::MAX),
        new_price: Some(dec("999999999999.999999999999999999")),
        new_quantity: Some(dec("999999999999.999999999999999999")),
        timestamp: ts(),
    }
}

fn maximal_cancel_all() -> Command {
    Command::CancelAllOrders {
        user_id: UserId::new(u64::MAX),
        symbol: SymbolId::new(u16::MAX),
        timestamp: ts(),
    }
}

fn check(name: &str, cmd: Command) {
    let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&cmd)
        .unwrap_or_else(|e| panic!("rkyv encode {name}: {e}"));
    assert!(
        bytes.len() <= SLOT_SIZE,
        "{name}: rkyv encoded size {} exceeds ring buffer slot_size {}",
        bytes.len(),
        SLOT_SIZE
    );
}

#[test]
fn maximal_new_order_fits_in_slot() {
    check("NewOrder", maximal_new_order());
}

#[test]
fn maximal_cancel_fits_in_slot() {
    check("CancelOrder", maximal_cancel());
}

#[test]
fn maximal_amend_fits_in_slot() {
    check("AmendOrder", maximal_amend());
}

#[test]
fn maximal_cancel_all_fits_in_slot() {
    check("CancelAllOrders", maximal_cancel_all());
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p exg-protocol --test slot_size`
Expected: 4 tests pass. The actual encoded size of `NewOrder` should be well under 4096 (≈ 200-300 bytes); we're guarding against future regression.

- [ ] **Step 3: Commit**

```bash
git add crates/exg-protocol/tests/slot_size.rs
git commit -m "test(protocol): assert all Command variants fit in ring buffer slot"
```

---

## Task 4 — `exg-matching-engine`: add `set_mark_price` setter

**Files:**
- Modify: `crates/exg-matching-engine/src/engine.rs` (after `pub fn new` on line 32-44; near `pub fn mark_price()` on line 960)

The engine has `mark_price()` getter at line 960 but no setter. Stage 0 needs to inject the static config mark price after construction.

- [ ] **Step 1: Write the failing test**

In `crates/exg-matching-engine/src/engine.rs` test module (or wherever existing engine tests live), add:

```rust
#[test]
fn set_mark_price_updates_internal_state() {
    let cfg = SymbolConfig {
        symbol: SymbolId::new(1),
        // ... use whatever existing test helper produces a valid SymbolConfig.
        // If a helper like `test_symbol_config()` exists, use it.
        ..Default::default()
    };
    let mut engine = MatchingEngine::new(cfg, 1);
    assert_eq!(engine.mark_price(), Decimal128::ZERO);
    engine.set_mark_price("60000".parse().unwrap());
    assert_eq!(engine.mark_price(), "60000".parse().unwrap());
}
```

(If `SymbolConfig` doesn't `derive(Default)`, locate the existing test helper that constructs one and reuse it; if none exists, build one inline using the same pattern as other tests in the file.)

- [ ] **Step 2: Run — verify fail**

Run: `cargo test -p exg-matching-engine set_mark_price_updates`
Expected: compile error, `set_mark_price` method doesn't exist.

- [ ] **Step 3: Add the setter**

In `crates/exg-matching-engine/src/engine.rs`, after the `pub fn new(...)` method:

```rust
    /// Set the mark price used by pre-trade risk checks. Stage 0 injects this
    /// once at boot from config; Stage 2+ replaces this with a real feed.
    pub fn set_mark_price(&mut self, price: Decimal128) {
        self.mark_price = price;
    }
```

- [ ] **Step 4: Run tests — verify pass**

Run: `cargo test -p exg-matching-engine`
Expected: all green (existing 41 + new 1 = 42).

- [ ] **Step 5: Commit**

```bash
git add crates/exg-matching-engine/src/engine.rs
git commit -m "feat(matching): add set_mark_price setter for stage 0 static feed"
```

---

## Task 5 — `exg-wal-dump`: implement library + binary + tests

**Files:**
- Rewrite: `crates/exg-wal-dump/src/lib.rs`
- Rewrite: `crates/exg-wal-dump/src/main.rs`
- Create: `crates/exg-wal-dump/tests/dump_tests.rs`

- [ ] **Step 1: Write the failing tests first**

Create `crates/exg-wal-dump/tests/dump_tests.rs`:

```rust
use exg_common::{OrderId, Side, SymbolId, TradeId, UnixMicros, UserId};
use exg_protocol::{Event, RejectReason};
use exg_wal::{WalConfig, WalWriter};
use exg_wal_dump::dump;
use tempfile::TempDir;

fn ts() -> UnixMicros {
    UnixMicros::from_micros(1_700_000_000_000_000)
}

fn dec(s: &str) -> exg_common::Decimal128 {
    s.parse().unwrap()
}

fn wal_cfg(dir: &std::path::Path) -> WalConfig {
    WalConfig {
        dir: dir.to_path_buf(),
        segment_size: 64 * 1024 * 1024,
        flush_interval_us: 1000,
        flush_every_n: 1,
    }
}

fn write_events(dir: &std::path::Path, events: &[Event]) {
    let mut w = WalWriter::open(wal_cfg(dir)).unwrap();
    for e in events {
        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(e).unwrap();
        w.append(&bytes).unwrap();
    }
    w.flush().unwrap();
}

#[test]
fn happy_dump_three_events() {
    let tmp = TempDir::new().unwrap();
    let events = vec![
        Event::OrderAccepted {
            order_id: OrderId::new(1),
            user_id: UserId::new(42),
            symbol: SymbolId::new(1),
            client_order_id: None,
            timestamp: ts(),
        },
        Event::OrderRejected {
            order_id: OrderId::new(2),
            user_id: UserId::new(42),
            reason: RejectReason::InsufficientMargin,
            timestamp: ts(),
        },
        Event::OrderCanceled {
            order_id: OrderId::new(1),
            user_id: UserId::new(42),
            symbol: SymbolId::new(1),
            remaining_qty: dec("0.5"),
            timestamp: ts(),
        },
    ];
    write_events(tmp.path(), &events);

    let mut out = Vec::new();
    dump(tmp.path(), 0, &mut out).unwrap();
    let s = String::from_utf8(out).unwrap();
    let lines: Vec<&str> = s.lines().collect();
    assert_eq!(lines.len(), 3, "expected 3 JSON lines, got {}: {s}", lines.len());
    assert!(lines[0].contains("OrderAccepted"));
    assert!(lines[1].contains("OrderRejected"));
    assert!(lines[1].contains("InsufficientMargin"));
    assert!(lines[2].contains("OrderCanceled"));
}

#[test]
fn empty_wal_produces_no_output() {
    let tmp = TempDir::new().unwrap();
    let mut out = Vec::new();
    dump(tmp.path(), 0, &mut out).unwrap();
    assert!(out.is_empty(), "expected empty output, got {:?}", out);
}

#[test]
fn from_seq_filters_earlier_events() {
    let tmp = TempDir::new().unwrap();
    let events: Vec<Event> = (0..8)
        .map(|i| Event::OrderAccepted {
            order_id: OrderId::new(i),
            user_id: UserId::new(42),
            symbol: SymbolId::new(1),
            client_order_id: None,
            timestamp: ts(),
        })
        .collect();
    write_events(tmp.path(), &events);

    let mut out = Vec::new();
    dump(tmp.path(), 5, &mut out).unwrap();
    let s = String::from_utf8(out).unwrap();
    let lines: Vec<&str> = s.lines().collect();
    assert_eq!(lines.len(), 3, "from_seq=5 over 8 events should yield 3");
    // Sequence prefix appears as "{seq}\t{json}"
    assert!(lines[0].starts_with("5\t"), "first line: {}", lines[0]);
    assert!(lines[2].starts_with("7\t"), "last line: {}", lines[2]);
}

#[test]
fn corrupted_wal_returns_error() {
    let tmp = TempDir::new().unwrap();
    let event = Event::OrderAccepted {
        order_id: OrderId::new(1),
        user_id: UserId::new(42),
        symbol: SymbolId::new(1),
        client_order_id: None,
        timestamp: ts(),
    };
    write_events(tmp.path(), &[event]);

    // Corrupt one byte in the segment file.
    let segments: Vec<_> = std::fs::read_dir(tmp.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("wal-"))
        .collect();
    assert!(!segments.is_empty());
    let seg_path = segments[0].path();
    let mut data = std::fs::read(&seg_path).unwrap();
    // Flip a byte in the middle (past the header).
    let mid = data.len() / 2;
    data[mid] ^= 0xFF;
    std::fs::write(&seg_path, data).unwrap();

    let mut out = Vec::new();
    let err = dump(tmp.path(), 0, &mut out);
    assert!(err.is_err(), "expected dump error for corrupt WAL");
}
```

- [ ] **Step 2: Run tests — verify they fail**

Run: `cargo test -p exg-wal-dump`
Expected: compile errors (`dump` function not yet exported).

- [ ] **Step 3: Implement `crates/exg-wal-dump/src/lib.rs`**

```rust
//! WAL event dumper library. Reads rkyv-encoded `exg_protocol::Event` records
//! from a WAL directory and writes them as one JSON line per record,
//! prefixed with the sequence number and a tab.
//!
//! Stage 0 §5.3 — verification tool for the demo and integration tests.

use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use exg_protocol::Event;
use exg_wal::WalReader;

/// Dump events from `wal_dir` starting at `from_seq`, writing one JSON line
/// per event to `out`.
///
/// Returns an error on CRC failure, malformed rkyv payload, or IO.
pub fn dump(wal_dir: &Path, from_seq: u64, out: &mut dyn Write) -> Result<()> {
    if !wal_dir.exists() {
        // Empty WAL dir behavior: no output, no error.
        return Ok(());
    }
    let mut reader = WalReader::open(wal_dir)
        .with_context(|| format!("opening WAL dir {}", wal_dir.display()))?;

    let mut write_err: Option<std::io::Error> = None;
    let result = reader.read_from(from_seq, |seq, payload| {
        if write_err.is_some() {
            return false;
        }
        let evt = match rkyv::from_bytes::<Event, rkyv::rancor::Error>(payload) {
            Ok(e) => e,
            Err(e) => {
                write_err = Some(std::io::Error::other(format!(
                    "rkyv decode at seq {seq}: {e}"
                )));
                return false;
            }
        };
        let json = match serde_json::to_string(&evt) {
            Ok(s) => s,
            Err(e) => {
                write_err = Some(std::io::Error::other(format!(
                    "json encode at seq {seq}: {e}"
                )));
                return false;
            }
        };
        if let Err(e) = writeln!(out, "{seq}\t{json}") {
            write_err = Some(e);
            return false;
        }
        true
    });

    if let Some(e) = write_err {
        return Err(anyhow::Error::new(e));
    }
    result.with_context(|| format!("reading WAL at {}", wal_dir.display()))?;
    Ok(())
}
```

- [ ] **Step 4: Implement `crates/exg-wal-dump/src/main.rs`**

```rust
use std::io::{self, BufWriter, Write as _};
use std::path::PathBuf;
use std::process::ExitCode;

use exg_wal_dump::dump;

fn print_usage() {
    eprintln!("Usage: exg-wal-dump --wal-dir <path> [--from-seq <N>]");
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let mut wal_dir: Option<PathBuf> = None;
    let mut from_seq: u64 = 0;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--wal-dir" => {
                i += 1;
                if i >= args.len() {
                    print_usage();
                    return ExitCode::from(2);
                }
                wal_dir = Some(PathBuf::from(&args[i]));
            }
            "--from-seq" => {
                i += 1;
                if i >= args.len() {
                    print_usage();
                    return ExitCode::from(2);
                }
                match args[i].parse() {
                    Ok(n) => from_seq = n,
                    Err(e) => {
                        eprintln!("--from-seq: {e}");
                        return ExitCode::from(2);
                    }
                }
            }
            "-h" | "--help" => {
                print_usage();
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("unknown argument: {other}");
                print_usage();
                return ExitCode::from(2);
            }
        }
        i += 1;
    }

    let Some(dir) = wal_dir else {
        print_usage();
        return ExitCode::from(2);
    };

    let stdout = io::stdout().lock();
    let mut out = BufWriter::new(stdout);
    if let Err(e) = dump(&dir, from_seq, &mut out) {
        let _ = out.flush();
        eprintln!("exg-wal-dump: {e:#}");
        return ExitCode::from(1);
    }
    if let Err(e) = out.flush() {
        eprintln!("exg-wal-dump: stdout flush: {e}");
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}
```

- [ ] **Step 5: Run tests — verify they pass**

Run: `cargo test -p exg-wal-dump`
Expected: 4 tests pass.

- [ ] **Step 6: Smoke test the binary**

Run:
```bash
TMP=$(mktemp -d)
cargo run -p exg-wal-dump -- --wal-dir "$TMP"
echo "exit=$?"
rm -rf "$TMP"
```
Expected: no output, exit 0.

- [ ] **Step 7: Commit**

```bash
git add crates/exg-wal-dump/
git commit -m "feat(wal-dump): implement WAL event dumper bin and tests"
```

---

## Task 6 — `exg-api-gateway`: Cargo deps + conversion::to_cancel/amend

**Files:**
- Modify: `crates/exg-api-gateway/Cargo.toml`
- Modify: `crates/exg-api-gateway/src/types.rs`
- Modify: `crates/exg-api-gateway/src/conversion.rs`

- [ ] **Step 1: Update `crates/exg-api-gateway/Cargo.toml`**

Append to `[dependencies]`:

```toml
actix-web = { workspace = true }
exg-config = { workspace = true }
exg-ringbuffer = { workspace = true }
exg-wal = { workspace = true }
exg-common = { workspace = true }
parking_lot = { workspace = true }
tokio = { workspace = true }
tracing = { workspace = true }
rkyv = { workspace = true }
```

Append to `[dev-dependencies]`:

```toml
tempfile = { workspace = true }
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p exg-api-gateway`
Expected: clean.

- [ ] **Step 3: Add request types to `types.rs`**

In `crates/exg-api-gateway/src/types.rs`, append (preserve serde conventions used by `PlaceOrderRequest`):

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelOrderRequest {
    /// Server-generated order ID returned by the place call.
    pub order_id: u64,
    pub symbol: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AmendOrderRequest {
    pub order_id: u64,
    pub symbol: String,
    /// Decimal as string, e.g. "59500".
    pub new_price: Option<String>,
    pub new_quantity: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaceOrderResponse {
    /// Stringified u64 per Binance convention (avoids JS 53-bit precision loss).
    pub order_id: String,
    pub client_order_id: Option<u64>,
    pub status: &'static str,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AckResponse {
    pub order_id: String,
    pub status: &'static str,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HealthResponse {
    pub status: &'static str,
}
```

- [ ] **Step 4: Write the failing tests**

In `crates/exg-api-gateway/src/conversion.rs` tests module (or in the existing `tests` mod in `lib.rs`), add:

```rust
#[test]
fn to_cancel_order_command_happy() {
    let req = CancelOrderRequest {
        order_id: 12345,
        symbol: "BTCUSDT".into(),
    };
    let cmd = to_cancel_order_command(
        &req,
        UserId::new(42),
        SymbolId::new(1),
        UnixMicros::from_micros(1),
    )
    .unwrap();
    match cmd {
        Command::CancelOrder { order_id, user_id, symbol, .. } => {
            assert_eq!(order_id, OrderId::new(12345));
            assert_eq!(user_id, UserId::new(42));
            assert_eq!(symbol, SymbolId::new(1));
        }
        _ => panic!("expected CancelOrder"),
    }
}

#[test]
fn to_amend_order_command_happy_price_only() {
    let req = AmendOrderRequest {
        order_id: 99,
        symbol: "BTCUSDT".into(),
        new_price: Some("60500".into()),
        new_quantity: None,
    };
    let cmd = to_amend_order_command(
        &req,
        UserId::new(7),
        SymbolId::new(1),
        UnixMicros::from_micros(2),
    )
    .unwrap();
    match cmd {
        Command::AmendOrder { order_id, new_price, new_quantity, .. } => {
            assert_eq!(order_id, OrderId::new(99));
            assert!(new_price.is_some());
            assert!(new_quantity.is_none());
        }
        _ => panic!("expected AmendOrder"),
    }
}

#[test]
fn to_amend_order_command_rejects_empty_amend() {
    let req = AmendOrderRequest {
        order_id: 99,
        symbol: "BTCUSDT".into(),
        new_price: None,
        new_quantity: None,
    };
    let err = to_amend_order_command(
        &req,
        UserId::new(7),
        SymbolId::new(1),
        UnixMicros::from_micros(2),
    )
    .unwrap_err();
    assert!(err.msg.contains("at least one of"), "msg: {}", err.msg);
}
```

- [ ] **Step 5: Run — verify fail**

Run: `cargo test -p exg-api-gateway to_cancel_order_command_happy`
Expected: compile error, function doesn't exist.

- [ ] **Step 6: Implement the two new conversion functions**

In `crates/exg-api-gateway/src/conversion.rs`, after `to_new_order_command`:

```rust
pub fn to_cancel_order_command(
    req: &CancelOrderRequest,
    user_id: UserId,
    symbol: SymbolId,
    ts: UnixMicros,
) -> Result<Command, ApiError> {
    Ok(Command::CancelOrder {
        order_id: OrderId::new(req.order_id),
        user_id,
        symbol,
        timestamp: ts,
    })
}

pub fn to_amend_order_command(
    req: &AmendOrderRequest,
    user_id: UserId,
    symbol: SymbolId,
    ts: UnixMicros,
) -> Result<Command, ApiError> {
    if req.new_price.is_none() && req.new_quantity.is_none() {
        return Err(ApiError::bad_request(
            "amend: at least one of newPrice or newQuantity must be present",
        ));
    }
    let new_price = req
        .new_price
        .as_deref()
        .map(|s| s.parse::<Decimal128>())
        .transpose()
        .map_err(|e| ApiError::bad_request(format!("newPrice: {e}")))?;
    let new_quantity = req
        .new_quantity
        .as_deref()
        .map(|s| s.parse::<Decimal128>())
        .transpose()
        .map_err(|e| ApiError::bad_request(format!("newQuantity: {e}")))?;
    Ok(Command::AmendOrder {
        order_id: OrderId::new(req.order_id),
        user_id,
        symbol,
        new_price,
        new_quantity,
        timestamp: ts,
    })
}
```

(Verify imports at top of `conversion.rs` include `Decimal128`, `OrderId`, `SymbolId`, `UnixMicros`, `UserId`, `Command`, `ApiError`, and the new request types.)

- [ ] **Step 7: Run tests — verify pass**

Run: `cargo test -p exg-api-gateway`
Expected: all green.

- [ ] **Step 8: Commit**

```bash
git add crates/exg-api-gateway/
git commit -m "feat(api-gateway): add cancel/amend command conversion + types"
```

---

## Task 7 — `exg-api-gateway`: state + handlers + app_factory

**Files:**
- Create: `crates/exg-api-gateway/src/state.rs`
- Create: `crates/exg-api-gateway/src/handlers.rs`
- Create: `crates/exg-api-gateway/src/app_factory.rs`
- Modify: `crates/exg-api-gateway/src/lib.rs` (export new modules)

This is the largest task. Split into 4 distinct commits if needed.

- [ ] **Step 1: Create `state.rs`**

```rust
// crates/exg-api-gateway/src/state.rs

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
```

- [ ] **Step 2: Create `handlers.rs`**

```rust
// crates/exg-api-gateway/src/handlers.rs

use actix_web::{HttpRequest, HttpResponse, web};
use exg_common::{OrderId, SymbolId, UnixMicros, UserId};
use exg_protocol::Command;
use tracing::{info, warn};

use crate::conversion::{
    to_amend_order_command, to_cancel_order_command, to_new_order_command,
};
use crate::error::{ApiError, ERR_INVALID_PARAMETER, ERR_TOO_MANY_REQUESTS, ERR_UNAUTHORIZED};
use crate::state::AppState;
use crate::types::{
    AckResponse, AmendOrderRequest, CancelOrderRequest, HealthResponse, PlaceOrderRequest,
    PlaceOrderResponse,
};

/// Extract `X-User-Id` numeric header → `UserId`.
fn extract_user_id(req: &HttpRequest) -> Result<UserId, ApiError> {
    let h = req
        .headers()
        .get("X-User-Id")
        .ok_or_else(|| ApiError::unauthorized("missing X-User-Id header"))?;
    let s = h
        .to_str()
        .map_err(|_| ApiError::unauthorized("X-User-Id is not valid ASCII"))?;
    let n: u64 = s
        .parse()
        .map_err(|_| ApiError::unauthorized("X-User-Id is not numeric"))?;
    Ok(UserId::new(n))
}

fn now() -> UnixMicros {
    let micros = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0);
    UnixMicros::from_micros(micros)
}

fn lookup_symbol_id(cfg: &exg_config::ExgConfig, name: &str) -> Result<SymbolId, ApiError> {
    cfg.trading
        .symbols
        .iter()
        .find(|s| s.name == name)
        .map(|s| SymbolId::new(s.id))
        .ok_or_else(|| ApiError::bad_request(format!("unknown symbol: {name}")))
}

fn enqueue(state: &AppState, cmd: &Command) -> Result<(), ApiError> {
    let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(cmd)
        .map_err(|e| ApiError::internal(format!("rkyv encode: {e}")))?;
    let producer = state.producer.lock();
    producer.try_push(&bytes).map_err(|e| {
        // The exg-ringbuffer error enum distinguishes Full / MessageTooLarge / etc.
        // Map per spec §7.1.
        let msg = format!("{e}");
        if msg.to_lowercase().contains("full") {
            ApiError::rate_limited()
        } else if msg.to_lowercase().contains("too large") {
            ApiError::bad_request("command too large for ring slot")
        } else {
            ApiError::internal(format!("ring buffer push: {msg}"))
        }
    })?;
    Ok(())
}

pub async fn health() -> HttpResponse {
    HttpResponse::Ok().json(HealthResponse { status: "ok" })
}

pub async fn place_order(
    state: web::Data<AppState>,
    req: HttpRequest,
    body: web::Json<PlaceOrderRequest>,
) -> Result<HttpResponse, ApiError> {
    let user_id = extract_user_id(&req)?;
    let symbol = lookup_symbol_id(&state.cfg, &body.symbol)?;
    let order_id = OrderId::new(state.snowflake.next_id());
    let ts = now();
    info!(target: "handler", path = "/order", user_id = user_id.get(), order_id = order_id.get(), "place_order in");

    let cmd = to_new_order_command(&body, user_id, order_id, ts).map_err(|e| {
        warn!(target: "conversion", reason = %e.msg, "to_new_order_command failed");
        e
    })?;
    enqueue(&state, &cmd)?;

    let resp = PlaceOrderResponse {
        order_id: order_id.get().to_string(),
        client_order_id: body.client_order_id,
        status: "ACCEPTED",
    };
    info!(target: "handler", path = "/order", status = 200, order_id = order_id.get(), "place_order out");
    Ok(HttpResponse::Ok().json(resp))
}

pub async fn cancel_order(
    state: web::Data<AppState>,
    req: HttpRequest,
    body: web::Json<CancelOrderRequest>,
) -> Result<HttpResponse, ApiError> {
    let user_id = extract_user_id(&req)?;
    let symbol = lookup_symbol_id(&state.cfg, &body.symbol)?;
    let ts = now();
    info!(target: "handler", path = "/order/cancel", user_id = user_id.get(), order_id = body.order_id, "cancel_order in");

    let cmd = to_cancel_order_command(&body, user_id, symbol, ts)?;
    enqueue(&state, &cmd)?;

    let resp = AckResponse {
        order_id: body.order_id.to_string(),
        status: "ACCEPTED",
    };
    info!(target: "handler", path = "/order/cancel", status = 200, "cancel_order out");
    Ok(HttpResponse::Ok().json(resp))
}

pub async fn amend_order(
    state: web::Data<AppState>,
    req: HttpRequest,
    body: web::Json<AmendOrderRequest>,
) -> Result<HttpResponse, ApiError> {
    let user_id = extract_user_id(&req)?;
    let symbol = lookup_symbol_id(&state.cfg, &body.symbol)?;
    let ts = now();
    info!(target: "handler", path = "/order/amend", user_id = user_id.get(), order_id = body.order_id, "amend_order in");

    let cmd = to_amend_order_command(&body, user_id, symbol, ts).map_err(|e| {
        warn!(target: "conversion", reason = %e.msg, "to_amend_order_command failed");
        e
    })?;
    enqueue(&state, &cmd)?;

    let resp = AckResponse {
        order_id: body.order_id.to_string(),
        status: "ACCEPTED",
    };
    info!(target: "handler", path = "/order/amend", status = 200, "amend_order out");
    Ok(HttpResponse::Ok().json(resp))
}
```

Note: `ApiError::unauthorized` / `internal` may not exist yet — check `error.rs`. If missing, add them (1-line constructors mirroring existing `bad_request` / `rate_limited`). Also confirm `UserId::get()` exists; if not, use whatever the existing accessor is (likely `into_inner()` or pub field `.0`).

- [ ] **Step 3: Create `app_factory.rs`**

```rust
// crates/exg-api-gateway/src/app_factory.rs

use actix_web::{App, web};

use crate::handlers::{amend_order, cancel_order, health, place_order};
use crate::state::AppState;

/// Build the Actix `App` with the Stage 0 route table.
///
/// Returns an opaque `App` so the caller can `HttpServer::new(|| build_app(state))`.
pub fn build_app(
    state: AppState,
) -> App<
    impl actix_web::dev::ServiceFactory<
        actix_web::dev::ServiceRequest,
        Config = (),
        Response = actix_web::dev::ServiceResponse,
        Error = actix_web::Error,
        InitError = (),
    >,
> {
    App::new()
        .app_data(web::Data::new(state))
        .route("/api/v1/health", web::get().to(health))
        .route("/api/v1/order", web::post().to(place_order))
        .route("/api/v1/order/cancel", web::post().to(cancel_order))
        .route("/api/v1/order/amend", web::post().to(amend_order))
}
```

- [ ] **Step 4: Export the new modules from `lib.rs`**

In `crates/exg-api-gateway/src/lib.rs`, add to the `pub mod` lines (top of file):

```rust
pub mod app_factory;
pub mod handlers;
pub mod state;
```

- [ ] **Step 5: Add handler unit tests**

Append to `crates/exg-api-gateway/src/handlers.rs` (or a new `#[cfg(test)] mod tests` block):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{http::StatusCode, test, web};
    use exg_common::SnowflakeGen;
    use exg_config::ExgConfig;
    use exg_ringbuffer::RingBuffer;
    use parking_lot::Mutex;
    use std::sync::Arc;

    fn test_state() -> AppState {
        let mut rb = RingBuffer::new(16, 4096).unwrap();
        let (producer, _consumer) = rb.split();
        AppState {
            producer: Arc::new(Mutex::new(producer)),
            snowflake: Arc::new(SnowflakeGen::new(1)),
            cfg: Arc::new(ExgConfig::default_config()),
        }
    }

    #[actix_web::test]
    async fn health_returns_ok() {
        let app = test::init_service(crate::app_factory::build_app(test_state())).await;
        let req = test::TestRequest::get().uri("/api/v1/health").to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[actix_web::test]
    async fn place_order_missing_header_returns_401() {
        let app = test::init_service(crate::app_factory::build_app(test_state())).await;
        let body = r#"{"symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT","timeInForce":"GTC","quantity":"0.001","price":"60000"}"#;
        let req = test::TestRequest::post()
            .uri("/api/v1/order")
            .insert_header(("Content-Type", "application/json"))
            .set_payload(body)
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let body: serde_json::Value = test::read_body_json(resp).await;
        assert_eq!(body["code"], ERR_UNAUTHORIZED);
    }

    #[actix_web::test]
    async fn place_order_non_numeric_header_returns_401() {
        let app = test::init_service(crate::app_factory::build_app(test_state())).await;
        let body = r#"{"symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT","timeInForce":"GTC","quantity":"0.001","price":"60000"}"#;
        let req = test::TestRequest::post()
            .uri("/api/v1/order")
            .insert_header(("X-User-Id", "abc"))
            .insert_header(("Content-Type", "application/json"))
            .set_payload(body)
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[actix_web::test]
    async fn place_order_malformed_json_returns_400() {
        let app = test::init_service(crate::app_factory::build_app(test_state())).await;
        let req = test::TestRequest::post()
            .uri("/api/v1/order")
            .insert_header(("X-User-Id", "42"))
            .insert_header(("Content-Type", "application/json"))
            .set_payload("not json")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert!(resp.status().is_client_error(), "got status {}", resp.status());
    }

    #[actix_web::test]
    async fn place_order_happy_returns_200_with_order_id() {
        let app = test::init_service(crate::app_factory::build_app(test_state())).await;
        let body = r#"{"symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT","timeInForce":"GTC","quantity":"0.001","price":"60000"}"#;
        let req = test::TestRequest::post()
            .uri("/api/v1/order")
            .insert_header(("X-User-Id", "42"))
            .insert_header(("Content-Type", "application/json"))
            .set_payload(body)
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body: PlaceOrderResponse = test::read_body_json(resp).await;
        assert_eq!(body.status, "ACCEPTED");
        assert!(!body.order_id.is_empty());
    }
}
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p exg-api-gateway`
Expected: all green (existing + new handler tests).

- [ ] **Step 7: Commit**

```bash
git add crates/exg-api-gateway/
git commit -m "feat(api-gateway): add AppState, handlers, app_factory for stage 0"
```

---

## Task 8 — `exg-server`: add Cargo deps + write `lib.rs` (`run_with_config`)

**Files:**
- Modify: `crates/exg-server/Cargo.toml`
- Create: `crates/exg-server/src/lib.rs`

The library function `run_with_config` is the testable seam — it starts the server in-process, returns a handle for tests to send requests, and exposes `shutdown()` so integration tests can perform deterministic teardown.

- [ ] **Step 1: Update `crates/exg-server/Cargo.toml`**

```toml
[package]
name = "exg-server"
edition.workspace = true
version.workspace = true

[lib]
path = "src/lib.rs"

[[bin]]
name = "exg-server"
path = "src/main.rs"

[dependencies]
exg-common = { workspace = true }
exg-config = { workspace = true }
exg-protocol = { workspace = true }
exg-ringbuffer = { workspace = true }
exg-wal = { workspace = true }
exg-matching-engine = { workspace = true }
exg-risk-engine = { workspace = true }
exg-api-gateway = { workspace = true }
tokio = { workspace = true }
actix-web = { workspace = true }
parking_lot = { workspace = true }
core_affinity = { workspace = true }
tracing = { workspace = true }
tracing-subscriber = { workspace = true }
metrics = { workspace = true }
metrics-exporter-prometheus = { workspace = true }
anyhow = { workspace = true }

[dev-dependencies]
reqwest = { version = "0.12", features = ["json"] }
tempfile = { workspace = true }
serde_json = { workspace = true }
```

(Add `reqwest = "0.12"` to workspace deps if not already there.)

- [ ] **Step 2: Write `crates/exg-server/src/lib.rs`**

```rust
//! Stage 0 server library. `run_with_config` is the testable entry point.
//! See spec §4.5 (startup) and §4.6 (shutdown).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};

use actix_web::dev::ServerHandle as ActixServerHandle;
use anyhow::{Context, Result, anyhow};
use exg_api_gateway::state::AppState;
use exg_api_gateway::app_factory::build_app;
use exg_common::{Decimal128, SnowflakeGen, SymbolId};
use exg_config::{ExgConfig, SymbolConfigEntry};
use exg_matching_engine::MatchingEngine;
use exg_protocol::{Command, Event};
use exg_ringbuffer::RingBuffer;
use exg_risk_engine::{MarginTier, SymbolConfig};
use exg_wal::{WalConfig as WalCfg, WalReader, WalWriter};
use parking_lot::Mutex;
use tracing::{debug, error, info, warn};

/// Handle returned by `run_with_config`; tests `shutdown()` it explicitly,
/// the binary awaits it alongside ctrl_c.
///
/// Named `ServerHandle` for the public API. Internally aliases
/// `actix_web::dev::ServerHandle` as `ActixServerHandle` to avoid name clash.
pub struct ServerHandle {
    pub bound_port: u16,
    pub actix_handle: ActixServerHandle,
    pub matching_thread: Option<JoinHandle<()>>,
    pub shutdown_flag: Arc<AtomicBool>,
}

impl ServerHandle {
    /// Execute the deterministic 5-step shutdown (spec §4.6).
    pub async fn shutdown(mut self) -> Result<()> {
        // Step 2: stop Actix gracefully (rejects new connections, awaits in-flight).
        self.actix_handle.stop(true).await;
        // Step 3: signal matching loop to exit.
        self.shutdown_flag.store(true, Ordering::Release);
        // Step 4: drain remaining commands + final fsync (inside the loop).
        if let Some(jh) = self.matching_thread.take() {
            jh.join().map_err(|_| anyhow!("matching thread panicked"))?;
        }
        Ok(())
    }
}

/// Boot the server with `cfg`. If `port_override` is `Some`, use it
/// (test harness uses port 0 to get an ephemeral port).
pub fn run_with_config(cfg: ExgConfig, port_override: Option<u16>) -> Result<ServerHandle> {
    // Step 3: host-binding invariant (spec §4.5)
    let allowed_hosts: &[&str] = &["127.0.0.1", "::1", "localhost"];
    if !allowed_hosts.contains(&cfg.server.host.as_str()) {
        return Err(anyhow!(
            "stage 0: server.host must be loopback ({allowed_hosts:?}); got {:?}. Stage 0 has no auth — binding non-loopback would let any attacker forge X-User-Id.",
            cfg.server.host
        ));
    }

    // Step 3a: single-symbol invariant
    if cfg.trading.symbols.len() != 1 {
        return Err(anyhow!(
            "stage 0: cfg.trading.symbols.len() must equal 1; got {}. Matching engine is single-symbol.",
            cfg.trading.symbols.len()
        ));
    }

    // Step 3b: WAL dir freshness
    let wal_dir = Path::new(&cfg.wal.dir);
    if wal_dir.exists() {
        let entries: Vec<_> = std::fs::read_dir(wal_dir)
            .with_context(|| format!("reading WAL dir {}", wal_dir.display()))?
            .filter_map(|e| e.ok())
            .collect();
        if !entries.is_empty() {
            return Err(anyhow!(
                "stage 0: WAL dir {} is not empty ({} entries). Stage 0 has no replay; please clear the dir or pick a fresh one. (See spec §4.5 step 3b.)",
                wal_dir.display(),
                entries.len()
            ));
        }
    }

    // Step 4-5: WAL writer
    let wal_cfg = WalCfg {
        dir: wal_dir.to_path_buf(),
        segment_size: cfg.wal.segment_size_mb * 1024 * 1024,
        flush_interval_us: cfg.wal.flush_interval_us,
        flush_every_n: cfg.wal.flush_every_n,
    };
    let wal = Arc::new(Mutex::new(
        WalWriter::open(wal_cfg).with_context(|| "opening WAL writer")?,
    ));

    // Step 6: ring buffer
    let mut rb = RingBuffer::new(cfg.ringbuffer.slot_count, cfg.ringbuffer.slot_size)
        .with_context(|| "initializing ring buffer")?;
    let (producer, consumer) = rb.split();
    let producer = Arc::new(Mutex::new(producer));

    // Step 7: symbol config → engine
    let sym_entry = &cfg.trading.symbols[0];
    let engine_sym = symbol_entry_to_risk_config(sym_entry)
        .with_context(|| "converting symbol config")?;
    let mut engine = MatchingEngine::new(engine_sym, cfg.server.node_id);

    // Step 8: mark price
    let mark: Decimal128 = sym_entry
        .mark_price
        .parse()
        .with_context(|| format!("parsing mark_price {:?}", sym_entry.mark_price))?;
    engine.set_mark_price(mark);

    // Step 9: snowflake
    let snowflake = Arc::new(SnowflakeGen::new(cfg.server.node_id));

    // Step 10: shutdown flag
    let shutdown_flag = Arc::new(AtomicBool::new(false));

    // Step 11: spawn matching thread
    let matching_thread = spawn_matching_thread(engine, consumer, Arc::clone(&wal), Arc::clone(&shutdown_flag));

    // Step 13: AppState
    let state = AppState {
        producer,
        snowflake,
        cfg: Arc::new(cfg.clone()),
    };

    // Step 14: Actix HttpServer
    let bind_port = port_override.unwrap_or(cfg.server.port);
    let bind_addr = (cfg.server.host.clone(), bind_port);
    let server = actix_web::HttpServer::new(move || build_app(state.clone()))
        .bind(bind_addr.clone())
        .with_context(|| format!("binding {}:{}", bind_addr.0, bind_addr.1))?;
    let bound_port = server.addrs().first().map(|a| a.port()).unwrap_or(bind_port);
    let server = server.run();
    let actix_handle = server.handle();

    // Detach the Actix server into the runtime; the caller awaits via handle.
    tokio::spawn(server);

    info!("exg-server stage 0 started on {}:{}", bind_addr.0, bound_port);
    Ok(ServerHandle {
        bound_port,
        actix_handle,
        matching_thread: Some(matching_thread),
        shutdown_flag,
    })
}

fn spawn_matching_thread(
    mut engine: MatchingEngine,
    consumer: exg_ringbuffer::Consumer,
    wal: Arc<Mutex<WalWriter>>,
    shutdown_flag: Arc<AtomicBool>,
) -> JoinHandle<()> {
    thread::Builder::new()
        .name("matching".into())
        .spawn(move || {
            // Best-effort CPU pin (macOS warns and continues per spec §7.2).
            if let Some(core) = core_affinity::get_core_ids()
                .and_then(|cs| cs.into_iter().next())
            {
                let ok = core_affinity::set_for_current(core);
                if !ok {
                    warn!(target: "matching", "cpu pin failed, continuing (likely macOS)");
                }
            }
            let mut buf = vec![0u8; 8192];
            loop {
                if shutdown_flag.load(Ordering::Acquire) {
                    break;
                }
                match consumer.try_pop(&mut buf) {
                    Ok(n) => {
                        let cmd: Command = rkyv::from_bytes::<Command, rkyv::rancor::Error>(&buf[..n])
                            .unwrap_or_else(|e| panic!("matching: rkyv decode at engine seq?: {e}"));
                        debug!(target: "matching", cmd_kind = std::mem::discriminant_name(&cmd), "processing");
                        let events = engine.process_command(&cmd);
                        for evt in &events {
                            let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(evt)
                                .unwrap_or_else(|e| panic!("matching: rkyv encode event: {e}"));
                            let mut w = wal.lock();
                            if let Err(e) = w.append(&bytes) {
                                error!(target: "wal", err = %e, "WAL append failed, panicking per spec §7.2");
                                panic!("WAL append failed: {e}");
                            }
                        }
                    }
                    Err(_e) => {
                        // ring buffer empty or transient — spin briefly.
                        std::hint::spin_loop();
                    }
                }
            }
            // Final flush before exit (spec §4.6 step 5).
            let mut w = wal.lock();
            if let Err(e) = w.flush() {
                error!(target: "wal", err = %e, "final WAL flush failed");
                panic!("final WAL flush failed: {e}");
            }
            info!("matching thread exiting cleanly");
        })
        .expect("spawn matching thread")
}

fn symbol_entry_to_risk_config(entry: &SymbolConfigEntry) -> Result<SymbolConfig> {
    // Convert decimal-as-string config fields to Decimal128, and assemble
    // a `risk_engine::SymbolConfig`. The exact field set is defined in
    // exg-risk-engine; this glue function is the only place the mapping lives.
    let tick_size: Decimal128 = entry.tick_size.parse().context("tick_size")?;
    let lot_size: Decimal128 = entry.lot_size.parse().context("lot_size")?;
    let min_notional: Decimal128 = entry.min_notional.parse().context("min_notional")?;
    let max_leverage: Decimal128 = entry.max_leverage.parse().context("max_leverage")?;
    let maker_fee: Decimal128 = entry.maker_fee.parse().context("maker_fee")?;
    let taker_fee: Decimal128 = entry.taker_fee.parse().context("taker_fee")?;
    let tiers: Result<Vec<MarginTier>> = entry
        .margin_tiers
        .iter()
        .map(|t| {
            Ok(MarginTier {
                notional_floor: t.notional_floor.parse().context("notional_floor")?,
                notional_cap: t.notional_cap.parse().context("notional_cap")?,
                maintenance_margin_rate: t
                    .maintenance_margin_rate
                    .parse()
                    .context("maintenance_margin_rate")?,
                maintenance_amount: t.maintenance_amount.parse().context("maintenance_amount")?,
            })
        })
        .collect();
    Ok(SymbolConfig {
        symbol: SymbolId::new(entry.id),
        tick_size,
        lot_size,
        min_notional,
        max_leverage,
        maker_fee,
        taker_fee,
        margin_tiers: tiers?,
    })
}
```

**NOTE for implementer:** the exact `SymbolConfig` / `MarginTier` field names in `exg-risk-engine` may differ from the placeholder above. Read `crates/exg-risk-engine/src/lib.rs` first; adjust field names to match. If `MarginTier` doesn't yet exist as a `pub struct`, raise it as a finding for Stage 1+ rather than restructuring it in Stage 0 — but the symbol→config mapping must work.

Similarly, `std::mem::discriminant_name` is not stable Rust — replace with manual match arms or simply omit the `cmd_kind` debug field if the API isn't available. Verify with `cargo check` and adjust.

- [ ] **Step 3: Verify compiles**

Run: `cargo check -p exg-server`
Expected: clean (adjust risk-engine field names as needed).

- [ ] **Step 4: Commit**

```bash
git add crates/exg-server/
git commit -m "feat(server): add run_with_config library entry point for stage 0"
```

---

## Task 9 — `exg-server`: rewrite `main.rs` to call `run_with_config`

**Files:**
- Rewrite: `crates/exg-server/src/main.rs` (currently 66 lines, TODO-only)

- [ ] **Step 1: Write the new `main.rs`**

```rust
//! Stage 0 server binary. Delegates to `exg_server::run_with_config`.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use metrics_exporter_prometheus::PrometheusBuilder;

#[actix_web::main]
async fn main() -> Result<()> {
    // Tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()),
        )
        .json()
        .init();

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        "exg-server stage 0 starting"
    );

    // Prometheus exporter (preserved from prior main.rs)
    PrometheusBuilder::new()
        .with_http_listener(([0, 0, 0, 0], 9000))
        .install()
        .expect("Failed to install Prometheus exporter");

    metrics::describe_counter!("exg_api_requests_total", "Total API requests");
    metrics::describe_histogram!(
        "exg_matching_engine_latency_seconds",
        "Matching engine order processing latency"
    );

    // Config
    let cfg_path: PathBuf = std::env::var_os("EXG_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("config/default.toml"));
    let cfg = exg_config::ExgConfig::load(&cfg_path)?;

    // Boot
    let handle = exg_server::run_with_config(cfg, None)?;
    tracing::info!(port = handle.bound_port, "exg-server ready");

    // Wait for ctrl_c, then graceful shutdown (spec §4.6).
    tokio::signal::ctrl_c().await.expect("ctrl_c handler");
    tracing::info!("ctrl_c received, shutting down");
    handle.shutdown().await?;
    tracing::info!("exg-server stage 0 stopped");
    Ok(())
}
```

- [ ] **Step 2: Build + smoke run**

```bash
cargo build -p exg-server
# Test boot fails on stale WAL (left over from earlier dev runs)
rm -rf data/wal
EXG_CONFIG=config/default.toml cargo run -p exg-server &
SERVER_PID=$!
sleep 2
curl -sf http://127.0.0.1:8080/api/v1/health
kill -TERM $SERVER_PID
wait $SERVER_PID 2>/dev/null || true
```
Expected: `{"status":"ok"}` from curl, clean shutdown.

- [ ] **Step 3: Commit**

```bash
git add crates/exg-server/src/main.rs
git commit -m "feat(server): wire stage 0 main.rs to run_with_config"
```

---

## Task 10 — `exg-server`: boot panic test suite

**Files:**
- Create: `crates/exg-server/tests/boot_panics.rs`

- [ ] **Step 1: Write the test file**

```rust
//! Stage 0 boot-time invariant guards (spec §9 invariants 1-4).
//! These tests ensure that misconfigurations fail loudly at startup
//! rather than silently misbehaving in production.

use exg_config::ExgConfig;
use tempfile::TempDir;

fn base_cfg(wal_dir: &std::path::Path) -> ExgConfig {
    let mut cfg = ExgConfig::default_config();
    cfg.wal.dir = wal_dir.to_string_lossy().into_owned();
    cfg.server.port = 0; // ephemeral
    cfg
}

#[tokio::test]
async fn boot_panics_on_non_loopback_host() {
    let tmp = TempDir::new().unwrap();
    let mut cfg = base_cfg(tmp.path());
    cfg.server.host = "0.0.0.0".into();
    let err = exg_server::run_with_config(cfg, None).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("loopback") || msg.contains("127.0.0.1"),
        "expected host-invariant message, got: {msg}"
    );
}

#[tokio::test]
async fn boot_panics_on_multiple_symbols() {
    let tmp = TempDir::new().unwrap();
    let mut cfg = base_cfg(tmp.path());
    let extra = cfg.trading.symbols[0].clone();
    cfg.trading.symbols.push(extra);
    let err = exg_server::run_with_config(cfg, None).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("symbols.len") || msg.contains("single-symbol"),
        "expected symbol-count message, got: {msg}"
    );
}

#[tokio::test]
async fn boot_panics_on_nonempty_wal_dir() {
    let tmp = TempDir::new().unwrap();
    // Drop a sentinel file into the WAL dir.
    std::fs::write(tmp.path().join("sentinel"), b"stale data").unwrap();
    let cfg = base_cfg(tmp.path());
    let err = exg_server::run_with_config(cfg, None).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("WAL dir") && msg.contains("not empty"),
        "expected WAL freshness message, got: {msg}"
    );
}

#[tokio::test]
async fn boot_panics_on_invalid_mark_price() {
    let tmp = TempDir::new().unwrap();
    let mut cfg = base_cfg(tmp.path());
    cfg.trading.symbols[0].mark_price = "-1".into();
    // validate() should reject this before run_with_config even sees it,
    // but call validate explicitly to confirm.
    let err = cfg.validate().unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("mark_price"), "got: {msg}");
}
```

- [ ] **Step 2: Run tests — verify pass**

Run: `cargo test -p exg-server --test boot_panics`
Expected: 4 tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/exg-server/tests/boot_panics.rs
git commit -m "test(server): add stage 0 boot-time invariant guard suite"
```

---

## Task 11 — `exg-server` e2e: happy + error paths

**Files:**
- Create: `crates/exg-server/tests/stage0_e2e.rs`

- [ ] **Step 1: Write the e2e test (happy + error paths)**

```rust
//! Stage 0 end-to-end integration tests.
//! Boots the server in-process via `run_with_config`, fires reqwest calls,
//! then walks the WAL to assert the expected event sequence.

use exg_config::ExgConfig;
use exg_protocol::Event;
use exg_wal::WalReader;
use reqwest::Client;
use std::time::Duration;
use tempfile::TempDir;

fn base_cfg(wal_dir: &std::path::Path) -> ExgConfig {
    let mut cfg = ExgConfig::default_config();
    cfg.wal.dir = wal_dir.to_string_lossy().into_owned();
    cfg.server.host = "127.0.0.1".into();
    cfg.server.port = 0;
    cfg
}

async fn boot_server(cfg: ExgConfig) -> (exg_server::ServerHandle, String) {
    let handle = exg_server::run_with_config(cfg, None).expect("server boot");
    let base = format!("http://127.0.0.1:{}", handle.bound_port);
    // Health-poll until ready (max 5s).
    let client = Client::new();
    for _ in 0..50 {
        if client
            .get(format!("{base}/api/v1/health"))
            .timeout(Duration::from_millis(100))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
        {
            return (handle, base);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("server did not become ready");
}

fn read_events(wal_dir: &std::path::Path) -> Vec<Event> {
    let mut reader = WalReader::open(wal_dir).unwrap();
    let mut events = Vec::new();
    reader
        .read_from(0, |_seq, payload| {
            let e: Event = rkyv::from_bytes::<Event, rkyv::rancor::Error>(payload).unwrap();
            events.push(e);
            true
        })
        .unwrap();
    events
}

#[actix_web::test]
async fn place_cancel_amend_happy_path() {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let wal_dir = std::path::PathBuf::from(&cfg.wal.dir);
    let (handle, base) = boot_server(cfg).await;
    let client = Client::new();

    // Place
    let place: serde_json::Value = client
        .post(format!("{base}/api/v1/order"))
        .header("X-User-Id", "42")
        .json(&serde_json::json!({
            "symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT",
            "timeInForce":"GTC","quantity":"0.001","price":"59000"
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let order_id: u64 = place["orderId"].as_str().unwrap().parse().unwrap();
    assert_eq!(place["status"], "ACCEPTED");

    // Amend
    let amend = client
        .post(format!("{base}/api/v1/order/amend"))
        .header("X-User-Id", "42")
        .json(&serde_json::json!({
            "orderId": order_id, "symbol":"BTCUSDT", "newPrice":"59500"
        }))
        .send()
        .await
        .unwrap();
    assert!(amend.status().is_success(), "amend status {}", amend.status());

    // Cancel
    let cancel = client
        .post(format!("{base}/api/v1/order/cancel"))
        .header("X-User-Id", "42")
        .json(&serde_json::json!({"orderId": order_id, "symbol":"BTCUSDT"}))
        .send()
        .await
        .unwrap();
    assert!(cancel.status().is_success(), "cancel status {}", cancel.status());

    // Give the matching thread a moment to drain.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Shutdown then inspect WAL.
    handle.shutdown().await.unwrap();
    let events = read_events(&wal_dir);
    assert!(!events.is_empty(), "WAL should contain events");
    // At minimum we expect OrderAccepted from the place call.
    assert!(
        events.iter().any(|e| matches!(e, Event::OrderAccepted { .. })),
        "expected at least one OrderAccepted, got: {events:?}"
    );
}

#[actix_web::test]
async fn missing_x_user_id_returns_401() {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let (handle, base) = boot_server(cfg).await;
    let client = Client::new();

    let resp = client
        .post(format!("{base}/api/v1/order"))
        .json(&serde_json::json!({
            "symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT",
            "timeInForce":"GTC","quantity":"0.001","price":"59000"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 401);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["code"], -2014);

    handle.shutdown().await.unwrap();
}

#[actix_web::test]
async fn malformed_json_returns_400() {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let (handle, base) = boot_server(cfg).await;
    let client = Client::new();

    let resp = client
        .post(format!("{base}/api/v1/order"))
        .header("X-User-Id", "42")
        .header("Content-Type", "application/json")
        .body("not json")
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_client_error(), "got {}", resp.status());

    handle.shutdown().await.unwrap();
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p exg-server --test stage0_e2e`
Expected: 3 tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/exg-server/tests/stage0_e2e.rs
git commit -m "test(server): add stage 0 e2e happy + error path coverage"
```

---

## Task 12 — `exg-server` e2e: backpressure + IDOR + shutdown ordering + dup client_order_id

**Files:**
- Append to: `crates/exg-server/tests/stage0_e2e.rs`

- [ ] **Step 1: Append the four scenario tests**

```rust
#[actix_web::test]
async fn backpressure_returns_503() {
    let tmp = TempDir::new().unwrap();
    let mut cfg = base_cfg(tmp.path());
    cfg.ringbuffer.slot_count = 4; // force tiny buffer
    let (handle, base) = boot_server(cfg).await;
    let client = Client::new();

    // Fire many concurrent requests, expect at least one 503.
    let mut joinset = tokio::task::JoinSet::new();
    for _ in 0..32 {
        let c = client.clone();
        let url = format!("{base}/api/v1/order");
        joinset.spawn(async move {
            c.post(url)
                .header("X-User-Id", "42")
                .json(&serde_json::json!({
                    "symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT",
                    "timeInForce":"GTC","quantity":"0.001","price":"59000"
                }))
                .send()
                .await
                .map(|r| r.status().as_u16())
                .unwrap_or(0)
        });
    }
    let mut saw_503 = false;
    while let Some(res) = joinset.join_next().await {
        if res.unwrap_or(0) == 503 {
            saw_503 = true;
        }
    }
    assert!(saw_503, "expected at least one 503 under backpressure");
    handle.shutdown().await.unwrap();
}

#[actix_web::test]
async fn idor_cancel_with_wrong_user_id_is_rejected() {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let wal_dir = std::path::PathBuf::from(&cfg.wal.dir);
    let (handle, base) = boot_server(cfg).await;
    let client = Client::new();

    // Place as user 42.
    let place: serde_json::Value = client
        .post(format!("{base}/api/v1/order"))
        .header("X-User-Id", "42")
        .json(&serde_json::json!({
            "symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT",
            "timeInForce":"GTC","quantity":"0.001","price":"59000"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let order_id: u64 = place["orderId"].as_str().unwrap().parse().unwrap();

    // Cancel as user 999 — must not actually cancel the order.
    let _ = client
        .post(format!("{base}/api/v1/order/cancel"))
        .header("X-User-Id", "999")
        .json(&serde_json::json!({"orderId": order_id, "symbol":"BTCUSDT"}))
        .send()
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(200)).await;
    handle.shutdown().await.unwrap();

    let events = read_events(&wal_dir);
    // Expect at least an OrderAccepted and an OrderRejected with reason OrderNotFound.
    let has_rejected = events.iter().any(|e| {
        matches!(
            e,
            Event::OrderRejected {
                reason: exg_protocol::RejectReason::OrderNotFound,
                ..
            }
        )
    });
    assert!(has_rejected, "expected OrderRejected/OrderNotFound, got: {events:?}");

    // The original order must not be in a canceled state.
    let canceled_for_original = events.iter().any(|e| matches!(
        e,
        Event::OrderCanceled { order_id: oid, .. } if oid.get() == order_id
    ));
    assert!(!canceled_for_original, "user 999 must not cancel user 42's order");
}

#[actix_web::test]
async fn shutdown_drains_pending_commands() {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let wal_dir = std::path::PathBuf::from(&cfg.wal.dir);
    let (handle, base) = boot_server(cfg).await;
    let client = Client::new();

    // Fire 20 concurrent orders; all that get 200 must end up in WAL.
    let mut joinset = tokio::task::JoinSet::new();
    for _ in 0..20 {
        let c = client.clone();
        let url = format!("{base}/api/v1/order");
        joinset.spawn(async move {
            c.post(url)
                .header("X-User-Id", "42")
                .json(&serde_json::json!({
                    "symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT",
                    "timeInForce":"GTC","quantity":"0.001","price":"59000"
                }))
                .send()
                .await
                .map(|r| r.status().as_u16())
                .unwrap_or(0)
        });
    }
    let mut accepted = 0;
    while let Some(res) = joinset.join_next().await {
        if res.unwrap_or(0) == 200 {
            accepted += 1;
        }
    }

    // Immediately shut down.
    handle.shutdown().await.unwrap();

    let events = read_events(&wal_dir);
    let accepted_in_wal = events
        .iter()
        .filter(|e| matches!(e, Event::OrderAccepted { .. }))
        .count();
    assert_eq!(
        accepted_in_wal, accepted,
        "every 200 ACCEPTED response must produce an OrderAccepted event in WAL"
    );
}

#[actix_web::test]
async fn duplicate_client_order_id_creates_two_orders() {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let wal_dir = std::path::PathBuf::from(&cfg.wal.dir);
    let (handle, base) = boot_server(cfg).await;
    let client = Client::new();

    let body = serde_json::json!({
        "symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT",
        "timeInForce":"GTC","quantity":"0.001","price":"59000",
        "clientOrderId": 12345
    });

    let r1: serde_json::Value = client
        .post(format!("{base}/api/v1/order"))
        .header("X-User-Id", "42")
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let r2: serde_json::Value = client
        .post(format!("{base}/api/v1/order"))
        .header("X-User-Id", "42")
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(r1["status"], "ACCEPTED");
    assert_eq!(r2["status"], "ACCEPTED");
    assert_ne!(
        r1["orderId"], r2["orderId"],
        "stage 0 must NOT deduplicate client_order_id (spec §9 invariant 9)"
    );

    tokio::time::sleep(Duration::from_millis(200)).await;
    handle.shutdown().await.unwrap();

    let events = read_events(&wal_dir);
    let accepted = events
        .iter()
        .filter(|e| matches!(e, Event::OrderAccepted { .. }))
        .count();
    assert!(accepted >= 2, "expected ≥ 2 OrderAccepted, got {accepted}");
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p exg-server --test stage0_e2e`
Expected: 7 tests total pass (3 from Task 11 + 4 here).

- [ ] **Step 3: Commit**

```bash
git add crates/exg-server/tests/stage0_e2e.rs
git commit -m "test(server): add backpressure, IDOR, shutdown ordering, dup coid"
```

---

## Task 13 — `scripts/demo-stage0.sh`

**Files:**
- Create: `scripts/demo-stage0.sh`

- [ ] **Step 1: Write the script**

```bash
#!/usr/bin/env bash
# Stage 0 cold-boot demo. Spec §5.3.
set -euo pipefail

WAL_DIR=$(mktemp -d /tmp/exg-stage0.XXXXXX)
PORT=8080
SERVER_PID=""

cleanup() {
    if [[ -n "${SERVER_PID}" ]]; then
        kill -TERM "${SERVER_PID}" 2>/dev/null || true
        wait "${SERVER_PID}" 2>/dev/null || true
    fi
    rm -rf "${WAL_DIR}"
}
trap cleanup EXIT

echo "── stage 0 demo ──"
echo "WAL dir: ${WAL_DIR}"
echo "Building release binaries..."
cargo build --release -p exg-server -p exg-wal-dump >/dev/null

echo "Starting exg-server..."
EXG_CONFIG=config/default.toml \
    EXG_WAL_DIR="${WAL_DIR}" \
    RUST_LOG=info \
    ./target/release/exg-server &
SERVER_PID=$!

# Wait up to 30s for health.
for i in {1..30}; do
    if curl -sf "http://127.0.0.1:${PORT}/api/v1/health" >/dev/null; then
        echo "server ready"
        break
    fi
    sleep 1
done

echo
echo "── place LIMIT buy ──"
RESP=$(curl -s -X POST "http://127.0.0.1:${PORT}/api/v1/order" \
    -H 'X-User-Id: 42' \
    -H 'Content-Type: application/json' \
    -d '{"symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT","timeInForce":"GTC","quantity":"0.001","price":"59000"}')
echo "${RESP}"
ORDER_ID=$(echo "${RESP}" | python3 -c 'import json,sys; print(json.load(sys.stdin)["orderId"])')

echo
echo "── amend order ${ORDER_ID} ──"
curl -s -X POST "http://127.0.0.1:${PORT}/api/v1/order/amend" \
    -H 'X-User-Id: 42' \
    -H 'Content-Type: application/json' \
    -d "{\"orderId\":${ORDER_ID},\"symbol\":\"BTCUSDT\",\"newPrice\":\"59500\"}"
echo

echo
echo "── cancel order ${ORDER_ID} ──"
curl -s -X POST "http://127.0.0.1:${PORT}/api/v1/order/cancel" \
    -H 'X-User-Id: 42' \
    -H 'Content-Type: application/json' \
    -d "{\"orderId\":${ORDER_ID},\"symbol\":\"BTCUSDT\"}"
echo

echo
echo "── shutting down ──"
kill -TERM "${SERVER_PID}"
wait "${SERVER_PID}" 2>/dev/null || true
SERVER_PID=""

echo
echo "── WAL contents ──"
./target/release/exg-wal-dump --wal-dir "${WAL_DIR}"
echo
echo "── demo complete ──"
```

- [ ] **Step 2: Make executable**

```bash
chmod +x scripts/demo-stage0.sh
```

- [ ] **Step 3: Note `EXG_WAL_DIR` env override**

The script uses `EXG_WAL_DIR` env var. `exg-config` already supports `EXG_{SECTION}_{KEY}` env override via the `config` crate, so `EXG_WAL_DIR` overrides `cfg.wal.dir`. Verify with `grep -n "with_prefix" crates/exg-config/src/lib.rs`. If the env prefix path differs, adjust the script accordingly.

- [ ] **Step 4: Run the demo end-to-end**

```bash
scripts/demo-stage0.sh
```
Expected: server starts → place/amend/cancel curls each return JSON → SIGTERM cleans up → `exg-wal-dump` prints at least 2 events (OrderAccepted + OrderCanceled, possibly more depending on amend semantics).

- [ ] **Step 5: Commit**

```bash
git add scripts/demo-stage0.sh
git commit -m "feat(scripts): add stage 0 cold-boot demo script"
```

---

## Task 14 — Final acceptance: verify spec §8.4 checklist

This task has no code changes — it executes the spec's acceptance checklist and confirms each item is green. If any item fails, file a bug fix as a sub-task, not a workaround.

- [ ] **Step 1: `cargo check --workspace`**

Run: `cargo check --workspace`
Expected: clean, 0 warnings.

- [ ] **Step 2: `cargo clippy --workspace -- -D warnings`**

Run: `cargo clippy --workspace -- -D warnings`
Expected: clean. Fix any clippy issues before continuing.

- [ ] **Step 3: `cargo fmt --check`**

Run: `cargo fmt --check`
Expected: clean. Run `cargo fmt` if needed and commit as `style: cargo fmt`.

- [ ] **Step 4: `cargo test --workspace`**

Run: `cargo test --workspace`
Expected: all green. Original 364 tests + new tests from Tasks 2-12 ≈ 400+ total.

- [ ] **Step 5: Run the demo script**

Run: `scripts/demo-stage0.sh`
Expected:
- Server starts and binds 127.0.0.1:8080
- All three curls return `200 ACCEPTED` JSON
- Process exits cleanly within ≤ 2 s of `SIGTERM`
- `exg-wal-dump` prints at least one `OrderAccepted` and one `OrderCanceled` event

- [ ] **Step 6: Verify the host-binding panic**

Run: `EXG_SERVER_HOST=0.0.0.0 cargo run --release -p exg-server`
Expected: process exits non-zero with stderr mentioning "loopback".

- [ ] **Step 7: Verify the WAL-not-empty panic**

```bash
mkdir -p /tmp/stale-wal
echo "junk" > /tmp/stale-wal/sentinel
EXG_WAL_DIR=/tmp/stale-wal cargo run --release -p exg-server
```
Expected: process exits non-zero with stderr mentioning "WAL dir" and "not empty". Clean up: `rm -rf /tmp/stale-wal`.

- [ ] **Step 8: Push branch (do NOT open PR automatically)**

```bash
git push -u origin feat/stage0-skeleton
```

- [ ] **Step 9: Hand off**

State to the user: **"Stage 0 implementation complete. Branch `feat/stage0-skeleton` pushed. All spec §8.4 acceptance items green. Ready for PR + /plan-design-review (skip if no UI scope) + Stage 1 brainstorming whenever you are."**

---

## Cross-Task Notes

### Risk-engine `SymbolConfig` field names

Task 8 (`symbol_entry_to_risk_config`) assumes field names that may not exactly match `exg-risk-engine::SymbolConfig`. Before starting Task 8, run:

```bash
grep -n "pub struct SymbolConfig\|pub struct MarginTier" crates/exg-risk-engine/src/**/*.rs
```

and adjust the mapping function accordingly. If a field expected by `MatchingEngine::new` is missing in `SymbolConfigEntry`, raise it as a Stage 0 blocker — do not introduce defaults silently.

### `ApiError` constructors

Tasks 6 and 7 reference `ApiError::unauthorized` and `ApiError::internal`. The existing module has `bad_request` and `rate_limited` (per the test on line 311). Before Task 6, confirm:

```bash
grep -n "pub fn .*request\|pub fn .*limited\|pub fn unauthorized\|pub fn internal" crates/exg-api-gateway/src/error.rs
```

If `unauthorized` / `internal` are missing, add 1-line constructors mirroring the existing pattern; this is part of Task 6.

### `Producer::try_push` error variants

Task 7 (`enqueue`) maps the `RingBufferError` enum to ApiError variants by string-match. Before Task 7, read `crates/exg-ringbuffer/src/error.rs` and use exact enum match arms instead. Stringy matching is fragile; this is technical debt the moment it lands. Adjust to:

```rust
producer.try_push(&bytes).map_err(|e| match e {
    RingBufferError::Full => ApiError::rate_limited(),
    RingBufferError::MessageTooLarge => ApiError::bad_request("command too large for ring slot"),
    other => ApiError::internal(format!("ring buffer push: {other}")),
})?;
```

### `core_affinity` API

`core_affinity::set_for_current(core)` returns `bool` in recent versions but may return `()` in older. Check `cargo doc -p core_affinity` or the crate source under `~/.cargo/registry`. Adjust the warn-on-fail branch accordingly.

### Time source

`UnixMicros::from_micros(SystemTime → u64)` ignores time-before-epoch. For stage 0 this is fine; flag for Stage 1 if a monotonic source is needed for replay determinism.

---

## Spec ↔ Plan Coverage Matrix

| Spec requirement | Task |
|---|---|
| §3 Non-goals (8 items) | enforced by omission |
| §4.1 Threading model | Task 8 (`spawn_matching_thread`) |
| §4.2 WAL strategy | Task 8 + 9 |
| §4.3 Ring buffer Mutex | Task 7 (`AppState`) |
| §4.4 Mark price & risk | Task 2 + Task 4 + Task 8 |
| §4.5 Startup sequence (incl. host/symbol/WAL asserts) | Task 8 + Task 10 |
| §4.6 Shutdown 5 steps | Task 8 (`ServerHandle::shutdown`) + Task 9 (`main`) + Task 12 (`shutdown_drains_pending_commands`) |
| §5 Component changes | Tasks 1–9 |
| §6 Data flow contract | Tasks 7, 11, 12 |
| §7.1 Binance error map | Task 6, 7 |
| §7.2 Panic conditions | Task 8 (matching loop), Task 10 (boot panics) |
| §7.3 Tracing instrumentation | Task 7 (handlers `info!`/`warn!`), Task 8 (matching `debug!`/`error!`) |
| §8.1 Unit tests | Tasks 2, 3, 4, 5, 7 |
| §8.2 Integration tests | Task 11 + Task 12 |
| §8.3 Demo script | Task 13 |
| §8.4 Acceptance checklist | Task 14 |
| §9 Invariants (10 items) | enforced across Tasks 7-12; tested in Task 10 + 12 |
| §11 Forward pointers | not implemented (Stage 1+) |

---

## Worktree Parallelization

Per the spec eng-review parallelization plan, Tasks 1-5 are independent and can be implemented in parallel worktrees:

```
Lane A (independent):  Task 1 (workspace)  →  Task 2 (config)
Lane B (independent):  Task 3 (protocol slot test)
Lane C (independent):  Task 4 (matching set_mark_price)
Lane D (depends on B,C — uses workspace member):  Task 5 (wal-dump)

After merge:
Lane E (depends on A): Task 6 → Task 7 (api-gateway)
Lane F (depends on E + C): Task 8 → Task 9 (server)
Lane G (depends on F): Task 10, 11, 12, 13 in sequence
Lane H: Task 14 (final acceptance)
```

Conflict: Lane A and Lane D both touch root `Cargo.toml` (Task 1 adds `exg-wal-dump` to members; Task 6 may need to update workspace deps if `actix-web` isn't already there). Resolve by merging Lane A first.

If executing sequentially in one session, just go in numeric order (1 → 14).
