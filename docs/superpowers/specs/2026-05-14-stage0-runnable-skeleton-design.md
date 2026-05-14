# Stage 0: Runnable Skeleton — Design

**Date**: 2026-05-14
**Status**: Approved (pending final user spec review before plan stage)
**Predecessor**: [`2026-03-30-perpetual-exchange-design.md`](./2026-03-30-perpetual-exchange-design.md)

## 1. Background

The repository currently has 16 Rust crates implementing the perpetual exchange (matching, risk, ledger, WAL, ring buffer, API gateway, services). 364 unit tests pass. However, the binary entry point `exg-server` is only ~66 lines that initializes the Prometheus exporter and does nothing else. None of the crates are wired together.

The user has chosen "production-ready, gateable" as the final delivery target. That delivery is decomposed into 8 sequential stages (0 → 7); each stage gets its own brainstorming → spec → plan → implementation cycle. **This document defines Stage 0 only.**

Subsequent stages (1 = persistence + auth, 2 = trading loop + WebSocket, 3 = wallet + invariant audit, 4 = trading frontend, 5 = admin backend + frontend, 6 = on-chain integration, 7 = operational hardening) will each produce their own spec.

## 2. Goal

Wire the command hot path end-to-end:

```
HTTP request  →  Actix worker  →  Ring Buffer  →  Matching Thread  →  WAL
```

A user can `curl POST /api/v1/order`, `POST /api/v1/order/cancel`, and `POST /api/v1/order/amend` against a running `exg-server` process and observe the resulting events on disk via a new `exg-wal-dump` binary. No frontend, no DB, no WebSocket, no downstream service tasks, no authentication.

## 3. Non-Goals (Deferred to Later Stages)

| Item | Stage |
|---|---|
| JWT / API key authentication | 1 |
| PostgreSQL via sqlx, migrations | 1 |
| Redis rate limiting backed by external store | 1-2 |
| NATS event fan-out | 2 |
| WebSocket push (depth/trade/kline/user@order) | 2 |
| Query endpoints (depth, positions, accounts, history) | 2 |
| Downstream service tokio tasks (clearing, market-data, order-service) | 2 |
| Real-time double-entry invariant audit | 3 |
| Deposit / withdrawal | 3 |
| On-chain alloy EVM integration | 6 |
| WAL snapshot strategy / restart recovery | 1 (folded into persistence stage) |
| Prometheus business metrics / Grafana dashboards | 7 |
| Docker image / k8s manifests | 7 |

## 4. Architecture

### 4.1 Threading Model

- **Main thread (Tokio multi-thread runtime)**: orchestrates startup, runs Actix HTTP server, awaits `ctrl_c`.
- **Matching OS thread**: dedicated, optionally CPU-pinned via `core_affinity` (warn-and-continue on macOS). Single writer. Hot loop:
  - `consumer.try_pop(&mut buf)` from input ring buffer
  - `rkyv::from_bytes::<Command>(&buf[..n])`
  - `engine.process_command(&cmd)` returning `Vec<Event>`
  - For each event: `rkyv::to_bytes(&evt)` → `wal.append(&bytes)`
  - On `try_pop` empty: `std::hint::spin_loop()` (Stage 7 will revisit for power efficiency)
- **No additional OS threads** are spawned in Stage 0. The output ring buffer for downstream consumers is **not** allocated; that's Stage 2.

### 4.2 WAL Strategy

- WAL append happens on the matching thread, **inline** with command processing.
- `WalWriter` internally batches `fsync`: triggers when **either** `flush_every_n` events accumulate **or** `flush_interval_us` microseconds elapse.
- Stage 0 uses the existing `exg-config` defaults: `flush_every_n=1000`, `flush_interval_us=1000` (1ms).
- On graceful shutdown the matching thread calls `wal.flush()` one last time before returning.
- **WAL write failure → `panic!()` → process exit.** No retry, no fallback. This is required by CLAUDE.md §8.2 (anti-fallback) and §2 (capital safety priority).

### 4.3 Ring Buffer Sharing

The input ring buffer is SPSC (`exg-ringbuffer`). Multiple Actix workers will hand off commands concurrently, which violates the SPSC single-producer constraint. Stage 0 resolves this by wrapping `Producer` in `Arc<Mutex<Producer>>` and serializing HTTP-side pushes through the lock. This trades throughput for safety; Stage 7 will revisit (options: pin Actix to `workers(1)`, or introduce a multi-producer adapter).

