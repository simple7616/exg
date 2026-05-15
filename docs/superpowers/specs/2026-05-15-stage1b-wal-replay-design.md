# Stage 1b — WAL Replay + Stage 1a Forward-Pointer Polish

**Date:** 2026-05-15
**Status:** Draft for CEO + Eng review
**Branch:** `feat/stage1b-wal-replay`
**Builds on:** Stage 0 (`4eecbe7`), Stage 1a (`eca73df`)

---

## 1. Overview

Stage 1b makes the exchange survive a restart with full matching-engine state recovered from the write-ahead log, and cleans up four forward pointers left open by Stage 1a's cross-task review.

Today (post Stage 1a) the server holds the order book entirely in process memory. A restart loses every open order; Stage 0's invariant 3 even forbids restart against a non-empty WAL directory to prevent silently splicing fresh state onto an old event timeline. Stage 1b lifts that restriction by replaying the WAL into a fresh engine before traffic resumes, so the process can be killed and restarted without operator data loss.

The four Stage 1a polish items ride in the same PR because they touch the same surface (handlers + auth) and would otherwise sit in a backlog accruing context-rot risk.

## 2. Scope

In scope:

1. **WAL replay on restart** — boot reads every WAL record, applies events to the matching engine, validates post-replay state, then opens HTTP listener.
2. **Stage 0 invariant 3 removed** — WAL directory may be non-empty; that is now the expected state.
3. **`OrderAccepted` event schema bump** — add `side`, `order_type`, `time_in_force`, `price`, `quantity`, `stop_price`, `reduce_only`, `visible_quantity`, `trailing_delta`, `trailing_peak_price` (10 new fields total) so the event carries enough data to rebuild a `BookOrder` for every order type today's API accepts: limit, market, stop-limit/stop-market, take-profit-limit/take-profit-market, trailing-stop, and iceberg. `visible_quantity` was the original 8th field added during Eng review (iceberg replay); `trailing_delta` + `trailing_peak_price` are required for trailing-stop replay correctness because the peak price reflects the mark price at acceptance time and cannot be reconstructed from later state. `expire_time` is derived in `apply_event` from `timestamp + 24h` when `time_in_force = Gtd` (no new event field needed — the engine uses the same formula). Pre-Stage-1b WAL files are not supported (no production data exists yet).
4. **`MatchingEngine::apply_event`** — new replay-only API that updates engine state from a historical event without re-running matching.
5. **`cancel_order` / `amend_order` per-user rate limit** — share the same `user:{N}` token bucket key with `place_order`.
6. **Login `||` short-circuit fix** — charge both email and IP buckets unconditionally so the IP bucket is always advanced.
7. **JWT tamper / token-reuse / kyc_level e2e** — three regression tests filling gaps from Stage 1a unit-only coverage.

Out of scope (forward pointers for Stage 2+):

- Snapshot creation / snapshot-and-tail replay. The snapshot APIs (`save_snapshot`, `load_latest_snapshot`, `take_snapshot`, `restore_from_snapshot`) already exist on disk and are deliberately not invoked. They get wired up when WAL replay time becomes a measurable boot bottleneck.
- WAL truncation policy. Stage 1b grows the WAL unboundedly; truncation after a successful snapshot is a Stage 2/3 concern.
- WAL corruption recovery modes other than fail-fast. Stage 1b boot-panics on any CRC mismatch, sequence gap, or unknown-order event; future stages may add `--repair` modes.
- Real KYC update endpoint. Stage 1b verifies the read path returns `kyc_level` correctly by mutating the DB row directly in a test. The write path lands in Stage 2.

## 3. Architecture changes

