# Stage 3 — Position Tracking + Ledger Wiring + Funding Settlement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire a same-thread `PostTradeProcessor` so trades drive live position state and funding ticks actually move user funds through the double-entry ledger, with hybrid replay (positions projected from fills; money movements are recorded WAL facts).

**Architecture:** A new `PostTradeProcessor` (owns `PositionManager` + `Ledger`) runs in the matching OS thread immediately after `MatchingEngine::process_command`, consuming engine events, mutating clearing/ledger state, emitting post-trade fact events. All events go through the one WAL; boot replay drives both `engine.apply_event` and `post_trade.apply_event`. Matching vs clearing bounded contexts stay separate; single-thread / single-WAL / single-replay invariants preserved.

**Tech Stack:** Rust 2024 workspace · `exg-clearing` (`PositionManager`, new `post_trade` module) · `exg-ledger` (`deposit`/`withdraw`/`settle_funding_checked`/`verify_all_invariants`) · `exg-protocol` (rkyv Command/Event, append-at-end) · actix-web (Stage 2 admin server) · rkyv (ring buffer/WAL) · sqlx + PostgreSQL host port 5433 · `#[sqlx::test]` e2e.

**Branch:** `feat/stage3-position-ledger-funding` (from `main` HEAD = Stage 3 spec commit; Stage 2 merged `28c9059`).

**Spec:** [docs/superpowers/specs/2026-05-15-stage3-position-ledger-funding-settlement-design.md](../specs/2026-05-15-stage3-position-ledger-funding-settlement-design.md)

---

## Source-verified API facts (do not re-derive — confirmed against the codebase)

- `crates/exg-ledger/src/operations.rs`:
  - `const SYSTEM_USER_ID: UserId = UserId(0);` (private; ledger journals all system-side entries itself — **never reference it from post_trade**).
  - `Ledger::new() -> Self`; `get_or_create_account(&mut self, UserId) -> &mut UserAccount`; `system_balance(&self, WalletType) -> Decimal128`; `journal(&self) -> &[JournalEntry]`.
  - `deposit(&mut self, user_id: UserId, amount: Decimal128, idempotency_key: &str, timestamp: UnixMicros) -> ExgResult<()>` — `amount` must be `> 0`; journals SYSTEM→user `WalletType::Funding`; idempotent on key.
  - `withdraw(&mut self, user_id, amount, idempotency_key, timestamp) -> ExgResult<()>` — `amount > 0`; user `WalletType::Funding` available must cover it else `ExgError::InsufficientBalance`; journals user→SYSTEM.
  - `settle_funding(&mut self, user_id, payment: Decimal128, idempotency_key: &str, timestamp) -> ExgResult<()>` — **signed** `payment`: `> 0` user pays, `< 0` user receives, `== 0` early `Ok`.
  - `settle_funding_checked(&mut self, user_id: UserId, symbol: SymbolId, funding_period_id: u64, payment: Decimal128, timestamp: UnixMicros) -> ExgResult<bool>` — builds key `funding_{period}_{user}_{symbol}`, delegates to `settle_funding`; returns "margin tapped" bool (ignored in Stage 3).
- `crates/exg-ledger/src/invariant.rs`: `verify_all_invariants(&self) -> ExgResult<()>` — per-account invariants + non-negativity of `NON_NEGATIVE_SYSTEM_WALLETS` (Funding **excluded** → imbalanced book safe).
- `crates/exg-clearing/src/position.rs`:
  - `PositionManager::new() -> Self`; `get_position(&self, UserId, SymbolId) -> Option<&Position>`; `position_count(&self) -> usize`; `all_positions(&self) -> impl Iterator<Item=&Position>`; `take_snapshot(&self) -> Vec<Position>`; `restore_from_snapshot(Vec<Position>) -> Self`.
  - `open_or_increase(&mut self, user_id: UserId, symbol: SymbolId, side: PositionSide, qty: Decimal128, price: Decimal128, leverage: Decimal128, margin_mode: MarginMode) -> &Position`.
  - `reduce_or_close(&mut self, user_id: UserId, symbol: SymbolId, qty: Decimal128, exit_price: Decimal128) -> ExgResult<(Decimal128 /*signed realized_pnl*/, Option<&Position>)>`.
  - `Position` is `exg_risk_engine::Position` with fields `user_id, symbol, side, size, entry_price, leverage, margin, unrealized_pnl, accumulated_funding, margin_mode`.
- `exg_common`: `PositionSide::{Long,Short,Both}`; `MarginMode::{Cross,Isolated}`; `Side::{Buy,Sell}`; `UserId::new(u64)/.value()`, `SymbolId::new(u16)/.value()`, `Decimal128` (`.is_positive()/.is_negative()/.is_zero()/.abs()`, `ZERO`), `UnixMicros::from_micros/.now()`.
- `exg-clearing/Cargo.toml` already deps `exg-common, exg-protocol, exg-risk-engine, exg-ledger`. `exg-clearing/src/lib.rs` = `pub mod clearing; pub mod position; pub mod risk_monitor;`.
- `exg-protocol/src/command.rs`: `Command` enum tail = `ComputeFunding { symbol, timestamp }` then `}`. `exg-protocol/src/event.rs`: `Event` enum tail = `LiquidationOrder { user_id, symbol, side, quantity, timestamp }` then `}`.
- `exg-matching-engine/src/engine.rs::process_command` match: arms `NewOrder/CancelOrder/AmendOrder/CancelAllOrders/UpdateMarkPrice/ComputeFunding` — **no catch-all** (adding a `Command` variant breaks exhaustiveness → must add an arm).
- `exg-matching-engine/src/replay.rs::apply_event` (`pub fn apply_event(&mut self, event: &Event) -> Result<(), ApplyError>`): arms end `... FundingRateUpdate => Ok-set-rate; LiquidationOrder => Err(UnexpectedVariant); }` — **no catch-all** (adding `Event` variants breaks exhaustiveness → must add an arm).
- `exg-server/src/lib.rs`: replay loop `reader.read_from(0, |seq, payload| { ... engine.apply_event(&event) ... })` (~lib.rs:307-345) inside a block that ends `replayed_count`; matching thread closure `move ||` with inner `process_one = |n, buf| { decode Command → engine.process_command(&cmd) → for evt: rkyv encode → matching_wal.lock().append }` (~lib.rs:414-432); `ServerHandle { bound_port, admin_bound_port, matching_thread, .. }`.
- `exg-api-gateway/src/admin.rs`: `check_admin_secret(req,&state.cfg.admin.admin_secret)`, `enqueue_admin(state,&cmd)`, `build_admin_app(state)` with `.route("/api/v1/admin/mark-price", web::post().to(..))`. `AdminMarkPriceRequest` in `src/types.rs` (camelCase, stringified decimals).

**rkyv hard constraint:** new `Command`/`Event` variants MUST be appended at the END of their enums so existing variant discriminants are unchanged → Stage 2-era WALs still decode (spec §8.5). Never reorder existing variants.

---

## File Structure

### New files

| Path | Responsibility |
|------|----------------|
| `crates/exg-clearing/src/post_trade.rs` | `PostTradeProcessor` (positions+ledger): `consume` (live), `apply_event` (replay), `handle_admin_credit`; the entire clearing-domain post-trade pipeline |
| `crates/exg-server/tests/stage3_e2e.rs` | e2e: admin-credit → open positions → funding tick moves funds → reboot survives; auth; negative amount; port isolation |
| `scripts/demo-stage3.sh` | Cold-boot demo: credit 2 users → cross orders → funding-tick → wal-dump shows FundingSettled/RealizedPnl → reboot replays |

### Modified files

| Path | Change |
|------|--------|
| `crates/exg-protocol/src/command.rs` | append `Command::AdminCredit` (enum end) |
| `crates/exg-protocol/src/event.rs` | append `AdminCredited` / `RealizedPnl` / `FundingSettled` (enum end) |
| `crates/exg-protocol/src/lib.rs` | extend `all_commands()`/`all_events()` test helpers if present |
| `crates/exg-matching-engine/src/engine.rs` | `process_command`: `Command::AdminCredit { .. } => Vec::new()` arm (engine ignores; clearing-domain) |
| `crates/exg-matching-engine/src/replay.rs` | `apply_event`: `Event::AdminCredited{..}|RealizedPnl{..}|FundingSettled{..} => Ok(())` no-op arm |
| `crates/exg-ledger/src/operations.rs` | **NEW** `settle_realized_pnl_capped` op (CEO C1/C1b: capped-debit, never errors on insufficiency, never drives user negative — used by both realized PnL and funding payment) |
| `crates/exg-clearing/src/lib.rs` | `pub mod post_trade;` |
| `crates/exg-api-gateway/src/types.rs` | `AdminCreditRequest { user_id: u64, amount: String }` |
| `crates/exg-api-gateway/src/admin.rs` | `admin_credit` handler + route `/api/v1/admin/credit` |
| `crates/exg-server/src/lib.rs` | construct `PostTradeProcessor`; replay loop dual-dispatch + `post_trade` ReplayError; matching `process_one` integrate `consume` + `AdminCredit` routing + WAL append order; `verify_all_invariants` post-replay & post-settlement; move `post_trade` into matching thread |
| `crates/exg-server/tests/boot_panics.rs` | +1: corrupt `FundingSettled`/unknown-user → boot abort |

### Test surface

