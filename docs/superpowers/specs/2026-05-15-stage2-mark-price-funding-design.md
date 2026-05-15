# Stage 2 — Mark Price + Funding Rate Service

**Date:** 2026-05-15
**Status:** Draft for CEO + Eng review
**Branch:** `feat/stage2-mark-price-funding`
**Builds on:** Stage 0 (`4eecbe7`), Stage 1a (`eca73df`), Stage 1b (`b196000`)

---

## 1. Overview

Stage 2 replaces the static `cfg.trading.symbols[0].mark_price` with a dynamic mark/index price driven by an authenticated admin endpoint, and computes the funding rate on an admin trigger. Both flow through the existing WAL command path and replay correctly.

Today the matching engine's `mark_price` / `index_price` are set once at boot from config and never change. Trailing-stop peak tracking and stop/take-profit triggering already key off `mark_price` (engine.rs:819-837), and the risk engine's funding math (`exg-risk-engine/src/funding.rs`) already exists as pure functions — but nothing drives price changes or computes funding. Stage 1b's `apply_event` explicitly rejects `MarkPriceUpdate` / `FundingRateUpdate` with `ApplyError::UnexpectedVariant`; Stage 2 makes them first-class.

Funding **settlement** (moving user funds per interval) requires position tracking + a ledger, which Stage 0's phase decomposition defers to a later stage. Stage 2 computes and emits the funding rate but does not settle.

## 2. Scope

In scope:

1. **`Command::UpdateMarkPrice`** + **`Command::ComputeFunding`** — two new ring-buffer commands (WAL'd via the existing Stage 0 hot path).
2. **Admin HTTP server** on `cfg.server.admin_port` (9090) — a second `actix_web::HttpServer` sharing `AppState`, with an `X-Admin-Secret` shared-secret middleware. Two routes: `POST /api/v1/admin/mark-price`, `POST /api/v1/admin/funding-tick`.
3. **`AdminConfig { admin_secret }`** + boot invariants #24 (length ≥ 32) and #25 (≠ placeholder), mirroring Stage 1a's JWT secret invariants #11/#12.
4. **`engine.update_mark_price` passive/active split** — `apply_mark_index_passive` (set mark/index + `update_trailing_peaks`) reused by both the live path and replay; the active trigger+match part is live-only.
5. **`engine.compute_funding`** — `premium = (mark - index) / index`; `calc_funding_rate(premium, interest_rate)`; store `last_funding_rate`; emit `FundingRateUpdate`.
6. **`apply_event` extension** (exg-matching-engine/replay.rs) — `MarkPriceUpdate` arm (passive only) and `FundingRateUpdate` arm (set `last_funding_rate`); both `UnexpectedVariant` rejections removed.
7. **`MatchingEngine` gains `last_funding_rate`** + `interest_rate` (threaded from `cfg.risk.interest_rate`); `EngineSnapshot` round-trips `last_funding_rate`.

Out of scope (forward pointers for Stage 3+):

- **Funding settlement** — actually debiting/crediting user funds per `funding_interval_hours`. Needs position tracking + ledger (later stage).
- **External price feed / oracle** — Stage 2 uses admin injection. HTTP/WS pull from a spot source is a Stage 5+ production concern.
- **Periodic funding timer** — `funding_interval_hours` (8h) production cadence. Stage 2 uses on-demand admin trigger for deterministic replay tests.
- **TWAP / impact-depth premium index** — `cfg.risk.impact_notional` is reserved for the production impact-bid/ask premium formula. Stage 2 uses the instantaneous `(mark - index) / index`.
- **Multi-symbol mark price** — single-symbol invariant (Stage 0 #1) still holds; symbol comes from `cfg.trading.symbols[0]`.

## 3. Architecture

```
 admin client                  admin HTTP server (NEW, bound to admin_port 9090)
   │  POST /api/v1/admin/mark-price {markPrice, indexPrice}        ┌─ X-Admin-Secret gate
   │  POST /api/v1/admin/funding-tick                              │  (constant-time compare)
   ▼                                                               ▼
 admin handlers ── build Command ──► Mutex<Producer> ──► ring buffer (shared with main 8080)
                                                              │
                                                              ▼
                              matching thread (single, lock-free) pops Command
                                  ├─ Command::UpdateMarkPrice → engine.update_mark_price()
                                  │     = apply_mark_index_passive (set mark/index +
                                  │       update_trailing_peaks)
                                  │     + active (check_stop_triggers + matcher)
                                  │     emits [MarkPriceUpdate, OrderFilled*, TradeExecuted*]
                                  └─ Command::ComputeFunding → engine.compute_funding()
                                        premium = (mark - index) / index
                                        rate = calc_funding_rate(premium, interest_rate)
                                        self.last_funding_rate = rate
                                        emits [FundingRateUpdate]
                                              │
                                              ▼
                                       WAL append (in WAL order)

 boot Step 3.6 replay (extends Stage 1b apply_event):
   apply_event(MarkPriceUpdate{mark,index})  → apply_mark_index_passive(mark,index)
                                                (NO trigger/match — triggered OrderFilled
                                                 events are separate WAL records, replayed
                                                 via their own arm)
   apply_event(FundingRateUpdate{rate})      → self.last_funding_rate = rate
```

**Component placement:** admin handlers + `X-Admin-Secret` middleware live as an `admin` module inside the existing `exg-api-gateway` crate (reuses `AppState`, `ApiError`, the `Mutex<Producer>` handle, error codes). No new crate. Logical isolation is enforced by the separate port + the secret middleware, not by a crate boundary.

**Why the ring buffer (not a side channel):** keeps the single-threaded-lock-free matching invariant (Stage 0) intact, gives automatic WAL ordering between price updates and the order events they trigger, and makes replay automatic — `apply_event` already consumes the WAL event stream. Identical reasoning to Stage 1b's "WAL is events, replay re-applies events" decision.

**Why passive/active split:** a live `Command::UpdateMarkPrice` legitimately triggers stop orders and produces fills — those fills are WAL'd as their own `OrderFilled` / `TradeExecuted` events immediately after the `MarkPriceUpdate` event. On replay, re-running the active trigger+match would double-count those fills (the exact anti-pattern Stage 1b established for `OrderAccepted`). Replay therefore runs only the passive half (set price + update trailing peaks — peak state is internal and not separately WAL'd, so it must be reconstructed).

## 4. Detailed Design

### 4.1 `Command` enum (exg-protocol/src/command.rs)

```rust
UpdateMarkPrice {
    symbol: SymbolId,
    mark_price: Decimal128,
    index_price: Decimal128,
    timestamp: UnixMicros,
},
ComputeFunding {
    symbol: SymbolId,
    timestamp: UnixMicros,
},
```

`process_command` dispatches: `UpdateMarkPrice` → `self.update_mark_price(mark, index)` (returns the full event vec); `ComputeFunding` → `self.compute_funding(symbol, timestamp)`.

### 4.2 `Event` enum — unchanged

`MarkPriceUpdate { symbol, mark_price, index_price, timestamp }` and `FundingRateUpdate { symbol, funding_rate, timestamp }` already exist (Stage 0, exg-protocol/src/event.rs). Stage 2 starts emitting them. No schema change → no rkyv break → existing WAL files from Stage 1b remain replayable (the new events simply weren't present before).

### 4.3 `MatchingEngine` changes (exg-matching-engine/src/engine.rs)

New state:

```rust
last_funding_rate: Decimal128,   // ZERO until first ComputeFunding
interest_rate: Decimal128,       // from cfg.risk.interest_rate, set at construction
```

`MatchingEngine::new` gains an `interest_rate: Decimal128` parameter. `interest_rate` lives in `cfg.risk.interest_rate` (a `[risk]` field, not `SymbolConfig`), so it is threaded explicitly. Call sites that must change:

- `exg-server/src/lib.rs` boot Step 3 — pass `cfg.risk.interest_rate` (parsed to `Decimal128`).
- `restore_from_snapshot(snapshot, config, node_id)` — gains a 4th param `interest_rate: Decimal128` and forwards it to `Self::new(config, node_id, interest_rate)`. The snapshot does NOT carry `interest_rate` (it is config, not engine state); the caller supplies it. `restore_from_snapshot` is test-only at runtime (snapshot is unused per Stage 1b), so this cascade is mechanical.
- Every `MatchingEngine::new(test_config(), 1)` / `test_engine()` helper across `engine.rs`, `replay.rs`, and any integration test — add a literal test interest rate (e.g. `dec("0.0001")`). Mechanical, not a behavior change.

`update_mark_price` currently has **no external callers** (Stage 1b's `apply_event` rejects `MarkPriceUpdate`, so nothing drives it today; `exg-clearing::position::update_mark_prices` is an unrelated plural function on the clearing position table, not a caller of the engine method). The signature change (adding `symbol` + `timestamp`, emitting `MarkPriceUpdate` first) only touches the new `process_command` dispatch and the engine's own unit tests.

Passive/active split:

```rust
/// Passive: set prices + reconstruct trailing-peak state. Used by the live
/// path (first half) AND replay. No triggering, no matching.
fn apply_mark_index_passive(&mut self, mark: Decimal128, index: Decimal128) {
    self.mark_price = mark;
    self.index_price = index;
    self.update_trailing_peaks();
}

/// Live path (process_command → Command::UpdateMarkPrice).
pub fn update_mark_price(
    &mut self,
    symbol: SymbolId,
    mark: Decimal128,
    index: Decimal128,
    timestamp: UnixMicros,
) -> Vec<Event> {
    self.apply_mark_index_passive(mark, index);
    let mut events = vec![Event::MarkPriceUpdate {
        symbol, mark_price: mark, index_price: index, timestamp,
    }];
    // active: check_stop_triggers + match (the existing logic, refactored to
    // append OrderFilled / TradeExecuted after the MarkPriceUpdate event)
    events.extend(self.trigger_and_match_stops(timestamp));
    events
}

pub fn compute_funding(
    &mut self,
    symbol: SymbolId,
    timestamp: UnixMicros,
) -> Vec<Event> {
    let premium = if self.index_price.is_zero() {
        Decimal128::ZERO
    } else {
        (self.mark_price - self.index_price) / self.index_price
    };
    let rate = exg_risk_engine::funding::calc_funding_rate(premium, self.interest_rate);
    self.last_funding_rate = rate;
    vec![Event::FundingRateUpdate { symbol, funding_rate: rate, timestamp }]
}
```

The existing `update_mark_price(&mut self, mark, index) -> Vec<Event>` (engine.rs:731) is refactored: the prelude (`self.mark_price = ...; self.index_price = ...; self.update_trailing_peaks();`) moves into `apply_mark_index_passive`; the remainder (`check_stop_triggers_internal` + per-order match loop) moves into `trigger_and_match_stops`. The current signature changes to take `symbol` + `timestamp` so it can emit the `MarkPriceUpdate` event itself (today it returns only the triggered fills; Stage 2 makes it emit `MarkPriceUpdate` first, then the fills).

### 4.4 `apply_event` extension (exg-matching-engine/src/replay.rs)

Replace the two `UnexpectedVariant` arms:

```rust
Event::MarkPriceUpdate { mark_price, index_price, .. } => {
    // Passive only — triggered OrderFilled/TradeExecuted events are separate
    // WAL records replayed via their own arms. Re-triggering here would
    // double-count fills (same principle as OrderAccepted not re-matching).
    self.apply_mark_index_passive(*mark_price, *index_price);
    Ok(())
}
Event::FundingRateUpdate { funding_rate, .. } => {
    self.set_last_funding_rate(*funding_rate);
    Ok(())
}
```

`apply_mark_index_passive` is exposed to `replay.rs` the same way Stage 1b exposed `orderbook_mut` etc. — a `#[doc(hidden)]` replay-only accessor, OR `apply_mark_index_passive` itself made `pub(crate)`-visible to the replay module. A new `#[doc(hidden)] pub fn set_last_funding_rate(&mut self, r: Decimal128)` accessor is added for the funding arm.

`MarkPriceUpdate` / `FundingRateUpdate` are removed from the `UnexpectedVariant` set. `LiquidationOrder` remains `UnexpectedVariant` (Stage 3+).

### 4.5 `EngineSnapshot` (exg-matching-engine/src/snapshot.rs)

Add `last_funding_rate: Decimal128`. `take_snapshot` captures it; `restore_from_snapshot` restores it. Consistent with Stage 1b's snapshot discipline — snapshot is unused at runtime but must stay structurally complete.

### 4.6 `AdminConfig` (exg-config/src/lib.rs)

```toml
[admin]
admin_secret = "CHANGE-ME-ADMIN-DEV-ONLY-MUST-BE-32-BYTES"
```

```rust
pub struct AdminConfig { pub admin_secret: String }
```

`ExgConfig` gains `pub admin: AdminConfig`. `default_config()` uses the placeholder. `validate()` (exg-config/src/validation.rs) gains the length + placeholder checks (mirrors Stage 1a JWT validation).

### 4.7 Admin module (exg-api-gateway/src/admin.rs)

- `X-Admin-Secret` middleware: extract header, **constant-time compare** against `cfg.admin.admin_secret` (use the same constant-time discipline as Stage 1a login — avoid a timing oracle on the secret). Missing/mismatch → `ApiError::unauthorized` (401 / -1002).
- `admin_mark_price(state, body: AdminMarkPriceRequest)`: parse `markPrice` / `indexPrice` as `Decimal128`; reject `indexPrice <= 0` AND `markPrice <= 0` with 400 / -1100 (indexPrice guard prevents `compute_funding` div-by-zero; markPrice guard — CEO review C5 — prevents a negative mark price mass-triggering every positive-stop sell order); build `Command::UpdateMarkPrice`; push to `state.producer`; emit a `tracing::info!(target: "admin", mark_price, index_price, "mark price injected")` audit line (CEO review C6); 200 `{status:"ACCEPTED"}`.
- `admin_funding_tick(state)`: no body; symbol = `cfg.trading.symbols[0].id`; build `Command::ComputeFunding`; emit a `tracing::info!(target: "admin", "funding tick")` audit line (CEO review C6); push; 200.
- `AdminMarkPriceRequest { mark_price: String, index_price: String }` (camelCase serde rename, stringified decimals — consistent with Stage 1a request shapes).

### 4.8 Boot lifecycle (exg-server/src/lib.rs)

- **Step 0** `validate_invariants`: + invariant #24 (`cfg.admin.admin_secret.len() >= 32`) + #25 (`!= placeholder`). Same `anyhow::bail!` pattern as JWT #11/#12.
- **Step 3** `MatchingEngine::new(symbol_config, node_id, cfg.risk.interest_rate)` — new param threaded.
- **Step 6**: after the main HTTP server binds to `(host, port)`, bind a **second `HttpServer`** to `(host, admin_port)` serving only the admin routes (`build_admin_app(state.clone())`). Both share the same `AppState` (same `Mutex<Producer>`). Both register into the same graceful-shutdown path so SIGINT drains both.

## 5. Data Flow & Error Handling

### 5.1 Admin command flow (live)

```
POST /admin/mark-price
  ├─ X-Admin-Secret missing/wrong ──► 401  ERR_UNAUTHORIZED (-1002)
  ├─ markPrice/indexPrice not Decimal128 ──► 400  ERR_INVALID_PARAMETER (-1100)
  ├─ indexPrice <= 0 ──► 400  ERR_INVALID_PARAMETER (-1100)  (funding div-by-zero guard)
  ├─ markPrice <= 0 ──► 400  ERR_INVALID_PARAMETER (-1100)  (mass stop-trigger guard, C5)
  ├─ ring buffer full ──► 429  ERR_TOO_MANY_REQUESTS (-1015)
  └─ ok ──► 200 {status:"ACCEPTED"}

POST /admin/funding-tick
  ├─ X-Admin-Secret missing/wrong ──► 401 (-1002)
  ├─ ring buffer full ──► 429 (-1015)
  └─ ok ──► 200 {status:"ACCEPTED"}
```

### 5.2 Replay flow (boot Step 3.6)

`apply_event` shadow paths:
- `MarkPriceUpdate` happy → passive set + peaks, Ok
- `MarkPriceUpdate` with `index_price == 0` in the event → still just sets the values (no funding math here; `compute_funding`'s div-guard is the protection point). Ok.
- `FundingRateUpdate` happy → set `last_funding_rate`, Ok
- All other Stage 1b arms unchanged. `LiquidationOrder` still `UnexpectedVariant` → boot panic (Stage 3+ territory; fail-fast preserved).

### 5.3 Boot panics

| Condition | Message | Step |
|-----------|---------|------|
| `admin_secret.len() < 32` | `Stage 2: admin.admin_secret must be at least 32 bytes, got N` | Step 0 |
| `admin_secret == placeholder` | `Stage 2: admin.admin_secret is the placeholder; override via EXG_ADMIN_SECRET` | Step 0 |
| (inherited) WAL replay failures | unchanged from Stage 1b §7.1 | Step 3.6 |

## 6. Invariants

Numbering continues from Stage 1b's #23.

Retained: Stage 0 #1–#10, Stage 1a #11–#20, Stage 1b #21–#23 (all unaffected; verified by regression baselines).

New in Stage 2:

- **#24** `cfg.admin.admin_secret.len() >= 32` at boot.
- **#25** `cfg.admin.admin_secret != placeholder` at boot.
- **#26** Admin endpoints reject missing/wrong `X-Admin-Secret` (constant-time compare) before producing any `Command`.
- **#27** `apply_event(MarkPriceUpdate)` is passive-only during replay — it never re-triggers stop/take-profit/trailing orders nor invokes the matcher. The triggered fills are independent WAL `OrderFilled` events.
- **#28** `compute_funding` never divides by zero — `index_price == 0` yields `premium = ZERO` (the live admin path also rejects `indexPrice <= 0` at 400 before the command is produced; #28 is the defense-in-depth engine-level guard).
- **#29** (CEO review C5) The admin mark-price endpoint rejects `markPrice <= 0` at 400 before producing a `Command::UpdateMarkPrice`. A non-positive mark price would make `mark <= stop_price` true for every positive-stop sell order, mass-triggering them. Symmetric with the `indexPrice <= 0` guard.
- **#30** (CEO review C6) Every accepted admin command emits a `tracing::info!(target: "admin", ...)` audit line before enqueue, so operators can reconstruct who moved the mark price / triggered funding and when, without `wal-dump`.

## 7. Testing

### 7.1 Unit (exg-matching-engine, replay.rs + engine.rs tests)

1. `update_mark_price_passive_sets_price_and_peaks_no_fills` — passive half: price set, trailing peak advanced, no OrderFilled emitted
2. `update_mark_price_full_triggers_stop_emits_fill` — mark crosses stop_price → MarkPriceUpdate + OrderFilled emitted in order
3. `apply_event_mark_price_update_passive_only` — resting stop with stop_price below new mark is NOT triggered during replay (no OrderFilled), price + peak still updated
4. `apply_event_funding_rate_update_sets_last_rate` — `last_funding_rate` updated
5. `compute_funding_positive_premium` — mark>index → positive rate via calc_funding_rate
6. `compute_funding_negative_premium` — mark<index → negative rate
7. `compute_funding_zero_index_no_panic` — index=0 → premium ZERO, rate finite, no panic
8. `compute_funding_sets_last_funding_rate`
9. `snapshot_round_trips_last_funding_rate`
10. `replay_round_trip_with_mark_price_and_funding` — live engine (place stop, update mark to trigger, compute funding) vs replayed engine from the emitted event stream → identical orderbook + last_funding_rate
11. `apply_event_liquidation_order_still_unexpected_variant` — Stage 3 boundary intact
12. (CEO review C8) `apply_event_mark_price_replay_preserves_trailing_peak` — accept a TrailingStop with a known `trailing_peak_price`, replay a sequence of `MarkPriceUpdate` events that advance the peak, assert the replayed order's `trailing_peak_price` equals the live engine's after the same `update_mark_price` sequence. Guards the Stage 1b B5/B6 silent-corruption-on-replay class for trailing peaks.

### 7.2 Integration (exg-server/tests/stage2_e2e.rs, `#[sqlx::test]`)

1. `admin_mark_price_inject_triggers_stop_order` — register/login, place stop, admin inject mark crossing stop, WAL shows OrderFilled
2. `admin_funding_tick_emits_funding_rate` — admin funding-tick, WAL contains FundingRateUpdate with rate = calc_funding_rate((mark-index)/index, interest_rate)
3. `admin_endpoint_missing_secret_returns_401`
4. `admin_endpoint_wrong_secret_returns_401`
5. `admin_endpoint_correct_secret_returns_200`
6. `admin_route_not_on_main_port` — `POST :8080/api/v1/admin/mark-price` → 404
7. `user_route_not_on_admin_port` — `POST :9090/api/v1/order` → 404
8. `admin_mark_price_bad_decimal_returns_400`
9. `admin_mark_price_zero_index_returns_400`
10. `replay_mark_price_trigger_survives_reboot` — inject mark that triggers a stop → kill → reboot → WAL replay. **Observable assertion (CEO review C10)**: record boot-1 WAL record count; after reboot (no new injects), scan the WAL and assert (a) the triggered stop's `OrderFilled` appears within the boot-1 record range, and (b) no NEW `OrderFilled` for that order_id was appended during boot-2 — proving replay did NOT re-trigger the stop. Not just "boot 2 didn't panic" (the weak proxy Stage 1b A4 rejected).
11. (CEO review C5) `admin_mark_price_negative_or_zero_mark_returns_400` — `markPrice: "0"` and `markPrice: "-1"` each → 400 / -1100; no `Command::UpdateMarkPrice` produced.

### 7.3 Boot panics (exg-server/tests/boot_panics.rs)

- `boot_panics_on_short_admin_secret`
- `boot_panics_on_placeholder_admin_secret`

Net: 7 → 9 boot panic tests.

### 7.4 Regression baselines (must stay green, source unchanged)

- stage0_e2e 7/7, stage1a_e2e 12/12, stage1b_e2e 16/16, exg-user-service 30/30
- exg-matching-engine `--lib` existing tests + Stage 1b replay 19 (note: `MatchingEngine::new` signature change touches every `test_engine()` helper — mechanical, not a behavior change)

## 8. Acceptance

PR passes when:

1. `cargo check --workspace` clean
2. `cargo clippy --workspace -- -D warnings` clean
3. `cargo fmt --check` clean
4. `cargo test --workspace` all green
5. New: stage2_e2e 11/11, engine/replay Stage 2 unit 12/12 (CEO review C5 added 1 e2e, C8 added 1 unit)
6. Regression: stage0_e2e 7/7, stage1a_e2e 12/12, stage1b_e2e 16/16, boot_panics 9/9 (was 7, +2)
7. New `scripts/demo-stage2.sh`: docker-compose postgres → migrate reset → boot → register/login → place stop order → admin inject mark price crossing stop → wal-dump shows OrderFilled → admin funding-tick → wal-dump shows FundingRateUpdate → ^C → reboot → server logs `WAL replay complete` with the mark-price + funding events counted → health check 200.

## 8.5 Rollback to Stage 1b (CEO review C3)

Rolling back Stage 2 is asymmetric with reading old WAL forward. Stage 2's `Event` enum is unchanged, so a Stage 2 binary replays a Stage 1b-era WAL fine. The reverse does NOT hold: Stage 1b's `apply_event` rejects `MarkPriceUpdate` / `FundingRateUpdate` with `ApplyError::UnexpectedVariant` → boot panic. So once Stage 2 has written any `MarkPriceUpdate` or `FundingRateUpdate` event to the WAL, reverting to Stage 1b code requires:

1. Stop the Stage 2 server (clean shutdown — both HTTP servers drain, matching thread joins).
2. `git revert <merge-commit>` to put Stage 1b code back.
3. `rm -rf data/wal` — Stage 1b cannot replay Stage 2 events. **All open orders are lost**; acceptable in dev (no production data).
4. Restart.

Symmetric to Stage 1b spec §9.6. A production-grade rollback (forward-compatible `apply_event` that skips unknown variants for one major version) is out of scope until Stage 5+; tracked in the forward pointers below.

## 9. Forward pointers (Stage 3+)

- **Funding settlement**: per-interval debit/credit of user funds. Needs position tracking + ledger.
- **Periodic funding timer**: production `funding_interval_hours` cadence (currently admin-triggered for deterministic tests).
- **External price feed / oracle**: HTTP/WS pull replacing admin injection.
- **TWAP + impact-depth premium index**: `cfg.risk.impact_notional`-based impact-bid/ask premium replacing instantaneous `(mark-index)/index`.
- **Multi-symbol mark price**: per-symbol price feeds when the single-symbol invariant (Stage 0 #1) is lifted.
- **Admin auth hardening**: shared-secret → mTLS / signed requests / per-operator audit log when admin surface grows.
- **Mark-price-triggered stop cascade is unbounded** (CEO review C2): one admin mark-price inject that crosses N resting stop orders triggers O(N) stop matches synchronously on the matching thread, all WAL-appended in one batch. Negligible at Stage 2 dev scale (few stops); a flash-crash-magnitude inject at production scale needs a circuit breaker / batched-trigger throttle. The single-symbol, admin-only, loopback constraints keep this benign until external feeds + multi-symbol land.
- **Forward-compatible replay for rollback** (CEO review C3): an `apply_event` that skips (rather than panics on) unknown event variants for one major version would make Stage N→N-1 rollback not require a WAL wipe. Defer until production traffic makes WAL preservation across rollback mandatory.

---

## Appendix A — Decisions log (from brainstorming)

| # | Decision | Choice |
|---|----------|--------|
| 1 | Scope boundary | mark price + funding rate calc, NO settlement |
| 2 | Mark price → engine path | new `Command::UpdateMarkPrice` through ring buffer |
| 3 | Mark/index price source | admin REST endpoint manual injection |
| 4 | Funding compute trigger | admin endpoint on-demand (`/admin/funding-tick`) |
| 5 | `apply_event(MarkPriceUpdate)` replay semantics | passive/active split; replay = passive only |
| 6 | premium_index formula | instantaneous `(mark - index) / index` |
| 7 | Admin endpoint placement/auth | independent `admin_port` 9090 + `X-Admin-Secret` shared-secret |
| 8 | Admin code location | `admin` module inside `exg-api-gateway` (no new crate) |