```
┌─────────────────────── boot path (Stage 1b) ───────────────────────┐
│                                                                    │
│  Step 0   validate_invariants(&cfg)                                │
│              (drop #3 'WAL must be empty';                         │
│               keep #1/#2/#4/#11/#12)                               │
│                                                                    │
│  Step 1   open WAL writer (existing)                               │
│  Step 2   allocate ring buffer (existing)                          │
│  Step 3   build empty MatchingEngine (existing)                    │
│  Step 3.5 PG pool + SELECT 1 ping (existing)                       │
│                                                                    │
│  Step 3.6 ★ NEW — WAL replay                                       │
│              WalReader::open(&cfg.wal.dir)                         │
│              for each (seq, payload) in reader.read_from(0, ..):   │
│                  ensure seq == expected (panic on gap)             │
│                  event = rkyv::decode(payload)                     │
│                  engine.apply_event(&event)?                       │
│              log "replayed N events through seq=K"                 │
│                                                                    │
│  Step 3.7 init_dummy_argon2_hash (existing)                        │
│                                                                    │
│  Step 3.8 ★ NEW — invariant 21 check                               │
│              assert replayed_count == wal_writer.current_seq()     │
│                                                                    │
│  Step 4   build AppState (engine carries replayed state)           │
│  Step 5   spawn matching thread (engine moves into thread)         │
│  Step 6   HTTP listener                                            │
│                                                                    │
└────────────────────────────────────────────────────────────────────┘
```

Replay runs on the boot thread synchronously before the matching thread is spawned. The engine is not yet visible to anything else, so we mutate it freely with no locking. Once Step 5 hands the engine to the matching thread, replay is finished.

Replay sits **after** PG ping (Step 3.5) so cheaper failure modes (bad DB credentials, network) fail before the potentially-multi-second WAL scan. Replay sits **before** `DUMMY_ARGON2_HASH` init (Step 3.7) because replay is engine-local and doesn't depend on auth state.

## 4. Detailed design

### 4.1 `MatchingEngine::apply_event`

New module `crates/exg-matching-engine/src/replay.rs`. Public surface:

```rust
#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    #[error("event references unknown order_id={0:?}")]
    UnknownOrder(OrderId),
    #[error("OrderAccepted for order_id={0:?} already present in book")]
    DuplicateOrder(OrderId),
    #[error("OrderFilled fill_qty {got} exceeds existing remaining {have}")]
    OverFill { got: Decimal128, have: Decimal128 },
    #[error("event variant {variant} unexpected during replay")]
    UnexpectedVariant { variant: &'static str },
}

impl MatchingEngine {
    pub fn apply_event(&mut self, event: &Event) -> Result<(), ApplyError>;
}
```

Dispatch table:

| Event variant       | Apply action                                                                                     | Error condition                                |
| ------------------- | ------------------------------------------------------------------------------------------------ | ---------------------------------------------- |
| `OrderAccepted`     | Construct `BookOrder` from event fields including `visible_quantity`, `trailing_delta`, `trailing_peak_price`. Derive `expire_time` from `timestamp + 24h` when `time_in_force = Gtd` (else `None`), and `hidden_qty = quantity - visible_qty` for iceberg orders (else `ZERO`). If `order_type.is_conditional()`, push to `stop_orders`. Else insert into `orderbook`. If GTD, also push `(expire_time, order_id)` to `expiry_heap`. | duplicate `order_id` → `DuplicateOrder`        |
| `OrderRejected`     | no-op (rejected orders never entered the book)                                                   | —                                              |
| `OrderCanceled`     | remove `order_id` from book                                                                      | `order_id` not present → `UnknownOrder`        |
| `OrderFilled`       | decrement `BookOrder.remaining_qty` by `fill_qty`; if result is zero, remove from book.          | order not present → `UnknownOrder`; `fill_qty > remaining` → `OverFill` |
| `TradeExecuted`     | No-op. The OrderFilled events covering both sides already updated the book; trade IDs come from `SnowflakeGen` (timestamp + node_id + counter), which cannot collide with prior IDs after a reboot because the timestamp component always advances. | — |
| `MarkPriceUpdate`   | (Stage 0/1a never write these) — return `UnexpectedVariant`                                      | always                                         |
| `FundingRateUpdate` | (Stage 0/1a never write these) — return `UnexpectedVariant`                                      | always                                         |
| `LiquidationOrder`  | (Stage 0/1a never write these) — return `UnexpectedVariant`                                      | always                                         |

Replay does **not** run through `matcher.rs` matching logic. `OrderAccepted` represents a previously-accepted order; the matching it caused has already been recorded as `OrderFilled` events that follow. Re-running matching would double-count fills.