- **Unit** (`exg-clearing` post_trade): ~10 (position projection, realized PnL deposit/withdraw, funding signed/imbalanced/idempotent, admin credit, verify_all_invariants).
- **Replay round-trip equivalence** (`exg-clearing` post_trade, mandatory — spec §7.2): 4 scenarios.
- **Integration** (`exg-server/tests/stage3_e2e.rs`): 5+.
- **boot_panics**: +1.
- **Regression baselines unchanged**: stage0_e2e 7, stage1a_e2e 12, stage1b_e2e 16, stage2_e2e 11, exg-user-service 30, exg-matching-engine `--lib`. The matching thread now also runs `consume`; baselines that place orders but never admin-credit must still pass (a position with a zero-balance user opens fine — no margin check, Q3; only a realized **loss** without funds fails, and baselines don't close at a loss against an unfunded user).

---

## Task overview

| # | Task | Files | Tests added |
|---|------|-------|-------------|
| 1 | Schema: `Command::AdminCredit` + 3 `Event` variants + engine no-op arms | command.rs, event.rs, protocol lib.rs, engine.rs, replay.rs | protocol round-trip |
| 2 | `PostTradeProcessor` skeleton + position projection from `OrderFilled` | post_trade.rs (NEW), clearing lib.rs | ~4 unit |
| 3 | Realized PnL (deposit/withdraw) + `handle_admin_credit` | post_trade.rs | ~3 unit |
| 4 | Funding settlement (`settle_funding_checked` signed) + mark tracking | post_trade.rs | ~4 unit |
| 5 | `apply_event` replay + round-trip equivalence matrix | post_trade.rs | 4 round-trip |
| 6 | Admin `/api/v1/admin/credit` endpoint | types.rs, admin.rs | 0 (e2e in T8) |
| 7 | Boot wiring: replay dual-dispatch + matching-thread integrate + invariants | server lib.rs | 0 (e2e in T8) |
| 8 | stage3_e2e (5+) + boot_panics (+1) + demo + full acceptance | stage3_e2e.rs (NEW), boot_panics.rs, demo-stage3.sh (NEW) | 5+ e2e + 1 boot |

Strict order: T1 (schema, workspace compiles) → T2→T3→T4→T5 (post_trade built + tested standalone in exg-clearing, no server dep) → T6 (admin endpoint) → T7 (server integration, depends T1+T5+T6) → T8 (e2e + acceptance).

---

## Task 1: Schema — `Command::AdminCredit` + 3 `Event` variants + engine no-op arms

**Files:**
- Modify: `crates/exg-protocol/src/command.rs`
- Modify: `crates/exg-protocol/src/event.rs`
- Modify: `crates/exg-protocol/src/lib.rs` (test helpers if present)
- Modify: `crates/exg-matching-engine/src/engine.rs` (`process_command` arm)
- Modify: `crates/exg-matching-engine/src/replay.rs` (`apply_event` arm)

### Why this is one task

Adding a `Command` variant makes `engine.process_command`'s match non-exhaustive; adding `Event` variants makes `engine.apply_event`'s match non-exhaustive — both are compile errors until their no-op arms are added. Schema + the two no-op arms must land together so the workspace compiles. No behavior beyond "engine ignores the clearing-domain variants" (post_trade consumes them, Tasks 2-5).

- [ ] **Step 1: Append `Command::AdminCredit`**

In `crates/exg-protocol/src/command.rs`, inside `pub enum Command { ... }`, immediately AFTER the `ComputeFunding { symbol, timestamp }` variant and BEFORE the enum's closing `}`:

```rust
    /// Stage 3: admin-injected balance credit (bootstraps wallets for
    /// settlement; produced by the admin HTTP server). Clearing-domain —
    /// the matching engine ignores it.
    AdminCredit {
        user_id: UserId,
        amount: Decimal128,
        idempotency_key: String,
        timestamp: UnixMicros,
    },
```

`UserId`, `Decimal128`, `UnixMicros` are already imported in command.rs (used by existing variants). `String` is rkyv-serializable (used elsewhere — verify `grep -n "client_order_id\|String" crates/exg-protocol/src/command.rs`; `NewOrder.client_order_id` is `Option<String>` so `String` rkyv is already in use).

- [ ] **Step 2: Append the 3 `Event` variants**

In `crates/exg-protocol/src/event.rs`, inside `pub enum Event { ... }`, immediately AFTER the `LiquidationOrder { .. }` variant and BEFORE the closing `}`:

```rust
    /// Stage 3: admin balance credit applied to the ledger (fact).
    /// Carries `idempotency_key` so replay re-applies with the exact
    /// original key (self-describing fact — like RealizedPnl/FundingSettled).
    AdminCredited {
        user_id: UserId,
        amount: Decimal128,
        idempotency_key: String,
        timestamp: UnixMicros,
    },
    /// Stage 3: realized PnL on a position reduce/close (fact).
    /// `amount` is signed: positive = profit (credit), negative = loss.
    RealizedPnl {
        user_id: UserId,
        symbol: SymbolId,
        amount: Decimal128,
        timestamp: UnixMicros,
    },
    /// Stage 3: funding payment settled for one position (fact).
    /// `amount` is signed: positive = user paid, negative = user received.
    FundingSettled {
        user_id: UserId,
        symbol: SymbolId,
        funding_period_id: u64,
        amount: Decimal128,
        timestamp: UnixMicros,
    },
```

`UserId`, `SymbolId`, `Decimal128`, `UnixMicros` already imported in event.rs.

- [ ] **Step 3: Extend protocol test helpers if present**

```bash
grep -n "fn all_commands\|fn all_events\|all_commands()\|all_events()" crates/exg-protocol/src/lib.rs
```

If `all_commands()` exists, append (use the file's existing `dec`/`sample_timestamp` helpers — match their exact names):

```rust
            Command::AdminCredit {
                user_id: UserId::new(42),
                amount: dec("1000"),
                idempotency_key: "ac_test_1".into(),
                timestamp: sample_timestamp(),
            },
```

If `all_events()` exists, append:

```rust
            Event::AdminCredited { user_id: UserId::new(42), amount: dec("1000"), idempotency_key: "ac_test_1".into(), timestamp: sample_timestamp() },
            Event::RealizedPnl { user_id: UserId::new(42), symbol: SymbolId::new(1), amount: dec("-25"), timestamp: sample_timestamp() },
            Event::FundingSettled { user_id: UserId::new(42), symbol: SymbolId::new(1), funding_period_id: 1, amount: dec("5"), timestamp: sample_timestamp() },
```

(If a helper does not exist, skip and note it in the report. These exercise rkyv + serde round-trip for the new variants.)

- [ ] **Step 4: Add the engine `process_command` no-op arm**

In `crates/exg-matching-engine/src/engine.rs`, in the `process_command` match (arms end at `Command::ComputeFunding { symbol, timestamp } => { .. }`), add before the match's closing `}`:

```rust
            // Stage 3: clearing-domain command — the matching engine
            // produces no events for it; PostTradeProcessor handles it
            // (routed in the matching thread).
            Command::AdminCredit { .. } => Vec::new(),
```

- [ ] **Step 5: Add the engine `apply_event` no-op arm**

In `crates/exg-matching-engine/src/replay.rs`, in `apply_event`, immediately before the match's closing `}` (after the `Event::LiquidationOrder { .. } => Err(ApplyError::UnexpectedVariant { variant: "LiquidationOrder" })` arm):

```rust
            // Stage 3: clearing-domain fact events — the matching engine
            // ignores them (mirrors OrderRejected/TradeExecuted Ok no-ops).
            // PostTradeProcessor::apply_event consumes them on replay.
            Event::AdminCredited { .. }
            | Event::RealizedPnl { .. }
            | Event::FundingSettled { .. } => Ok(()),
```

(Leave `LiquidationOrder => Err(UnexpectedVariant)` unchanged — Stage 4.)

- [ ] **Step 6: Verify**

```bash
cargo check --workspace --all-targets 2>&1 | tail -5
cargo test -p exg-protocol 2>&1 | tail -8
cargo test -p exg-matching-engine --lib 2>&1 | grep "test result" | tail -2
```

Expected: clean compile (`--all-targets` — benches too, Stage 2 E2 lesson); exg-protocol round-trip green incl. new variants; exg-matching-engine `--lib` unchanged count, all green (the no-op arms add no behavior).

- [ ] **Step 7: Commit**

```bash
git add crates/exg-protocol/src/command.rs crates/exg-protocol/src/event.rs \
        crates/exg-protocol/src/lib.rs crates/exg-matching-engine/src/engine.rs \
        crates/exg-matching-engine/src/replay.rs
git commit -m "$(cat <<'EOF'
feat(protocol): add Stage 3 AdminCredit command + 3 post-trade fact events

- Command::AdminCredit + Event::{AdminCredited,RealizedPnl,FundingSettled}
  appended at enum end (rkyv discriminant stability — Stage 2 WALs still
  decode; spec §8.5)
- engine.process_command: AdminCredit => Vec::new() (clearing-domain,
  engine ignores; post_trade handles it)
- engine.apply_event: 3 new fact events => Ok(()) no-op (mirrors
  OrderRejected/TradeExecuted; post_trade.apply_event consumes on replay)
- AdminCredited carries idempotency_key (self-describing fact — replay
  re-applies with the exact original key)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: `PostTradeProcessor` skeleton + position projection from `OrderFilled`

**Files:**
- Create: `crates/exg-clearing/src/post_trade.rs`
- Modify: `crates/exg-clearing/src/lib.rs`

### Step 0: Verify the real `OrderFilled` field set

```bash
sed -n '/    OrderFilled {/,/    },/p' crates/exg-protocol/src/event.rs
```

Confirm the exact fields (Stage 2 review established: `order_id, trade_id, user_id, symbol, side, fill_price, fill_qty, is_maker, remaining_qty, timestamp`). The `consume` code below destructures `user_id, symbol, side, fill_qty, fill_price` — adjust field names to the real ones if they differ.

- [ ] **Step 1: `pub mod post_trade;` + failing tests (TDD red)**

In `crates/exg-clearing/src/lib.rs` add `pub mod post_trade;` after `pub mod position;`.

Create `crates/exg-clearing/src/post_trade.rs` with the struct + a position-projection test module. First write the failing tests:

```rust
//! Stage 3 — post-trade pipeline (clearing bounded context).
//!
//! Owned by the matching OS thread. `consume` reacts to engine events
//! (positions + ledger money moves, emitting fact events); `apply_event`
//! re-applies the WAL on replay (positions re-projected from OrderFilled;
//! money from recorded facts). No I/O, no locks — single-thread.

use exg_common::{Decimal128, MarginMode, PositionSide, Side, SymbolId, UnixMicros, UserId};
use exg_ledger::Ledger;
use exg_protocol::Event;

use crate::position::PositionManager;

pub struct PostTradeProcessor {
    positions: PositionManager,
    ledger: Ledger,
    mark_price: Decimal128,
    funding_period_id: u64,
}

impl PostTradeProcessor {
    pub fn new() -> Self {
        Self {
            positions: PositionManager::new(),
            ledger: Ledger::new(),
            mark_price: Decimal128::ZERO,
            funding_period_id: 0,
        }
    }

    /// Read-only accessors for tests / boot invariant checks.
    pub fn ledger(&self) -> &Ledger { &self.ledger }
    pub fn positions(&self) -> &PositionManager { &self.positions }

    /// Map a fill `Side` to the `PositionSide` it opens.
    fn opening_side(side: Side) -> PositionSide {
        match side {
            Side::Buy => PositionSide::Long,
            Side::Sell => PositionSide::Short,
        }
    }
}

impl Default for PostTradeProcessor {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use exg_protocol::Event;

    fn dec(s: &str) -> Decimal128 { s.parse().unwrap() }
    fn ts() -> UnixMicros { UnixMicros::from_micros(1_700_000_000_000_000) }

    fn filled(user: u64, side: Side, qty: &str, price: &str) -> Event {
        // Adjust to the real OrderFilled field set verified in Step 0.
        Event::OrderFilled {
            order_id: exg_common::OrderId::new(user),
            trade_id: exg_common::TradeId::new(1),
            user_id: UserId::new(user),
            symbol: SymbolId::new(1),
            side,
            fill_price: dec(price),
            fill_qty: dec(qty),
            is_maker: false,
            remaining_qty: Decimal128::ZERO,
            timestamp: ts(),
        }
    }

    #[test]
    fn fill_opens_long_position_no_money() {
        let mut pt = PostTradeProcessor::new();
        let out = pt.consume(&[filled(42, Side::Buy, "2", "60000")], ts());
        let pos = pt.positions().get_position(UserId::new(42), SymbolId::new(1)).unwrap();
        assert_eq!(pos.size, dec("2"));
        assert_eq!(pos.side, PositionSide::Long);
        assert_eq!(pos.entry_price, dec("60000"));
        // Opening produces no money fact event.
        assert!(!out.iter().any(|e| matches!(e, Event::RealizedPnl { .. })));
    }

    #[test]
    fn same_side_fill_increases_weighted_avg_entry() {
        let mut pt = PostTradeProcessor::new();
        pt.consume(&[filled(42, Side::Buy, "2", "60000")], ts());
        pt.consume(&[filled(42, Side::Buy, "2", "62000")], ts());
        let pos = pt.positions().get_position(UserId::new(42), SymbolId::new(1)).unwrap();
        assert_eq!(pos.size, dec("4"));
        assert_eq!(pos.entry_price, dec("61000")); // (2*60000 + 2*62000)/4
    }

    #[test]
    fn opposite_fill_reduces_position() {
        let mut pt = PostTradeProcessor::new();
        pt.consume(&[filled(42, Side::Buy, "3", "60000")], ts());
        pt.consume(&[filled(42, Side::Sell, "1", "61000")], ts());
        let pos = pt.positions().get_position(UserId::new(42), SymbolId::new(1)).unwrap();
        assert_eq!(pos.size, dec("2")); // 3 long - 1 sold = 2 long
    }

    #[test]
    fn trade_executed_does_not_double_count_position() {
        let mut pt = PostTradeProcessor::new();
        pt.consume(&[
            filled(42, Side::Buy, "2", "60000"),
            Event::TradeExecuted {
                // same trade described again — MUST NOT touch positions
                trade_id: exg_common::TradeId::new(1),
                symbol: SymbolId::new(1),
                price: dec("60000"),
                qty: dec("2"),
                buyer_order_id: exg_common::OrderId::new(42),
                seller_order_id: exg_common::OrderId::new(43),
                buyer_user_id: UserId::new(42),
                seller_user_id: UserId::new(43),
                buyer_fee: Decimal128::ZERO,
                seller_fee: Decimal128::ZERO,
                timestamp: ts(),
            },
        ], ts());
        let pos = pt.positions().get_position(UserId::new(42), SymbolId::new(1)).unwrap();
        assert_eq!(pos.size, dec("2"), "TradeExecuted must not double-count");
    }
}
```

Run: `cargo test -p exg-clearing post_trade 2>&1 | tail` → RED (`consume` not defined). Adjust `Event::OrderFilled`/`Event::TradeExecuted` literals to the **real** field sets (Step 0 + `sed -n '/    TradeExecuted {/,/    },/p' crates/exg-protocol/src/event.rs`).

- [ ] **Step 2: Implement `consume` position projection (green)**

Add to `impl PostTradeProcessor`:

```rust
    /// Live path: react to engine events. Stage 2: positions only
    /// (money in Tasks 3-4). Positions project from `OrderFilled` ONLY —
    /// `TradeExecuted` describes the same trade and would double-count
    /// (spec invariant 34).
    pub fn consume(&mut self, events: &[Event], _ts: UnixMicros) -> Vec<Event> {
        let mut out = Vec::new();
        for e in events {
            if let Event::OrderFilled { user_id, symbol, side, fill_qty, fill_price, .. } = e {
                self.apply_fill_to_position(*user_id, *symbol, *side, *fill_qty, *fill_price);
                // RealizedPnl emission added in Task 3.
            }
        }
        out
    }

    /// Position-keeping: a fill in the position's direction (or no
    /// position) increases; an opposite fill reduces/closes (flipping if
    /// it exceeds current size). Returns the signed realized PnL produced
    /// by any reduction (0 when only opening/increasing) — Task 3 uses it.
    fn apply_fill_to_position(
        &mut self,
        user_id: UserId,
        symbol: SymbolId,
        side: Side,
        qty: Decimal128,
        price: Decimal128,
    ) -> Decimal128 {
        let fill_side = Self::opening_side(side);
        let cur = self.positions.get_position(user_id, symbol).map(|p| (p.side, p.size));
        match cur {
            None => {
                self.positions.open_or_increase(
                    user_id, symbol, fill_side, qty, price,
                    Decimal128::ONE, MarginMode::Cross,
                );
                Decimal128::ZERO
            }
            Some((pos_side, pos_size)) if pos_side == fill_side || pos_size.is_zero() => {
                self.positions.open_or_increase(
                    user_id, symbol, fill_side, qty, price,
                    Decimal128::ONE, MarginMode::Cross,
                );
                Decimal128::ZERO
            }
            Some((_pos_side, pos_size)) => {
                // Opposite fill → reduce/close.
                let reduce_qty = qty.min(pos_size);
                let (pnl, _) = self
                    .positions
                    .reduce_or_close(user_id, symbol, reduce_qty, price)
                    .expect("reduce_or_close on an existing position");
                let leftover = qty - reduce_qty;
                if leftover.is_positive() {
                    // Flipped: open the remainder on the opposite side.
                    self.positions.open_or_increase(
                        user_id, symbol, fill_side, leftover, price,
                        Decimal128::ONE, MarginMode::Cross,
                    );
                }
                pnl
            }
        }
    }
```

Confirm `Decimal128::ONE` exists (`grep -n "pub const ONE\|const ONE" crates/exg-common/src/decimal*.rs`); if not, use `"1".parse::<Decimal128>().unwrap()` or a `dec("1")` local. `Decimal128::ONE` for leverage is a placeholder — Stage 3 does not enforce margin (Q3); `Position.margin` is informational only.

- [ ] **Step 3: Run — green**

```bash
cargo test -p exg-clearing post_trade 2>&1 | tail -8
cargo clippy -p exg-clearing 2>&1 | tail -3
```

Expected: 4 new tests pass; clippy clean. (Existing exg-clearing tests untouched.)

- [ ] **Step 4: Commit**

```bash
git add crates/exg-clearing/src/post_trade.rs crates/exg-clearing/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(clearing): PostTradeProcessor skeleton + position projection

- new post_trade module: PostTradeProcessor { positions, ledger,
  mark_price, funding_period_id }
- consume() projects positions from OrderFilled ONLY (TradeExecuted
  excluded — anti-double-count, spec invariant 34); open/increase vs
  reduce/close/flip position-keeping
- money (realized PnL / funding / admin credit) in Tasks 3-4

4 unit tests: open long, weighted-avg increase, opposite reduce,
TradeExecuted no-double-count.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Realized PnL (deposit/withdraw) + `handle_admin_credit`

**Files:**
- Modify: `crates/exg-clearing/src/post_trade.rs`

- [ ] **Step 1: Failing tests (red)**

Add to the `post_trade.rs` test module:

```rust
    #[test]
    fn admin_credit_deposits_funding_wallet() {
        let mut pt = PostTradeProcessor::new();
        let out = pt.handle_admin_credit(UserId::new(42), dec("5000"), "ac_1", ts());
        assert!(matches!(out[0], Event::AdminCredited { .. }));
        let bal = pt.ledger()
            .get_balance(UserId::new(42), exg_ledger::WalletType::Funding)
            .unwrap();
        assert_eq!(bal.available, dec("5000"));
        pt.ledger().verify_all_invariants().unwrap();
    }

    #[test]
    fn realized_profit_credits_user() {
        let mut pt = PostTradeProcessor::new();
        pt.handle_admin_credit(UserId::new(42), dec("10000"), "ac_2", ts());
        // open long 1 @60000, close 1 @61000 → +1000 profit
        let out = pt.consume(&[
            filled(42, Side::Buy, "1", "60000"),
            filled(42, Side::Sell, "1", "61000"),
        ], ts());
        let pnl = out.iter().find_map(|e| match e {
            Event::RealizedPnl { amount, .. } => Some(*amount),
            _ => None,
        }).expect("RealizedPnl emitted");
        assert_eq!(pnl, dec("1000"));
        let bal = pt.ledger()
            .get_balance(UserId::new(42), exg_ledger::WalletType::Funding).unwrap();
        assert_eq!(bal.available, dec("11000")); // 10000 + 1000 profit
        pt.ledger().verify_all_invariants().unwrap();
    }

    #[test]
    fn realized_loss_debits_user() {
        let mut pt = PostTradeProcessor::new();
        pt.handle_admin_credit(UserId::new(42), dec("10000"), "ac_3", ts());
        // long 1 @60000, close 1 @59000 → -1000 loss
        let out = pt.consume(&[
            filled(42, Side::Buy, "1", "60000"),
            filled(42, Side::Sell, "1", "59000"),
        ], ts());
        let pnl = out.iter().find_map(|e| match e {
            Event::RealizedPnl { amount, .. } => Some(*amount), _ => None,
        }).unwrap();
        assert_eq!(pnl, dec("-1000"));
        let bal = pt.ledger()
            .get_balance(UserId::new(42), exg_ledger::WalletType::Funding).unwrap();
        assert_eq!(bal.available, dec("9000"));
        pt.ledger().verify_all_invariants().unwrap();
    }
```

`exg_ledger::WalletType` + `Ledger::get_balance(user, WalletType) -> Option<&WalletBalance>` (`.available` field) are verified APIs. Run → RED (`handle_admin_credit` missing; `consume` emits no `RealizedPnl`).

- [ ] **Step 2: Implement `handle_admin_credit` + realized-PnL emission (green)**

Add to `impl PostTradeProcessor`:

```rust
    /// Admin-credit a user's Funding wallet (ledger journals SYSTEM→user;
    /// idempotent on `idempotency_key`). Emits the AdminCredited fact.
    pub fn handle_admin_credit(
        &mut self,
        user_id: UserId,
        amount: Decimal128,
        idempotency_key: &str,
        ts: UnixMicros,
    ) -> Vec<Event> {
        self.ledger.get_or_create_account(user_id);
        self.ledger
            .deposit(user_id, amount, idempotency_key, ts)
            .expect("admin credit deposit (amount > 0 enforced at handler)");
        vec![Event::AdminCredited {
            user_id,
            amount,
            idempotency_key: idempotency_key.to_owned(),
            timestamp: ts,
        }]
    }

    /// Settle a signed realized PnL vs the system Funding pool, returning
    /// the **actually-moved** signed amount (the value recorded in the
    /// RealizedPnl fact event). CEO review C1/C1b: a loss the user cannot
    /// cover MUST NOT panic the single-threaded exchange and MUST NOT
    /// drive `verify_account_invariant` (forbids negative user available)
    /// to fail. Uses the new capped-debit ledger primitive
    /// `settle_realized_pnl` (Step 0 below): profit → SYSTEM→user; loss →
    /// move only `min(loss, user.Funding.available)` (user floored at 0),
    /// uncollected remainder = implicit bad debt absorbed by the SYSTEM
    /// Funding pool (allowed negative). Returns the signed moved amount.
    fn settle_realized_pnl(
        &mut self,
        user_id: UserId,
        symbol: SymbolId,
        pnl: Decimal128,
        seq_tag: u64,
        ts: UnixMicros,
    ) -> Decimal128 {
        if pnl.is_zero() {
            return Decimal128::ZERO;
        }
        let key = format!("pnl_{seq_tag}_{}_{}", user_id.value(), symbol.value());
        self.ledger.get_or_create_account(user_id);
        // settle_realized_pnl_capped: signed pnl; never errors on
        // insufficiency, never drives user available negative; returns the
        // signed amount actually moved (== pnl for profit or covered loss,
        // == -(available) for an underfunded loss). Idempotent on `key`.
        let moved = self
            .ledger
            .settle_realized_pnl_capped(user_id, pnl, &key, ts)
            .expect("settle_realized_pnl_capped is infallible for a known account");
        // CEO review C4: realized-PnL audit line (spec invariant 39).
        if !moved.is_zero() {
            tracing::info!(
                target: "post_trade",
                user_id = user_id.value(),
                symbol = symbol.value(),
                %moved,
                "realized pnl"
            );
        }
        moved
    }
```

- [ ] **Step 0 (CEO review C1/C1b): add the capped-debit ledger primitive**

`reduce_or_close`/funding can produce a debit a user cannot cover. No
existing `exg-ledger` op is safe here: `withdraw` and `settle_funding`
**error `InsufficientBalance`** (→ `.expect()` → matching-thread panic →
whole single-threaded exchange down for ALL users — a user's normal
losing close cannot be allowed to do that); `transfer` is same-user only;
`verify_account_invariant` (invariant.rs) **hard-forbids** a negative
user wallet `available`, so the user cannot simply go negative either.

Add to `crates/exg-ledger/src/operations.rs` (TDD: write the ledger unit
test first — see this task's test step):

```rust
    /// Stage 3 (CEO review C1/C1b): settle a SIGNED realized-PnL/funding
    /// amount vs the SYSTEM Funding pool, capping a debit at the user's
    /// available so the user is never driven negative and the call never
    /// errors on insufficiency. Returns the signed amount actually moved.
    ///
    /// `signed > 0` (credit / user receives): user Funding available +=
    /// signed; balanced journal SYSTEM→user (RealizedPnl entry_type).
    /// `signed < 0` (debit / user pays): moved = min(|signed|, available);
    /// user available -= moved; balanced journal user→SYSTEM for `moved`.
    /// The uncollected `|signed| - moved` is implicit bad debt — NOT
    /// journaled (only balanced transfers stay on the books, preserving
    /// verify_global_invariant); it surfaces as the SYSTEM Funding pool
    /// going more negative than user credits, which verify_all_invariants
    /// explicitly permits (Funding ∉ NON_NEGATIVE_SYSTEM_WALLETS).
    /// Idempotent on `idempotency_key`. Never returns Err for a known
    /// account; `AccountNotFound` only if the user account is absent.
    pub fn settle_realized_pnl_capped(
        &mut self,
        user_id: UserId,
        signed: Decimal128,
        idempotency_key: &str,
        timestamp: UnixMicros,
    ) -> ExgResult<Decimal128> {
        if signed.is_zero() {
            return Ok(Decimal128::ZERO);
        }
        if self.check_idempotency(idempotency_key) {
            return Ok(Decimal128::ZERO); // already applied (replay no-op)
        }
        let account = self
            .accounts
            .get_mut(&user_id)
            .ok_or(ExgError::AccountNotFound(user_id))?;
        let bal = account.wallet_mut(WalletType::Funding);
        let moved: Decimal128;
        if signed.is_positive() {
            bal.available = bal.available + signed;
            // Eng review E1: double-entry — system delta = -(user delta).
            // A credit to the user DECREASES the SYSTEM Funding pool.
            // (Mirrors real settle_funding's user-receives branch, which
            // passes a NEGATIVE payment to add_system_balance.) Using the
            // wrong sign (+signed) makes user_total+system_total drift by
            // 2*signed → verify_global_invariant fails → panic.
            self.add_system_balance(WalletType::Funding, -signed);
            moved = signed;
            let id = self.next_id();
            self.append_journal(JournalEntry {
                id,
                debit_user: SYSTEM_USER_ID,
                debit_wallet: WalletType::Funding,
                debit_field: BalanceField::Available,
                credit_user: user_id,
                credit_wallet: WalletType::Funding,
                credit_field: BalanceField::Available,
                amount: signed,
                entry_type: JournalEntryType::FundingPayment,
                idempotency_key: idempotency_key.to_owned(),
                timestamp,
            });
        } else {
            let owed = signed.abs();
            let cover = owed.min(bal.available);
            bal.available = bal.available - cover;
            // Eng review E1: system delta = -(user delta). The user PAYS
            // `cover`, so the SYSTEM Funding pool RECEIVES it → +cover.
            // (Mirrors real settle_funding's user-pays branch:
            // add_system_balance(Funding, +payment).) `moved` is the
            // SIGNED user delta (negative for a debit) — that is what the
            // RealizedPnl/FundingSettled fact records.
            self.add_system_balance(WalletType::Funding, cover);
            moved = -cover;
            if cover.is_positive() {
                let id = self.next_id();
                self.append_journal(JournalEntry {
                    id,
                    debit_user: user_id,
                    debit_wallet: WalletType::Funding,
                    debit_field: BalanceField::Available,
                    credit_user: SYSTEM_USER_ID,
                    credit_wallet: WalletType::Funding,
                    credit_field: BalanceField::Available,
                    amount: cover,
                    entry_type: JournalEntryType::FundingPayment,
                    idempotency_key: idempotency_key.to_owned(),
                    timestamp,
                });
            }
            // owed - cover = implicit bad debt: intentionally NOT journaled
            // and NOT added anywhere (keeps user_total+system_total
            // unchanged → verify_global_invariant balanced). The shortfall
            // surfaces because the winning side got a full credit
            // (system -win) while this losing side only paid `cover`
            // (system +cover), so the SYSTEM Funding pool nets negative by
            // exactly the uncollected amount — which verify_all_invariants
            // permits (Funding ∉ NON_NEGATIVE_SYSTEM_WALLETS). Stage 4 adds
            // explicit bad-debt accounting.
        }
        Ok(moved)
    }
```

Eng-review-verified against real source (do not re-derive): `JournalEntry`
fields = `{id, debit_user, debit_wallet, debit_field, credit_user,
credit_wallet, credit_field, amount, entry_type, idempotency_key,
timestamp}`; `BalanceField::{Available,Frozen,Margin}`;
`JournalEntryType::FundingPayment` (audit-only here — `verify_global_invariant`
only sums `Deposit`/`Withdrawal`); `add_system_balance(w, amount)` does
`system_accounts[w] += amount` (NOT inverted); `SYSTEM_USER_ID =
UserId(0)` (private const, in-crate — usable); `Decimal128` has `impl
Neg` (use `-x`) + `ONE`/`ZERO`. **Eng review E1 (sign, load-bearing):**
SYSTEM Funding-pool delta MUST be `-(user available delta)` — credit
user → `add_system_balance(Funding, -signed)`; debit user →
`add_system_balance(Funding, +cover)`. Wrong sign drifts
`user_total+system_total` by 2× → `verify_global_invariant` panic.

- [ ] **Step 0b (Eng review E2): dedicated `settle_realized_pnl_capped` ledger unit tests**

In the `crates/exg-ledger/src/operations.rs` test module (TDD: write
these RED before Step 0's impl; reuse the file's `dec`/`ts`/`uid`/
`setup_futures_user` helpers — `grep -n "fn dec\|fn ts\|fn uid\|fn setup_futures_user" crates/exg-ledger/src/operations.rs`):

```rust
    #[test]
    fn srpc_profit_credits_user_debits_pool_invariants_hold() {
        let mut l = Ledger::new();
        l.get_or_create_account(uid(1));
        let moved = l.settle_realized_pnl_capped(uid(1), dec("500"), "k1", ts(1)).unwrap();
        assert_eq!(moved, dec("500"));
        assert_eq!(l.get_balance(uid(1), WalletType::Funding).unwrap().available, dec("500"));
        // system Funding pool decreased by the credit (E1 sign).
        assert_eq!(l.system_balance(WalletType::Funding), dec("-500"));
        l.verify_all_invariants().unwrap(); // user_total+system_total == net_external (0)
    }

    #[test]
    fn srpc_covered_loss_debits_user_credits_pool() {
        let mut l = Ledger::new();
        l.get_or_create_account(uid(1));
        l.settle_realized_pnl_capped(uid(1), dec("1000"), "fund", ts(1)).unwrap(); // fund first
        let moved = l.settle_realized_pnl_capped(uid(1), dec("-300"), "k2", ts(2)).unwrap();
        assert_eq!(moved, dec("-300"));
        assert_eq!(l.get_balance(uid(1), WalletType::Funding).unwrap().available, dec("700"));
        l.verify_all_invariants().unwrap();
    }

    #[test]
    fn srpc_underfunded_loss_caps_at_zero_pool_goes_negative_no_err() {
        let mut l = Ledger::new();
        l.get_or_create_account(uid(1));
        l.settle_realized_pnl_capped(uid(1), dec("100"), "fund", ts(1)).unwrap();
        // owe 500, only 100 available → cover 100, user→0, never Err.
        let moved = l.settle_realized_pnl_capped(uid(1), dec("-500"), "k3", ts(2)).unwrap();
        assert_eq!(moved, dec("-100"));
        assert_eq!(l.get_balance(uid(1), WalletType::Funding).unwrap().available, dec("0"));
        // implicit bad debt: pool net negative by the uncollected amount.
        // (pool: -100 from the fund credit, +100 from the covered debit = 0;
        //  the bad debt surfaces vs a winner credit in a real trade — here
        //  with no winner, assert the user is floored and invariants hold.)
        l.verify_all_invariants().unwrap();
    }

    #[test]
    fn srpc_idempotent_on_key() {
        let mut l = Ledger::new();
        l.get_or_create_account(uid(1));
        l.settle_realized_pnl_capped(uid(1), dec("200"), "dup", ts(1)).unwrap();
        let again = l.settle_realized_pnl_capped(uid(1), dec("200"), "dup", ts(2)).unwrap();
        assert_eq!(again, dec("0"), "duplicate key is a no-op");
        assert_eq!(l.get_balance(uid(1), WalletType::Funding).unwrap().available, dec("200"));
        l.verify_all_invariants().unwrap();
    }

    #[test]
    fn srpc_zero_is_noop() {
        let mut l = Ledger::new();
        l.get_or_create_account(uid(1));
        assert_eq!(l.settle_realized_pnl_capped(uid(1), Decimal128::ZERO, "z", ts(1)).unwrap(), Decimal128::ZERO);
        l.verify_all_invariants().unwrap();
    }
```

Adjust `WalletType`/`get_balance`/`system_balance`/helper names to the
real `operations.rs` test conventions (verified APIs:
`system_balance(WalletType) -> Decimal128`,
`get_balance(UserId, WalletType) -> Option<&WalletBalance>` with
`.available`). These five are the direct E1 regression guard.
```

Then thread PnL through `consume`. Replace the `consume` `OrderFilled` branch body with:

```rust
            match e {
                Event::OrderFilled { user_id, symbol, side, fill_qty, fill_price, .. } => {
                    let pnl = self.apply_fill_to_position(
                        *user_id, *symbol, *side, *fill_qty, *fill_price,
                    );
                    if !pnl.is_zero() {
                        // seq_tag: a per-consume monotonic discriminator so
                        // each realized-PnL settlement has a unique
                        // idempotency key. Use the running event index.
                        self.settle_realized_pnl(*user_id, *symbol, pnl, self.next_pnl_seq(), *_ts_unused);
                        out.push(Event::RealizedPnl {
                            user_id: *user_id, symbol: *symbol, amount: pnl,
                            timestamp: *ts_param,
                        });
                    }
                }
                _ => {}
            }
```

To make idempotency keys stable across live↔replay, do NOT use a transient counter; derive `seq_tag` deterministically. Add a field `pnl_seq: u64` to the struct (monotonic, incremented per emitted `RealizedPnl`), persisted-by-replay because replay re-applies `RealizedPnl` facts with the SAME key. Concretely:

- Add `pnl_seq: u64` to `PostTradeProcessor` (init `0` in `new()`).
- `fn next_pnl_seq(&mut self) -> u64 { self.pnl_seq += 1; self.pnl_seq }`.
- Live: `consume` calls `next_pnl_seq()` for the key; the emitted `RealizedPnl` event does NOT carry the seq (key is `pnl_{seq}_{user}_{symbol}`), so replay must reconstruct the SAME sequence. Because replay re-applies `RealizedPnl` events in WAL order and increments `pnl_seq` identically (Task 5 `apply_event` calls the same `next_pnl_seq()` path), the keys match. **This determinism is load-bearing — Task 5 must increment `pnl_seq` in the exact same order.** (Document this in the code comment.)

Fix the snippet's placeholder names: `consume(&mut self, events: &[Event], ts_param: UnixMicros)` — use `ts_param` consistently for the emitted event timestamps; drop the earlier `_ts`/`_ts_unused` placeholders. Final `consume`:

```rust
    pub fn consume(&mut self, events: &[Event], ts_param: UnixMicros) -> Vec<Event> {
        let mut out = Vec::new();
        for e in events {
            if let Event::OrderFilled { user_id, symbol, side, fill_qty, fill_price, .. } = e {
                let pnl = self.apply_fill_to_position(*user_id, *symbol, *side, *fill_qty, *fill_price);
                if !pnl.is_zero() {
                    let seq = self.next_pnl_seq();
                    // CEO C1/C1b: record the ACTUALLY-MOVED signed amount
                    // (capped at the user's available for a loss), not the
                    // notional pnl — replay applies this fact directly.
                    let moved = self.settle_realized_pnl(*user_id, *symbol, pnl, seq, ts_param);
                    if !moved.is_zero() {
                        out.push(Event::RealizedPnl {
                            user_id: *user_id, symbol: *symbol, amount: moved, timestamp: ts_param,
                        });
                    }
                }
            }
        }
        out
    }
```

- [ ] **Step 3: Run — green**

```bash
cargo test -p exg-clearing post_trade 2>&1 | tail -10
cargo clippy -p exg-clearing 2>&1 | tail -3
```

Expected: all Task 2 + 3 new tests pass; clippy clean. `verify_all_invariants()` holds in every test.

- [ ] **Step 4: Commit**

```bash
git add crates/exg-clearing/src/post_trade.rs
git commit -m "$(cat <<'EOF'
feat(clearing): post_trade realized PnL (deposit/withdraw) + admin credit

- handle_admin_credit → ledger.deposit (SYSTEM→user Funding), emits
  AdminCredited
- consume reduce/close → signed realized PnL → deposit(profit)/
  withdraw(loss) vs SYSTEM Funding, emits RealizedPnl
- deterministic pnl_seq idempotency key (pnl_{seq}_{user}_{symbol}) —
  replay re-applies the same key (load-bearing for Task 5)
- realized loss exceeding Funding = fail-fast (spec §5.4 → Stage 4)

3 unit tests: admin credit deposit, realized profit credit, realized
loss debit; verify_all_invariants holds throughout.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Funding settlement (`settle_funding_checked` signed) + mark tracking

**Files:**
- Modify: `crates/exg-clearing/src/post_trade.rs`

- [ ] **Step 1: Failing tests (red)**

Add to the test module:

```rust
    #[test]
    fn mark_price_update_tracked_for_notional() {
        let mut pt = PostTradeProcessor::new();
        pt.consume(&[Event::MarkPriceUpdate {
            symbol: SymbolId::new(1), mark_price: dec("60000"),
            index_price: dec("60000"), timestamp: ts(),
        }], ts());
        // No position → funding tick is a no-op but mark must be stored.
        let out = pt.consume(&[Event::FundingRateUpdate {
            symbol: SymbolId::new(1), funding_rate: dec("0.0001"), timestamp: ts(),
        }], ts());
        assert!(!out.iter().any(|e| matches!(e, Event::FundingSettled { .. })));
    }

    #[test]
    fn funding_long_pays_short_receives() {
        let mut pt = PostTradeProcessor::new();
        pt.handle_admin_credit(UserId::new(1), dec("100000"), "c1", ts());
        pt.handle_admin_credit(UserId::new(2), dec("100000"), "c2", ts());
        // user1 long 1 @60000, user2 short 1 @60000 (they cross)
        pt.consume(&[filled(1, Side::Buy, "1", "60000")], ts());
        pt.consume(&[filled(2, Side::Sell, "1", "60000")], ts());
        pt.consume(&[Event::MarkPriceUpdate {
            symbol: SymbolId::new(1), mark_price: dec("60000"),
            index_price: dec("60000"), timestamp: ts(),
        }], ts());
        // rate 0.01 → long pays 60000*1*0.01 = 600 ; short receives 600
        let out = pt.consume(&[Event::FundingRateUpdate {
            symbol: SymbolId::new(1), funding_rate: dec("0.01"), timestamp: ts(),
        }], ts());
        let settled: Vec<_> = out.iter().filter_map(|e| match e {
            Event::FundingSettled { user_id, amount, funding_period_id, .. } =>
                Some((user_id.value(), *amount, *funding_period_id)),
            _ => None,
        }).collect();
        assert_eq!(settled.len(), 2);
        // long (user1) pays +600 ; short (user2) receives -600
        assert!(settled.iter().any(|(u, a, p)| *u == 1 && *a == dec("600") && *p == 1));
        assert!(settled.iter().any(|(u, a, p)| *u == 2 && *a == dec("-600") && *p == 1));
        let b1 = pt.ledger().get_balance(UserId::new(1), exg_ledger::WalletType::Funding).unwrap();
        let b2 = pt.ledger().get_balance(UserId::new(2), exg_ledger::WalletType::Funding).unwrap();
        assert_eq!(b1.available, dec("99400")); // 100000 - 600
        assert_eq!(b2.available, dec("100600")); // 100000 + 600
        pt.ledger().verify_all_invariants().unwrap();
    }

    #[test]
    fn funding_period_id_increments_per_tick() {
        let mut pt = PostTradeProcessor::new();
        pt.handle_admin_credit(UserId::new(1), dec("100000"), "c1", ts());
        pt.consume(&[filled(1, Side::Buy, "1", "60000")], ts());
        pt.consume(&[Event::MarkPriceUpdate {
            symbol: SymbolId::new(1), mark_price: dec("60000"),
            index_price: dec("60000"), timestamp: ts(),
        }], ts());
        let o1 = pt.consume(&[Event::FundingRateUpdate { symbol: SymbolId::new(1), funding_rate: dec("0.001"), timestamp: ts() }], ts());
        let o2 = pt.consume(&[Event::FundingRateUpdate { symbol: SymbolId::new(1), funding_rate: dec("0.001"), timestamp: ts() }], ts());
        let p1 = o1.iter().find_map(|e| if let Event::FundingSettled { funding_period_id, .. } = e { Some(*funding_period_id) } else { None }).unwrap();
        let p2 = o2.iter().find_map(|e| if let Event::FundingSettled { funding_period_id, .. } = e { Some(*funding_period_id) } else { None }).unwrap();
        assert_eq!((p1, p2), (1, 2));
    }

    #[test]
    fn funding_imbalanced_book_invariants_hold() {
        let mut pt = PostTradeProcessor::new();
        pt.handle_admin_credit(UserId::new(1), dec("100000"), "c1", ts());
        // only a long, no offsetting short → Funding pool nets negative,
        // which verify_all_invariants explicitly permits.
        pt.consume(&[filled(1, Side::Buy, "2", "60000")], ts());
        pt.consume(&[Event::MarkPriceUpdate { symbol: SymbolId::new(1), mark_price: dec("60000"), index_price: dec("60000"), timestamp: ts() }], ts());
        pt.consume(&[Event::FundingRateUpdate { symbol: SymbolId::new(1), funding_rate: dec("0.01"), timestamp: ts() }], ts());
        pt.ledger().verify_all_invariants().unwrap();
    }
```

Run → RED (`consume` ignores MarkPriceUpdate/FundingRateUpdate).

- [ ] **Step 2: Implement mark tracking + funding settlement (green)**

Extend the `consume` loop to also match `MarkPriceUpdate` and `FundingRateUpdate`:

```rust
    pub fn consume(&mut self, events: &[Event], ts_param: UnixMicros) -> Vec<Event> {
        let mut out = Vec::new();
        for e in events {
            match e {
                Event::OrderFilled { user_id, symbol, side, fill_qty, fill_price, .. } => {
                    let pnl = self.apply_fill_to_position(*user_id, *symbol, *side, *fill_qty, *fill_price);
                    if !pnl.is_zero() {
                        let seq = self.next_pnl_seq();
                        let moved = self.settle_realized_pnl(*user_id, *symbol, pnl, seq, ts_param);
                        if !moved.is_zero() {
                            out.push(Event::RealizedPnl { user_id: *user_id, symbol: *symbol, amount: moved, timestamp: ts_param });
                        }
                    }
                }
                Event::MarkPriceUpdate { mark_price, .. } => {
                    self.mark_price = *mark_price;
                }
                Event::FundingRateUpdate { funding_rate, .. } => {
                    out.extend(self.settle_funding(*funding_rate, ts_param));
                }
                _ => {}
            }
        }
        out
    }

    /// Settle funding for every open position at the current mark.
    /// `payment = size * mark_price * rate`, passed SIGNED to the ledger
    /// (>0 long pays / <0 short receives — settle_funding handles both).
    /// One atomic batch per tick (spec invariant 33); verify invariants
    /// after. Returns one FundingSettled fact per position.
    fn settle_funding(&mut self, rate: Decimal128, ts: UnixMicros) -> Vec<Event> {
        // Snapshot (user,symbol,size,side) first — the capped primitive
        // borrows the ledger mutably; avoid aliasing the positions iter.
        let rows: Vec<(UserId, SymbolId, Decimal128, PositionSide)> = self
            .positions
            .all_positions()
            .filter(|p| !p.size.is_zero())
            .map(|p| (p.user_id, p.symbol, p.size, p.side))
            .collect();
        // CEO review C3: a funding tick with open positions but no mark
        // price set would charge everyone 0 and look like success — a
        // silent correctness failure. Warn + skip (no period bump, no
        // events).
        if !rows.is_empty() && self.mark_price.is_zero() {
            tracing::warn!(
                target: "post_trade",
                open_positions = rows.len(),
                "funding tick skipped: mark_price unset"
            );
            return Vec::new();
        }
        self.funding_period_id += 1;
        let period = self.funding_period_id;
        let mark = self.mark_price;
        let mut out = Vec::new();
        let mut settled_count = 0u64;
        let mut total_abs = Decimal128::ZERO;
        for (user_id, symbol, size, side) in rows {
            // notional always positive (size is magnitude); a Short pays a
            // sign-flipped amount so long/short directions net.
            let notional = size * mark;
            let signed_rate = match side {
                PositionSide::Long | PositionSide::Both => rate,
                PositionSide::Short => Decimal128::ZERO - rate,
            };
            let payment = notional * signed_rate; // >0 user pays, <0 receives
            if payment.is_zero() {
                continue;
            }
            // CEO C1/C1b: route funding through the SAME capped-debit
            // primitive as realized PnL. A Long that cannot cover a
            // funding payment must NOT panic the exchange; the moved
            // amount (capped) is what the FundingSettled fact records.
            // Deterministic idempotency key matches settle_funding_checked's
            // scheme so live↔replay align.
            self.ledger.get_or_create_account(user_id);
            let key = format!("funding_{period}_{}_{}", user_id.value(), symbol.value());
            let moved = self
                .ledger
                .settle_realized_pnl_capped(user_id, payment, &key, ts)
                .expect("capped funding settle is infallible for a known account");
            if moved.is_zero() {
                continue;
            }
            settled_count += 1;
            total_abs = total_abs + moved.abs();
            out.push(Event::FundingSettled {
                user_id, symbol, funding_period_id: period, amount: moved, timestamp: ts,
            });
        }
        // CEO review C4: settlement audit line before the invariant gate.
        tracing::info!(
            target: "post_trade",
            period, settled_count, %total_abs,
            "funding batch"
        );
        self.ledger
            .verify_all_invariants()
            .expect("ledger invariants after funding batch (spec invariant 32)");
        out
    }
```

`Decimal128::ZERO - rate` negates (confirm `Sub` is impl'd for `Decimal128` — it is, used in engine `mark - index`). If a `.neg()`/unary `-` exists prefer it (`grep -n "impl Neg\|fn neg\|Sub for Decimal128" crates/exg-common/src/decimal*.rs`).

- [ ] **Step 3: Run — green**

```bash
cargo test -p exg-clearing post_trade 2>&1 | tail -12
cargo clippy -p exg-clearing -- -D warnings 2>&1 | tail -3
```

Expected: Tasks 2-4 tests all green; clippy clean.

- [ ] **Step 4: Commit**

```bash
git add crates/exg-clearing/src/post_trade.rs
git commit -m "$(cat <<'EOF'
feat(clearing): post_trade funding settlement + mark tracking

- consume tracks mark from MarkPriceUpdate; FundingRateUpdate →
  settle_funding for every open position
- payment = size*mark*rate, SIGNED (long pays +, short receives -);
  ledger.settle_funding_checked handles direction internally; key
  funding_{period}_{user}_{symbol} (deterministic, replay-safe)
- funding_period_id ++ per tick; atomic batch; verify_all_invariants
  after each batch (spec invariants 32/33)
- imbalanced book nets into Funding pool (verify_all_invariants allows)

4 unit tests: mark tracked, long-pays/short-receives, period increments,
imbalanced-book invariants hold.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: `apply_event` replay + round-trip equivalence matrix

**Files:**
- Modify: `crates/exg-clearing/src/post_trade.rs`

### Why this is the keystone correctness task

Spec invariant 36 (Stage 2 P1 regression guard): on replay, money is **never recomputed** — `FundingRateUpdate` is a settlement no-op, money state comes only from recorded `AdminCredited`/`RealizedPnl`/`FundingSettled` facts; positions re-project from `OrderFilled`. Correctness is defined by **live↔replay state equivalence**, proven by the mandatory 4-scenario matrix (spec §7.2).

- [ ] **Step 1: Failing round-trip tests (red)**

Add to the test module a helper that runs a command/event script live, collects ALL emitted events, replays them into a fresh processor, and asserts full-state equivalence:

```rust
    /// Assert live vs replayed PostTradeProcessor are byte-identical in
    /// observable state: positions (size+entry+side per user/symbol),
    /// every user Funding balance, system Funding pool, journal length.
    fn assert_equivalent(live: &PostTradeProcessor, replayed: &PostTradeProcessor, users: &[u64]) {
        for &u in users {
            let lp = live.positions().get_position(UserId::new(u), SymbolId::new(1));
            let rp = replayed.positions().get_position(UserId::new(u), SymbolId::new(1));
            assert_eq!(lp.map(|p| (p.size, p.entry_price, p.side)),
                       rp.map(|p| (p.size, p.entry_price, p.side)), "position u{u}");
            let lb = live.ledger().get_balance(UserId::new(u), exg_ledger::WalletType::Funding).map(|b| b.available);
            let rb = replayed.ledger().get_balance(UserId::new(u), exg_ledger::WalletType::Funding).map(|b| b.available);
            assert_eq!(lb, rb, "funding balance u{u}");
        }
        assert_eq!(
            live.ledger().system_balance(exg_ledger::WalletType::Funding),
            replayed.ledger().system_balance(exg_ledger::WalletType::Funding),
            "system Funding pool");
        assert_eq!(live.ledger().journal().len(), replayed.ledger().journal().len(), "journal len");
    }

    fn replay_all(events: &[Event]) -> PostTradeProcessor {
        let mut pt = PostTradeProcessor::new();
        for e in events { pt.apply_event(e).expect("apply_event"); }
        pt
    }

    #[test]
    fn rt_admin_credit_open_funding_tick() {
        let mut live = PostTradeProcessor::new();
        let mut all = Vec::new();
        all.extend(live.handle_admin_credit(UserId::new(1), dec("100000"), "c1", ts()));
        all.extend(live.handle_admin_credit(UserId::new(2), dec("100000"), "c2", ts()));
        all.extend(live.consume(&[filled(1, Side::Buy, "1", "60000")], ts()));
        all.extend(live.consume(&[filled(2, Side::Sell, "1", "60000")], ts()));
        all.extend(live.consume(&[Event::MarkPriceUpdate { symbol: SymbolId::new(1), mark_price: dec("60000"), index_price: dec("60000"), timestamp: ts() }], ts()));
        all.extend(live.consume(&[Event::FundingRateUpdate { symbol: SymbolId::new(1), funding_rate: dec("0.01"), timestamp: ts() }], ts()));
        let replayed = replay_all(&all);
        assert_equivalent(&live, &replayed, &[1, 2]);
    }

    #[test]
    fn rt_partial_close_realized_pnl() {
        let mut live = PostTradeProcessor::new();
        let mut all = Vec::new();
        all.extend(live.handle_admin_credit(UserId::new(1), dec("100000"), "c1", ts()));
        all.extend(live.consume(&[filled(1, Side::Buy, "3", "60000")], ts()));
        all.extend(live.consume(&[filled(1, Side::Sell, "1", "61000")], ts())); // +1000 realized
        let replayed = replay_all(&all);
        assert_equivalent(&live, &replayed, &[1]);
    }

    #[test]
    fn rt_imbalanced_book_funding_net() {
        let mut live = PostTradeProcessor::new();
        let mut all = Vec::new();
        all.extend(live.handle_admin_credit(UserId::new(1), dec("100000"), "c1", ts()));
        all.extend(live.consume(&[filled(1, Side::Buy, "2", "60000")], ts()));
        all.extend(live.consume(&[Event::MarkPriceUpdate { symbol: SymbolId::new(1), mark_price: dec("60000"), index_price: dec("60000"), timestamp: ts() }], ts()));
        all.extend(live.consume(&[Event::FundingRateUpdate { symbol: SymbolId::new(1), funding_rate: dec("0.01"), timestamp: ts() }], ts()));
        let replayed = replay_all(&all);
        assert_equivalent(&live, &replayed, &[1]);
    }

    #[test]
    fn rt_funding_rate_update_is_settlement_noop_on_replay() {
        // Replaying ONLY a FundingRateUpdate (no recorded FundingSettled)
        // must move NO funds — settlement state comes solely from facts.
        let mut pt = PostTradeProcessor::new();
        pt.handle_admin_credit(UserId::new(1), dec("100000"), "c1", ts());
        pt.apply_event(&Event::OrderFilled {
            order_id: exg_common::OrderId::new(1), trade_id: exg_common::TradeId::new(1),
            user_id: UserId::new(1), symbol: SymbolId::new(1), side: Side::Buy,
            fill_price: dec("60000"), fill_qty: dec("1"), is_maker: false,
            remaining_qty: Decimal128::ZERO, timestamp: ts(),
        }).unwrap();
        pt.apply_event(&Event::MarkPriceUpdate { symbol: SymbolId::new(1), mark_price: dec("60000"), index_price: dec("60000"), timestamp: ts() }).unwrap();
        let before = pt.ledger().get_balance(UserId::new(1), exg_ledger::WalletType::Funding).unwrap().available;
        pt.apply_event(&Event::FundingRateUpdate { symbol: SymbolId::new(1), funding_rate: dec("0.01"), timestamp: ts() }).unwrap();
        let after = pt.ledger().get_balance(UserId::new(1), exg_ledger::WalletType::Funding).unwrap().available;
        assert_eq!(before, after, "FundingRateUpdate replay must not settle (invariant 36)");
    }

    // CEO review C1/C1b (spec §7.2 #5): underfunded loss replays
    // equivalently. Live: fund small, open, close at a loss exceeding
    // balance (capped at available). Reboot. Assert user available (==0),
    // SYSTEM Funding pool, positions, journal length identical; the
    // capped RealizedPnl.amount replays as a pure fact (no re-cap),
    // verify_all_invariants holds.
    #[test]
    fn rt_underfunded_loss_equivalent() {
        let mut live = PostTradeProcessor::new();
        let mut all = Vec::new();
        all.extend(live.handle_admin_credit(UserId::new(1), dec("100"), "c1", ts()));
        all.extend(live.consume(&[filled(1, Side::Buy, "1", "60000")], ts()));
        // close at 59000 → loss 1000 ≫ available 100 → capped to 100 moved
        all.extend(live.consume(&[filled(1, Side::Sell, "1", "59000")], ts()));
        let lb = live.ledger().get_balance(UserId::new(1), exg_ledger::WalletType::Funding).unwrap().available;
        assert_eq!(lb, dec("0"), "user floored at 0, never negative");
        live.ledger().verify_all_invariants().unwrap();
        let rp = all.iter().find_map(|e| match e {
            Event::RealizedPnl { amount, .. } => Some(*amount), _ => None }).unwrap();
        assert_eq!(rp, dec("-100"), "RealizedPnl records the MOVED (capped) amount");
        let replayed = replay_all(&all);
        assert_equivalent(&live, &replayed, &[1]);
        replayed.ledger().verify_all_invariants().unwrap();
    }
```

Also add the CEO C1/C1b/C3 **unit** tests to the Task 3 / Task 4
test modules per spec §7.1 #8/#9/#10 (the implementer writes these as
TDD-red in the task that owns the behavior — #8/#9 underfunded
loss/funding caps-at-zero-no-panic in Task 3/4, #10 zero-mark
warn-skip in Task 4 — each asserting no panic, user `available == 0`,
SYSTEM pool negative, `verify_all_invariants` holds; #10 asserts no
`FundingSettled` and `funding_period_id` unchanged).

Run → RED (`apply_event` undefined; new tests fail).

- [ ] **Step 2: Implement `apply_event` (green)**

Add to `impl PostTradeProcessor`:

```rust
    /// Replay path. Positions re-project from OrderFilled (pure additive,
    /// no money). Money comes ONLY from recorded fact events, re-applied
    /// with the SAME deterministic idempotency keys (ledger no-ops on a
    /// duplicate key, so this is safe even if a prior partial run already
    /// applied some). FundingRateUpdate is a settlement NO-OP on replay —
    /// money state is reconstructed from recorded FundingSettled facts
    /// (spec invariant 36 — the Stage 2 P1 regression guard).
    pub fn apply_event(&mut self, e: &Event) -> exg_common::ExgResult<()> {
        match e {
            Event::OrderFilled { user_id, symbol, side, fill_qty, fill_price, .. } => {
                // Re-project position ONLY. Do NOT touch pnl_seq here.
                // (T3 SHIPPED REALITY: live advances pnl_seq ONLY when a
                // RealizedPnl event is actually emitted — `next_pnl_seq()`
                // was removed; the uncoverable-loss case short-circuits
                // with no seq/key/event. So on replay the seq advances
                // ONLY in the RealizedPnl fact arm below, once per fact —
                // mirroring live exactly. Discard the recomputed PnL.)
                let _ = self.apply_fill_to_position(*user_id, *symbol, *side, *fill_qty, *fill_price);
            }
            Event::MarkPriceUpdate { mark_price, .. } => {
                self.mark_price = *mark_price;
            }
            Event::FundingRateUpdate { .. } => {
                // Settlement NO-OP on replay (invariant 36). Keep the
                // period counter aligned so a post-replay live tick uses
                // the next id.
                self.funding_period_id += 1;
            }
            Event::AdminCredited { user_id, amount, idempotency_key, timestamp } => {
                self.ledger.get_or_create_account(*user_id);
                // Self-describing fact: re-apply with the exact recorded
                // key; ledger no-ops a duplicate (idempotent).
                let _ = self.ledger.deposit(*user_id, *amount, idempotency_key, *timestamp);
            }
            Event::RealizedPnl { user_id, symbol, amount, timestamp } => {
                // CEO C1/C1b: `amount` is the ALREADY-MOVED (capped) signed
                // value — re-apply it directly via the same capped
                // primitive + same key. No re-cap (already capped live;
                // the primitive is idempotent on the key anyway).
                // T3 SHIPPED REALITY: live advances pnl_seq exactly once
                // per EMITTED RealizedPnl (inside its `!moved.is_zero()`
                // guard; `next_pnl_seq()` removed). Replay mirrors that:
                // advance once per replayed RealizedPnl fact, here only.
                self.pnl_seq += 1;
                let seq = self.pnl_seq;
                let key = format!("pnl_{seq}_{}_{}", user_id.value(), symbol.value());
                self.ledger.get_or_create_account(*user_id);
                let _ = self.ledger.settle_realized_pnl_capped(*user_id, *amount, &key, *timestamp);
            }
            Event::FundingSettled { user_id, symbol, funding_period_id, amount, timestamp } => {
                // T4 SHIPPED REALITY — sign bridge: `FundingSettled.amount`
                // is in the funding convention (positive = user PAID), but
                // `settle_realized_pnl_capped` uses the PnL convention
                // (positive = CREDIT user). The live funding path passes
                // `-payment` and records `fact_amount = -moved`. Replay must
                // mirror EXACTLY: pass `-amount` so a user who paid is
                // re-debited (not credited). Same key as live
                // (funding_{period}_{user}_{symbol}); idempotent.
                self.ledger.get_or_create_account(*user_id);
                let key = format!("funding_{}_{}_{}", funding_period_id, user_id.value(), symbol.value());
                let _ = self.ledger.settle_realized_pnl_capped(*user_id, -*amount, &key, *timestamp);
                if *funding_period_id > self.funding_period_id {
                    self.funding_period_id = *funding_period_id;
                }
            }
            _ => {} // engine-domain events ignored by post_trade
        }
        Ok(())
    }
```

**Idempotency-key alignment.** `RealizedPnl`/`FundingSettled` carry enough to rebuild their keys (`pnl_seq` reconstructed by replaying in order; `funding_period_id` is in the event). `AdminCredited` carries `idempotency_key` (defined that way in Task 1 — self-describing fact), so replay re-applies with the exact original key. The `AdminCredited` arm in the `apply_event` match above is therefore:

```rust
            Event::AdminCredited { user_id, amount, idempotency_key, timestamp } => {
                self.ledger.get_or_create_account(*user_id);
                let _ = self.ledger.deposit(*user_id, *amount, idempotency_key, *timestamp);
            }
```

(Replace the earlier `admin_credit_key()` placeholder line in the match with this. `handle_admin_credit` (Task 3) already emits `Event::AdminCredited { user_id, amount, idempotency_key: idempotency_key.to_owned(), timestamp: ts }` — keys match by construction.)

`exg_common::ExgResult` is the workspace result alias (used by ledger). Confirm import path (`grep -n "pub type ExgResult\|ExgResult" crates/exg-common/src/error.rs`).

- [ ] **Step 3: Run — green (the equivalence matrix is the spec of correctness)**

```bash
cargo test -p exg-clearing post_trade 2>&1 | tail -14
cargo clippy -p exg-clearing -- -D warnings 2>&1 | tail -3
```

Expected: all Task 2-5 tests pass — crucially the **5** `rt_*` round-trip-equivalence tests (the 4 base + `rt_underfunded_loss_equivalent`, CEO C1/C1b) plus `rt_funding_rate_update_is_settlement_noop_on_replay`. If any `rt_*` fails, the live/replay paths diverge — fix the divergence (do NOT weaken the assertion; this is the Stage 2 P1 discipline).

- [ ] **Step 4: Commit**

```bash
git add crates/exg-clearing/src/post_trade.rs
git commit -m "$(cat <<'EOF'
feat(clearing): post_trade replay apply_event + round-trip equivalence

- apply_event: positions re-projected from OrderFilled (no money);
  AdminCredited/RealizedPnl/FundingSettled re-apply RECORDED amounts via
  the same deterministic idempotency keys (ledger no-ops duplicates);
  FundingRateUpdate is a settlement NO-OP on replay (invariant 36 —
  Stage 2 P1 regression guard); pnl_seq advanced identically to live
- AdminCredited gains idempotency_key (self-describing fact, replay-safe)

4 mandatory round-trip equivalence tests (spec §7.2) + funding-rate
replay no-op: live vs replayed identical positions/balances/system
pool/journal length.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Admin `/api/v1/admin/credit` endpoint

**Files:**
- Modify: `crates/exg-api-gateway/src/types.rs`
- Modify: `crates/exg-api-gateway/src/admin.rs`

### Step 0: Re-read the Stage 2 admin patterns to mirror exactly

```bash
sed -n '1,135p' crates/exg-api-gateway/src/admin.rs
grep -n "AdminMarkPriceRequest" crates/exg-api-gateway/src/types.rs
```

Match: `check_admin_secret(&req, &state.cfg.admin.admin_secret)?`; the `tracing::info!(target:"admin", ..)` audit line; `enqueue_admin(&state, &cmd)?`; the `bad_request` 400 / `unauthorized` 401 mapping; the `build_admin_app` route registration; `AdminMarkPriceRequest`'s camelCase + stringified-decimal shape.

- [ ] **Step 1: `AdminCreditRequest` in types.rs**

In `crates/exg-api-gateway/src/types.rs`, mirroring `AdminMarkPriceRequest`'s derive/rename style:

```rust
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminCreditRequest {
    pub user_id: u64,
    /// Decimal string, must be > 0.
    pub amount: String,
}
```

- [ ] **Step 2: `admin_credit` handler + route in admin.rs**

In `crates/exg-api-gateway/src/admin.rs`, add (mirroring `admin_mark_price` exactly — same imports, `web::Data<AppState>`, `HttpRequest`, `web::Json<_>`, `ApiError`):

```rust
pub async fn admin_credit(
    state: web::Data<AppState>,
    req: HttpRequest,
    body: web::Json<crate::types::AdminCreditRequest>,
) -> Result<HttpResponse, ApiError> {
    check_admin_secret(&req, &state.cfg.admin.admin_secret)?;
    let amount: Decimal128 = body
        .amount
        .parse()
        .map_err(|_| ApiError::bad_request("amount must be a decimal"))?;
    if !amount.is_positive() {
        return Err(ApiError::bad_request("amount must be positive"));
    }
    let user_id = UserId::new(body.user_id);
    let ts = UnixMicros::now();
    // CEO review C2: a ts-only key collides when two same-user credits
    // land in the same microsecond (e2e/demo issue rapid sequential
    // credits) → second deposit silently no-ops → silently lost funds.
    // Embed a process-unique monotonic counter so every accepted credit
    // gets a distinct key. The command carries it; replay re-applies the
    // recorded key (deterministic, still idempotent cross-machine).
    static ADMIN_CREDIT_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = ADMIN_CREDIT_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let idempotency_key = format!("admincredit_{}_{}_{}", body.user_id, ts.as_micros(), n);
    tracing::info!(target: "admin", user_id = body.user_id, amount = %amount, "admin credit");
    let cmd = Command::AdminCredit { user_id, amount, idempotency_key, timestamp: ts };
    enqueue_admin(&state, &cmd)?;
    Ok(HttpResponse::Ok().json(serde_json::json!({ "status": "ACCEPTED" })))
}
```

Confirm the imports already present in admin.rs (`Command`, `UserId`, `Decimal128`, `UnixMicros`, `web`, `HttpRequest`, `HttpResponse`, `ApiError`) — `admin_mark_price` uses `Command`/`SymbolId`/`Decimal128`/`UnixMicros`; add `UserId` to the existing `use exg_common::{...}` if missing. `UnixMicros::as_micros()` is verified (ids.rs:107).

In `build_admin_app`, add the route alongside the existing ones:

```rust
        .route("/api/v1/admin/credit", web::post().to(admin_credit))
```

- [ ] **Step 3: Verify**

```bash
cargo check -p exg-api-gateway 2>&1 | tail -5
cargo clippy -p exg-api-gateway -- -D warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -2
```

Expected: clean (depends on Task 1's `Command::AdminCredit` — already on the branch). Endpoint behavior is covered by e2e in Task 8.

- [ ] **Step 4: Commit**

```bash
git add crates/exg-api-gateway/src/types.rs crates/exg-api-gateway/src/admin.rs
git commit -m "$(cat <<'EOF'
feat(api-gateway): admin /api/v1/admin/credit endpoint

- AdminCreditRequest (camelCase, stringified amount)
- admin_credit handler: X-Admin-Secret gate (Stage 2 inv 26), amount<=0
  → 400, tracing audit (inv 30), enqueues Command::AdminCredit with
  idempotency_key admincredit_{user}_{ts}; route on the admin server

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: Boot wiring — replay dual-dispatch + matching-thread integrate + invariants

**Files:**
- Modify: `crates/exg-server/src/lib.rs`

### Step 0: Re-read the exact anchors

```bash
grep -n "fn run_with_config_with_pool\|reader.read_from(0\|engine.apply_event\|let mut engine = MatchingEngine::new\|process_one = |n\|engine.process_command\|matching_wal.lock().append\|ServerHandle {\|replayed_count\|use exg_clearing\|use exg_protocol" crates/exg-server/src/lib.rs
```

Confirm: the replay block (`let replayed_count = { ... reader.read_from(0, |seq, payload| { ... engine.apply_event(&event) ... }) ...; replayed_count };`), the matching-thread `process_one` closure, the `ServerHandle { .. }` constructor, and that `exg-clearing` is a dep of `exg-server` (`grep -n exg-clearing crates/exg-server/Cargo.toml`; if absent, add `exg-clearing = { workspace = true }`).

- [ ] **Step 1: Construct `PostTradeProcessor` before replay**

Add `use exg_clearing::post_trade::PostTradeProcessor;` to the imports. Immediately before the replay block (before `let replayed_count = {`), add:

```rust
    // ── Stage 3: post-trade processor (positions + ledger) ────────────────
    let mut post_trade = PostTradeProcessor::new();
```

- [ ] **Step 2: Dual-dispatch in the replay loop**

Inside the `reader.read_from(0, |seq, payload| { ... })` closure, immediately AFTER the existing `if let Err(e) = engine.apply_event(&event) { replay_err = Some(ReplayError::Apply { seq, msg: format!("{e}") }); return false; }`, add:

```rust
                if let Err(e) = post_trade.apply_event(&event) {
                    replay_err = Some(ReplayError::Apply { seq, msg: format!("post_trade: {e}") });
                    return false;
                }
```

(`post_trade.apply_event` returns `ExgResult<()>`; `format!("{e}")` works via `ExgError: Display`.) After the replay block, add a post-replay invariant gate before Step 3.7:

```rust
    // Stage 3 invariant 32: ledger consistent after replay.
    post_trade
        .ledger()
        .verify_all_invariants()
        .map_err(|e| anyhow::anyhow!("Stage 3: ledger invariants violated after replay: {e}"))?;
```

- [ ] **Step 3: Integrate `consume` + `AdminCredit` routing into the matching thread**

The matching thread closure must own `post_trade` (move it in). The `process_one` closure currently: decode `cmd` → `engine.process_command(&cmd)` → append each event. Replace its body so post-trade runs and ALL events (engine ++ post-trade) are WAL-appended in order, and `AdminCredit` routes to `handle_admin_credit`:

```rust
            let mut process_one = |n: usize, buf: &[u8]| {
                let owned: Vec<u8> = buf[..n].to_vec();
                let cmd: Command = match rkyv::from_bytes::<Command, rkyv::rancor::Error>(&owned) {
                    Ok(c) => c,
                    Err(e) => panic!("matching thread: rkyv decode Command failed: {e}"),
                };
                let now = exg_common::UnixMicros::now();
                let mut all_events: Vec<Event> = Vec::new();
                match &cmd {
                    Command::AdminCredit { user_id, amount, idempotency_key, timestamp } => {
                        all_events.extend(post_trade.handle_admin_credit(
                            *user_id, *amount, idempotency_key, *timestamp,
                        ));
                    }
                    _ => {
                        let engine_events = engine.process_command(&cmd);
                        all_events.extend(post_trade.consume(&engine_events, now));
                        // engine events first, then post-trade facts — WAL order
                        let mut ordered = engine_events;
                        ordered.extend(all_events);
                        all_events = ordered;
                    }
                }
                for evt in &all_events {
                    let bytes = match rkyv::to_bytes::<rkyv::rancor::Error>(evt) {
                        Ok(b) => b,
                        Err(e) => panic!("matching thread: rkyv encode Event failed: {e}"),
                    };
                    if let Err(e) = matching_wal.lock().append(&bytes) {
                        panic!("matching thread: WAL append failed: {e}");
                    }
                }
            };
```

`post_trade` is captured by the `move ||` thread closure (like `engine`). It is `!Sync`-irrelevant (single thread, owned). Confirm the thread closure can take `post_trade` by move — it is declared `let mut post_trade` before the replay block and only used by reference in replay; ensure no later use after the thread spawn (it is moved into the thread). The post-replay `verify_all_invariants()` (Step 2) runs BEFORE the thread spawn, so the borrow ends before the move — good.

- [ ] **Step 4: Verify (regression-critical — Stage 2 precedent)**

```bash
docker compose up -d postgres
sleep 3
cargo check --workspace --all-targets 2>&1 | tail -5
cargo clippy --workspace -- -D warnings 2>&1 | tail -5
cargo fmt --check 2>&1 | tail -2
DATABASE_URL=postgres://exg:exg_dev_password@localhost:5433/exg cargo test -p exg-server --test stage0_e2e --test stage1a_e2e --test stage1b_e2e --test stage2_e2e --test boot_panics 2>&1 | grep -E "test result|FAILED|error\[" | tail
```

Expected: workspace + all-targets clean; clippy/fmt clean; **regression baselines green: stage0 7, stage1a 12, stage1b 16, stage2 11, boot_panics (prior count)**. The matching thread now also runs `consume`; baselines never admin-credit and never close at a loss against an unfunded user, so they pass (opening a position with no balance is allowed — no margin check). If any baseline hangs on shutdown, the post_trade move/borrow ordering is wrong — fix it (do not paper over).

- [ ] **Step 5: Commit**

```bash
git add crates/exg-server/src/lib.rs crates/exg-server/Cargo.toml
git commit -m "$(cat <<'EOF'
feat(server): wire PostTradeProcessor into boot replay + matching thread

- construct PostTradeProcessor before replay; replay loop dual-dispatches
  every WAL record to engine.apply_event + post_trade.apply_event
  (ReplayError::Apply on post_trade failure → boot abort)
- post-replay verify_all_invariants gate (spec invariant 32)
- matching thread: AdminCredit → post_trade.handle_admin_credit; other
  cmds → engine.process_command → post_trade.consume; WAL appends engine
  events then post-trade facts in order; post_trade moved into the thread
- single-thread invariant preserved (no Arc/Mutex on post_trade);
  Stage 0 §9 shutdown drain intact

Regression baselines (stage0/1a/1b/2 + boot_panics) green.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: stage3_e2e + boot_panics + demo + full acceptance

**Files:**
- Create: `crates/exg-server/tests/stage3_e2e.rs`
- Modify: `crates/exg-server/tests/boot_panics.rs`
- Create: `scripts/demo-stage3.sh`

### Step 0: Reuse the Stage 2 e2e harness verbatim

`crates/exg-server/tests/stage2_e2e.rs` has the `base_cfg`/`boot`/`register_and_login`/WAL-scan helpers. Copy that file's imports + `base_cfg` (already sets `admin_port = 0`, `admin_secret`) + `boot` + the `WalReader`/`payload.to_vec()`/`rkyv::from_bytes` scan idiom. Confirm field names against the real `AdminCreditRequest` (camelCase `userId`,`amount`) and the auth response (`accessToken`).

- [ ] **Step 1: Write `crates/exg-server/tests/stage3_e2e.rs` (5 tests)**

```rust
//! Stage 3 e2e: admin credit → open positions → funding tick moves
//! funds → reboot survives. #[sqlx::test] per-test DB isolation.

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
    cfg.server.admin_port = 0;
    cfg.auth.jwt_secret = "stage3-test-secret-padding-32-bytes-ok".into();
    cfg.admin.admin_secret = "stage3-admin-secret-padding-32-bytesok".into();
    cfg
}

const ADMIN_SECRET: &str = "stage3-admin-secret-padding-32-bytesok";

async fn boot(cfg: ExgConfig, pool: PgPool) -> (exg_server::ServerHandle, String, String) {
    let handle = exg_server::run_with_config_with_pool(cfg, Some(pool)).await.expect("boot");
    let base = format!("http://127.0.0.1:{}", handle.bound_port);
    let admin = format!("http://127.0.0.1:{}", handle.admin_bound_port);
    let client = Client::new();
    for _ in 0..50 {
        if client.get(format!("{base}/api/v1/health")).timeout(Duration::from_millis(100))
            .send().await.map(|r| r.status().is_success()).unwrap_or(false) {
            return (handle, base, admin);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("server not ready");
}

async fn register_and_login(client: &Client, base: &str, email: &str) -> String {
    let _ = client.post(format!("{base}/api/v1/auth/register"))
        .json(&serde_json::json!({"email": email, "password": "hunter2hunter2"})).send().await.unwrap();
    let resp: serde_json::Value = client.post(format!("{base}/api/v1/auth/login"))
        .json(&serde_json::json!({"email": email, "password": "hunter2hunter2"}))
        .send().await.unwrap().json().await.unwrap();
    resp["accessToken"].as_str().unwrap().to_string()
}

/// The order-placement handler resolves the acting user via
/// `extract_user_id_from_jwt` → `UserId::new(claims.user_id)`
/// (handlers.rs:16-29). Decode the same JWT to learn the id we must
/// admin-credit so the credited user == the user that opens the position.
/// `JWT_SECRET` MUST equal `base_cfg().auth.jwt_secret`.
const JWT_SECRET: &str = "stage3-test-secret-padding-32-bytes-ok";

fn jwt_user_id(token: &str) -> u64 {
    let claims = exg_user_service::verify_jwt(JWT_SECRET.as_bytes(), token)
        .expect("decode test JWT");
    claims.user_id
}

#[sqlx::test(migrations = "../../migrations")]
async fn admin_credit_then_funding_tick_moves_funds(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let wal_dir = std::path::PathBuf::from(tmp.path());
    let cfg = base_cfg(tmp.path());
    let (handle, base, admin) = boot(cfg, pool).await;
    let client = Client::new();

    // Register/login each user once; derive the real user id from the JWT
    // (the order handler resolves the actor the same way), admin-credit
    // that exact id, then place the order under the same token.
    let t1 = register_and_login(&client, &base, "s3a@e.com").await;
    let t2 = register_and_login(&client, &base, "s3b@e.com").await;
    let uid1 = jwt_user_id(&t1);
    let uid2 = jwt_user_id(&t2);
    for (uid, tag) in [(uid1, "s3a"), (uid2, "s3b")] {
        let r = client.post(format!("{admin}/api/v1/admin/credit"))
            .header("X-Admin-Secret", ADMIN_SECRET)
            .json(&serde_json::json!({"userId": uid, "amount": "100000"}))
            .send().await.unwrap();
        assert_eq!(r.status().as_u16(), 200, "admin credit {tag}");
    }
    // user1 buys, user2 sells @60000 — they cross and open opposing positions.
    client.post(format!("{base}/api/v1/order")).header("Authorization", format!("Bearer {t1}"))
        .json(&serde_json::json!({"symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT","timeInForce":"GTC","quantity":"1","price":"60000"}))
        .send().await.unwrap();
    client.post(format!("{base}/api/v1/order")).header("Authorization", format!("Bearer {t2}"))
        .json(&serde_json::json!({"symbol":"BTCUSDT","side":"SELL","orderType":"LIMIT","timeInForce":"GTC","quantity":"1","price":"60000"}))
        .send().await.unwrap();

    client.post(format!("{admin}/api/v1/admin/mark-price")).header("X-Admin-Secret", ADMIN_SECRET)
        .json(&serde_json::json!({"markPrice":"60000","indexPrice":"60000"})).send().await.unwrap();
    let r = client.post(format!("{admin}/api/v1/admin/funding-tick"))
        .header("X-Admin-Secret", ADMIN_SECRET).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 200);

    tokio::time::sleep(Duration::from_millis(300)).await;
    handle.shutdown().await.unwrap();

    let mut reader = WalReader::open(&wal_dir).unwrap();
    let (mut saw_credit, mut saw_settled) = (false, false);
    reader.read_from(0, |_s, p| {
        let owned: Vec<u8> = p.to_vec();
        match rkyv::from_bytes::<Event, rkyv::rancor::Error>(&owned).unwrap() {
            Event::AdminCredited { .. } => saw_credit = true,
            Event::FundingSettled { .. } => saw_settled = true,
            _ => {}
        }
        true
    }).unwrap();
    assert!(saw_credit, "WAL has AdminCredited");
    assert!(saw_settled, "WAL has FundingSettled");
}

#[sqlx::test(migrations = "../../migrations")]
async fn funding_settlement_survives_reboot(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    // Boot 1: credit + open + funding tick, then shutdown.
    {
        let (handle, base, admin) = boot(cfg.clone(), pool.clone()).await;
        let client = Client::new();
        let t1 = register_and_login(&client, &base, "rb1@e.com").await;
        let t2 = register_and_login(&client, &base, "rb2@e.com").await;
        for uid in [jwt_user_id(&t1), jwt_user_id(&t2)] {
            client.post(format!("{admin}/api/v1/admin/credit")).header("X-Admin-Secret", ADMIN_SECRET)
                .json(&serde_json::json!({"userId": uid, "amount":"100000"})).send().await.unwrap();
        }
        client.post(format!("{base}/api/v1/order")).header("Authorization", format!("Bearer {t1}"))
            .json(&serde_json::json!({"symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT","timeInForce":"GTC","quantity":"1","price":"60000"})).send().await.unwrap();
        client.post(format!("{base}/api/v1/order")).header("Authorization", format!("Bearer {t2}"))
            .json(&serde_json::json!({"symbol":"BTCUSDT","side":"SELL","orderType":"LIMIT","timeInForce":"GTC","quantity":"1","price":"60000"})).send().await.unwrap();
        client.post(format!("{admin}/api/v1/admin/mark-price")).header("X-Admin-Secret", ADMIN_SECRET)
            .json(&serde_json::json!({"markPrice":"60000","indexPrice":"60000"})).send().await.unwrap();
        client.post(format!("{admin}/api/v1/admin/funding-tick")).header("X-Admin-Secret", ADMIN_SECRET).send().await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        handle.shutdown().await.unwrap();
    }
    // Boot 2: replay must succeed (post-replay verify_all_invariants gate)
    // and health green. (Observable: boot 2 does not panic AND health 200;
    // the verify_all_invariants gate in Task 7 Step 2 makes a divergent
    // replay fail the boot, so a green boot 2 is a real assertion that the
    // ledger reconstructed consistently — not the weak Stage-1b proxy.)
    let (handle2, base2, _a2) = boot(cfg, pool).await;
    let client = Client::new();
    let resp = client.get(format!("{base2}/api/v1/health")).send().await.unwrap();
    assert!(resp.status().is_success(), "boot 2 replay healthy");
    handle2.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn admin_credit_missing_secret_401(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let (handle, _b, admin) = boot(base_cfg(tmp.path()), pool).await;
    let r = Client::new().post(format!("{admin}/api/v1/admin/credit"))
        .json(&serde_json::json!({"userId":1,"amount":"100"})).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 401);
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn admin_credit_negative_amount_400(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let (handle, _b, admin) = boot(base_cfg(tmp.path()), pool).await;
    let c = Client::new();
    for bad in ["0", "-1"] {
        let r = c.post(format!("{admin}/api/v1/admin/credit")).header("X-Admin-Secret", ADMIN_SECRET)
            .json(&serde_json::json!({"userId":1,"amount":bad})).send().await.unwrap();
        assert_eq!(r.status().as_u16(), 400, "amount {bad}");
    }
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn admin_credit_route_not_on_main_port(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let (handle, base, _a) = boot(base_cfg(tmp.path()), pool).await;
    let r = Client::new().post(format!("{base}/api/v1/admin/credit"))
        .header("X-Admin-Secret", ADMIN_SECRET)
        .json(&serde_json::json!({"userId":1,"amount":"100"})).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 404, "admin route must not be on main port");
    handle.shutdown().await.unwrap();
}
```

Implementer notes:
- **User-id resolution**: the e2e needs the real user ids the API assigned to the registered emails to admin-credit them. Determine how the API exposes it: decode the JWT `sub` claim from the login `accessToken`, OR a `/api/v1/account` endpoint. `grep -n "sub\|user_id\|claims" crates/exg-user-service/src/*.rs crates/exg-api-gateway/src/handlers.rs`. Replace the `uid1=1/uid2=2` placeholder with the resolved ids. This MUST be real (no hard-coded guess) — adapt the test to the actual id source. If the order-placement path itself opens the position under the JWT's user, the credited user id must match that same id.
- Copy the exact `WalReader`/`payload.to_vec()`/`rkyv::from_bytes` idiom from `stage2_e2e.rs` (alignment workaround).
- If crossing LIMIT orders don't fill at the same price/size, adjust quantities/prices so they cross deterministically (mirror how stage2_e2e crosses a stop with a resting bid).

- [ ] **Step 2: boot_panics +1**

Append to `crates/exg-server/tests/boot_panics.rs` (match the file's existing `base_cfg`/`run_with_config` idiom + `{err:#}` formatting):

Mirror the existing `boot_panics_on_corrupt_wal_crc` exactly (it uses
`exg_wal::{WalConfig, WalWriter}` to write a segment then `std::fs::write`
to corrupt bytes, then `exg_server::run_with_config(cfg)` asserts `Err`).
The Stage-3-specific variant produces a real Stage-3 WAL via the running
server, then tampers the last segment so replay fails — proving the
post-trade replay path is fail-fast (no silent skip):

```rust
#[actix_web::test]
async fn boot_panics_on_corrupt_post_trade_wal() {
    use std::time::Duration;
    let tmp = TempDir::new().unwrap();
    let wal_dir = tmp.path().to_path_buf();

    // Boot 1: produce a real WAL containing post-trade fact events.
    {
        let cfg = base_cfg(tmp.path()); // boot_panics base_cfg (Task 7 set admin fields)
        let pool = exg_server::test_pg_pool().await; // use the same pool helper the
                                                     // other boot_panics tests use; if
                                                     // none, use run_with_config (no DB
                                                     // path needed for this boot) — match
                                                     // the file's existing idiom.
        let (handle, _b, admin) = {
            let h = exg_server::run_with_config_with_pool(cfg, Some(pool)).await.expect("boot1");
            let a = format!("http://127.0.0.1:{}", h.admin_bound_port);
            (h, String::new(), a)
        };
        let c = reqwest::Client::new();
        // wait for health on the main port, then admin-credit (produces an
        // AdminCredited fact in the WAL).
        tokio::time::sleep(Duration::from_millis(300)).await;
        c.post(format!("{admin}/api/v1/admin/credit"))
            .header("X-Admin-Secret", "a".repeat(32)) // matches boot_panics base_cfg admin_secret
            .json(&serde_json::json!({"userId": 1, "amount": "1000"}))
            .send().await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        handle.shutdown().await.unwrap();
    }

    // Corrupt the last WAL segment's bytes (flip a byte in the payload
    // region) so replay rkyv-decode / CRC / post_trade.apply_event fails.
    let mut seg: Option<std::path::PathBuf> = None;
    for e in std::fs::read_dir(&wal_dir).unwrap() {
        let p = e.unwrap().path();
        if p.extension().map(|x| x == "log").unwrap_or(false) { seg = Some(p); }
    }
    let seg = seg.expect("a WAL segment exists");
    let mut bytes = std::fs::read(&seg).unwrap();
    assert!(bytes.len() > 40, "segment has records");
    let i = bytes.len() - 8; // inside the last record's payload/CRC tail
    bytes[i] ^= 0xFF;
    std::fs::write(&seg, &bytes).unwrap();

    // Boot 2 must return Err (fail-fast — corrupt WAL never silently
    // continues; same discipline as boot_panics_on_corrupt_wal_crc).
    let cfg2 = base_cfg(tmp.path());
    let result = exg_server::run_with_config(cfg2).await;
    assert!(result.is_err(), "corrupted Stage-3 WAL must fail boot, got Ok");
}
```

Implementer notes (verify against `boot_panics.rs`, do not assume):
- Match the file's real boot/pool idiom. The other Stage-2-era tests in
  this file use `run_with_config` (no DB) for pure boot-panic checks and
  the e2e files use `run_with_config_with_pool` with `#[sqlx::test]`. If
  boot 1 needs a PG pool (it does — registration/admin path), make this
  an `#[sqlx::test(migrations = "../../migrations")]` taking `pool:
  PgPool` and reuse the `stage3_e2e` `boot` helper for boot 1 instead of
  hand-rolling — whichever matches the existing corrupt-WAL test's
  structure. The invariant: **boot 2 over a tampered Stage-3 WAL returns
  `Err`** (CRC/decode or `post_trade.apply_event`/post-replay
  `verify_all_invariants` — any is acceptable; the point is fail-fast).
- `"a".repeat(32)` must equal `boot_panics.rs::base_cfg`'s `admin_secret`
  (Task 7 Step 5 set it). Confirm and match exactly.
- This is a **real** test — no empty body, no `todo!()`. If a structural
  detail (pool helper name) differs, adapt to the real one; do not
  weaken the boot-2-returns-Err assertion.

- [ ] **Step 3: `scripts/demo-stage3.sh`**

Mirror `scripts/demo-stage2.sh` (copy it as the base — same config-rewrite python block, same boot/shutdown helpers, same `EXG_CONFIG`/admin secret). Flow:

```bash
#!/usr/bin/env bash
set -euo pipefail
# ... (copy scripts/demo-stage2.sh preamble: WAL_DIR, TMP_CFG, ADMIN_SECRET,
#      cleanup trap, start_server/stop_server, docker compose up postgres,
#      migrate reset, cargo build --release -p exg-server -p exg-wal-dump,
#      config rewrite for wal dir + jwt + admin secret)
echo "─ boot 1 ─"; start_server
# register 2 users, capture tokens (reuse demo-stage2 register/login pattern)
# resolve their user ids (decode JWT sub or /account) — same as e2e
echo "─ admin credit both users 100000 ─"
curl -s -X POST "http://127.0.0.1:${ADMIN_PORT}/api/v1/admin/credit" -H "X-Admin-Secret: $ADMIN_SECRET" -H 'Content-Type: application/json' -d '{"userId":1,"amount":"100000"}'; echo
curl -s -X POST "http://127.0.0.1:${ADMIN_PORT}/api/v1/admin/credit" -H "X-Admin-Secret: $ADMIN_SECRET" -H 'Content-Type: application/json' -d '{"userId":2,"amount":"100000"}'; echo
echo "─ user1 BUY 1 @60000, user2 SELL 1 @60000 (cross) ─"
# curl two /api/v1/order posts with the two bearer tokens
echo "─ admin mark-price 60000 ─"
curl -s -X POST "http://127.0.0.1:${ADMIN_PORT}/api/v1/admin/mark-price" -H "X-Admin-Secret: $ADMIN_SECRET" -H 'Content-Type: application/json' -d '{"markPrice":"60000","indexPrice":"60000"}'; echo
echo "─ admin funding-tick ─"
curl -s -X POST "http://127.0.0.1:${ADMIN_PORT}/api/v1/admin/funding-tick" -H "X-Admin-Secret: $ADMIN_SECRET"; echo
sleep 1; echo "─ shutdown 1 ─"; stop_server
echo "─ WAL (expect AdminCredited, FundingRateUpdate, FundingSettled) ─"
./target/release/exg-wal-dump --wal-dir "${WAL_DIR}" | tail -30
echo "─ boot 2: replay ─"; start_server
curl -sf "http://127.0.0.1:${PORT}/api/v1/health"; echo
echo "─ shutdown 2 ─"; stop_server
echo "─ demo complete ─"
```

`chmod +x scripts/demo-stage3.sh`. It is a manual artifact (not CI-gated); target is "runs clean locally". Adapt the user-id resolution exactly as the e2e does.

- [ ] **Step 4: Full acceptance**

```bash
docker compose up -d postgres
sleep 3
DATABASE_URL=postgres://exg:exg_dev_password@localhost:5433/exg cargo check --workspace --all-targets
DATABASE_URL=postgres://exg:exg_dev_password@localhost:5433/exg cargo clippy --workspace -- -D warnings
cargo fmt --check
DATABASE_URL=postgres://exg:exg_dev_password@localhost:5433/exg cargo test -p exg-clearing 2>&1 | grep "test result" | tail
DATABASE_URL=postgres://exg:exg_dev_password@localhost:5433/exg cargo test -p exg-server --test stage3_e2e 2>&1 | grep -E "test result|FAILED" | tail
DATABASE_URL=postgres://exg:exg_dev_password@localhost:5433/exg cargo test -p exg-server --test stage0_e2e --test stage1a_e2e --test stage1b_e2e --test stage2_e2e --test boot_panics 2>&1 | grep -E "test result|FAILED" | tail
DATABASE_URL=postgres://exg:exg_dev_password@localhost:5433/exg cargo test --workspace 2>&1 | grep -E "test result:|FAILED|error\[" | tail -30
bash scripts/demo-stage3.sh 2>&1 | tail -25
```

Expected:
- workspace + all-targets clean; clippy/fmt clean.
- exg-clearing post_trade unit ~13 (incl. CEO C1/C1b underfunded-loss/funding + C3 zero-mark) + 5 round-trip equivalence (incl. CEO underfunded-loss) + ledger `settle_realized_pnl_capped` unit, green.
- stage3_e2e 5/5; regression stage0 7, stage1a 12, stage1b 16, stage2 11 green; boot_panics prior+1.
- whole `cargo test --workspace` green.
- demo: clean exit; wal-dump shows `AdminCredited` + `FundingRateUpdate` + `FundingSettled`; boot 2 health 200.

If `cargo fmt --check` flags: `cargo fmt`, separate `style: cargo fmt across stage 3` commit BEFORE the e2e commit.

- [ ] **Step 5: Commit**

```bash
git add crates/exg-server/tests/stage3_e2e.rs crates/exg-server/tests/boot_panics.rs scripts/demo-stage3.sh
git commit -m "$(cat <<'EOF'
test(server): Stage 3 e2e (5) + boot panic (1) + demo

stage3_e2e: admin-credit→cross orders→funding-tick moves funds (WAL has
AdminCredited+FundingSettled); funding settlement survives reboot
(post-replay verify_all_invariants gate makes a green boot 2 a real
assertion); admin-credit missing-secret 401; negative amount 400;
admin route not on main port.

boot_panics +1: corrupt post-trade WAL → boot Err (fail-fast).

scripts/demo-stage3.sh: credit 2 users → cross → funding-tick →
wal-dump → reboot replays.

Regression baselines (stage0 7, stage1a 12, stage1b 16, stage2 11) green.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Spec ↔ Plan Coverage Matrix

| Spec section | Task |
|--------------|------|
| §2.1 PostTradeProcessor | Task 2 (skeleton) + 3 + 4 + 5 |
| §2.2 live position tracking | Task 2 |
| §2.3 realized PnL | Task 3 |
| §2.4 funding settlement | Task 4 |
| §2.5 minimal admin credit | Task 1 (command) + Task 3 (`handle_admin_credit`) + Task 6 (endpoint) |
| §2.6 replay extension | Task 5 + Task 7 (boot dual-dispatch) |
| §4.1 Command::AdminCredit | Task 1 |
| §4.2 Event variants | Task 1 |
| §4.3 system account (SYSTEM_USER_ID internal) | Task 3/4 (use deposit/withdraw/settle_funding — ledger journals SYSTEM itself) |
| §4.4 PostTradeProcessor consume/apply_event | Tasks 2-5 |
| §4.5 ledger usage (signed funding, deposit/withdraw PnL, idempotency) | Task 3 + Task 4 |
| §4.6 boot lifecycle / dispatch rule | Task 1 (engine no-op arms) + Task 7 |
| §4.7 admin credit endpoint | Task 6 |
| §5 data flow / error handling (fail-fast, underfunded-loss) | Task 3 (withdraw fail-fast) + Task 7 (replay abort) |
| §6 invariants 31-36 | T1 (append-at-end §8.5), T3/T4 (31 idempotency, 32 verify, 35 double-entry), T4 (33 atomic batch), T2/T5 (34 OrderFilled-only projection), T5 (36 funding replay no-op) |
| §7.1 unit | Tasks 2-4 |
| §7.2 replay round-trip equivalence (mandatory 4) | Task 5 |
| §7.3 integration | Task 8 |
| §7.4 boot panics | Task 8 |
| §7.5 regression baselines | Task 7 (verify) + Task 8 (verify) |
| §8 acceptance | Task 8 |
| §8.5 rollback (append-at-end) | Task 1 |
| §9 forward pointers | Spec doc (no code) |

All spec sections covered.

---

## GSTACK REVIEW REPORT

| Review | Trigger | Why | Runs | Status | Findings |
|--------|---------|-----|------|--------|----------|
| CEO Review | `/plan-ceo-review` | Scope & strategy | 1 | CLEAR (PLAN) | mode: HOLD_SCOPE, 1 CRITICAL + 3 medium/low, 0 critical gaps unresolved, all 4 applied (C1/C1b, C2, C3, C4) |
| Codex Review | `/codex review` | Independent 2nd opinion | 0 | — | — |
| Eng Review | `/plan-eng-review` | Architecture & tests (required) | 1 | CLEAR (PLAN) | FULL_REVIEW, 2 findings applied (E1 P1, E2 P2), 0 critical gaps, scope proceed-as-is |
| Design Review | `/plan-design-review` | UI/UX gaps | 0 | SKIPPED | no UI scope |
| DX Review | `/plan-devex-review` | Developer experience gaps | 0 | — | — |

**CEO Review findings (HOLD_SCOPE, applied to spec + plan):**
- **C1/C1b (CRITICAL, applied)** — underfunded realized loss / funding payment via `.expect()` on `withdraw`/`settle_funding_checked` would panic the single-threaded matching thread → whole exchange down for ALL users on one user's normal losing close. `verify_account_invariant` forbids negative user `available` (so "let user go negative" is infeasible — would re-panic via inv #32). Resolution: new `Ledger::settle_realized_pnl_capped` (plan Task 3 Step 0) moves only `min(owed, available)`, user floored at 0, never errors; uncollected remainder = implicit bad debt absorbed by the SYSTEM Funding pool (allowed negative); `RealizedPnl`/`FundingSettled.amount` records the actually-moved value → replay pure fact-apply, invariant 36 + `verify_all_invariants` intact. Same primitive reused for funding payment. New inv #37.
- **C2 (Medium, applied)** — `admincredit_{user}_{ts_micros}` key collides under same-microsecond same-user credits → silent lost funds. Task 6 adds a process `AtomicU64` discriminator to the key; command carries it, replay re-applies recorded key.
- **C3 (Medium, applied)** — funding tick with open positions but `mark_price == 0` → silent zero-charge "success". Task 4 `settle_funding` warns + skips (no period bump, no events). New inv #38.
- **C4 (Low, applied)** — post_trade money moves had no operational log. Task 3/4 add `tracing::info!(target:"post_trade", ...)` for realized-PnL and funding-batch (parity with Stage 2 inv #30). New inv #39.

Non-findings (verified): architecture brainstorm-locked + sound (same-thread, single-WAL, bounded contexts); DRY/TDD structure; `settle_funding` O(positions) negligible at single-symbol scale; rollback §8.5 symmetric with Stage 2; substrate reversible + Stage-4-reusable. Outside voice skipped (user, consistent with Stage 0-2). Section 11 Design SKIPPED (no UI).

**Eng Review findings (FULL_REVIEW, source-grounded, applied to plan + spec):**
- **E1 [P1] (conf 9/10, applied)** — `settle_realized_pnl_capped`'s `add_system_balance` sign was inverted in BOTH branches (plan pseudocode). Verified vs real `add_system_balance` (`pool += amount`) + `verify_global_invariant` (`user_total+system_total == net_external`, only Deposit/Withdrawal feed net_external) + real `settle_funding`: rule is **system delta = −(user available delta)**. Wrong sign drifts the total by 2× → `verify_global_invariant` fails → `verify_all_invariants().expect()` panics the exchange on every profitable close / funding receipt / covered loss. Fixed plan Task 3 Step 0 (`-signed` credit / `+cover` debit, via `impl Neg`) + spec §4.5 sign rule + bad-debt comment.
- **E2 [P2] (conf 8/10, applied)** — the new highest-risk primitive (E1 lived here) had no dedicated `exg-ledger`-level unit test (only indirect `post_trade` coverage). Added Task 3 Step 0b: 5 RED-first ledger unit tests (profit sign, covered-loss sign, underfunded cap-at-0, idempotent, zero no-op) — the direct E1 regression guard.

Source-verified, no finding: `OrderFilled`/`TradeExecuted` fields exactly match plan assumptions; `JournalEntry`/`BalanceField`/`JournalEntryType`/`SYSTEM_USER_ID`/`Decimal128 ONE/Neg/ZERO` all confirmed; `verify_global_invariant` ignores FundingPayment (bad-debt-not-journaled is invariant-safe); matching-thread `post_trade` borrow-then-move is correct + noted; Stage 0 §9 drain preserved; append-at-end rkyv compat holds. Performance: O(open positions)/tick negligible at single-symbol scale. Outside voice skipped (user, consistent with Stage 0-2 + Stage 3 CEO). Step 0 complexity gate (14 files) → proceed-as-is (CEO HOLD_SCOPE already adjudicated; user declined SCOPE_REDUCTION).

**UNRESOLVED:** 0. **VERDICT:** CEO + ENG CLEARED — both required gates passed, all findings applied. Plan + Spec CLEAR. Proceed to `superpowers:subagent-driven-development` execution of the 8 tasks (each TDD red-green-commit + two-stage review spec→quality) → final cross-task review → PR → merge.
