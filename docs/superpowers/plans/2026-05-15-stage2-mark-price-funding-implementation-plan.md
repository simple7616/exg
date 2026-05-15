# Stage 2 — Mark Price + Funding Rate Service Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the static config mark price with an admin-driven dynamic mark/index price and an on-demand funding-rate computation, both flowing through the WAL command path and replaying correctly.

**Architecture:** Two new ring-buffer commands (`UpdateMarkPrice`, `ComputeFunding`) produced by an authenticated admin HTTP server on a separate port (9090). `engine.update_mark_price` is split into a passive half (set price + trailing peaks, reused by replay) and an active half (trigger + match, live-only). `apply_event` learns the `MarkPriceUpdate` (passive) and `FundingRateUpdate` arms, removing the Stage 1b `UnexpectedVariant` rejections. No `Event` schema change, so Stage 1b WAL files stay replayable.

**Tech Stack:** Rust 2024 workspace · actix-web (second HttpServer for admin) · rkyv (ring-buffer/WAL) · `subtle` crate (constant-time secret compare — NEW workspace dep) · sqlx + PostgreSQL on host port 5433 · `#[sqlx::test]` for e2e.

**Branch:** `feat/stage2-mark-price-funding` (HEAD `9d3c825`, 1 commit beyond `main` = spec).

**Spec:** [docs/superpowers/specs/2026-05-15-stage2-mark-price-funding-design.md](../specs/2026-05-15-stage2-mark-price-funding-design.md)

---

## File Structure

### New files

| Path | Responsibility |
|------|----------------|
| `crates/exg-api-gateway/src/admin.rs` | `X-Admin-Secret` middleware + `admin_mark_price` / `admin_funding_tick` handlers + `build_admin_app` |
| `crates/exg-server/tests/stage2_e2e.rs` | 10 integration tests (admin inject, funding tick, auth, port isolation, replay) |
| `scripts/demo-stage2.sh` | Cold-boot demo: place stop → admin inject mark crosses stop → wal-dump fill → funding-tick → wal-dump rate → reboot replay |

### Modified files

| Path | Change |
|------|--------|
| `crates/exg-protocol/src/command.rs` | + `Command::UpdateMarkPrice` + `Command::ComputeFunding` |
| `crates/exg-protocol/src/lib.rs` | test helper `all_commands()` (if present) gains the 2 variants |
| `crates/exg-config/src/lib.rs` | + `AdminConfig { admin_secret }`; `ExgConfig.admin`; `default_config()` placeholder |
| `crates/exg-config/src/validation.rs` | + admin_secret length + placeholder checks |
| `config/default.toml` | + `[admin]` section |
| `crates/exg-matching-engine/src/engine.rs` | `MatchingEngine::new` + `interest_rate` param; `last_funding_rate` field; `apply_mark_index_passive` + `trigger_and_match_stops` split; `compute_funding`; `set_last_funding_rate` accessor; `process_command` dispatch + 2 commands; `restore_from_snapshot` +1 param; all `test_engine`/`MatchingEngine::new` test sites |
| `crates/exg-matching-engine/src/snapshot.rs` | `EngineSnapshot.last_funding_rate` round-trip |
| `crates/exg-matching-engine/src/replay.rs` | `apply_event` `MarkPriceUpdate` + `FundingRateUpdate` arms; remove 2 `UnexpectedVariant`; `test_engine` signature; new unit tests |
| `crates/exg-api-gateway/src/types.rs` | + `AdminMarkPriceRequest` |
| `crates/exg-api-gateway/src/lib.rs` | `pub mod admin;` |
| `crates/exg-api-gateway/Cargo.toml` | + `subtle` dep (if not workspace) |
| `Cargo.toml` (workspace) | + `subtle` to `[workspace.dependencies]` |
| `crates/exg-server/src/lib.rs` | invariant #24/#25; `MatchingEngine::new` interest_rate threading; 2nd `HttpServer` on admin_port; `ServerHandle` dual-server shutdown |
| `crates/exg-server/tests/boot_panics.rs` | + 2 admin_secret boot panic tests |

### Test surface

- **Unit** (exg-matching-engine engine.rs + replay.rs): ~11 tests (passive/active split, compute_funding, snapshot round-trip, replay arms).
- **Integration** (exg-server stage2_e2e.rs): 10 tests.
- **boot_panics**: net +2 (9 → 11).
- **Regression baselines unchanged**: stage0_e2e 7/7, stage1a_e2e 12/12, stage1b_e2e 16/16, exg-user-service 30/30. The `MatchingEngine::new` signature change mechanically touches every `test_engine()` helper — that is a compile cascade, not a behavior change; those suites must stay green.

Workspace total target: current ~468 + ~23 ≈ ~491.

---

## Task overview

| # | Task | Files | Tests added |
|---|------|-------|-------------|
| 1 | Command variants + AdminConfig + config validation + toml | command.rs, protocol lib.rs, config lib.rs, validation.rs, default.toml | config unit tests |
| 2 | `MatchingEngine::new` + `interest_rate` cross-cutting cascade | engine.rs (sig + all test sites + restore_from_snapshot), replay.rs test_engine | 0 new (cascade only) |
| 3 | engine: passive/active split + compute_funding + dispatch | engine.rs | ~6 unit |
| 4 | EngineSnapshot last_funding_rate round-trip | snapshot.rs, engine.rs | 1 unit |
| 5 | replay.rs apply_event MarkPriceUpdate + FundingRateUpdate | replay.rs | ~4 unit |
| 6 | admin module (middleware + handlers + build_admin_app) | admin.rs (NEW), types.rs, api-gateway lib.rs, Cargo.toml, workspace Cargo.toml | 0 new (covered in T8) |
| 7 | server: invariants 24/25 + interest_rate threading + 2nd HttpServer + dual shutdown | server lib.rs | 0 new (covered in T8) |
| 8 | stage2_e2e (11) + boot_panics (2) + demo + final acceptance | stage2_e2e.rs (NEW), boot_panics.rs, demo-stage2.sh (NEW) | 11 e2e + 2 boot |

Strict execution order — each task depends on the previous. T1→T2 (config/command before engine), T2→T3 (signature cascade green before logic), T3→T4→T5 (engine logic before snapshot/replay), T6 independent of T2-5 but T7 depends on T1+T3+T6, T8 last.

---

## Task 1: Command variants + AdminConfig + config validation

**Files:**
- Modify: `crates/exg-protocol/src/command.rs`
- Modify: `crates/exg-protocol/src/lib.rs` (test helper, if it has `all_commands()`)
- Modify: `crates/exg-config/src/lib.rs`
- Modify: `crates/exg-config/src/validation.rs`
- Modify: `config/default.toml`

### Why this is one task

The `Command` enum change forces `process_command`'s match to be non-exhaustive (compile error) until Task 3 adds the arms — that is expected and acceptable mid-plan because Task 1 only adds the variants + config; Task 3 adds the dispatch. To keep Task 1 self-contained and compiling, Task 1 adds a temporary `Command::UpdateMarkPrice { .. } | Command::ComputeFunding { .. } => Vec::new()` catch-all in `process_command` that Task 3 replaces. Config + command schema land together because the admin handler (Task 6) needs both.

- [ ] **Step 1: Add the two Command variants**

In `crates/exg-protocol/src/command.rs`, inside `pub enum Command { ... }`, after the `CancelAllOrders { ... }` variant, add:

```rust
    /// Stage 2: admin-injected mark/index price. Drives stop/trailing
    /// triggering + funding premium. Produced by the admin HTTP server.
    UpdateMarkPrice {
        symbol: SymbolId,
        mark_price: Decimal128,
        index_price: Decimal128,
        timestamp: UnixMicros,
    },
    /// Stage 2: admin-triggered funding rate computation.
    ComputeFunding {
        symbol: SymbolId,
        timestamp: UnixMicros,
    },
```

`SymbolId`, `Decimal128`, `UnixMicros` are already imported in command.rs (used by existing variants).

- [ ] **Step 2: Add a temporary catch-all in `process_command`**

In `crates/exg-matching-engine/src/engine.rs`, the `process_command` match (around line 49-108) ends with the `Command::CancelAllOrders { ... } => self.handle_cancel_all(...)` arm. Add immediately after it, before the closing `}` of the match:

```rust
            // Stage 2 Task 1 placeholder — replaced with real dispatch in Task 3.
            Command::UpdateMarkPrice { .. } | Command::ComputeFunding { .. } => Vec::new(),
```

(This keeps the workspace compiling between Task 1 and Task 3. Task 3 deletes this arm and adds the real dispatch.)

- [ ] **Step 3: Update protocol test helper if present**

```bash
grep -n "fn all_commands\|all_commands()" crates/exg-protocol/src/lib.rs
```

If `all_commands()` exists (a Vec of every Command variant for rkyv round-trip tests), append two entries:

```rust
            Command::UpdateMarkPrice {
                symbol: SymbolId::new(1),
                mark_price: dec("60000"),
                index_price: dec("59950"),
                timestamp: sample_timestamp(),
            },
            Command::ComputeFunding {
                symbol: SymbolId::new(1),
                timestamp: sample_timestamp(),
            },
```

(Use the same `dec` / `sample_timestamp` helpers the file already uses. If `all_commands()` does not exist, skip this step.)

- [ ] **Step 4: Add `AdminConfig` to exg-config**

In `crates/exg-config/src/lib.rs`, near the `AuthConfig` struct, add:

```rust
#[derive(Debug, Clone, serde::Deserialize)]
pub struct AdminConfig {
    pub admin_secret: String,
}
```

Add to `ExgConfig`:

```rust
    pub admin: AdminConfig,
```

(Place it after the `pub auth: AuthConfig,` field — match the field ordering with the `[admin]` toml section order.)

In `default_config()` (around line 175-210), add to the constructed `ExgConfig`:

```rust
            admin: AdminConfig {
                admin_secret: "CHANGE-ME-ADMIN-DEV-ONLY-MUST-BE-32-BYTES".into(),
            },
```

- [ ] **Step 5: Add the `[admin]` section to config/default.toml**

In `config/default.toml`, after the `[auth]` section, add:

```toml
[admin]
admin_secret = "CHANGE-ME-ADMIN-DEV-ONLY-MUST-BE-32-BYTES"
```