`apply_event` is **not** idempotent. The caller (boot replay) must apply events strictly in WAL order.

### 4.2 `OrderAccepted` event schema bump

Current shape:

```rust
OrderAccepted { order_id, user_id, symbol, client_order_id, timestamp }
```

New shape (10 new fields total — extended from 7 to 10 during Eng review B6):

```rust
OrderAccepted {
    order_id, user_id, symbol, client_order_id, timestamp,
    // Replay-required fields (Eng review B5/B6):
    side: Side,
    order_type: OrderType,
    time_in_force: TimeInForce,
    price: Decimal128,                       // for market orders, the reference price at accept time
    quantity: Decimal128,                    // original submitted quantity
    stop_price: Option<Decimal128>,          // for stop / take-profit conditional orders
    reduce_only: bool,
    visible_quantity: Option<Decimal128>,    // iceberg visible slice (None for non-iceberg)
    trailing_delta: Option<Decimal128>,      // trailing-stop offset
    trailing_peak_price: Option<Decimal128>, // trailing-stop reference price at accept (= mark_price at the time)
}
```

All 10 new fields are derivable from `Command::NewOrder` input + engine state at accept time:
- `side`, `order_type`, `time_in_force`, `price`, `quantity`, `stop_price`, `reduce_only`, `visible_quantity`, `trailing_delta` come directly from the command.
- `trailing_peak_price` comes from `self.mark_price` at accept time (engine.rs:241-244). The engine sets it for trailing orders; the event must carry it because mark_price at replay time may differ.

`expire_time` (used for GTD orders) is NOT a new event field. `apply_event` reconstructs it from `timestamp + 24h` when `time_in_force = Gtd`, matching engine.rs:215-224. The engine also pushes `(expire_time, order_id)` into `expiry_heap` for GTD orders — `apply_event` does the same.

For market orders without a price (some venues accept market orders with no price field): the engine has a working reference price internally; emit that. Tests use limit orders so this is not exercised in Stage 1b.

Existing test assertions use `matches!(e, Event::OrderAccepted { .. })` with the rest-pattern, so adding fields does not break them. The `wal-dump` tool prints the variant name only; new fields render automatically via `Debug`.

### 4.3 Boot replay loop

```rust
// pseudo-rust, simplified
let mut reader = WalReader::open(&cfg.wal.dir)?;
let mut expected_seq: u64 = 0;
let mut replayed: u64 = 0;
let mut last_seq: u64 = 0;
reader.read_from(0, |seq, payload| {
    if seq != expected_seq {
        // Sequence gap mid-stream — corrupt WAL.
        return false;          // stops replay; outer code bails
    }
    let event = rkyv::from_bytes::<Event, rkyv::rancor::Error>(payload)
        .expect("WAL event decode");
    engine.apply_event(&event).expect("apply_event");
    expected_seq = seq + 1;
    replayed += 1;
    last_seq = seq;
    true
})?;
info!("replayed {replayed} events, last_seq={last_seq}");
```

Real implementation propagates errors instead of `expect()`; the structure above is illustrative.

Trailing partial records (last segment truncated mid-record by an unfsynced crash) are handled by `WalReader` already: `decode_record` returns `Incomplete`, the reader stops at that segment cleanly. This is fine — those events are not durable.

CRC mismatch surfaces as `WalError::Corrupt { sequence, reason }` from `WalReader::read_from`; boot bails with the sequence and reason in the message.

### 4.4 Polish #1 — cancel/amend per-user bucket

`crates/exg-api-gateway/src/handlers.rs`, in both `cancel_order` and `amend_order`, immediately after `extract_user_id_from_jwt(...)?`, insert:

```rust
{
    let now_ts = UnixMicros::now();
    let key = format!("user:{}", user_id.value());
    let mut limiter = state.rate_limiter.lock();
    if !limiter.consume(&key, now_ts) {
        return Err(ApiError::user_rate_limited("rate limit exceeded for user"));
    }
}
```

Same key (`"user:{N}"`) as `place_order` — the three operations share one bucket. Spec §9 invariant 17 is reworded to "per-user rate limit on authenticated order endpoints (place / cancel / amend)".

### 4.5 Polish #2 — login `||` short-circuit fix