The consumer is moved (not shared) into the matching thread closure — there is exactly one consumer.

### 4.4 Mark Price & Pre-Trade Risk

- Matching engine's `MatchingEngine::new(SymbolConfig, node_id)` initializes `mark_price = Decimal128::ZERO`. We add a `set_mark_price(Decimal128)` setter (or extend the constructor) and call it after construction with the value loaded from `[[trading.symbols]].mark_price` in config.
- The full `risk-engine` pre-trade check stays in the loop. **No `unsafe_skip_risk_check` feature flag.** Stage 2 will replace the static mark price with a real feed; the engine's interface does not change.

### 4.5 Startup Sequence (in `exg-server::main`)

1. `tracing_subscriber::fmt().with_env_filter(RUST_LOG||"info").json().init()`
2. `ExgConfig::load("config/default.toml")` → `validate()` → panic on failure
3. **Host-binding invariant**: assert `cfg.server.host ∈ {"127.0.0.1", "::1", "localhost"}`. Stage 0 has no auth — binding to a public interface would let any attacker forge `X-User-Id`. Violation panics at startup with a clear message. This assert is removed in Stage 1 once JWT middleware lands.
3a. **Symbol whitelist invariant**: assert `cfg.trading.symbols.len() == 1`. Stage 0's `MatchingEngine` is single-symbol; silently dropping additional config entries is a footgun. Violation panics. Stage 2 (multi-symbol routing) removes this assert.
3b. **WAL freshness invariant**: assert the WAL dir is empty (or does not exist). If it contains prior segments, `WalWriter::open` would append-continue the sequence, splicing fresh in-memory state onto an old event timeline silently. Stage 0 has no replay, so this would produce a Frankenstein WAL that confuses Stage 1's future replay. Violation panics with a message instructing the operator to clear the WAL dir. Stage 1 (snapshot + replay) replaces this assert with a real recovery path.
4. Build `exg_wal::WalConfig` from `cfg.wal` (convert `segment_size_mb` → bytes)
5. `WalWriter::open(wal_cfg)` → `Arc<Mutex<WalWriter>>`
6. `RingBuffer::new(slot_count, slot_size)` → `split()` → `(Producer, Consumer)`
7. Convert `cfg.trading.symbols[0]` → `exg_risk_engine::SymbolConfig` (a new conversion function in `exg-api-gateway` or `exg-config`; spec leaves that placement to the plan stage)
8. `MatchingEngine::new(symbol_cfg, cfg.server.node_id)` + `set_mark_price(...)`
9. `SnowflakeGen::new(cfg.server.node_id)` → `Arc<SnowflakeGen>`
10. `shutdown_flag = Arc<AtomicBool>::new(false)`
11. Spawn matching OS thread (move: `engine`, `consumer`, `wal`, `shutdown_flag`)
12. Install Prometheus exporter on `:9000` (preserve existing behavior)
13. Build `AppState { producer: Arc<Mutex<Producer>>, snowflake: Arc<SnowflakeGen>, cfg: Arc<ExgConfig> }`
14. Start Actix: `let server = actix_web::HttpServer::new(move || app_factory::build_app(state.clone())).bind((cfg.server.host, cfg.server.port))?.run();` — keep the `Server` handle.
15. `tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = (&mut server) => {} }` — await either ctrl_c or server crash.

### 4.6 Shutdown Sequence (deterministic, **HTTP 200 = enqueued** must remain truthful)

Order is load-bearing. If reversed, a HTTP worker may push a command into the ring buffer *after* the matching thread has exited, leaving the response `200 ACCEPTED` but no corresponding WAL event. That breaks the §6.1 semantic contract.

1. `tokio::signal::ctrl_c().await` (or server crash)
2. **`server.handle().stop(true).await`** — Actix graceful stop: rejects new connections, awaits in-flight handlers to return. After this point no new `producer.lock()` calls happen.
3. `shutdown_flag.store(true, Ordering::Release)` — signals the matching loop's exit condition.
4. `matching_thread.join().expect("matching thread panicked")` — matching loop drains any commands still in the ring buffer (since step 2 already stopped HTTP, only already-enqueued commands remain), then exits.
5. Inside matching thread's final iteration: `wal.lock().flush()?` — guarantees all WAL data is fsync'd before process exit.

Invariant: after step 5, `WalReader::open` over the WAL dir must read back every event corresponding to every `200 ACCEPTED` response that the HTTP server returned during this process's lifetime. Verified by `stage0_e2e.rs` and the demo script.