- [ ] **Step 6: Add validation (invariants #24/#25)**

In `crates/exg-config/src/validation.rs::validate`, after the JWT secret block (around line 105-116), add:

```rust
    // Stage 2 §6 invariant 24/25: admin secret length + placeholder check.
    const ADMIN_SECRET_PLACEHOLDER: &str = "CHANGE-ME-ADMIN-DEV-ONLY-MUST-BE-32-BYTES";
    if cfg.admin.admin_secret.len() < 32 {
        return Err(ConfigError::Validation(format!(
            "admin.admin_secret must be at least 32 bytes, got {}",
            cfg.admin.admin_secret.len()
        )));
    }
    if cfg.admin.admin_secret == ADMIN_SECRET_PLACEHOLDER {
        return Err(ConfigError::Validation(
            "admin.admin_secret is the placeholder default; set EXG_ADMIN_SECRET env var to a 32+ byte production secret".into(),
        ));
    }
```

Confirm the placeholder string is exactly 41 chars (`CHANGE-ME-ADMIN-DEV-ONLY-MUST-BE-32-BYTES`) — ≥ 32, so only the placeholder check (not the length check) fires for the default. That is the intended behavior (default config fails validation on the placeholder check, same as Stage 1a jwt_secret).

- [ ] **Step 7: Write a config validation unit test**

In `crates/exg-config/src/validation.rs` test module (or wherever config tests live — `grep -n "mod tests" crates/exg-config/src/validation.rs crates/exg-config/src/lib.rs`), add:

```rust
    #[test]
    fn admin_secret_placeholder_rejected() {
        // Eng review: must advance past the JWT check first — default_config
        // has BOTH placeholders and jwt_secret is validated first, so a
        // `|| jwt_secret` disjunction would short-circuit and never exercise
        // invariant 25. Set a valid jwt_secret, then assert ONLY the admin
        // message so the test genuinely fails if #25 is absent.
        let mut cfg = ExgConfig::default_config();
        cfg.auth.jwt_secret = "a".repeat(32);
        let err = cfg.validate().unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("admin.admin_secret"),
            "expected admin secret placeholder rejection, got: {msg}"
        );
    }

    #[test]
    fn admin_secret_too_short_rejected() {
        let mut cfg = ExgConfig::default_config();
        cfg.auth.jwt_secret = "a".repeat(32); // pass JWT check first
        cfg.admin.admin_secret = "short".into();
        let err = cfg.validate().unwrap_err();
        assert!(format!("{err}").contains("admin.admin_secret must be at least 32"));
    }

    #[test]
    fn admin_secret_valid_passes() {
        let mut cfg = ExgConfig::default_config();
        cfg.auth.jwt_secret = "a".repeat(32);
        cfg.admin.admin_secret = "b".repeat(32);
        assert!(cfg.validate().is_ok());
    }
```

(If the test module imports differ, match the file's existing test imports. `ExgConfig` may need `use crate::ExgConfig;`.)

- [ ] **Step 8: Verify**

```bash
cargo check --workspace 2>&1 | tail -5
cargo test -p exg-config -p exg-protocol 2>&1 | tail -10
```

Expected: clean compile; config tests green (incl. the 3 new). Existing tests that call `ExgConfig::default_config()` then `validate()` may now fail on the admin placeholder — search and fix them to set `cfg.admin.admin_secret = "b".repeat(32)` alongside any existing `cfg.auth.jwt_secret` override:

```bash
grep -rn "default_config()" crates/ | grep -i "validate\|jwt_secret" | head
```

Any test that already overrides `jwt_secret` to pass validation must also override `admin_secret`. This is the same cascade Stage 1a created for jwt_secret.

- [ ] **Step 9: Commit**

```bash
git add crates/exg-protocol/src/command.rs crates/exg-protocol/src/lib.rs \
        crates/exg-config/src/lib.rs crates/exg-config/src/validation.rs \
        config/default.toml crates/exg-matching-engine/src/engine.rs
git commit -m "$(cat <<'EOF'
feat(protocol,config): add Stage 2 mark-price/funding commands + AdminConfig

- Command::UpdateMarkPrice + Command::ComputeFunding variants (temporary
  no-op dispatch in process_command, replaced in Task 3)
- AdminConfig { admin_secret } + [admin] toml section
- validation invariants 24/25: admin_secret length >= 32 + not placeholder
  (same pattern as Stage 1a jwt_secret 11/12)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: `MatchingEngine::new` + `interest_rate` cross-cutting cascade

**Files:**
- Modify: `crates/exg-matching-engine/src/engine.rs` (`new` sig + struct field + `restore_from_snapshot` sig + every internal test site)
- Modify: `crates/exg-matching-engine/src/replay.rs` (`test_engine` helper)

### Why this is one task

`MatchingEngine::new(symbol_config, node_id)` gains a 3rd param `interest_rate: Decimal128`. Rust will not compile until **every** call site is updated in the same change. The complete, workspace-wide call-site set (grep-verified, Eng review E2/E3) is:

1. **Production caller** — `crates/exg-server/src/lib.rs:219` (2-arg today; fixed in Task 7, but the signature must be consistent).
2. **`restore_from_snapshot`** (`engine.rs:995`) — calls `Self::new`; gains a 4th param.
3. **`deserialize_snapshot`** (`crates/exg-matching-engine/src/snapshot.rs:29-37`) — a `pub`, **non-test**, in-crate wrapper that calls `Self::restore_from_snapshot(snapshot, config, node_id)` (3 args). Eng review E3: this is `restore_from_snapshot`'s non-test in-crate caller (the "test-only" wording was incomplete) — it must also gain + forward an `interest_rate` param or it will not compile.
4. **engine.rs unit tests** — 19 `MatchingEngine::new(test_config(), 1)` sites + 1 `restore_from_snapshot(snapshot, test_config(), 1)` site (engine.rs:1576).
5. **replay.rs** — `test_engine()` (replay.rs:195) + any other `MatchingEngine::new(` in that file.
6. **`crates/exg-matching-engine/benches/matching.rs:53, 73, 125`** — 3 `MatchingEngine::new(test_config(), 1)` sites. Eng review E2: `cargo check/test/clippy --workspace` do **not** compile benches (need `--all-targets`/`--benches`), so omitting these passes Task 8 acceptance green while `scripts/bench.sh` / `cargo bench` / `scripts/test.sh --all` are broken — the exact undocumented-downstream-callsite class Stage 1b's eng review caught. The bench file does **not** import a `dec` helper; use `"0.0001".parse().unwrap()` inline (its `Decimal128` is already in scope via `test_config`).

Splitting this across tasks leaves the workspace red. Task 2 does ONLY the mechanical cascade + stores the field; Task 3 uses it.

- [ ] **Step 1: Add the struct field**

In `crates/exg-matching-engine/src/engine.rs`, in `pub struct MatchingEngine { ... }`, add after `sequence: u64,`:

```rust
    /// Stage 2: clamp interest rate for funding (from cfg.risk.interest_rate).
    interest_rate: Decimal128,
    /// Stage 2: last computed funding rate. ZERO until first ComputeFunding.
    last_funding_rate: Decimal128,
```

- [ ] **Step 2: Update `MatchingEngine::new` signature + body**

Replace:

```rust
    pub fn new(symbol_config: SymbolConfig, node_id: u16) -> Self {
        let symbol = symbol_config.symbol;
        Self {
            orderbook: OrderBook::new(symbol),
            symbol_config,
            mark_price: Decimal128::ZERO,
            index_price: Decimal128::ZERO,
            stop_orders: Vec::new(),
            expiry_heap: BinaryHeap::new(),
            trade_id_gen: SnowflakeGen::new(node_id),
            sequence: 0,
        }
    }
```

with:

```rust
    pub fn new(
        symbol_config: SymbolConfig,
        node_id: u16,
        interest_rate: Decimal128,
    ) -> Self {
        let symbol = symbol_config.symbol;
        Self {
            orderbook: OrderBook::new(symbol),
            symbol_config,
            mark_price: Decimal128::ZERO,
            index_price: Decimal128::ZERO,
            stop_orders: Vec::new(),
            expiry_heap: BinaryHeap::new(),
            trade_id_gen: SnowflakeGen::new(node_id),
            sequence: 0,
            interest_rate,
            last_funding_rate: Decimal128::ZERO,
        }
    }
```

At this point `interest_rate` and `last_funding_rate` are stored but unread. Rust warns "field never read". Add `#[allow(dead_code)]` on **both** fields temporarily with a comment `// Task 2: stored; consumed in Task 3`. Task 3 removes the allow.

- [ ] **Step 3: Update `restore_from_snapshot` signature**

Find `pub fn restore_from_snapshot(snapshot, config, node_id) -> Self` (around line 995). Change to:

```rust
    pub fn restore_from_snapshot(
        snapshot: EngineSnapshot,
        config: SymbolConfig,
        node_id: u16,
        interest_rate: Decimal128,
    ) -> Self {
        let mut engine = Self::new(config, node_id, interest_rate);
        // Leave every line below this point exactly as it is today (the
        // `engine.mark_price = snapshot.mark_price;` ... block). Only the
        // signature and this single `Self::new(...)` call gain the new
        // `interest_rate` argument. Task 4 adds one more line here.
```

(The snapshot does NOT carry `interest_rate` — it is config, not engine state. The caller supplies it. `restore_from_snapshot` is test-only at runtime since snapshot is unused per Stage 1b — *but* it has one non-test in-crate caller, `deserialize_snapshot`, fixed in the next step.)

- [ ] **Step 3b: Thread `interest_rate` through `deserialize_snapshot` (Eng review E3)**

In `crates/exg-matching-engine/src/snapshot.rs`, `deserialize_snapshot` (lines 29-37) currently is:

```rust
    pub fn deserialize_snapshot(
        data: &[u8],
        config: exg_risk_engine::SymbolConfig,
        node_id: u16,
    ) -> Self {
        let snapshot: EngineSnapshot =
            serde_json::from_slice(data).expect("snapshot deserialization failed");
        Self::restore_from_snapshot(snapshot, config, node_id)
    }
```

Change to:

```rust
    pub fn deserialize_snapshot(
        data: &[u8],
        config: exg_risk_engine::SymbolConfig,
        node_id: u16,
        interest_rate: Decimal128,
    ) -> Self {
        let snapshot: EngineSnapshot =
            serde_json::from_slice(data).expect("snapshot deserialization failed");
        Self::restore_from_snapshot(snapshot, config, node_id, interest_rate)
    }
```

`Decimal128` is already imported in snapshot.rs (`EngineSnapshot.mark_price` etc.). Grep `deserialize_snapshot(` workspace-wide — there are **zero external callers** (verified: only the definition), so no further cascade beyond this signature; this fix is purely to keep the crate compiling.

- [ ] **Step 4: Update every test call site in engine.rs**

```bash
grep -n "MatchingEngine::new(\|restore_from_snapshot(" crates/exg-matching-engine/src/engine.rs
```

For each `MatchingEngine::new(test_config(), 1)` → `MatchingEngine::new(test_config(), 1, dec("0.0001"))`. For each `restore_from_snapshot(snap, cfg, 1)` → `restore_from_snapshot(snap, cfg, 1, dec("0.0001"))`. The `dec` helper already exists in engine.rs tests; if a test uses a different decimal helper name, match it. Use `"0.0001"` (the config default interest rate) consistently.

- [ ] **Step 5: Update `replay.rs` `test_engine()` helper**

In `crates/exg-matching-engine/src/replay.rs` tests, `test_engine()` calls `MatchingEngine::new(cfg, 1)`. Change to `MatchingEngine::new(cfg, 1, dec("0.0001"))` (the `dec` helper exists in replay.rs tests).

Also any other `MatchingEngine::new(` in replay.rs (e.g. `replay_then_take_snapshot_round_trip` builds two engines). Grep and fix all:

```bash
grep -n "MatchingEngine::new(" crates/exg-matching-engine/src/replay.rs
```

- [ ] **Step 5b: Update `benches/matching.rs` (Eng review E2)**

```bash
grep -n "MatchingEngine::new(" crates/exg-matching-engine/benches/matching.rs
```

Three sites (53, 73, 125): `MatchingEngine::new(test_config(), 1)` → `MatchingEngine::new(test_config(), 1, "0.0001".parse().unwrap())`. The bench file has no `dec` helper; the inline `"0.0001".parse().unwrap()` yields `Decimal128` (already in scope via `test_config`). Benches are NOT built by `cargo check/test/clippy --workspace`, so this must be verified with `--all-targets` in Step 6.

- [ ] **Step 6: Verify the cascade is complete**

```bash
cargo check --workspace --all-targets 2>&1 | tail -15
```

`--all-targets` is mandatory here (Eng review E2) — it is the only invocation that compiles `benches/matching.rs`; a plain `cargo check --workspace` would hide a broken bench. Expected: the ONLY remaining error is in `crates/exg-server/src/lib.rs` (boot calls `MatchingEngine::new(symbol_config, node_id)` with 2 args) — that is fixed in Task 7. Note it and proceed. If errors appear anywhere ELSE (esp. `benches/matching.rs` or `snapshot.rs`), the cascade is incomplete — fix them now.

```bash
cargo test -p exg-matching-engine --lib 2>&1 | tail -8
```

If exg-matching-engine compiles standalone (it does not depend on exg-server), its tests should run green (the new fields are inert). 61 + existing all pass.

- [ ] **Step 7: Commit**

```bash
git add crates/exg-matching-engine/src/engine.rs crates/exg-matching-engine/src/replay.rs \
        crates/exg-matching-engine/src/snapshot.rs crates/exg-matching-engine/benches/matching.rs
git commit -m "$(cat <<'EOF'
refactor(matching-engine): thread interest_rate into MatchingEngine::new

Stage 2 funding needs cfg.risk.interest_rate at the engine. Add it as a
3rd MatchingEngine::new param + last_funding_rate field (both inert until
Task 3). restore_from_snapshot + deserialize_snapshot (its non-test
in-crate caller, Eng review E3) gain a matching 4th param (snapshot does
not carry interest_rate — it is config, caller supplies). All engine.rs +
replay.rs + benches/matching.rs (Eng review E2) call sites cascaded;
verified with cargo check --workspace --all-targets. exg-server boot
caller fixed in Task 7.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: engine — passive/active split + compute_funding + dispatch

**Files:**
- Modify: `crates/exg-matching-engine/src/engine.rs`

### Step 1: Read the current `update_mark_price`

```bash
grep -n "pub fn update_mark_price\|fn update_trailing_peaks\|fn check_stop_triggers_internal" crates/exg-matching-engine/src/engine.rs
sed -n '731,790p' crates/exg-matching-engine/src/engine.rs
```

Confirm the current shape: `pub fn update_mark_price(&mut self, mark_price, index_price) -> Vec<Event>` does (a) set `self.mark_price`/`self.index_price`, (b) `self.update_trailing_peaks()`, (c) `check_stop_triggers_internal()` + per-order match loop, returning only the triggered fill events (no `MarkPriceUpdate` event today).

- [ ] **Step 2: Add the failing unit tests first (TDD red)**

In the engine.rs `#[cfg(test)] mod tests`, add:

```rust
    #[test]
    fn update_mark_price_passive_sets_price_and_peaks_no_fills() {
        let mut engine = MatchingEngine::new(test_config(), 1, dec("0.0001"));
        engine.set_mark_price(dec("60000"));
        // No resting stop orders → no fills, just price set.
        let events = engine.update_mark_price(
            SymbolId::new(1),
            dec("61000"),
            dec("60950"),
            sample_ts(),
        );
        assert_eq!(engine.mark_price(), dec("61000"));
        // First event must be MarkPriceUpdate; no OrderFilled with empty book.
        assert!(matches!(events[0], Event::MarkPriceUpdate { .. }));
        assert!(!events.iter().any(|e| matches!(e, Event::OrderFilled { .. })));
    }

    #[test]
    fn compute_funding_positive_premium() {
        let mut engine = MatchingEngine::new(test_config(), 1, dec("0.0001"));
        engine.update_mark_price(SymbolId::new(1), dec("60600"), dec("60000"), sample_ts());
        let events = engine.compute_funding(SymbolId::new(1), sample_ts());
        // premium = (60600-60000)/60000 = 0.01 ; rate = clamp(0.01 + 0.0001, ±0.0075) = 0.0075
        match &events[0] {
            Event::FundingRateUpdate { funding_rate, .. } => {
                assert_eq!(*funding_rate, dec("0.0075")); // clamped upper
            }
            _ => panic!("expected FundingRateUpdate"),
        }
        assert_eq!(engine.last_funding_rate(), dec("0.0075"));
    }

    #[test]
    fn compute_funding_negative_premium() {
        let mut engine = MatchingEngine::new(test_config(), 1, dec("0.0001"));
        engine.update_mark_price(SymbolId::new(1), dec("59400"), dec("60000"), sample_ts());
        let events = engine.compute_funding(SymbolId::new(1), sample_ts());
        // premium = (59400-60000)/60000 = -0.01 ; rate = clamp(-0.01+0.0001, ±0.0075) = -0.0075
        match &events[0] {
            Event::FundingRateUpdate { funding_rate, .. } => {
                assert_eq!(*funding_rate, dec("-0.0075"));
            }
            _ => panic!("expected FundingRateUpdate"),
        }
    }

    #[test]
    fn compute_funding_zero_index_no_panic() {
        let mut engine = MatchingEngine::new(test_config(), 1, dec("0.0001"));
        // index_price stays ZERO (never set a positive index).
        let events = engine.compute_funding(SymbolId::new(1), sample_ts());
        // premium = ZERO (div-by-zero guard) ; rate = clamp(0 + 0.0001) = 0.0001
        match &events[0] {
            Event::FundingRateUpdate { funding_rate, .. } => {
                assert_eq!(*funding_rate, dec("0.0001"));
            }
            _ => panic!("expected FundingRateUpdate"),
        }
    }

    #[test]
    fn process_command_update_mark_price_dispatches() {
        let mut engine = MatchingEngine::new(test_config(), 1, dec("0.0001"));
        let events = engine.process_command(&Command::UpdateMarkPrice {
            symbol: SymbolId::new(1),
            mark_price: dec("62000"),
            index_price: dec("61900"),
            timestamp: sample_ts(),
        });
        assert_eq!(engine.mark_price(), dec("62000"));
        assert!(matches!(events[0], Event::MarkPriceUpdate { .. }));
    }

    #[test]
    fn process_command_compute_funding_dispatches() {
        let mut engine = MatchingEngine::new(test_config(), 1, dec("0.0001"));
        engine.process_command(&Command::UpdateMarkPrice {
            symbol: SymbolId::new(1),
            mark_price: dec("60300"),
            index_price: dec("60000"),
            timestamp: sample_ts(),
        });
        let events = engine.process_command(&Command::ComputeFunding {
            symbol: SymbolId::new(1),
            timestamp: sample_ts(),
        });
        assert!(matches!(events[0], Event::FundingRateUpdate { .. }));
    }
```

If the engine.rs test module lacks a `sample_ts()` helper, add `fn sample_ts() -> UnixMicros { UnixMicros::from_micros(1_700_000_000_000_000) }` to the test module (or reuse whatever timestamp helper exists — grep first).

- [ ] **Step 3: Run tests — verify they fail (red)**

```bash
cargo test -p exg-matching-engine --lib 2>&1 | grep -E "compute_funding|update_mark_price_passive|process_command_(update_mark|compute)" | tail
```

Expected: compile errors (`compute_funding` / `last_funding_rate` not found, `update_mark_price` signature mismatch). That is the red state.

- [ ] **Step 4: Implement the passive/active split + compute_funding + accessors**

In `crates/exg-matching-engine/src/engine.rs`, remove the `#[allow(dead_code)]` added in Task 2 on `interest_rate` / `last_funding_rate`. Replace the existing `pub fn update_mark_price(&mut self, mark_price, index_price) -> Vec<Event>` with:

```rust
    /// Passive: set mark/index price + reconstruct trailing-peak state.
    /// Reused by the live path (first half) AND replay. No triggering,
    /// no matching — Stage 2 §3 passive/active split.
    pub(crate) fn apply_mark_index_passive(
        &mut self,
        mark: Decimal128,
        index: Decimal128,
    ) {
        self.mark_price = mark;
        self.index_price = index;
        self.update_trailing_peaks();
    }

    /// Live path (process_command → Command::UpdateMarkPrice). Emits
    /// MarkPriceUpdate first, then any OrderFilled/TradeExecuted from
    /// triggered stop/take-profit/trailing orders.
    pub fn update_mark_price(
        &mut self,
        symbol: SymbolId,
        mark: Decimal128,
        index: Decimal128,
        timestamp: UnixMicros,
    ) -> Vec<Event> {
        self.apply_mark_index_passive(mark, index);
        let mut events = vec![Event::MarkPriceUpdate {
            symbol,
            mark_price: mark,
            index_price: index,
            timestamp,
        }];
        events.extend(self.trigger_and_match_stops(timestamp));
        events
    }

    /// Active half: check stop triggers + run them through the matcher.
    /// Live-only. (Body moved from the old update_mark_price tail.)
    fn trigger_and_match_stops(&mut self, timestamp: UnixMicros) -> Vec<Event> {
        let mut events = Vec::new();
        let mut triggered = self.check_stop_triggers_internal();
        for mut order in triggered.drain(..) {
            match order.order_type {
                OrderType::StopMarket
                | OrderType::TakeProfitMarket
                | OrderType::TrailingStop => {
                    order.order_type = OrderType::Market;
                    order.price = match order.side {
                        Side::Buy => Decimal128::MAX,
                        Side::Sell => Decimal128::ZERO,
                    };
                }
                OrderType::StopLimit | OrderType::TakeProfitLimit => {
                    order.order_type = OrderType::Limit;
                }
                _ => {}
            }
            let match_result = matcher::match_order(&mut self.orderbook, &mut order);
            if !match_result.rejected {
                self.emit_fill_events(
                    &mut events,
                    &match_result.fills,
                    order.symbol,
                    timestamp,
                    &order,
                );
                if !order.remaining_qty.is_zero() && order.order_type.is_limit() {
                    self.orderbook.insert_order(order);
                }
            }
        }
        events
    }

    /// Stage 2: compute funding rate from the instantaneous premium.
    /// premium = (mark - index) / index ; ZERO when index == 0
    /// (div-by-zero guard — invariant 28). rate via risk-engine clamp.
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
        vec![Event::FundingRateUpdate {
            symbol,
            funding_rate: rate,
            timestamp,
        }]
    }

    /// Last funding rate (observability / snapshot / replay).
    pub fn last_funding_rate(&self) -> Decimal128 {
        self.last_funding_rate
    }

    /// Replay-only accessor — set last_funding_rate during apply_event.
    #[doc(hidden)]
    pub fn set_last_funding_rate(&mut self, rate: Decimal128) {
        self.last_funding_rate = rate;
    }
```

Notes for the implementer:
- The old `update_mark_price` body's set+peaks prelude moves into `apply_mark_index_passive`; its trigger+match tail moves into `trigger_and_match_stops`. Verify the moved code matches the original (the snippet above mirrors engine.rs:736-784 — re-read and preserve any detail like the `match_result.rejected` check).
- `apply_mark_index_passive` is `pub(crate)` so `replay.rs` (same crate, different module) can call it. `set_last_funding_rate` is `#[doc(hidden)] pub` like Stage 1b's `orderbook_mut`.
- `exg_risk_engine::funding::calc_funding_rate` — confirm `exg-matching-engine/Cargo.toml` already depends on `exg-risk-engine` (it does — engine uses SymbolConfig from it). If the path differs, `grep -n "exg_risk_engine" crates/exg-matching-engine/src/engine.rs`.
- **Eng review E4 (deliberate behavior delta):** the original `update_mark_price` body computed `let timestamp = UnixMicros::now();` *inside* the per-triggered-order loop (engine.rs:765). The refactor threads the caller's `timestamp` (the `Command::UpdateMarkPrice.timestamp`) into `trigger_and_match_stops` and uses it for `emit_fill_events`. This is intentional and *more* correct — WAL-deterministic, replay-aligned — and replay never runs the active half so it cannot diverge. Preserve this on purpose; do not "restore" a fresh `UnixMicros::now()` per order.

- [ ] **Step 5: Replace the Task 1 placeholder dispatch with real dispatch**

In `process_command`, replace:

```rust
            // Stage 2 Task 1 placeholder — replaced with real dispatch in Task 3.
            Command::UpdateMarkPrice { .. } | Command::ComputeFunding { .. } => Vec::new(),
```

with:

```rust
            Command::UpdateMarkPrice {
                symbol,
                mark_price,
                index_price,
                timestamp,
            } => self.update_mark_price(*symbol, *mark_price, *index_price, *timestamp),
            Command::ComputeFunding { symbol, timestamp } => {
                self.compute_funding(*symbol, *timestamp)
            }
```

`Command` is already imported in engine.rs.

- [ ] **Step 5b: Fix the existing `update_mark_price` test call sites (Eng review E1)**

The signature change from `update_mark_price(mark, index)` to `update_mark_price(symbol, mark, index, timestamp)` **plus** the unconditional `MarkPriceUpdate` prepend breaks the existing engine.rs tests both mechanically (arg count) and semantically (event-vec shape). This step is mandatory — without it the plan's "existing tests stay green" criterion is false.

```bash
grep -n "\.update_mark_price(dec(" crates/exg-matching-engine/src/engine.rs
```

Six call sites: **1441, 1456, 1486, 1490, 1494, 1571** (line numbers pre-edit; re-grep to confirm).

**Mechanical (all 6):** `engine.update_mark_price(dec("X"), dec("X"))` → `engine.update_mark_price(SymbolId::new(1), dec("X"), dec("X"), sample_ts())`. (`sample_ts()` is the helper added in Step 2; `SymbolId` already imported.)

**Semantic — `test_trailing_stop` (the test starting at engine.rs:1452):** it has two assertions that are now WRONG because `update_mark_price` always returns `vec![MarkPriceUpdate, ..]` (never empty):

```rust
        // Price goes up — peak should track
        let events = engine.update_mark_price(dec("52000"), dec("52000"));
        assert!(events.is_empty()); // no trigger yet               // ← NOW FALSE
        // Price drops but not enough
        let events = engine.update_mark_price(dec("51500"), dec("51500"));
        assert!(events.is_empty());                                  // ← NOW FALSE
```

Replace each `assert!(events.is_empty());` with an assertion on the *fill* property, not the empty property:

```rust
        let events = engine.update_mark_price(SymbolId::new(1), dec("52000"), dec("52000"), sample_ts());
        assert!(!events.iter().any(is_filled), "peak tracking only, no trigger yet");
```

```rust
        let events = engine.update_mark_price(SymbolId::new(1), dec("51500"), dec("51500"), sample_ts());
        assert!(!events.iter().any(is_filled), "still above trigger, no fill");
```

The final `update_mark_price(dec("51000"), ...)` in that test already asserts via `let fills: Vec<_> = events.iter().filter(|e| is_filled(e)).collect(); assert!(!fills.is_empty());` — that survives the prepend unchanged (only the arg-count mechanical fix applies). `is_filled` already exists in the engine.rs test module (used by the surrounding stop tests). The stop-trigger test at ~1441 uses `assert!(!events.is_empty())` + `is_filled` filtering — only the mechanical 4-arg fix is needed there (a non-empty vec stays non-empty). The two snapshot tests (1571, and `test_snapshot_serde_roundtrip` ~1588) discard the return value — mechanical fix only.

- [ ] **Step 6: Run tests — verify green**

```bash
cargo test -p exg-matching-engine --lib 2>&1 | tail -12
```

Expected: all 6 new tests pass; `test_trailing_stop` + the stop/snapshot tests pass with the Step 5b fixes; Stage 1b replay 19 still green (replay.rs `MatchingEngine::new` was cascaded in Task 2; its `apply_event` arms are untouched until Task 5). Note: `process_command` does `self.sequence += 1` at the top for every command including the 2 new — that is correct (each command is one WAL-sequenced action).

- [ ] **Step 7: Commit**

```bash
git add crates/exg-matching-engine/src/engine.rs
git commit -m "$(cat <<'EOF'
feat(matching-engine): mark-price passive/active split + compute_funding

- apply_mark_index_passive (set price + trailing peaks) — pub(crate),
  reused by replay (Stage 2 §3)
- update_mark_price now emits MarkPriceUpdate first, then triggered
  fills via trigger_and_match_stops (active, live-only)
- compute_funding: premium=(mark-index)/index (ZERO when index==0,
  invariant 28); calc_funding_rate clamp; stores last_funding_rate
- process_command dispatches UpdateMarkPrice + ComputeFunding (Task 1
  placeholder removed)
- last_funding_rate() + #[doc(hidden)] set_last_funding_rate() accessors

6 unit tests: passive no-fills, funding +/- premium, zero-index guard,
both command dispatches.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: EngineSnapshot last_funding_rate round-trip

**Files:**
- Modify: `crates/exg-matching-engine/src/snapshot.rs`
- Modify: `crates/exg-matching-engine/src/engine.rs` (`take_snapshot` / `restore_from_snapshot`)

### Step 1: Read current snapshot shape

```bash
grep -n "pub struct EngineSnapshot\|last_funding_rate\|fn take_snapshot\|fn restore_from_snapshot" crates/exg-matching-engine/src/snapshot.rs crates/exg-matching-engine/src/engine.rs
sed -n '/pub struct EngineSnapshot/,/^}/p' crates/exg-matching-engine/src/snapshot.rs
```

- [ ] **Step 2: Add the failing test (red)**

In engine.rs test module:

```rust
    #[test]
    fn snapshot_round_trips_last_funding_rate() {
        let mut engine = MatchingEngine::new(test_config(), 1, dec("0.0001"));
        engine.update_mark_price(SymbolId::new(1), dec("60600"), dec("60000"), sample_ts());
        engine.compute_funding(SymbolId::new(1), sample_ts());
        let saved = engine.last_funding_rate();
        assert_ne!(saved, Decimal128::ZERO);

        let snap = engine.take_snapshot();
        let restored = MatchingEngine::restore_from_snapshot(
            snap, test_config(), 1, dec("0.0001"),
        );
        assert_eq!(restored.last_funding_rate(), saved);
    }
```

```bash
cargo test -p exg-matching-engine --lib snapshot_round_trips_last_funding_rate 2>&1 | tail -5
```

Expected: fail (`EngineSnapshot` has no `last_funding_rate`; restored value is ZERO).

- [ ] **Step 3: Add the field to EngineSnapshot**

In `crates/exg-matching-engine/src/snapshot.rs`, add to `pub struct EngineSnapshot`:

```rust
    pub last_funding_rate: Decimal128,
```

(Place it next to `mark_price` / `index_price` if those are snapshot fields — keep related price/funding state together. `Decimal128` is already imported there.)

- [ ] **Step 4: Populate it in take_snapshot / restore_from_snapshot**

In `engine.rs::take_snapshot` (around line 980-990), add to the constructed `EngineSnapshot`:

```rust
            last_funding_rate: self.last_funding_rate,
```

In `engine.rs::restore_from_snapshot`, after the existing `engine.mark_price = snapshot.mark_price; engine.index_price = snapshot.index_price;` lines, add:

```rust
        engine.last_funding_rate = snapshot.last_funding_rate;
```

- [ ] **Step 5: Run — verify green**

```bash
cargo test -p exg-matching-engine --lib 2>&1 | tail -8
```

Expected: `snapshot_round_trips_last_funding_rate` passes; all existing snapshot tests + Stage 1b replay round-trip still green (the new field defaults to ZERO for engines that never computed funding, which matches old snapshots semantically).

- [ ] **Step 6: Commit**

```bash
git add crates/exg-matching-engine/src/snapshot.rs crates/exg-matching-engine/src/engine.rs
git commit -m "$(cat <<'EOF'
feat(matching-engine): snapshot round-trips last_funding_rate

EngineSnapshot gains last_funding_rate; take_snapshot captures it,
restore_from_snapshot restores it. Snapshot stays structurally complete
(Stage 1b discipline) even though it is unused at runtime.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: replay.rs apply_event MarkPriceUpdate + FundingRateUpdate

**Files:**
- Modify: `crates/exg-matching-engine/src/replay.rs`

### Step 1: Read current apply_event arms

```bash
grep -n "MarkPriceUpdate\|FundingRateUpdate\|LiquidationOrder\|UnexpectedVariant" crates/exg-matching-engine/src/replay.rs
```

Confirm Stage 1b currently has:

```rust
Event::MarkPriceUpdate { .. } => Err(ApplyError::UnexpectedVariant { variant: "MarkPriceUpdate" }),
Event::FundingRateUpdate { .. } => Err(ApplyError::UnexpectedVariant { variant: "FundingRateUpdate" }),
Event::LiquidationOrder { .. } => Err(ApplyError::UnexpectedVariant { variant: "LiquidationOrder" }),
```

- [ ] **Step 2: Add failing tests (red)**

In the replay.rs `#[cfg(test)] mod tests`, add:

```rust
    #[test]
    fn apply_event_mark_price_update_passive_only() {
        let mut engine = test_engine();
        // Resting stop-sell with stop_price 59000; if replay re-triggered,
        // a mark of 58000 (<= stop) would fire it and produce an OrderFilled.
        let accept = accept_event_full(
            7,
            OrderType::StopMarket,
            TimeInForce::Gtc,
            "1.0",
            "59000",
            None,
            None,
            None,
            Some(dec("59000")),
        );
        engine.apply_event(&accept).unwrap();
        // Replay a MarkPriceUpdate that WOULD trigger the stop if active.
        engine
            .apply_event(&Event::MarkPriceUpdate {
                symbol: SymbolId::new(1),
                mark_price: dec("58000"),
                index_price: dec("58000"),
                timestamp: ts(),
            })
            .unwrap();
        // Passive only: price set, stop NOT triggered (still in stop_orders,
        // no OrderFilled was produced because apply_event returns no events).
        assert_eq!(engine.mark_price(), dec("58000"));
        assert_eq!(engine.stop_orders_mut().len(), 1, "stop must NOT trigger on replay");
    }

    #[test]
    fn apply_event_funding_rate_update_sets_last_rate() {
        let mut engine = test_engine();
        engine
            .apply_event(&Event::FundingRateUpdate {
                symbol: SymbolId::new(1),
                funding_rate: dec("0.0042"),
                timestamp: ts(),
            })
            .unwrap();
        assert_eq!(engine.last_funding_rate(), dec("0.0042"));
    }

    #[test]
    fn apply_event_mark_price_replay_preserves_trailing_peak() {
        // CEO review C8: guards the Stage 1b B5/B6 silent-corruption-on-replay
        // class for trailing peaks. A trailing-stop's peak must end up
        // identical whether the engine processed the mark sequence live
        // (update_mark_price) or replayed it (apply_event passive half).
        //
        // Use a STRICTLY ASCENDING mark sequence so neither engine triggers
        // the trailing stop (a trailing-sell triggers only on a downward
        // reversal past peak - delta). This isolates the peak-fidelity
        // property; the non-trigger-on-replay property is already covered by
        // apply_event_mark_price_update_passive_only.

        // accept_event_full(order_id, type, tif, qty, price, visible,
        //                    trailing_delta, trailing_peak_price, stop_price)
        let accepted = accept_event_full(
            9,
            OrderType::TrailingStop,
            TimeInForce::Gtc,
            "1.0",
            "60000",
            None,
            Some(dec("100")),    // trailing_delta
            Some(dec("60000")),  // initial trailing_peak_price (= mark at accept)
            Some(dec("59900")),  // stop_price
        );
        let ascending = ["60500", "61000", "61500"];

        // LIVE engine: full update_mark_price path advances the peak each step.
        let mut live = test_engine();
        live.apply_event(&accepted).unwrap();
        for px in ascending {
            let _ = live.update_mark_price(SymbolId::new(1), dec(px), dec(px), ts());
        }
        assert_eq!(
            live.stop_orders_mut().len(),
            1,
            "ascending marks must not trigger a trailing sell"
        );
        let live_peak = live.stop_orders_mut()[0].trailing_peak_price;

        // REPLAYED engine: same OrderAccepted, same marks as MarkPriceUpdate
        // events through the passive replay half.
        let mut replayed = test_engine();
        replayed.apply_event(&accepted).unwrap();
        for px in ascending {
            replayed
                .apply_event(&Event::MarkPriceUpdate {
                    symbol: SymbolId::new(1),
                    mark_price: dec(px),
                    index_price: dec(px),
                    timestamp: ts(),
                })
                .unwrap();
        }
        let replayed_peak = replayed.stop_orders_mut()[0].trailing_peak_price;

        assert_eq!(
            live_peak,
            Some(dec("61500")),
            "live peak should track the 61500 high"
        );
        assert_eq!(
            replayed_peak, live_peak,
            "replayed trailing_peak_price must equal live — no silent drift"
        );
    }

    #[test]
    fn apply_event_liquidation_order_still_unexpected_variant() {
        let mut engine = test_engine();
        let err = engine
            .apply_event(&Event::LiquidationOrder {
                user_id: UserId::new(42),
                symbol: SymbolId::new(1),
                side: Side::Buy,
                quantity: dec("1.0"),
                timestamp: ts(),
            })
            .unwrap_err();
        assert!(matches!(
            err,
            ApplyError::UnexpectedVariant { variant: "LiquidationOrder" }
        ));
    }

    #[test]
    fn replay_round_trip_with_mark_price_and_funding() {
        use exg_protocol::Command;
        let mut live = test_engine();
        // Place a resting limit, then a mark-price update + funding tick.
        live.process_command(&Command::NewOrder {
            order_id: OrderId::new(1),
            user_id: UserId::new(42),
            symbol: SymbolId::new(1),
            side: Side::Buy,
            order_type: OrderType::Limit,
            time_in_force: TimeInForce::Gtc,
            price: Some(dec("59000")),
            quantity: dec("0.001"),
            stop_price: None,
            trailing_delta: None,
            visible_quantity: None,
            reduce_only: false,
            margin_mode: exg_common::MarginMode::Cross,
            leverage: Some(dec("10")),
            client_order_id: None,
            timestamp: ts(),
        });
        let mut events = Vec::new();
        events.extend(live.process_command(&Command::UpdateMarkPrice {
            symbol: SymbolId::new(1),
            mark_price: dec("60600"),
            index_price: dec("60000"),
            timestamp: ts(),
        }));
        events.extend(live.process_command(&Command::ComputeFunding {
            symbol: SymbolId::new(1),
            timestamp: ts(),
        }));
        // Rebuild events list from the start (include the NewOrder's events).
        // Simpler: re-run the whole command stream and collect all events.
        let mut live2 = test_engine();
        let mut all_events = Vec::new();
        for cmd in [
            Command::NewOrder {
                order_id: OrderId::new(1),
                user_id: UserId::new(42),
                symbol: SymbolId::new(1),
                side: Side::Buy,
                order_type: OrderType::Limit,
                time_in_force: TimeInForce::Gtc,
                price: Some(dec("59000")),
                quantity: dec("0.001"),
                stop_price: None,
                trailing_delta: None,
                visible_quantity: None,
                reduce_only: false,
                margin_mode: exg_common::MarginMode::Cross,
                leverage: Some(dec("10")),
                client_order_id: None,
                timestamp: ts(),
            },
            Command::UpdateMarkPrice {
                symbol: SymbolId::new(1),
                mark_price: dec("60600"),
                index_price: dec("60000"),
                timestamp: ts(),
            },
            Command::ComputeFunding {
                symbol: SymbolId::new(1),
                timestamp: ts(),
            },
        ] {
            all_events.extend(live2.process_command(&cmd));
        }
        let _ = events; // first run kept for clarity; live2/all_events is authoritative

        let mut replayed = test_engine();
        for evt in &all_events {
            replayed.apply_event(evt).unwrap();
        }
        assert_eq!(
            live2.orderbook().order_count(),
            replayed.orderbook().order_count()
        );
        assert_eq!(live2.mark_price(), replayed.mark_price());
        assert_eq!(live2.last_funding_rate(), replayed.last_funding_rate());
    }
```

(`accept_event_full`, `test_engine`, `ts`, `dec` already exist in replay.rs tests from Stage 1b. `exg_common::MarginMode` import: replay.rs tests may need `use exg_common::MarginMode;` — match how Stage 1b's `replay_then_take_snapshot_round_trip` constructs `Command::NewOrder` (it already includes margin_mode/leverage per the Stage 1b implementer's deviation note; copy that exact shape).)

```bash
cargo test -p exg-matching-engine --lib replay 2>&1 | tail -8
```

Expected: the 3 new arms fail (`MarkPriceUpdate`/`FundingRateUpdate` currently return `UnexpectedVariant`; round-trip mark/funding assertions fail).

- [ ] **Step 3: Replace the two UnexpectedVariant arms**

In `crates/exg-matching-engine/src/replay.rs::apply_event`, replace:

```rust
            Event::MarkPriceUpdate { .. } => Err(ApplyError::UnexpectedVariant {
                variant: "MarkPriceUpdate",
            }),
            Event::FundingRateUpdate { .. } => Err(ApplyError::UnexpectedVariant {
                variant: "FundingRateUpdate",
            }),
```

with:

```rust
            Event::MarkPriceUpdate {
                mark_price,
                index_price,
                ..
            } => {
                // Passive only — triggered OrderFilled/TradeExecuted events
                // are separate WAL records replayed via their own arms.
                // Re-triggering here would double-count fills (same principle
                // as OrderAccepted not re-matching). Invariant 27.
                self.apply_mark_index_passive(*mark_price, *index_price);
                Ok(())
            }
            Event::FundingRateUpdate { funding_rate, .. } => {
                self.set_last_funding_rate(*funding_rate);
                Ok(())
            }
```

Leave the `Event::LiquidationOrder { .. } => Err(ApplyError::UnexpectedVariant { variant: "LiquidationOrder" })` arm unchanged (Stage 3+).

`apply_mark_index_passive` is `pub(crate)` (Task 3) so it is callable from the replay module. `set_last_funding_rate` is `#[doc(hidden)] pub` (Task 3).

- [ ] **Step 4: Run — verify green**

```bash
cargo test -p exg-matching-engine --lib 2>&1 | tail -10
```

Expected: the 5 new tests pass (4 + `apply_event_mark_price_replay_preserves_trailing_peak` per CEO review C8); Stage 1b replay 19 + all engine tests still green. Unit count for replay module: 19 (Stage 1b) + 5 (Stage 2) = 24.

- [ ] **Step 5: Commit**

```bash
git add crates/exg-matching-engine/src/replay.rs
git commit -m "$(cat <<'EOF'
feat(matching-engine): apply_event handles MarkPriceUpdate + FundingRateUpdate

- MarkPriceUpdate replay = apply_mark_index_passive (set price + trailing
  peaks), NO re-trigger (triggered fills are separate WAL events;
  invariant 27, same as OrderAccepted not re-matching)
- FundingRateUpdate replay = set_last_funding_rate
- LiquidationOrder still UnexpectedVariant (Stage 3+)
- Removes the two Stage 1b UnexpectedVariant rejections

4 unit tests: passive-only (stop not re-triggered on replay), funding
rate set, liquidation still rejected, full mark+funding round-trip.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: admin module (middleware + handlers + build_admin_app)

**Files:**
- Create: `crates/exg-api-gateway/src/admin.rs`
- Modify: `crates/exg-api-gateway/src/types.rs`
- Modify: `crates/exg-api-gateway/src/lib.rs`
- Modify: `crates/exg-api-gateway/Cargo.toml`
- Modify: `Cargo.toml` (workspace `[workspace.dependencies]`)

### Step 1: Add `subtle` dependency

`subtle` is NOT currently a workspace dep (verified). In the workspace root `Cargo.toml`, under `[workspace.dependencies]`, add (alphabetically near `sqlx`):

```toml
subtle = "2.6"
```

In `crates/exg-api-gateway/Cargo.toml` `[dependencies]`:

```toml
subtle = { workspace = true }
```

```bash
cargo fetch 2>&1 | tail -2
```

- [ ] **Step 2: Add `AdminMarkPriceRequest` to types.rs**

In `crates/exg-api-gateway/src/types.rs`, add (match the existing camelCase serde rename style used by `RegisterRequest` etc.):

```rust
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminMarkPriceRequest {
    /// Decimal string, e.g. "60000.5".
    pub mark_price: String,
    /// Decimal string. Must be > 0 (funding div-by-zero guard).
    pub index_price: String,
}
```

- [ ] **Step 3: Write `crates/exg-api-gateway/src/admin.rs`**

```rust
//! Stage 2 — admin HTTP surface (separate port, X-Admin-Secret gated).
//!
//! Bound to `cfg.server.admin_port` (9090) by a second actix HttpServer
//! that shares `AppState` (same Mutex<Producer>) with the main 8080
//! server. Two routes: inject mark/index price, trigger funding.

use actix_web::{App, HttpRequest, HttpResponse, web};
use exg_common::UnixMicros;
use exg_protocol::Command;
use subtle::ConstantTimeEq;

use crate::error::ApiError;
use crate::state::AppState;
use crate::types::AdminMarkPriceRequest;

/// Constant-time compare of the `X-Admin-Secret` header against the
/// configured secret. Missing/mismatch → 401 (ERR_UNAUTHORIZED).
/// Invariant 26: rejected before any Command is produced.
fn check_admin_secret(req: &HttpRequest, expected: &str) -> Result<(), ApiError> {
    let provided = req
        .headers()
        .get("X-Admin-Secret")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| ApiError::unauthorized("missing X-Admin-Secret header"))?;
    let a = provided.as_bytes();
    let b = expected.as_bytes();
    // ConstantTimeEq requires equal-length slices for a meaningful compare;
    // length mismatch is an immediate (still constant-time-safe) reject.
    let ok = a.len() == b.len() && bool::from(a.ct_eq(b));
    if !ok {
        return Err(ApiError::unauthorized("invalid X-Admin-Secret"));
    }
    Ok(())
}

pub async fn admin_mark_price(
    state: web::Data<AppState>,
    req: HttpRequest,
    body: web::Json<AdminMarkPriceRequest>,
) -> Result<HttpResponse, ApiError> {
    check_admin_secret(&req, &state.cfg.admin.admin_secret)?;

    let mark_price: exg_common::Decimal128 = body
        .mark_price
        .parse()
        .map_err(|_| ApiError::bad_request("markPrice must be a decimal"))?;
    let index_price: exg_common::Decimal128 = body
        .index_price
        .parse()
        .map_err(|_| ApiError::bad_request("indexPrice must be a decimal"))?;
    if index_price <= exg_common::Decimal128::ZERO {
        return Err(ApiError::bad_request("indexPrice must be positive"));
    }
    // CEO review C5: a non-positive mark price makes `mark <= stop_price`
    // true for every positive-stop sell order → mass-trigger cascade.
    // Symmetric with the indexPrice guard above. Invariant 29.
    if mark_price <= exg_common::Decimal128::ZERO {
        return Err(ApiError::bad_request("markPrice must be positive"));
    }

    let symbol = exg_common::SymbolId::new(state.cfg.trading.symbols[0].id);
    // CEO review C6: audit line for the high-privilege market-impacting
    // action before enqueue. Invariant 30.
    tracing::info!(
        target: "admin",
        mark_price = %mark_price,
        index_price = %index_price,
        "mark price injected"
    );
    let cmd = Command::UpdateMarkPrice {
        symbol,
        mark_price,
        index_price,
        timestamp: UnixMicros::now(),
    };
    enqueue_admin(&state, &cmd)?;
    Ok(HttpResponse::Ok().json(serde_json::json!({ "status": "ACCEPTED" })))
}

pub async fn admin_funding_tick(
    state: web::Data<AppState>,
    req: HttpRequest,
) -> Result<HttpResponse, ApiError> {
    check_admin_secret(&req, &state.cfg.admin.admin_secret)?;
    let symbol = exg_common::SymbolId::new(state.cfg.trading.symbols[0].id);
    // CEO review C6: audit line before enqueue (invariant 30).
    tracing::info!(target: "admin", "funding tick");
    let cmd = Command::ComputeFunding {
        symbol,
        timestamp: UnixMicros::now(),
    };
    enqueue_admin(&state, &cmd)?;
    Ok(HttpResponse::Ok().json(serde_json::json!({ "status": "ACCEPTED" })))
}

/// Push an admin-produced command into the shared ring buffer. Mirrors
/// the order handlers' enqueue (same Mutex<Producer>, same backpressure
/// → 429 mapping).
fn enqueue_admin(state: &AppState, cmd: &Command) -> Result<(), ApiError> {
    let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(cmd)
        .map_err(|e| ApiError::internal(format!("rkyv encode: {e}")))?;
    let producer = state.producer.lock();
    producer.try_push(&bytes).map_err(|e| {
        use exg_ringbuffer::RingBufferError;
        match e {
            RingBufferError::WouldBlock => ApiError::rate_limited(),
            RingBufferError::MessageTooLarge { .. } => {
                ApiError::bad_request("command too large for ring slot")
            }
            other => ApiError::internal(format!("ring buffer push: {other}")),
        }
    })?;
    Ok(())
}

/// Build the admin-only Actix App (bound to admin_port by exg-server).
pub fn build_admin_app(
    state: AppState,
) -> App<
    impl actix_web::dev::ServiceFactory<
        actix_web::dev::ServiceRequest,
        Config = (),
        Response = actix_web::dev::ServiceResponse<actix_web::body::BoxBody>,
        Error = actix_web::Error,
        InitError = (),
    >,
> {
    App::new()
        .app_data(web::Data::new(state))
        .route("/api/v1/admin/mark-price", web::post().to(admin_mark_price))
        .route("/api/v1/admin/funding-tick", web::post().to(admin_funding_tick))
}
```

Implementer notes:
- Mirror the exact `build_app` return-type signature from `crates/exg-api-gateway/src/app_factory.rs` (the `App<impl ServiceFactory<...>>` bound). Copy it verbatim from there to avoid type mismatch — the snippet above is the Stage 1a shape; confirm it still matches.
- `enqueue_admin` duplicates the order-handler `enqueue`. That duplication is intentional and acceptable for Stage 2 (different module); a shared helper is a future cleanup. (If `enqueue` in handlers.rs is already `pub(crate)`, reuse it instead — `grep -n "fn enqueue" crates/exg-api-gateway/src/handlers.rs`; if `pub(crate)` reuse, else keep the local copy.)
- `state.cfg.admin.admin_secret` — `AppState.cfg` is `Arc<ExgConfig>`; `.admin.admin_secret` resolves after Task 1. `state.cfg.trading.symbols[0].id` is how handlers.rs resolves the single symbol — match that exact access pattern (`grep -n "symbols\[0\]" crates/exg-api-gateway/src/handlers.rs`).
- `ApiError::unauthorized` / `bad_request` / `rate_limited` / `internal` all exist from Stage 1a.

- [ ] **Step 4: Export the module**

In `crates/exg-api-gateway/src/lib.rs`, add:

```rust
pub mod admin;
```

(alongside the existing `pub mod handlers; pub mod app_factory;` etc.)

- [ ] **Step 5: Verify compile**

```bash
cargo check -p exg-api-gateway 2>&1 | tail -8
```

Expected: clean (depends on Task 1's `AdminConfig` being merged — it is, Task 1 < Task 6).

- [ ] **Step 6: Commit**

```bash
git add crates/exg-api-gateway/src/admin.rs crates/exg-api-gateway/src/types.rs \
        crates/exg-api-gateway/src/lib.rs crates/exg-api-gateway/Cargo.toml Cargo.toml
git commit -m "$(cat <<'EOF'
feat(api-gateway): admin module — mark-price inject + funding-tick

- admin.rs: X-Admin-Secret constant-time check (subtle crate, new
  workspace dep), admin_mark_price + admin_funding_tick handlers,
  build_admin_app (separate Actix App for the admin port)
- AdminMarkPriceRequest (camelCase, stringified decimals)
- indexPrice <= 0 rejected at 400 (funding div-by-zero guard,
  invariant 28 live-path half)
- enqueue_admin mirrors order-handler enqueue (shared Mutex<Producer>,
  same 429 backpressure mapping)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: server — invariants + interest_rate threading + 2nd HttpServer + dual shutdown

**Files:**
- Modify: `crates/exg-server/src/lib.rs`

### Step 1: Add boot invariants #24/#25

In `crates/exg-server/src/lib.rs::validate_invariants`, after the Stage 1a JWT block (the `cfg.auth.jwt_secret.len() < 32` / placeholder bails), add:

```rust
    // Stage 2 §6 invariant 24/25: admin secret length + placeholder.
    const ADMIN_SECRET_PLACEHOLDER: &str = "CHANGE-ME-ADMIN-DEV-ONLY-MUST-BE-32-BYTES";
    if cfg.admin.admin_secret.len() < 32 {
        bail!(
            "Stage 2: admin.admin_secret must be at least 32 bytes, got {}",
            cfg.admin.admin_secret.len()
        );
    }
    if cfg.admin.admin_secret == ADMIN_SECRET_PLACEHOLDER {
        bail!("Stage 2: admin.admin_secret is the placeholder; override via EXG_ADMIN_SECRET");
    }
```

(`cfg.validate()` already runs at invariant 0 and covers the same via exg-config — this is the defense-in-depth boot-level duplicate, exactly the Stage 1a pattern for jwt_secret.)

### Step 2: Thread interest_rate into MatchingEngine::new

Find the boot `MatchingEngine::new(symbol_config, cfg.server.node_id)` call (Step 3 of `run_with_config_with_pool`, ~line 211 region — `grep -n "MatchingEngine::new" crates/exg-server/src/lib.rs`). Change to:

```rust
    let interest_rate: Decimal128 = cfg
        .risk
        .interest_rate
        .parse()
        .with_context(|| format!("invalid risk.interest_rate: {}", cfg.risk.interest_rate))?;
    let mut engine = MatchingEngine::new(symbol_config, cfg.server.node_id, interest_rate);
```

`Decimal128` is already imported in lib.rs (used for mark_price parsing). `cfg.risk.interest_rate` is a `String` ("0.0001").

- [ ] **Step 3: Add the 2nd HttpServer bound to admin_port**

In `run_with_config_with_pool`, Step 6 (after the main server `tokio::spawn(server)` and before constructing `ServerHandle`), add:

```rust
    // ── Step 6b (Stage 2): admin HTTP server on admin_port ────────────────
    let admin_port = cfg.server.admin_port;
    let admin_listener = TcpListener::bind((host.as_str(), admin_port))
        .with_context(|| format!("failed to bind admin {host}:{admin_port}"))?;
    let admin_bound_port = admin_listener
        .local_addr()
        .context("failed to get admin local addr")?
        .port();
    let admin_state = state.clone();
    let admin_server = actix_web::HttpServer::new(move || {
        exg_api_gateway::admin::build_admin_app(admin_state.clone())
    })
    .listen(admin_listener)
    .context("actix admin HttpServer::listen failed")?
    .run();
    let admin_actix_handle = admin_server.handle();
    tokio::spawn(admin_server);

    info!(
        host = %host,
        admin_port = admin_bound_port,
        "exg-server admin listening"
    );
```

`TcpListener` + `actix_web` + `info!` are already imported. Use port 0 support for tests (admin_port 0 → OS-assigned; expose `admin_bound_port` on the handle).

- [ ] **Step 4: Extend `ServerHandle` for dual-server shutdown**

In the `ServerHandle` struct add:

```rust
    /// Stage 2: admin server actix handle (second HttpServer).
    pub admin_bound_port: u16,
    admin_actix_handle: ActixServerHandle,
```

In the `ServerHandle { ... }` constructor at the end of `run_with_config_with_pool`, add `admin_bound_port` + `admin_actix_handle`.

In `ServerHandle::shutdown`, BEFORE `self.actix_handle.stop(true).await;` (Step 2 of the graceful drain), add:

```rust
        // Step 2 (Stage 2): drain the admin server first — it produces
        // commands into the same ring buffer the matching thread drains.
        // Stopping it before the main server keeps the Stage 0 §9 drain
        // invariant intact (no new commands after shutdown begins).
        self.admin_actix_handle.stop(true).await;
```

The matching-thread join (Step 4) is unchanged and still drains every queued command (including any admin-produced one) before exit — the Stage 0 §9 shutdown-drain invariant holds because both HTTP servers stop accepting before the matching thread is signalled.

- [ ] **Step 5: Verify**

```bash
cargo check --workspace 2>&1 | tail -5
DATABASE_URL=postgres://exg:exg_dev_password@localhost:5433/exg \
  cargo test -p exg-server --test stage1a_e2e --test stage1b_e2e --test stage0_e2e 2>&1 | grep "test result" | tail
```

Expected: workspace clean. Regression baselines green: stage0 7/7, stage1a 12/12, stage1b 16/16 — the dual-server change must not break the existing shutdown-drain behavior. If any stage0/1a/1b test hangs on shutdown, the admin-server stop ordering is wrong — admin must stop before main, both before matching-thread signal.

NOTE: existing stage0/1a/1b e2e tests construct `ExgConfig` via `base_cfg` which calls `default_config()` — that now has the admin placeholder which fails `validate_invariants`. Every `base_cfg` helper across stage0_e2e.rs / stage1a_e2e.rs / stage1b_e2e.rs / boot_panics.rs must set `cfg.admin.admin_secret = "a".repeat(32)` (same cascade Stage 1a created for jwt_secret). Fix all `base_cfg` helpers in this step:

```bash
grep -rn "fn base_cfg" crates/exg-server/tests/
```

Add `cfg.admin.admin_secret = "a".repeat(32);` next to the existing `cfg.auth.jwt_secret = ...` line in each.

- [ ] **Step 6: Commit**

```bash
git add crates/exg-server/src/lib.rs crates/exg-server/tests/
git commit -m "$(cat <<'EOF'
feat(server): admin HttpServer on admin_port + invariants 24/25

- validate_invariants: admin_secret length + placeholder (24/25),
  defense-in-depth over cfg.validate()
- MatchingEngine::new threaded with cfg.risk.interest_rate (parsed)
- Step 6b: 2nd actix HttpServer bound to admin_port, shares AppState
  (same Mutex<Producer>); admin_bound_port exposed for tests
- ServerHandle.shutdown stops the admin server BEFORE the main server,
  both before the matching-thread signal — Stage 0 §9 drain invariant
  preserved
- All e2e base_cfg helpers set admin_secret (same cascade as Stage 1a
  jwt_secret) to keep stage0/1a/1b regression baselines green

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: stage2_e2e + boot_panics + demo + final acceptance

**Files:**
- Create: `crates/exg-server/tests/stage2_e2e.rs`
- Modify: `crates/exg-server/tests/boot_panics.rs`
- Create: `scripts/demo-stage2.sh`

### Step 1: Write `crates/exg-server/tests/stage2_e2e.rs`

```rust
//! Stage 2 e2e: admin mark-price inject + funding tick + auth + port
//! isolation + replay. #[sqlx::test] per-test DB isolation.

use exg_config::ExgConfig;
use exg_protocol::Event;
use exg_wal::WalReader;
use reqwest::Client;
use sqlx::PgPool;
use std::time::Duration;
use tempfile::TempDir;

fn base_cfg(wal_dir: &std::path::Path) -> ExgConfig {
    let mut cfg = ExgConfig::default_config();
    cfg.wal.dir = wal_dir.to_string_lossy().into_owned();
    cfg.server.host = "127.0.0.1".into();
    cfg.server.port = 0;
    cfg.server.admin_port = 0; // OS-assigned for tests
    cfg.auth.jwt_secret = "stage2-test-secret-padding-32-bytes-ok".into();
    cfg.admin.admin_secret = "stage2-admin-secret-padding-32-bytesok".into();
    cfg
}

async fn boot(cfg: ExgConfig, pool: PgPool) -> (exg_server::ServerHandle, String, String) {
    let handle = exg_server::run_with_config_with_pool(cfg, Some(pool))
        .await
        .expect("server boot");
    let base = format!("http://127.0.0.1:{}", handle.bound_port);
    let admin = format!("http://127.0.0.1:{}", handle.admin_bound_port);
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
            return (handle, base, admin);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("server not ready");
}

const ADMIN_SECRET: &str = "stage2-admin-secret-padding-32-bytesok";

async fn register_and_login(client: &Client, base: &str, email: &str) -> String {
    let _ = client
        .post(format!("{base}/api/v1/auth/register"))
        .json(&serde_json::json!({"email": email, "password": "hunter2hunter2"}))
        .send()
        .await
        .unwrap();
    let resp: serde_json::Value = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&serde_json::json!({"email": email, "password": "hunter2hunter2"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    resp["accessToken"].as_str().unwrap().to_string()
}

#[sqlx::test(migrations = "../../migrations")]
async fn admin_mark_price_inject_triggers_stop_order(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let wal_dir = std::path::PathBuf::from(tmp.path());
    let cfg = base_cfg(tmp.path());
    let (handle, base, admin) = boot(cfg, pool).await;
    let client = Client::new();
    let token = register_and_login(&client, &base, "s2stop@e.com").await;

    // Place a STOP_MARKET sell triggered when mark <= 59000.
    client
        .post(format!("{base}/api/v1/order"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&serde_json::json!({
            "symbol":"BTCUSDT","side":"SELL","orderType":"STOP_MARKET",
            "timeInForce":"GTC","quantity":"0.001","stopPrice":"59000"
        }))
        .send()
        .await
        .unwrap();
    // Also rest a buy limit so the triggered market sell has a counterparty.
    let token2 = register_and_login(&client, &base, "s2buy@e.com").await;
    client
        .post(format!("{base}/api/v1/order"))
        .header("Authorization", format!("Bearer {token2}"))
        .json(&serde_json::json!({
            "symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT",
            "timeInForce":"GTC","quantity":"0.001","price":"59000"
        }))
        .send()
        .await
        .unwrap();

    // Admin injects a mark price that crosses the stop.
    let resp = client
        .post(format!("{admin}/api/v1/admin/mark-price"))
        .header("X-Admin-Secret", ADMIN_SECRET)
        .json(&serde_json::json!({"markPrice":"58000","indexPrice":"58000"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);

    tokio::time::sleep(Duration::from_millis(300)).await;
    handle.shutdown().await.unwrap();

    // WAL must contain a MarkPriceUpdate and an OrderFilled (triggered stop).
    let mut reader = WalReader::open(&wal_dir).unwrap();
    let (mut saw_mark, mut saw_fill) = (false, false);
    reader
        .read_from(0, |_seq, payload| {
            let owned: Vec<u8> = payload.to_vec();
            let e: Event = rkyv::from_bytes::<Event, rkyv::rancor::Error>(&owned).unwrap();
            match e {
                Event::MarkPriceUpdate { .. } => saw_mark = true,
                Event::OrderFilled { .. } => saw_fill = true,
                _ => {}
            }
            true
        })
        .unwrap();
    assert!(saw_mark, "WAL must contain MarkPriceUpdate");
    assert!(saw_fill, "stop order must have triggered an OrderFilled");
}

#[sqlx::test(migrations = "../../migrations")]
async fn admin_funding_tick_emits_funding_rate(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let wal_dir = std::path::PathBuf::from(tmp.path());
    let cfg = base_cfg(tmp.path());
    let (handle, _base, admin) = boot(cfg, pool).await;
    let client = Client::new();

    client
        .post(format!("{admin}/api/v1/admin/mark-price"))
        .header("X-Admin-Secret", ADMIN_SECRET)
        .json(&serde_json::json!({"markPrice":"60600","indexPrice":"60000"}))
        .send()
        .await
        .unwrap();
    let resp = client
        .post(format!("{admin}/api/v1/admin/funding-tick"))
        .header("X-Admin-Secret", ADMIN_SECRET)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);

    tokio::time::sleep(Duration::from_millis(300)).await;
    handle.shutdown().await.unwrap();

    let mut reader = WalReader::open(&wal_dir).unwrap();
    let mut rate = None;
    reader
        .read_from(0, |_seq, payload| {
            let owned: Vec<u8> = payload.to_vec();
            let e: Event = rkyv::from_bytes::<Event, rkyv::rancor::Error>(&owned).unwrap();
            if let Event::FundingRateUpdate { funding_rate, .. } = e {
                rate = Some(funding_rate);
            }
            true
        })
        .unwrap();
    // premium = (60600-60000)/60000 = 0.01 → clamp(0.01+0.0001, ±0.0075) = 0.0075
    assert_eq!(rate.unwrap(), "0.0075".parse().unwrap());
}

#[sqlx::test(migrations = "../../migrations")]
async fn admin_endpoint_missing_secret_returns_401(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let (handle, _base, admin) = boot(cfg, pool).await;
    let client = Client::new();
    let resp = client
        .post(format!("{admin}/api/v1/admin/funding-tick"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 401);
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn admin_endpoint_wrong_secret_returns_401(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let (handle, _base, admin) = boot(cfg, pool).await;
    let client = Client::new();
    let resp = client
        .post(format!("{admin}/api/v1/admin/funding-tick"))
        .header("X-Admin-Secret", "wrong-secret-wrong-secret-wrong!")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 401);
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn admin_endpoint_correct_secret_returns_200(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let (handle, _base, admin) = boot(cfg, pool).await;
    let client = Client::new();
    let resp = client
        .post(format!("{admin}/api/v1/admin/funding-tick"))
        .header("X-Admin-Secret", ADMIN_SECRET)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn admin_route_not_on_main_port(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let (handle, base, _admin) = boot(cfg, pool).await;
    let client = Client::new();
    let resp = client
        .post(format!("{base}/api/v1/admin/funding-tick"))
        .header("X-Admin-Secret", ADMIN_SECRET)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 404, "admin route must NOT exist on 8080");
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn user_route_not_on_admin_port(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let (handle, _base, admin) = boot(cfg, pool).await;
    let client = Client::new();
    let resp = client
        .get(format!("{admin}/api/v1/health"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 404, "user route must NOT exist on admin port");
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn admin_mark_price_bad_decimal_returns_400(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let (handle, _base, admin) = boot(cfg, pool).await;
    let client = Client::new();
    let resp = client
        .post(format!("{admin}/api/v1/admin/mark-price"))
        .header("X-Admin-Secret", ADMIN_SECRET)
        .json(&serde_json::json!({"markPrice":"not-a-number","indexPrice":"60000"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 400);
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn admin_mark_price_zero_index_returns_400(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let (handle, _base, admin) = boot(cfg, pool).await;
    let client = Client::new();
    let resp = client
        .post(format!("{admin}/api/v1/admin/mark-price"))
        .header("X-Admin-Secret", ADMIN_SECRET)
        .json(&serde_json::json!({"markPrice":"60000","indexPrice":"0"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 400);
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn replay_mark_price_trigger_survives_reboot(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());

    // Boot 1: rest a buy limit + a stop-sell, admin inject mark crossing
    // the stop → triggers a fill. Kill.
    {
        let (handle, base, admin) = boot(cfg.clone(), pool.clone()).await;
        let client = Client::new();
        let tb = register_and_login(&client, &base, "rb-buy@e.com").await;
        client
            .post(format!("{base}/api/v1/order"))
            .header("Authorization", format!("Bearer {tb}"))
            .json(&serde_json::json!({
                "symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT",
                "timeInForce":"GTC","quantity":"0.001","price":"59000"
            }))
            .send()
            .await
            .unwrap();
        let ts = register_and_login(&client, &base, "rb-stop@e.com").await;
        client
            .post(format!("{base}/api/v1/order"))
            .header("Authorization", format!("Bearer {ts}"))
            .json(&serde_json::json!({
                "symbol":"BTCUSDT","side":"SELL","orderType":"STOP_MARKET",
                "timeInForce":"GTC","quantity":"0.001","stopPrice":"59000"
            }))
            .send()
            .await
            .unwrap();
        client
            .post(format!("{admin}/api/v1/admin/mark-price"))
            .header("X-Admin-Secret", ADMIN_SECRET)
            .json(&serde_json::json!({"markPrice":"58000","indexPrice":"58000"}))
            .send()
            .await
            .unwrap();
        client
            .post(format!("{admin}/api/v1/admin/funding-tick"))
            .header("X-Admin-Secret", ADMIN_SECRET)
            .send()
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        handle.shutdown().await.unwrap();
    }

    // CEO review C10: observable assertion, NOT the weak "boot 2 didn't
    // panic" proxy (Stage 1b A4 rejected that). Count boot-1 WAL records,
    // then after reboot (NO new injects) assert: (a) the triggered stop's
    // OrderFilled appears within the boot-1 record range, and (b) NO new
    // OrderFilled for that order_id was appended during boot-2 — proving
    // replay applied the historical fill but did NOT re-trigger the stop.
    let wal_dir = std::path::PathBuf::from(
        // base_cfg sets cfg.wal.dir = tmp.path(); reuse it.
        cfg.wal.dir.clone(),
    );
    let mut reader = WalReader::open(&wal_dir).unwrap();
    let mut boot1_records: u64 = 0;
    let mut filled_order_ids_boot1: Vec<u64> = Vec::new();
    reader
        .read_from(0, |_seq, payload| {
            boot1_records += 1;
            let owned: Vec<u8> = payload.to_vec();
            let e: Event = rkyv::from_bytes::<Event, rkyv::rancor::Error>(&owned).unwrap();
            if let Event::OrderFilled { order_id, .. } = e {
                filled_order_ids_boot1.push(order_id.value());
            }
            true
        })
        .unwrap();
    assert!(
        !filled_order_ids_boot1.is_empty(),
        "boot 1 must have recorded at least one OrderFilled (triggered stop)"
    );

    // Boot 2: replay applies MarkPriceUpdate (passive) + the historical
    // OrderFilled (its own WAL event) + FundingRateUpdate. Health green.
    let (handle2, base2, _admin2) = boot(cfg, pool).await;
    let client = Client::new();
    let resp = client.get(format!("{base2}/api/v1/health")).send().await.unwrap();
    assert!(resp.status().is_success());
    handle2.shutdown().await.unwrap();

    // Re-scan: assert NO new OrderFilled was appended during boot-2 (replay
    // is passive — the MarkPriceUpdate must not re-trigger the stop and
    // produce a duplicate fill).
    let mut reader2 = WalReader::open(&wal_dir).unwrap();
    let mut total_records: u64 = 0;
    let mut filled_after_boot1 = 0u64;
    reader2
        .read_from(0, |_seq, payload| {
            total_records += 1;
            if total_records > boot1_records {
                let owned: Vec<u8> = payload.to_vec();
                let e: Event =
                    rkyv::from_bytes::<Event, rkyv::rancor::Error>(&owned).unwrap();
                if matches!(e, Event::OrderFilled { .. }) {
                    filled_after_boot1 += 1;
                }
            }
            true
        })
        .unwrap();
    assert_eq!(
        filled_after_boot1, 0,
        "replay must NOT re-trigger the stop — no new OrderFilled after boot 1 \
         (would be a double-fill silent corruption)"
    );
}
```

Implementer notes:
- `replay_mark_price_trigger_survives_reboot` needs `use exg_wal::WalReader;` + `use exg_protocol::Event;` + `rkyv` at the top of `stage2_e2e.rs` (Stage 1b's stage1b_e2e.rs already uses this exact WAL-scan pattern — copy its imports + the `payload.to_vec()` alignment workaround).
- `cfg.wal.dir` is set by `base_cfg` to `tmp.path()`; the WalReader scans it after both servers have shut down (safe — no concurrent writer).
- Confirm the order request JSON field for stop price is `stopPrice` (camelCase) — `grep -n "stop_price\|stopPrice" crates/exg-api-gateway/src/types.rs`. Match the actual `PlaceOrderRequest` field rename.
- `STOP_MARKET` string → `OrderType::StopMarket` per `conversion.rs::string_to_order_type` (verified in Stage 1b review). If the e2e stop order is rejected for a missing required field (e.g. price for stop-limit), use STOP_MARKET (no price needed) as written.
- If `admin_mark_price_inject_triggers_stop_order` doesn't see the fill: the triggered stop becomes a market order needing a resting counterparty — the test rests a buy limit at 59000 first. Verify the matcher fills a market sell against a resting bid. If timing-flaky, bump the 300ms sleep.

- [ ] **Step 1b: Add the `admin_mark_price_negative_or_zero_mark_returns_400` e2e (CEO review C5)**

Append to `stage2_e2e.rs`:

```rust
#[sqlx::test(migrations = "../../migrations")]
async fn admin_mark_price_negative_or_zero_mark_returns_400(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let (handle, _base, admin) = boot(cfg, pool).await;
    let client = Client::new();
    for bad in ["0", "-1"] {
        let resp = client
            .post(format!("{admin}/api/v1/admin/mark-price"))
            .header("X-Admin-Secret", ADMIN_SECRET)
            .json(&serde_json::json!({"markPrice": bad, "indexPrice": "60000"}))
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status().as_u16(),
            400,
            "markPrice {bad} must be rejected (mass stop-trigger guard)"
        );
    }
    handle.shutdown().await.unwrap();
}
```

stage2_e2e count: 10 → 11 (C5).

- [ ] **Step 2: Add 2 boot_panics tests**

Append to `crates/exg-server/tests/boot_panics.rs`:

```rust
#[actix_web::test]
async fn boot_panics_on_short_admin_secret() {
    let tmp = TempDir::new().unwrap();
    let mut cfg = base_cfg(tmp.path());
    cfg.admin.admin_secret = "short".into();
    let result = exg_server::run_with_config(cfg).await;
    let err = result.err().expect("expected Err");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("admin.admin_secret") && msg.contains("32 bytes"),
        "expected admin secret length panic, got: {msg}"
    );
}

#[actix_web::test]
async fn boot_panics_on_placeholder_admin_secret() {
    let tmp = TempDir::new().unwrap();
    let mut cfg = base_cfg(tmp.path());
    cfg.admin.admin_secret = "CHANGE-ME-ADMIN-DEV-ONLY-MUST-BE-32-BYTES".into();
    let result = exg_server::run_with_config(cfg).await;
    let err = result.err().expect("expected Err");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("admin.admin_secret") && msg.contains("placeholder"),
        "expected admin secret placeholder panic, got: {msg}"
    );
}
```

(Task 7 already updated `base_cfg` in boot_panics.rs to set a valid `admin_secret`; these two tests override it to the bad values. Confirm `base_cfg` in boot_panics.rs sets `cfg.admin.admin_secret = "a".repeat(32)` — Task 7 Step 5 did this.)

- [ ] **Step 3: Write `scripts/demo-stage2.sh`**

```bash
#!/usr/bin/env bash
# Stage 2 demo: place stop → admin inject mark crosses stop → wal-dump
# fill → admin funding-tick → wal-dump rate → reboot replays.
set -euo pipefail

WAL_DIR=$(mktemp -d /tmp/exg-stage2.XXXXXX)
PORT=8080
ADMIN_PORT=9090
SERVER_PID=""
TMP_CFG=$(mktemp /tmp/exg-stage2-cfg.XXXXXX.toml)
ADMIN_SECRET="demo-stage2-admin-secret-32-bytes-ok"

cleanup() {
    if [[ -n "${SERVER_PID}" ]]; then
        kill -INT "${SERVER_PID}" 2>/dev/null || true
        wait "${SERVER_PID}" 2>/dev/null || true
    fi
    rm -rf "${WAL_DIR}"
    rm -f "${TMP_CFG}"
}
trap cleanup EXIT

start_server() {
    EXG_CONFIG="$TMP_CFG" RUST_LOG=info ./target/release/exg-server &
    SERVER_PID=$!
    for i in {1..30}; do
        curl -sf "http://127.0.0.1:${PORT}/api/v1/health" >/dev/null && return 0
        sleep 1
    done
    echo "server not ready" >&2; return 1
}
stop_server() {
    [[ -n "${SERVER_PID}" ]] && { kill -INT "${SERVER_PID}"; wait "${SERVER_PID}" 2>/dev/null || true; SERVER_PID=""; }
}

echo "── stage 2 demo ──"
docker compose up -d postgres
sleep 2
echo "─ migrate ─"; scripts/migrate.sh reset
echo "─ build ─"; cargo build --release -p exg-server -p exg-wal-dump >/dev/null

echo "─ prepare config ─"
cp config/default.toml "$TMP_CFG"
python3 - <<PY
import re
with open('$TMP_CFG') as f: c = f.read()
c = re.sub(r'dir = "\\./data/wal"', f'dir = "$WAL_DIR"', c)
c = re.sub(r'jwt_secret = "CHANGE-ME-DEV-ONLY-MUST-BE-AT-LEAST-32-BYTES-OK"', 'jwt_secret = "demo-stage2-jwt-secret-32-bytes-okk"', c)
c = re.sub(r'admin_secret = "CHANGE-ME-ADMIN-DEV-ONLY-MUST-BE-32-BYTES"', 'admin_secret = "$ADMIN_SECRET"', c)
with open('$TMP_CFG','w') as f: f.write(c)
PY

echo
echo "─ boot 1 ─"; start_server

TOK=$(curl -s -X POST "http://127.0.0.1:${PORT}/api/v1/auth/register" -H 'Content-Type: application/json' -d '{"email":"demo2@example.com","password":"hunter2hunter2"}' >/dev/null; \
      curl -s -X POST "http://127.0.0.1:${PORT}/api/v1/auth/login" -H 'Content-Type: application/json' -d '{"email":"demo2@example.com","password":"hunter2hunter2"}' | python3 -c 'import json,sys;print(json.load(sys.stdin)["accessToken"])')

echo "─ rest a buy limit @59000 ─"
curl -s -X POST "http://127.0.0.1:${PORT}/api/v1/order" -H "Authorization: Bearer $TOK" -H 'Content-Type: application/json' \
  -d '{"symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT","timeInForce":"GTC","quantity":"0.001","price":"59000"}'; echo

echo "─ place STOP_MARKET sell, stop @59000 ─"
curl -s -X POST "http://127.0.0.1:${PORT}/api/v1/order" -H "Authorization: Bearer $TOK" -H 'Content-Type: application/json' \
  -d '{"symbol":"BTCUSDT","side":"SELL","orderType":"STOP_MARKET","timeInForce":"GTC","quantity":"0.001","stopPrice":"59000"}'; echo

echo "─ admin inject mark price 58000 (crosses stop) ─"
curl -s -X POST "http://127.0.0.1:${ADMIN_PORT}/api/v1/admin/mark-price" -H "X-Admin-Secret: $ADMIN_SECRET" -H 'Content-Type: application/json' \
  -d '{"markPrice":"58000","indexPrice":"58000"}'; echo

echo "─ admin funding-tick ─"
curl -s -X POST "http://127.0.0.1:${ADMIN_PORT}/api/v1/admin/funding-tick" -H "X-Admin-Secret: $ADMIN_SECRET"; echo

sleep 1
echo "─ shutdown 1 ─"; stop_server

echo
echo "─ WAL after boot 1 (expect MarkPriceUpdate, OrderFilled, FundingRateUpdate) ─"
./target/release/exg-wal-dump --wal-dir "${WAL_DIR}" | tail -25

echo
echo "─ boot 2: replay ─"; start_server
echo "─ health ─"; curl -sf "http://127.0.0.1:${PORT}/api/v1/health"; echo
echo "─ shutdown 2 ─"; stop_server
echo "─ demo complete ─"
```

```bash
chmod +x scripts/demo-stage2.sh
```

- [ ] **Step 4: Run full acceptance**

```bash
docker compose up -d postgres
DATABASE_URL=postgres://exg:exg_dev_password@localhost:5433/exg cargo check --workspace
# Eng review E2: --all-targets compiles benches/matching.rs (the MatchingEngine::new
# 3rd-param cascade site that plain --workspace silently skips).
DATABASE_URL=postgres://exg:exg_dev_password@localhost:5433/exg cargo check --workspace --all-targets
DATABASE_URL=postgres://exg:exg_dev_password@localhost:5433/exg cargo clippy --workspace -- -D warnings
cargo fmt --check
DATABASE_URL=postgres://exg:exg_dev_password@localhost:5433/exg cargo test --workspace
DATABASE_URL=postgres://exg:exg_dev_password@localhost:5433/exg cargo test -p exg-server --test stage2_e2e
DATABASE_URL=postgres://exg:exg_dev_password@localhost:5433/exg cargo test -p exg-server --test stage1b_e2e
DATABASE_URL=postgres://exg:exg_dev_password@localhost:5433/exg cargo test -p exg-server --test stage1a_e2e
DATABASE_URL=postgres://exg:exg_dev_password@localhost:5433/exg cargo test -p exg-server --test stage0_e2e
DATABASE_URL=postgres://exg:exg_dev_password@localhost:5433/exg cargo test -p exg-server --test boot_panics
cargo test -p exg-matching-engine --lib
scripts/demo-stage2.sh
```

Expected:
- workspace all green (~493 tests)
- stage2_e2e 11/11 (10 base + 1 new `admin_mark_price_negative_or_zero_mark_returns_400` per CEO C5; C10 strengthened the existing `replay_mark_price_trigger_survives_reboot` assertions without changing the count)
- stage1b_e2e 16/16, stage1a_e2e 12/12, stage0_e2e 7/7 (regression baselines)
- boot_panics 11/11 (9 + 2 new)
- exg-matching-engine --lib: existing 61 + Stage 1b replay 19 + Stage 2 (~6 engine + 1 snapshot + 5 replay incl. CEO C8 trailing-peak) ≈ 92
- demo: clean exit, wal-dump shows MarkPriceUpdate + OrderFilled + FundingRateUpdate, boot 2 replays + health green

**Rollback note (CEO review C3):** spec §8.5 documents the Stage 2 → Stage 1b rollback (Stage 1b's `apply_event` rejects `MarkPriceUpdate`/`FundingRateUpdate` → `rm -rf data/wal` required). No code in this task — the procedure lives in the spec; this line is the plan-side pointer so the implementer knows the rollback section exists and must stay in sync if the events change.

If `cargo fmt --check` flags issues: `cargo fmt`, stage modified `.rs`, separate `style: cargo fmt` commit BEFORE the e2e commit.

- [ ] **Step 5: Commit**

```bash
git add crates/exg-server/tests/stage2_e2e.rs crates/exg-server/tests/boot_panics.rs scripts/demo-stage2.sh
git commit -m "$(cat <<'EOF'
test(server): Stage 2 e2e suite (10) + boot panics (2) + demo

stage2_e2e (10): admin mark-price triggers stop, funding-tick emits
rate, missing/wrong/correct admin secret, admin-route-not-on-8080,
user-route-not-on-9090, bad decimal 400, zero index 400, replay of
mark-price trigger survives reboot.

boot_panics +2: short admin_secret, placeholder admin_secret (9 -> 11).

scripts/demo-stage2.sh: place stop → admin inject mark crosses stop →
wal-dump fill → funding-tick → wal-dump rate → reboot replays.

Regression baselines (stage0 7, stage1a 12, stage1b 16) green.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

If `cargo fmt` needed changes during Step 4, commit them first as `style: cargo fmt across stage 2`.

---

## Spec ↔ Plan Coverage Matrix

| Spec section | Task |
|--------------|------|
| §2 scope 1 Command::UpdateMarkPrice/ComputeFunding | Task 1 |
| §2 scope 2 admin HTTP server | Task 6 + Task 7 |
| §2 scope 3 AdminConfig + invariants 24/25 | Task 1 (config) + Task 7 (boot) |
| §2 scope 4 update_mark_price passive/active split | Task 3 |
| §2 scope 5 compute_funding | Task 3 |
| §2 scope 6 apply_event extension | Task 5 |
| §2 scope 7 last_funding_rate + interest_rate threading | Task 2 + Task 3 + Task 4 |
| §4.1 Command enum | Task 1 |
| §4.3 MatchingEngine changes | Task 2 (sig) + Task 3 (logic) |
| §4.4 apply_event extension | Task 5 |
| §4.5 EngineSnapshot | Task 4 |
| §4.6 AdminConfig | Task 1 |
| §4.7 admin module | Task 6 |
| §4.8 boot lifecycle | Task 7 |
| §5 data flow / error handling | Task 6 (handler errors) + Task 7 (boot panics) |
| §6 invariants 24-30 | Task 1 (24/25 cfg), Task 7 (24/25 boot), Task 3 (28 engine), Task 5 (27 replay), Task 6 (29 markPrice guard + 30 audit log — CEO C5/C6) |
| §7.1 unit tests | Task 3 + Task 4 + Task 5 (incl. CEO C8 trailing-peak) |
| §7.2 integration tests | Task 8 (incl. CEO C5 negative-mark e2e + C10 observable replay) |
| §7.3 boot panics | Task 8 |
| §7.4 regression baselines | Task 2 + Task 7 (cascade fixes) + Task 8 (verify) |
| §8 acceptance | Task 8 |
| §8.5 rollback to Stage 1b (CEO C3) | Spec doc; Task 8 plan-side pointer |
| §9 forward pointers (incl. CEO C2 stop-cascade, C3 fwd-compat replay) | Spec doc |

All spec sections covered. CEO review C2–C10 (6 findings) applied: C2/C3 → spec §8.5+§9, C5 → invariant 29 + Task 6 guard + Task 8 e2e, C6 → invariant 30 + Task 6 audit lines, C8 → Task 5 unit, C10 → Task 8 observable assertion.

---

## GSTACK REVIEW REPORT

| Review | Trigger | Why | Runs | Status | Findings |
|--------|---------|-----|------|--------|----------|
| CEO Review | `/plan-ceo-review` | Scope & strategy | 1 | CLEAR (PLAN) | mode: HOLD_SCOPE, 0 critical gaps, 6 findings all accepted (C2,C3,C5,C6,C8,C10) |
| Codex Review | `/codex review` | Independent 2nd opinion | 0 | — | — |
| Eng Review | `/plan-eng-review` | Architecture & tests (required) | 1 | CLEAR (PLAN) | FULL_REVIEW, 0 critical gaps, 3 findings applied (E1,E2,E3) + 1 observation noted (E4) |
| Design Review | `/plan-design-review` | UI/UX gaps | 0 | SKIPPED | no UI scope |
| DX Review | `/plan-devex-review` | Developer experience gaps | 0 | — | — |

**UNRESOLVED:** 0 across all reviews.

**Eng Review findings (FULL_REVIEW, applied to plan):**
- **E1 (High, applied)** — Task 3 Step 5b added: the `update_mark_price` 4-arg signature + unconditional `MarkPriceUpdate` prepend breaks the 6 existing engine.rs call sites mechanically AND inverts `test_trailing_stop`'s two `assert!(events.is_empty())` (now always non-empty). Step 5b enumerates the mechanical fixes + the `is_empty()`→`!any(is_filled)` semantic rewrite; Step 6 expectation corrected.
- **E2 (High, applied)** — Task 2 cascade omitted `benches/matching.rs:53,73,125`; `cargo check/test/clippy --workspace` skip benches so acceptance would go green while `scripts/bench.sh`/`cargo bench` are broken (Stage 1b undocumented-downstream-callsite class). Added Step 5b (bench fix, inline `"0.0001".parse().unwrap()`) + `cargo check --workspace --all-targets` to Step 6 and Task 8 acceptance.
- **E3 (Medium, applied)** — Task 2 cascade omitted `snapshot.rs:29-37 deserialize_snapshot` (`pub`, non-test, in-crate caller of `restore_from_snapshot`). Added Step 3b threading `interest_rate` through it; corrected the "test-only" wording.
- **E4 (Low, noted)** — active-half timestamp now sourced from `Command.timestamp` instead of per-order `UnixMicros::now()` (engine.rs:765). Deliberate + more correct (WAL-deterministic; replay skips active half). Flagged in Task 3 Step 4 implementer notes so it is preserved on purpose.

Non-findings (verified against codebase): dual-server shutdown ordering (plan's "BEFORE `self.actix_handle.stop(true).await`" matches `lib.rs:56` exactly — Stage 0 §9 drain preserved); passive/active body fidelity (plan snippet == `engine.rs:736-783`); `subtle` `ct_eq` idiom (CEO C4 standard); admin_secret `base_cfg` cascade enumerated in Task 7 Step 5; test matrix reconciles to spec §7 (12 unit, §7.1 #8 folded into the positive-premium test — property covered).

**VERDICT:** ENG CLEARED — both required gates (CEO HOLD_SCOPE + Eng FULL_REVIEW) passed, all findings applied. Plan + Spec CLEAR. Proceed to `superpowers:subagent-driven-development` execution of the 8 tasks.