`handlers.rs::login`, replace:

```rust
if !limiter.consume(&email_key, now) || !limiter.consume(&ip_key, now) {
    return Err(...);
}
```

with:

```rust
let email_ok = limiter.consume(&email_key, now_ts);
let ip_ok = limiter.consume(&ip_key, now_ts);
if !email_ok || !ip_ok {
    return Err(ApiError::user_rate_limited("login rate limit exceeded"));
}
```

Both buckets are charged regardless of which fails first. Symmetric for register's per-IP bucket (already correct — only one bucket).

### 4.6 Polish #3 — invariant 3 removal

In `crates/exg-server/src/lib.rs::validate_invariants`, delete the "WAL directory must be empty" block (currently lines ~98–118). Boot no longer rejects a non-empty WAL dir. The Stage 0 boot-panic test `boot_panics_on_nonempty_wal_dir` is removed in the same change.

### 4.7 Polish #4 — JWT / kyc e2e

Three new tests in `stage1b_e2e.rs`:

- `tampered_jwt_signature_returns_401`: register + login → mangle the last 4 bytes of the JWT → call `/api/v1/me` → expect 401 + `code: -1002`.
- `token_reuse_within_expiry_succeeds`: register + login → call `/me` twice with the same token → both 200, same user_id. Verifies JWTs are not single-use.
- `kyc_level_reflected_in_me`: register + login → `sqlx::query("UPDATE users SET kyc_level = $1 WHERE user_id = $2")` directly → call `/me` → expect `kycLevel = 2`. Exercises the read path; the write endpoint is Stage 2 scope.

## 5. Event schema migration

`OrderAccepted` gains seven fields. Implications:

- **rkyv compatibility**: rkyv archived layouts are not forward-compatible across field additions to non-Option variants. Any pre-Stage-1b WAL file becomes unreadable. Acceptable — no production WAL exists.
- **Snapshot file forward-compat**: `EngineSnapshot` serializes `BookOrder` as serde JSON (`snapshot.rs`); not affected by the event schema bump. (Snapshot is unused in Stage 1b but kept compiling.)
- **Tests**: `stage0_e2e.rs` and `stage1a_e2e.rs` use rest-pattern matching (`OrderAccepted { .. }`), unaffected.
- **`engine.process_command`** must be updated to populate the new fields from the `NewOrder` command. Straightforward field copy.

## 6. Data flow

Steady-state path is unchanged (Stage 1a flow). The replay path is a one-shot boot-time process:

```
Boot starts
   │
   ▼
validate_invariants ──── fails ──► panic
   │ ok
   ▼
WalWriter::open  ◄── reads existing segments to compute next_sequence
   │
   ▼
build empty MatchingEngine
   │
   ▼
PgPool::connect + SELECT 1 ──── fails ──► panic
   │
   ▼
WalReader::open
   │
   ▼
read_from(0) ──► for each event ──► engine.apply_event ──── err ──► panic
   │                                                          │ ok
   ▼                                                          ▼
init DUMMY_ARGON2_HASH                                    next event
   │
   ▼
invariant 21: engine.next_seq() == wal_writer.next_seq() ──── fails ──► panic
   │ ok
   ▼
spawn matching thread (consumes ring buffer → engine.process_command → WAL append)
   │
   ▼
HTTP listener open ── steady state ──
```

After Step 6 the flow is identical to Stage 1a: HTTP → handlers → ring buffer → matching thread → engine.process_command → WAL append → response. The matching thread inherits an engine whose state is the replay outcome.

## 7. Error handling

### 7.1 New boot panics

| Trigger step | Condition                                  | Message format                                                                       |
| ------------ | ------------------------------------------ | ------------------------------------------------------------------------------------ |
| Step 1 (`WalWriter::open`) | WAL CRC mismatch in a non-trailing position | `WAL writer open failed: corrupt at sequence {seq}: CRC mismatch` (passed through from `WalError::Corrupt`) |
| Step 3.6 (replay) | WAL sequence gap (`seq != expected_seq`)   | `WAL replay failed: sequence gap at expected={expected}, got={seq}`                  |
| Step 3.6 (replay) | rkyv decode failure                        | `WAL replay failed: rkyv decode at sequence {seq}: {err}`                            |
| Step 3.6 (replay) | `apply_event` returns `Err`                | `WAL replay failed at sequence {seq}: {apply_err}`                                   |
| Step 3.8 | Invariant 21 violation                     | `invariant 21 violated: replayed_count={c}, wal_writer.current_seq={w}`              |