## 5. Component Changes

### 5.1 No-Touch Crates

`exg-common`, `exg-protocol`, `exg-ringbuffer`, `exg-wal`, `exg-risk-engine`, `exg-matching-engine` need **zero source changes** for Stage 0. `MatchingEngine` may grow a `set_mark_price` setter (one line) if it doesn't already accept mark price post-construction — confirmed at plan stage.

### 5.2 Modified

**`exg-config`** — `crates/exg-config/src/lib.rs`

- `SymbolConfigEntry` gains `pub mark_price: String` (decimal-as-string).
- `default_btcusdt()` adds `mark_price: "60000".into()`.
- `validation::validate` adds: each symbol's `mark_price` parses as `Decimal128` and is > 0.
- Tests in `crates/exg-config/src/tests.rs` add cases for the new field.

**`config/default.toml`**

- `[[trading.symbols]]` gains `mark_price = "60000"`.

**`exg-api-gateway`** — `crates/exg-api-gateway/`

New modules:
- `state.rs`: `pub struct AppState { producer: Arc<Mutex<Producer>>, snowflake: Arc<SnowflakeGen>, cfg: Arc<ExgConfig> }`
- `handlers.rs`: `health` (GET), `place_order` (POST), `cancel_order` (POST), `amend_order` (POST). Each handler: extract `X-User-Id` → call existing `conversion::to_*_command` → `rkyv::to_bytes` → `producer.lock().try_push` → JSON response.
- `app_factory.rs`: `pub fn build_app(state: AppState) -> App<...>` mounts `/api/v1/health`, `/api/v1/order`, `/api/v1/order/cancel`, `/api/v1/order/amend`.

`conversion.rs` extensions if missing:
- `to_cancel_order_command(req, user_id, ts) -> Result<Command, ApiError>`
- `to_amend_order_command(req, user_id, ts) -> Result<Command, ApiError>`

`Cargo.toml`: depend on `actix-web`, `exg-config`, `exg-ringbuffer`, `exg-wal`, `parking_lot`.

**`exg-server`** — `crates/exg-server/src/main.rs`

Rewrite per §4.5. Replace the entire `main()` body (preserving Prometheus init and tracing setup).

`Cargo.toml`: add workspace deps `exg-config`, `exg-protocol`, `exg-common`, `exg-ringbuffer`, `exg-wal`, `exg-matching-engine`, `exg-risk-engine`, `exg-api-gateway`, `parking_lot`, `core_affinity`, `actix-web`.

### 5.3 New

**`crates/exg-wal-dump/`** (new workspace member)

- `Cargo.toml`: depends on `exg-wal`, `exg-protocol`, `rkyv`, `serde_json`, `clap` (or std arg parse).
- `src/main.rs`: args `--wal-dir <path>`, optional `--from-seq <u64>`. Opens `WalReader`, iterates `read_from(start_seq, |seq, bytes| { ... })`, for each record: `rkyv::from_bytes::<Event>(bytes)` → `serde_json::to_string(&event)` → print with sequence prefix `{seq}\t{json}`.
- CRC failure → returns the wrapped `WalError`, non-zero exit.
- Unit test: builds a temp WAL with 3 rkyv-encoded events using `WalWriter`, runs the dump logic (extract to a library helper for testability), asserts 3 JSON lines with correct fields.

**Workspace root** — `Cargo.toml`

- Add `crates/exg-wal-dump` to `[workspace] members`.
- Add `exg-wal-dump = { path = "crates/exg-wal-dump" }` to `[workspace.dependencies]` (consistency with other workspace crates).

**Integration test** — `crates/exg-server/tests/stage0_e2e.rs`

A single `#[tokio::test]` (or `actix_web::rt::test`) that:
1. Creates a `TempDir` for WAL.
2. Builds `ExgConfig::default_config()` with overrides for `wal.dir` and a `:0` ephemeral port.
3. Boots the server in-process (extract a `run_with_config(cfg) -> Handle` helper from `main`).
4. `reqwest` calls each endpoint, asserting status codes and response shapes.
5. Sends shutdown, joins matching thread.
6. Opens `WalReader` on the temp dir, walks events, asserts the sequence.

**Demo script** — `scripts/demo-stage0.sh`

Bash script per §5 of the brainstorm; uses `mktemp -d` for WAL, starts `cargo run -p exg-server`, polls health endpoint, runs three `curl` commands, kills the server with SIGTERM, then runs `cargo run -p exg-wal-dump --` to print events.