Note on ordering: `WalWriter::open` already calls `recover_state` (Stage 0 implementation) which scans existing segments, truncates incomplete tail records, and returns `WalError::Corrupt` on any mid-stream CRC mismatch. This means CRC failures surface at Step 1, before replay even begins — operators see a writer-open panic, not a replay panic. Stage 1b leaves that behavior intact and only adds the replay-step panics (sequence gap, decode, apply_event errors).

Every WAL-related panic message contains the literal string `WAL` so operators can grep. Every message includes the offending sequence number where applicable.

### 7.2 Removed boot panic

| Condition (Stage 0)                        | Status (Stage 1b)                                          |
| ------------------------------------------ | ---------------------------------------------------------- |
| WAL directory non-empty (invariant 3)      | **Removed.** Non-empty WAL is the expected state.          |

### 7.3 Steady-state error handling

Unchanged. Handler errors map to `ApiError` codes per Stage 1a §7.1.

## 8. Testing

### 8.1 Unit tests (`crates/exg-matching-engine/src/replay.rs::tests`)

Nineteen tests, all in-process, no fixtures (Eng review B12 added stop-order paths, B13 added other UnexpectedVariant arms, B6 added schema-field paths):

1. `apply_order_accepted_inserts_book_order` (Limit, non-conditional)
2. `apply_order_accepted_conditional_pushes_to_stop_orders` (StopLimit / TrailingStop / Iceberg — B12)
3. `apply_order_accepted_gtd_pushes_to_expiry_heap` (B6 — verifies GTD heap insertion)
4. `apply_order_accepted_iceberg_preserves_visible_quantity` (B6 — visible_qty round-trip)
5. `apply_order_accepted_trailing_preserves_peak_price` (B6 — trailing_peak_price round-trip)
6. `apply_order_canceled_removes_book_order`
7. `apply_order_canceled_removes_from_stop_orders` (B12 — fallback path for conditional cancel)
8. `apply_order_filled_decrements_remaining_qty`
9. `apply_order_filled_zero_removes_book_order`
10. `apply_trade_executed_is_noop_on_book`
11. `apply_order_rejected_is_noop`
12. `apply_duplicate_order_accepted_returns_err`
13. `apply_unknown_order_canceled_returns_err`
14. `apply_unknown_order_filled_returns_err`
15. `apply_over_fill_returns_err`
16. `apply_mark_price_update_returns_unexpected_variant`
17. `apply_funding_rate_update_returns_unexpected_variant` (B13)
18. `apply_liquidation_order_returns_unexpected_variant` (B13)
19. `replay_then_take_snapshot_round_trip` (golden round-trip via process_command → events → apply_event into empty engine; compare order counts)

### 8.2 Integration tests (`crates/exg-server/tests/stage1b_e2e.rs`)

Sixteen tests, each `#[sqlx::test(migrations = "../../migrations")]`:

Replay correctness (9 — the duplicate `replay_engine_state_matches_pre_kill_snapshot` was removed during Eng review B10; it duplicated the unit-level round-trip test):
- `boot_replays_empty_wal_succeeds`
- `boot_replays_single_order_restores_orderbook`
- `boot_replays_place_cancel_restores_empty_orderbook`
- `boot_replays_place_amend_restores_amended_price`
- `boot_replays_matched_trade_restores_post_match_state`
- `place_then_kill_then_place_continues_sequence`
- `boot_replays_three_orders_inspectable_via_wal`
- `boot_with_only_rejected_events_succeeds`
- **`replay_survived_order_matches_post_reboot_taker`** (CEO review A4): boot 1 places maker bid; kill; boot 2 places aggressive taker at maker price; second WAL contains an `OrderFilled` event referencing the maker `order_id` from boot 1 — proves replay restored the order to the matchable book, not just to "boot didn't panic."

The three corruption tests (`boot_panics_on_corrupt_wal_crc`, `boot_panics_on_sequence_gap`, `boot_panics_on_unknown_order_filled` — the third added during Eng review B11) live in `boot_panics.rs` rather than `stage1b_e2e.rs` because they assert boot-time panic behavior, not steady-state replay correctness.

Polish (7):
- `cancel_order_rate_limit`
- `amend_order_rate_limit`
- `mixed_place_cancel_share_user_bucket`
- `login_charges_ip_bucket_even_when_email_exhausted`
- `tampered_jwt_signature_returns_401`
- `token_reuse_within_expiry_succeeds`
- `kyc_level_reflected_in_me`

### 8.3 Existing test deltas

- `boot_panics.rs`: remove `boot_panics_on_nonempty_wal_dir` (invariant 3 gone); add `boot_panics_on_corrupt_wal_crc`, `boot_panics_on_sequence_gap`, `boot_panics_on_unknown_order_filled` (the third added during Eng review B11). Result: 9/9 (was 7, net +2).
- `stage0_e2e.rs`: no source change (rest-pattern match is forward-compat). Result: 7/7.
- `stage1a_e2e.rs`: no source change. Result: 12/12.

Acceptance target: **35 new tests + all existing tests still pass** (19 unit + 16 e2e — boot_panics counted in "existing test deltas" above).

## 9. Invariants

Numbering continues from Stage 1a's #20.

### Removed in Stage 1b

- ~~**#3** WAL directory must be empty at boot.~~ — replaced by replay.

### Retained from Stage 0 + 1a

- #1 single-symbol config
- #2 loopback host bind
- #4 mark price > 0
- #5–#10 Stage 0 matching invariants
- #11 jwt_secret length ≥ 32
- #12 jwt_secret ≠ placeholder
- #13 handler-side dedup before ring-buffer push
- #14 Argon2id with random salt
- #15 constant-time login (dummy hash)
- #16 login response identical for unknown-email vs wrong-password
- #17 per-user rate limit on **place / cancel / amend** (reworded from "order endpoints")
- #18 per-email + per-IP rate limit on login (both buckets charged unconditionally — Polish #2)
- #19 JWT verification before any state-mutating handler logic
- #20 PG connection ping on boot

### New in Stage 1b

- **#21** Post-replay sequence consistency: the number of events applied during Step 3.6 equals `wal_writer.current_sequence()` (the next WAL sequence to assign). This catches reader/writer disagreement about how many records exist on disk. NOTE: `MatchingEngine.sequence` is a per-command counter and is **not** comparable to WAL per-event sequences; do not conflate them.
- **#22** WAL replay is fail-fast: any CRC mismatch, sequence gap, rkyv decode error, or `apply_event` error during boot is fatal.
- **#23** Replay is single-threaded and synchronous: no other thread reads or writes the engine state during Step 3.6.

## 9.5 Migration from Stage 1a (CEO review A7)

Stage 1b's `OrderAccepted` schema change is **not rkyv-forward-compatible**. WAL files written by any Stage 0 or Stage 1a binary cannot be decoded by Stage 1b. Pre-upgrade actions for any environment that ran Stage 1a:

```bash
# stop the Stage 1a server (clean shutdown)
docker compose stop exg-server   # or kill -INT <pid>

# wipe the WAL (no production data exists; dev/CI loses all open orders)
rm -rf data/wal

# OR — if forensic access is desired — rename rather than delete
mv data/wal data/wal-stage1a-archive
```

CI environments must clear their WAL caches once on the Stage 1b boot. The demo script (`scripts/demo-stage1b.sh`) uses `mktemp -d` so the demo path is unaffected.

After the cleanup, Stage 1b boot encounters an empty WAL, replays zero events, and proceeds normally. From that point on, every WAL record uses the new schema and replay is fully functional.

## 9.6 Rollback to Stage 1a (CEO review A8)

Stage 1b is dev-only — no production users are affected by a rollback. Procedure if Stage 1b ships and immediately breaks:

1. Stop the Stage 1b server (clean shutdown — `kill -INT <pid>` lets the matching thread drain).
2. `git revert <merge-commit>` to put Stage 1a code back.
3. `rm -rf data/wal` — Stage 1a's invariant 3 ("WAL must be empty") will otherwise reject boot. **All Stage 1b orders are lost**; this is acceptable in dev.
4. Restart.