## 6. Data Flow Contract

### 6.1 Request semantics

`HTTP 200` on a command endpoint means **the command bytes were enqueued into the input ring buffer**. It does **not** mean the order was accepted, matched, or rejected. The authoritative outcome lives in WAL events, retrievable via `exg-wal-dump` (Stage 0) or WebSocket (Stage 2).

This is intentional — LMAX-style asynchronous command acknowledgement. Stage 2 adds a real fill-by-fill response channel; Stage 0 deliberately does not paper over the gap with a synthetic poll loop.

### 6.2 ID assignment

- `user_id`: parsed from `X-User-Id` request header. Stage 1 replaces this with JWT claims; the contract `(user_id: UserId) -> Command` does not change.
- `order_id`: server-side Snowflake (`SnowflakeGen::next_id`).
- `client_order_id`: passed through if present, `None` otherwise.

### 6.3 Response shape

Success (place):
```json
{ "orderId": "<u64-as-string>", "clientOrderId": <opt>, "status": "ACCEPTED" }
```

Success (cancel / amend):
```json
{ "orderId": "<u64-as-string>", "status": "ACCEPTED" }
```

(Per Binance convention, integer IDs are serialized as strings to avoid JS 53-bit precision loss.)

Error: `{ "code": <i32-binance-code>, "msg": "<string>" }`.

## 7. Error Handling

### 7.1 HTTP → ApiError map

| Trigger | HTTP | code | msg |
|---|---|---|---|
| Missing/non-numeric `X-User-Id` | 401 | -1002 | `missing or invalid X-User-Id header` |
| JSON body parse error | 400 | -1100 | `illegal request body: <serde err>` |
| `to_*_command` validation (missing price for LIMIT, unknown symbol, …) | 400 | -1100 | `<field>: <reason>` |
| Ring buffer full (`Producer::try_push` Err) | 429 | -1015 | `Too many requests` |
| Command bytes exceed ring slot size | 400 | -1100 | `command too large for ring slot` |
| rkyv encode failure (should not happen in practice) | 500 | -1000 | `internal serialization error` |

All errors serialized as `{"code": <i32>, "msg": "<string>"}` via `exg_api_gateway::ApiError`.

### 7.2 Panic conditions (process exits via `panic=abort`)

Allowed:
- Startup: config load/validate, WAL open, RingBuffer init, port bind failures.
- Matching loop: WAL append/flush IO error; rkyv decode of command bytes failure; rkyv encode of event failure.

Not allowed (would constitute reverse-CLAUDE.md fallback):
- WAL retry / write to alternative location.
- Respawning the matching thread.
- Sleeping/retrying on ring buffer full (returns 429 instead).
- `let _ = result;` patterns that drop errors silently.

`core_affinity::set_for_current` failure on macOS is **warned and continued** — it is a platform limitation, not a correctness issue.

### 7.3 Tracing Instrumentation (minimum)

Production-grade instrumentation lands in Stage 7, but Stage 0 still needs enough to debug a local boot. Required `tracing` calls:

- HTTP handlers: `tracing::info!(target: "handler", method=..., path=..., user_id=...)` on entry; `tracing::info!(..., status=..., latency_us=...)` on exit. One span per request.
- Conversion failures (in `to_*_command`): `tracing::warn!(target: "conversion", reason=...)` before returning `ApiError`.
- Matching loop: `tracing::debug!(target: "matching", cmd_type=..., seq=...)` for each command processed (debug-level so it's off by default).
- Pre-panic logs: before `wal.append` / `wal.flush` panics, log `tracing::error!(target: "wal", err=..., sequence=...)` so the cause is captured even though the process is about to abort.
- `core_affinity` failure: `tracing::warn!(target: "matching", err=..., "cpu pin failed, continuing")`.

`RUST_LOG=info` is the default boot level (already wired in `main.rs`); `RUST_LOG=debug,handler=info` is the standard local-debug recipe.

## 8. Test Plan

### 8.1 Unit tests (added)

| Location | Cases |
|---|---|
| `exg-config/src/tests.rs` | `mark_price` parses; missing field → validation error; non-positive → error |
| `exg-api-gateway/src/conversion.rs` | `to_cancel_order_command` happy / missing order_id; `to_amend_order_command` happy / both new fields empty |
| `exg-api-gateway/src/handlers.rs` | `actix_web::test::call_service` against each handler; verify 200/400/401/429 status and body shape |
| `exg-api-gateway/src/state.rs` | `AppState::clone` shares the same `Producer` lock and snowflake |
| `exg-wal-dump` | given a temp WAL with 3 known events, the dump helper outputs 3 JSON lines with correct field values |
| `exg-protocol` (new property test) | for every `Command` variant constructed at maximal-field size (e.g. NewOrder with Iceberg + StopLimit + leverage + client_order_id), `rkyv::to_bytes(&cmd).len() <= 4096`. Catches the hidden coupling between protocol field growth and `cfg.ringbuffer.slot_size`. |
| `exg-api-gateway::handlers` | `X-User-Id: abc` (non-numeric) → 401 code=-1002 (covers the parse-fail branch separately from missing-header). |
| `exg-wal-dump` (extended) | (a) corrupting one byte mid-record → dump tool exits non-zero, stderr mentions corruption. (b) empty WAL dir → exit 0, zero output lines. (c) `--from-seq 5` over an 8-event WAL → exactly 3 output lines starting at sequence 5. |
| `exg-server` boot-panic suite (§9 invariants 1-4 guards) | Four `#[test]`s that spawn `cargo run -p exg-server` (or call an extracted `run_with_config(cfg)` library function) with specific malformed configs and assert panic/exit:<br>(a) `EXG_SERVER_HOST=0.0.0.0` → panic mentioning host invariant<br>(b) WAL dir pre-populated with a segment → panic mentioning WAL freshness<br>(c) `cfg.trading.symbols.len() == 2` → panic mentioning symbol whitelist<br>(d) `cfg.trading.symbols[0].mark_price = "-1"` → `ExgConfig::validate` returns error, `main` panics with the validation message.<br>Implementation note: extract `pub fn run_with_config(cfg) -> Handle` from `main` so tests can call it directly without spawning a subprocess; subprocess form is acceptable but `cargo build --bin exg-server` becomes a test prerequisite. |

### 8.2 Integration test

`crates/exg-server/tests/stage0_e2e.rs` — see §5.3. Additional scenarios this test must cover:

- **Backpressure**: spin up the server with a small `cfg.ringbuffer.slot_count` (test uses 2) and fire many concurrent `POST /order` requests; at least one must respond `429` with `code = -1015`.
- **IDOR**: place an order as `X-User-Id: 42`, then send `POST /order/cancel` for that `order_id` with `X-User-Id: 999`. The cancel should produce an `OrderRejected { OrderNotFound }` event in WAL; the original order must remain (verifiable by a subsequent legitimate cancel that succeeds).
- **Shutdown ordering**: fire N concurrent `POST /order`, then `SIGTERM` mid-flight; assert WAL contains exactly the events for the requests that received `200 ACCEPTED` (no fewer, no more) — see §4.6.
- **Duplicate `client_order_id` is accepted twice**: fire two `POST /order` with identical `client_order_id`, assert both responses `200 ACCEPTED` with distinct `orderId`s, and both `OrderAccepted` events land in WAL. Guards invariant §9 #9 against future silent dedup additions.

### 8.3 Demo script

`scripts/demo-stage0.sh` — runnable from cold workspace.

### 8.4 Acceptance Checklist

Stage 0 is complete iff **all** of the following pass:

- [ ] `cargo check --workspace` — 0 warnings
- [ ] `cargo clippy --workspace -- -D warnings` — 0 warnings
- [ ] `cargo fmt --check` — clean
- [ ] `cargo test --workspace` — all green (existing 364 + Stage 0 additions)
- [ ] `crates/exg-server/tests/stage0_e2e.rs` — passes
- [ ] `scripts/demo-stage0.sh` — runs end-to-end from cold; the WAL dump on stdout contains at least one `OrderAccepted` event (from the place call) and at least one `OrderCanceled` event (from the cancel call). The amend call's footprint depends on whether the engine implements amend as in-place modification (no event) or as cancel-replace (`OrderCanceled` + `OrderAccepted`); both are acceptable, but the demo must observably differ before/after amend (e.g. distinct event sequence, or the post-amend cancel uses the same order_id).
- [ ] WAL directory after demo contains at least one segment file; `WalReader::open` succeeds
- [ ] `SIGTERM` to the server triggers a clean exit in ≤ 2 seconds with WAL flushed
- [ ] Negative cases by curl: `-d 'malformed'` → `-1100`; no `X-User-Id` header → `-1002`
- [ ] Boot-time host-binding assert: `EXG_SERVER_HOST=0.0.0.0 cargo run -p exg-server` panics at startup with a message naming Stage 0's no-auth policy
- [ ] Shutdown ordering: `stage0_e2e.rs` includes a scenario that sends N concurrent `POST /order` then `SIGTERM`s mid-flight; asserts the WAL contains exactly the events for the requests that received `200 ACCEPTED`

## 9. Invariants (must hold throughout implementation)

Drawn from `CLAUDE.md` and the original system spec — violations block merge regardless of test status:

1. The matching engine remains the single writer for orderbook and risk state. All mutations happen on the matching OS thread.
2. All financial values flow as `Decimal128`. No `f64` is introduced anywhere in the request path.
3. WAL is the source of truth. Any event handed to a downstream consumer (Stage 2+) must have already been WAL-acknowledged.
4. No fallback paths for WAL failure. Process exits.
5. Errors at API boundary use Binance-compatible codes. No new code outside that table.
6. No new `_` ignored errors or `try { } catch { }` swallowing.
7. `gen` is reserved in Rust 2024 — use `id_gen` / `sf` / `rng` for generator variable names.
8. **IDOR guard**: when `MatchingEngine` processes `CancelOrder` / `AmendOrder`, if the command's `user_id` does not match the order owner recorded in the orderbook, it must emit `OrderRejected { OrderNotFound }` and leave the order untouched. Stage 0 has no auth, so an attacker forging `X-User-Id` must not be able to cancel another user's resting orders even on a local host. (If the engine's current implementation already enforces this, the integration test below merely verifies it; if not, this is a bug to fix as part of Stage 0.)
9. **Duplicate `client_order_id` are NOT deduplicated** in Stage 0. Two POSTs with identical `client_order_id` create two distinct `order_id`s. Stage 1+ adds dedup at the auth/middleware layer once a per-user index exists. Clients in Stage 0 must not rely on `client_order_id` for idempotency.
10. **Two distinct sequence counters exist** — do not conflate them. `MatchingEngine.sequence` (engine internal, command count, increments per `process_command` call) is logically separate from the WAL byte-stream sequence (`WalWriter::current_sequence`, event write index). Most spec references to "sequence" mean the WAL sequence (the one `WalReader::read_from(seq, ...)` and `exg-wal-dump --from-seq <N>` use). When a future spec needs to reference engine-internal `sequence`, qualify it as "engine command sequence" explicitly.

## 10. Open Questions (resolved during plan stage, not blocking spec approval)

1. Whether `MatchingEngine` already has a public `set_mark_price` / where to place the symbol-config → `risk_engine::SymbolConfig` conversion helper.
2. Whether `exg-api-gateway::conversion` already has `to_cancel_order_command` and `to_amend_order_command` — if yes, reuse; if no, add.
3. Exact `actix_web::test` vs `reqwest`-against-bound-port style for the e2e test — implementation-detail, plan stage decides.

## 11. Forward Pointers (Stage 1+ obligations)

When Stage 1 lands, the following Stage 0 shortcuts must be replaced:
- `X-User-Id` header → JWT middleware injection (interface to handlers unchanged).
- In-memory matching state → persisted snapshots + WAL replay on restart.
- Static `mark_price` from config → fed by an oracle / mark price service.
- Boot-time host-binding assert (§4.5 step 3) is removed once JWT auth means non-loopback binding is safe.
- `client_order_id` deduplication (per §9 invariant 9) added at the auth/middleware layer when a per-user index becomes available.

These are explicit forward-pointers, not Stage 0 technical debt.

## GSTACK REVIEW REPORT

| Review | Trigger | Why | Runs | Status | Findings |
|--------|---------|-----|------|--------|----------|
| CEO Review | `/plan-ceo-review` | Scope & strategy | 1 | CLEAR (HOLD_SCOPE) | mode: HOLD_SCOPE, 0 critical gaps, 6 findings raised+fixed |
| Eng Review | `/plan-eng-review` | Architecture & tests (required) | 1 | CLEAR (FULL_REVIEW) | 9 issues raised, 0 critical gaps |
| Design Review | `/plan-design-review` | UI/UX gaps | 0 | — | n/a (no UI scope) |
| Outside Voice | `/codex` plan review | Independent 2nd opinion | 0 | SKIPPED | offered, user deferred |

- **UNRESOLVED:** 0 across all reviews.
- **VERDICT:** CEO + Eng CLEARED — ready for implementation plan. Next: `superpowers:writing-plans` to produce the step-by-step task list, then TDD implementation.