A production-grade rollback (one that preserves orders across schema changes) is out of scope until Stage 5+. See forward pointer "Production rollback strategy."

## 10. Acceptance

PR passes when:

1. `cargo check --workspace` clean.
2. `cargo clippy --workspace -- -D warnings` clean.
3. `cargo fmt --check` clean.
4. `cargo test --workspace` all green (≥390 tests).
5. New tests: replay unit 19/19, stage1b_e2e 16/16 (CEO A4 added 1, Eng B10 removed 1 duplicate).
6. Existing tests: stage0_e2e 7/7, stage1a_e2e 12/12, boot_panics 9/9 (was 7, net +2 after Eng B11), user-service 30/30.
7. New script `scripts/demo-stage1b.sh` walks: docker-compose up → migrate reset → boot → place order → ^C → boot again → GET /me/orders shows order persists (or equivalent — use take_snapshot inspection if no orders endpoint yet) → ^C → wal-dump shows event count > 0.

## 11. Forward pointers (Stage 2+)

- **Snapshot creation cadence**: enable existing `WalWriter::save_snapshot` + `WalReader::load_latest_snapshot`. Trigger TBD: every N events, every M seconds, or graceful-shutdown only.
- **Snapshot-and-tail replay**: load snapshot → tail WAL from `snapshot.sequence`. Boot-time win when WAL grows beyond ~10k events.
- **WAL truncation**: after a successful snapshot, drop segments fully covered by it. Keep at least one segment back for forensics.
- **KYC update endpoint**: `/api/v1/kyc/level` (or similar). Wires up the write path Stage 1b stubs out with direct SQL.
- **Mark price service**: replaces `cfg.trading.symbols[0].mark_price` static value. Adds `Event::MarkPriceUpdate` to WAL — requires extending `apply_event` to handle the variant rather than `UnexpectedVariant`.
- **Liquidation engine**: adds `Event::LiquidationOrder` writes — same `apply_event` extension.
- **Funding rate service**: same pattern.
- **WAL replay performance**: Stage 1b is single-threaded byte-by-byte. If 100k+ events become routine, consider parallel decode (`apply_event` itself must stay serial, but rkyv decode can pipeline).
- **WAL volume-loss sentinel** (CEO review A1): an empty WAL directory is currently indistinguishable from "fresh install" vs "volume not mounted / WAL deleted." When Stage 2 lands persistent metadata in PG, record a `wal_existed_at_seq` row on each non-empty boot. If a subsequent boot sees `wal_existed_at_seq > 0` in PG but the WAL dir is empty, panic with "WAL volume missing — refuse to boot with empty engine." Until then, operators must verify the WAL mount manually before each restart.
- **Replay observability** (CEO review A5): expose `wal_replay_duration_seconds` and `wal_replay_events_total` as Prometheus metrics. Replay corruption emits `wal_replay_corruption_total{reason="crc|gap|decode|apply|invariant_21"}` for alerting. Wire up alongside the Stage 5+ metrics stack rollout; for now the single `info!` line at boot is enough for dev visibility.
- **Production rollback strategy** (CEO review A8): schema versioning + reversible migrations. When event schemas change between stages, the new code must accept previous-stage events for one major version, and rollback must not require WAL deletion. Out of scope until Stage 5+ when production traffic exists.

---

## Appendix A — Decisions log (from brainstorming)

| Decision                                   | Choice                                             |
| ------------------------------------------ | -------------------------------------------------- |
| Stage 1b scope                             | Replay + 4 polish in one PR                        |
| Replay strategy                            | Pure WAL replay (no snapshot)                      |
| WAL content semantic                       | Events (preserve Stage 0 design); add `apply_event`|
| CRC mismatch handling                      | Boot panic                                         |
| Cancel/amend rate limit                    | Extend per-user bucket                             |
| `OrderAccepted` missing replay fields      | Extend event schema (breaking change OK pre-prod)  |
| Boot lifecycle placement                   | Replay = Step 3.6 (post PG ping, pre DUMMY init)   |
