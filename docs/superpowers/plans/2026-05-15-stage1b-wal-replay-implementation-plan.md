# Stage 1b — WAL Replay + Polish Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the exchange survive a restart by replaying the WAL into a fresh matching engine; clean up four Stage 1a forward pointers in the same PR.

**Architecture:** Boot lifecycle gains `Step 3.6 — WAL replay` between PG ping and DUMMY init. `MatchingEngine::apply_event(&Event)` reverse-maps WAL records (`OrderAccepted` → insert, `OrderCanceled` → remove, `OrderFilled` → decrement-or-remove, `TradeExecuted` → no-op since both sides' `OrderFilled` events already updated the book) onto a freshly-built engine. Stage 0 invariant 3 (`WAL must be empty at boot`) is removed; new invariants 21–23 govern replay consistency. The `OrderAccepted` event schema gains seven fields so replay can rebuild a full `BookOrder`. Polish: cancel/amend share `user:{N}` token bucket; login `||` short-circuit fix; JWT tamper / token reuse / kyc_level e2e gaps closed.

**Tech Stack:** Rust 2024 workspace · rkyv (event encoding) · sqlx + PostgreSQL on host port 5433 · jsonwebtoken + Argon2id (Stage 1a libs) · `#[sqlx::test]` for per-test DB isolation · `tempfile::TempDir` for WAL dirs.

**Branch:** `feat/stage1b-wal-replay` (HEAD `e55e707`, 2 commits beyond `main` — spec + invariant-21 fix).

**Spec:** [docs/superpowers/specs/2026-05-15-stage1b-wal-replay-design.md](../specs/2026-05-15-stage1b-wal-replay-design.md)

---

## File Structure

### New files

| Path | Responsibility |
|------|----------------|
| `crates/exg-matching-engine/src/replay.rs` | `apply_event(&Event) -> Result<(), ApplyError>` impl block + `ApplyError` enum + 10 unit tests |
| `crates/exg-server/tests/stage1b_e2e.rs` | 16 integration tests covering replay + four polish items |
| `scripts/demo-stage1b.sh` | Cold-boot demo: place → kill → reboot → snapshot inspect → wal-dump |

### Modified files

| Path | Change summary |
|------|----------------|
| `crates/exg-protocol/src/event.rs` | `OrderAccepted` gains 7 fields (`side`, `order_type`, `time_in_force`, `price`, `quantity`, `stop_price`, `reduce_only`) |
| `crates/exg-protocol/src/lib.rs` | `all_events()` helper (2 OrderAccepted constructors) updated to populate new fields |
| `crates/exg-matching-engine/src/engine.rs` | 5 `Event::OrderAccepted` constructors fill new fields from in-scope locals; new public method `engine.orderbook()` already exists (used by replay tests); add `replayed_count` not needed at engine level |
| `crates/exg-matching-engine/src/lib.rs` | `pub mod replay;` line added |
| `crates/exg-server/src/lib.rs` | Remove invariant 3 (WAL must be empty); add Step 3.6 WAL replay block; add Step 3.8 invariant 21 check |
| `crates/exg-server/tests/boot_panics.rs` | Remove `boot_panics_on_nonempty_wal_dir` (invariant 3 gone) |
| `crates/exg-api-gateway/src/handlers.rs` | Insert per-user rate-limit block in `cancel_order` and `amend_order`; rewrite login `\|\|` to charge both buckets unconditionally |

### Test surface

- **Unit (in `replay.rs`):** 12 tests covering each dispatch arm + error paths + a round-trip golden test.
- **Integration (`stage1b_e2e.rs`):** 17 tests — 10 replay correctness + 7 polish (the 10th replay test is `replay_survived_order_matches_post_reboot_taker`, added per CEO review A4).
- **Existing test deltas:** `boot_panics.rs` net +1 (removes 1 obsolete test, adds 2 new corruption tests). `stage0_e2e.rs` + `stage1a_e2e.rs` source unchanged (rest-pattern matches forward-compat with `OrderAccepted` field additions).

Target counts:
- Workspace tests rise by `12 + 17 + (2 - 1) = +30`
- `cargo test --workspace` total grows from ~390 to ~420

---

## Task overview

| # | Task | Files touched | Tests added |
|---|------|---------------|-------------|
| 1 | `OrderAccepted` schema bump + engine fillers | event.rs, lib.rs (protocol tests), engine.rs (5 sites) | 0 new; existing tests stay green |
| 2 | `apply_event` + `ApplyError` + 12 unit tests | replay.rs (NEW), matching-engine lib.rs | 12 unit |
| 3 | Boot lifecycle: remove invariant 3, add Step 3.6 + 3.8 | server lib.rs | 0 (existing test removal in Task 4) |
| 4 | Boot panic test update + new corrupt/gap tests | boot_panics.rs | net +2 |
| 5 | Handler polish (cancel/amend bucket via `consume_user_bucket` helper + login double-consume) | api-gateway handlers.rs | 0 new (covered in Task 6) |
| 6 | Stage 1b e2e suite (17 cases: 10 replay + 7 polish) | stage1b_e2e.rs (NEW) | 17 integration |
| 7 | Demo script + WAL-replay-failed runbook + final acceptance | scripts/demo-stage1b.sh (NEW), docs/runbooks/wal-replay-failed.md (NEW) | — |

Total LOC delta (excluding tests): ~250 prod + ~600 test. Plan execution order is strict — each task depends on the previous.

### Stage 1a → Stage 1b cutover (CEO review A7)

The `OrderAccepted` schema change in Task 1 is **not rkyv-forward-compatible**. Any environment that ran Stage 1a has WAL files Stage 1b cannot decode. Before running any Stage 1b test that hits a pre-existing `data/wal` directory:

```bash
rm -rf data/wal           # destroys all open orders — acceptable, no production data
# OR, to keep forensics:
mv data/wal data/wal-stage1a-archive
```

CI environments must clear `data/wal` once on the Stage 1b boot. The `#[sqlx::test]` and `TempDir`-based test paths are immune (fresh dir per test).

### Rollback path (CEO review A8)

If Stage 1b merges and breaks production: `git revert <merge>` → `rm -rf data/wal` (Stage 1a invariant 3 rejects non-empty WAL) → restart. All in-flight orders lost. Acceptable because Stage 1b ships into a dev-only environment. Spec §9.6 documents the full procedure.

---

## Task 1: `OrderAccepted` event schema bump

**Files:**
- Modify: `crates/exg-protocol/src/event.rs:46-52` (struct fields)
- Modify: `crates/exg-protocol/src/lib.rs:147-164` (test helper `all_events()`)
- Modify: `crates/exg-matching-engine/src/engine.rs` — 5 sites at lines `253, 265, 573, 609, 621` (use `grep -n "Event::OrderAccepted" crates/exg-matching-engine/src/engine.rs` to find current line numbers; comments below refer to current behavior)

### Why this is one task

Stage 0/1a Events are rkyv-encoded into a fixed binary layout. Adding fields without updating all constructor sites won't compile. Schema bump + every construction site must land in one atomic commit; tests must stay green throughout.

### Step 1: Update the `Event::OrderAccepted` variant

In `crates/exg-protocol/src/event.rs`, replace the existing variant (lines ~46-52) with:

```rust
    OrderAccepted {
        order_id: OrderId,
        user_id: UserId,
        symbol: SymbolId,
        client_order_id: Option<u64>,
        timestamp: UnixMicros,
        // Stage 1b: fields needed to rebuild a BookOrder during WAL replay.
        side: Side,
        order_type: OrderType,
        time_in_force: TimeInForce,
        /// Effective price recorded at accept time. For limit-like orders this is the
        /// submitted price; for market-like orders it is the sentinel (Decimal128::MAX for
        /// buy, Decimal128::ZERO for sell — same as `BookOrder.price`).
        price: Decimal128,
        /// Original submitted quantity.
        quantity: Decimal128,
        stop_price: Option<Decimal128>,
        reduce_only: bool,
    },
```

`Side`, `OrderType`, `TimeInForce` already derive `rkyv::Archive/Serialize/Deserialize` (see `crates/exg-common/src/types.rs`) so the variant compiles. `Side` is already imported at the top of `event.rs`. Add `OrderType, TimeInForce` to the existing `use exg_common::{...}` line — replace line 1 with:

```rust
use exg_common::{Decimal128, OrderId, OrderType, Side, SymbolId, TimeInForce, TradeId, UnixMicros, UserId};
```

- [ ] **Step 2: Run `cargo check --workspace` to enumerate the broken constructor sites**

```bash
cargo check --workspace 2>&1 | grep -E "missing.*field|missing structure field" | head -30
```

Expected: 7 errors listing the OrderAccepted construction sites missing the new fields (2 in `exg-protocol/src/lib.rs::all_events()`, 5 in `exg-matching-engine/src/engine.rs`).

- [ ] **Step 3: Fix `exg-protocol/src/lib.rs` test helper**

In `crates/exg-protocol/src/lib.rs`, locate the `all_events()` helper (currently around line 149). Replace the two `Event::OrderAccepted { ... }` blocks with:

```rust
            Event::OrderAccepted {
                order_id: OrderId::new(1001),
                user_id: UserId::new(42),
                symbol: SymbolId::new(1),
                client_order_id: Some(9999),
                timestamp: sample_timestamp(),
                side: Side::Buy,
                order_type: OrderType::Limit,
                time_in_force: TimeInForce::Gtc,
                price: dec("50000.5"),
                quantity: dec("1.0"),
                stop_price: None,
                reduce_only: false,
            },
            Event::OrderAccepted {
                order_id: OrderId::new(1002),
                user_id: UserId::new(43),
                symbol: SymbolId::new(2),
                client_order_id: None,
                timestamp: sample_timestamp(),
                side: Side::Sell,
                order_type: OrderType::Market,
                time_in_force: TimeInForce::Ioc,
                price: Decimal128::ZERO,
                quantity: dec("0.5"),
                stop_price: None,
                reduce_only: true,
            },
```

The `dec`, `Side`, `OrderType`, `TimeInForce` symbols are already imported in this test module — no new `use` line required.

- [ ] **Step 4: Fix `exg-matching-engine/src/engine.rs` — 5 construction sites**

The 5 sites are inside `handle_new_order` (2 emissions: conditional path, then accept-before-match) and `handle_amend_order` (3 emissions: amended-success, qty_down-only, no-change). Local variables `side`, `order_type`, `time_in_force`, `effective_price`, `quantity`, `stop_price`, `reduce_only` are already in scope at every emission site in `handle_new_order`. In `handle_amend_order`, those fields live on `existing` (the pre-amend BookOrder) or `amended` (the post-amend BookOrder).

**Site 1** — `handle_new_order` conditional path (the `is_conditional` branch). Replace the existing `Event::OrderAccepted` block with:

```rust
            events.push(Event::OrderAccepted {
                order_id,
                user_id,
                symbol,
                client_order_id,
                timestamp,
                side,
                order_type,
                time_in_force,
                price: effective_price,
                quantity,
                stop_price,
                reduce_only,
            });
```

**Site 2** — `handle_new_order` accept-before-match path. Same body as Site 1 (locals in scope are identical). Replace the existing `Event::OrderAccepted` block with the same code.

**Site 3** — `handle_amend_order` amended-success path. Inside the `if let Ok(amended) = ...` arm, replace with:

```rust
            events.push(Event::OrderAccepted {
                order_id,
                user_id,
                symbol,
                client_order_id: amended.client_order_id,
                timestamp,
                side: amended.side,
                order_type: amended.order_type,
                time_in_force: amended.time_in_force,
                price: amended.price,
                quantity: amended.original_qty,
                stop_price: amended.stop_price,
                reduce_only: amended.is_reduce_only,
            });
```

**Site 4** — `handle_amend_order` qty_down branch. Replace the `vec![Event::OrderAccepted { ... }]` with:

```rust
            vec![Event::OrderAccepted {
                order_id,
                user_id,
                symbol,
                client_order_id: self
                    .orderbook
                    .get_order(order_id)
                    .and_then(|o| o.client_order_id),
                timestamp,
                side: existing.side,
                order_type: existing.order_type,
                time_in_force: existing.time_in_force,
                price: existing.price,
                quantity: existing.original_qty,
                stop_price: existing.stop_price,
                reduce_only: existing.is_reduce_only,
            }]
```

(NOTE: `existing` is captured earlier in the function via `let existing = match self.orderbook.get_order(order_id) { ... }` — confirm the binding name by reading the surrounding code.)

**Site 5** — `handle_amend_order` no-change branch. Replace the final `vec![Event::OrderAccepted { ... }]` with:

```rust
            vec![Event::OrderAccepted {
                order_id,
                user_id,
                symbol,
                client_order_id: existing.client_order_id,
                timestamp,
                side: existing.side,
                order_type: existing.order_type,
                time_in_force: existing.time_in_force,
                price: existing.price,
                quantity: existing.original_qty,
                stop_price: existing.stop_price,
                reduce_only: existing.is_reduce_only,
            }]
```

- [ ] **Step 5: Run `cargo check --workspace` to verify clean**

```bash
cargo check --workspace 2>&1 | tail -5
```

Expected: `Finished` line, no errors. (Warnings about unused imports are tolerable; clippy gate runs in Task 7.)

- [ ] **Step 6: Run existing test suites — they must still pass**

```bash
DATABASE_URL=postgres://exg:exg_dev_password@localhost:5433/exg \
  cargo test -p exg-protocol -p exg-matching-engine 2>&1 | tail -10
```

Expected: all green. The `OrderAccepted { .. }` rest-pattern matches in stage0_e2e / stage1a_e2e are forward-compat — no rewrites needed there.

- [ ] **Step 7: Commit**

```bash
git add crates/exg-protocol/src/event.rs \
        crates/exg-protocol/src/lib.rs \
        crates/exg-matching-engine/src/engine.rs
git commit -m "$(cat <<'EOF'
feat(protocol): extend OrderAccepted with replay-required fields

WAL replay (Stage 1b) needs to rebuild a BookOrder from a single
OrderAccepted event. Add: side, order_type, time_in_force, price,
quantity, stop_price, reduce_only. All 5 emission sites in the
matching engine populate the new fields from in-scope locals.

Breaking change to event.rs schema; rkyv-encoded WAL files predating
this commit cannot be replayed. Acceptable — no production WAL.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: `apply_event` + `ApplyError` + unit tests

**Files:**
- Create: `crates/exg-matching-engine/src/replay.rs`
- Modify: `crates/exg-matching-engine/src/lib.rs` (export the new module)

### Step 1: Add `pub mod replay;` to lib.rs

In `crates/exg-matching-engine/src/lib.rs`, add to the existing module declarations:

```rust
pub mod engine;
pub mod matcher;
pub mod orderbook;
pub mod replay;       // ← NEW
pub mod snapshot;

pub use engine::MatchingEngine;
pub use matcher::Fill;
pub use orderbook::{BookOrder, DepthLevel, OrderBook, PriceLevel};
pub use replay::ApplyError;   // ← NEW
pub use snapshot::EngineSnapshot;
```

- [ ] **Step 2: Write `crates/exg-matching-engine/src/replay.rs` skeleton with the failing test for `OrderAccepted`**

Create the file:

```rust
//! Stage 1b — WAL replay. Apply historical events to a freshly-built engine.
//!
//! Each WAL record is one `Event`. `apply_event` reverse-maps the event onto
//! the matching engine's mutable state without re-running matching: the
//! matching that produced these events has already been recorded as
//! subsequent `OrderFilled` events. Re-running matching during replay would
//! double-count fills.
//!
//! Replay is **not** idempotent. The caller must apply events in WAL order.

use exg_common::{Decimal128, OrderId};
use exg_protocol::Event;

use crate::engine::MatchingEngine;
use crate::orderbook::BookOrder;

#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    #[error("event references unknown order_id={0:?}")]
    UnknownOrder(OrderId),
    #[error("OrderAccepted for order_id={0:?} already present in book")]
    DuplicateOrder(OrderId),
    #[error("OrderFilled fill_qty {got} exceeds existing remaining {have}")]
    OverFill {
        got: Decimal128,
        have: Decimal128,
    },
    #[error("event variant {variant} unexpected during replay")]
    UnexpectedVariant {
        variant: &'static str,
    },
}

impl MatchingEngine {
    /// Apply a historical event to the engine state during WAL replay.
    ///
    /// **Replay-only.** Must NOT be called on a live engine (no concurrency
    /// protection; will produce nonsense state if interleaved with
    /// `process_command`).
    pub fn apply_event(&mut self, event: &Event) -> Result<(), ApplyError> {
        match event {
            Event::OrderAccepted {
                order_id,
                user_id,
                symbol,
                client_order_id,
                timestamp,
                side,
                order_type,
                time_in_force,
                price,
                quantity,
                stop_price,
                reduce_only,
            } => {
                if self.orderbook_mut().get_order(*order_id).is_some() {
                    return Err(ApplyError::DuplicateOrder(*order_id));
                }
                let book_order = BookOrder {
                    order_id: *order_id,
                    user_id: *user_id,
                    symbol: *symbol,
                    side: *side,
                    price: *price,
                    remaining_qty: *quantity,
                    original_qty: *quantity,
                    order_type: *order_type,
                    time_in_force: *time_in_force,
                    is_reduce_only: *reduce_only,
                    timestamp: *timestamp,
                    visible_qty: None,
                    hidden_qty: Decimal128::ZERO,
                    trailing_delta: None,
                    trailing_peak_price: None,
                    expire_time: None,
                    client_order_id: *client_order_id,
                    stop_price: *stop_price,
                };
                // Conditional orders sit in stop_orders; everything else on the book.
                if order_type.is_conditional() {
                    self.stop_orders_mut().push(book_order);
                } else {
                    self.orderbook_mut().insert_order(book_order);
                }
                Ok(())
            }
            Event::OrderRejected { .. } => Ok(()),
            Event::OrderCanceled { order_id, .. } => {
                if self.orderbook_mut().remove_order(*order_id).is_none() {
                    // Also try stop_orders (conditional orders).
                    let removed = self
                        .stop_orders_mut()
                        .iter()
                        .position(|o| o.order_id == *order_id)
                        .map(|i| self.stop_orders_mut().remove(i));
                    if removed.is_none() {
                        return Err(ApplyError::UnknownOrder(*order_id));
                    }
                }
                Ok(())
            }
            Event::OrderFilled {
                order_id,
                fill_qty,
                remaining_qty,
                ..
            } => {
                let book = self.orderbook_mut();
                let existing = match book.get_order(*order_id) {
                    Some(o) => o,
                    None => return Err(ApplyError::UnknownOrder(*order_id)),
                };
                if *fill_qty > existing.remaining_qty {
                    return Err(ApplyError::OverFill {
                        got: *fill_qty,
                        have: existing.remaining_qty,
                    });
                }
                if remaining_qty.is_zero() {
                    book.remove_order(*order_id);
                } else {
                    book.update_qty(*order_id, *remaining_qty);
                }
                Ok(())
            }
            Event::TradeExecuted { .. } => Ok(()),
            Event::MarkPriceUpdate { .. } => Err(ApplyError::UnexpectedVariant {
                variant: "MarkPriceUpdate",
            }),
            Event::FundingRateUpdate { .. } => Err(ApplyError::UnexpectedVariant {
                variant: "FundingRateUpdate",
            }),
            Event::LiquidationOrder { .. } => Err(ApplyError::UnexpectedVariant {
                variant: "LiquidationOrder",
            }),
        }
    }
}
```

Two accessors are referenced (`orderbook_mut`, `stop_orders_mut`) that do not yet exist on `MatchingEngine`. The next step adds them.

- [ ] **Step 3: Add `orderbook_mut` and `stop_orders_mut` to `engine.rs`**

In `crates/exg-matching-engine/src/engine.rs`, find the existing `pub fn orderbook(&self) -> &OrderBook` accessor (around line 962) and add right after it:

```rust
    /// Mutable orderbook access — replay-only.
    #[doc(hidden)]
    pub fn orderbook_mut(&mut self) -> &mut OrderBook {
        &mut self.orderbook
    }

    /// Mutable stop-orders access — replay-only.
    #[doc(hidden)]
    pub fn stop_orders_mut(&mut self) -> &mut Vec<BookOrder> {
        &mut self.stop_orders
    }
```

- [ ] **Step 4: Run `cargo check -p exg-matching-engine`**

```bash
cargo check -p exg-matching-engine 2>&1 | tail -5
```

Expected: `Finished`. If `thiserror` is not yet a dep of `exg-matching-engine`, add it to `crates/exg-matching-engine/Cargo.toml`:

```toml
[dependencies]
thiserror = { workspace = true }
```

(`thiserror` is already in the workspace deps — used by `exg-common` and `exg-wal`.)

- [ ] **Step 5: Add the 10 unit tests to `replay.rs`**

Append to the bottom of `crates/exg-matching-engine/src/replay.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use exg_common::{
        Decimal128, OrderId, OrderType, Side, SymbolId, TimeInForce, TradeId, UnixMicros, UserId,
    };
    use exg_protocol::{Event, RejectReason};
    use exg_risk_engine::{MarginTier, SymbolConfig};

    fn dec(s: &str) -> Decimal128 {
        s.parse().unwrap()
    }

    fn ts() -> UnixMicros {
        UnixMicros::from_micros(1_700_000_000_000_000)
    }

    fn test_engine() -> MatchingEngine {
        let cfg = SymbolConfig {
            symbol: SymbolId::new(1),
            tick_size: dec("0.01"),
            lot_size: dec("0.001"),
            min_notional: dec("10"),
            max_leverage: dec("125"),
            maker_fee: dec("0.0002"),
            taker_fee: dec("0.0005"),
            margin_tiers: vec![MarginTier {
                notional_floor: dec("0"),
                notional_cap: dec("50000"),
                maintenance_margin_rate: dec("0.004"),
                maintenance_amount: dec("0"),
            }],
            impact_notional: dec("200"),
        };
        MatchingEngine::new(cfg, 1)
    }

    fn accept_event(order_id: u64, qty: &str, price: &str) -> Event {
        Event::OrderAccepted {
            order_id: OrderId::new(order_id),
            user_id: UserId::new(42),
            symbol: SymbolId::new(1),
            client_order_id: None,
            timestamp: ts(),
            side: Side::Buy,
            order_type: OrderType::Limit,
            time_in_force: TimeInForce::Gtc,
            price: dec(price),
            quantity: dec(qty),
            stop_price: None,
            reduce_only: false,
        }
    }

    #[test]
    fn apply_order_accepted_inserts_book_order() {
        let mut engine = test_engine();
        engine.apply_event(&accept_event(1, "1.0", "50000")).unwrap();
        let order = engine.orderbook().get_order(OrderId::new(1)).unwrap();
        assert_eq!(order.remaining_qty, dec("1.0"));
        assert_eq!(order.price, dec("50000"));
    }

    #[test]
    fn apply_order_canceled_removes_book_order() {
        let mut engine = test_engine();
        engine.apply_event(&accept_event(1, "1.0", "50000")).unwrap();
        engine
            .apply_event(&Event::OrderCanceled {
                order_id: OrderId::new(1),
                user_id: UserId::new(42),
                symbol: SymbolId::new(1),
                remaining_qty: dec("1.0"),
                timestamp: ts(),
            })
            .unwrap();
        assert!(engine.orderbook().get_order(OrderId::new(1)).is_none());
    }

    #[test]
    fn apply_order_filled_decrements_remaining_qty() {
        let mut engine = test_engine();
        engine.apply_event(&accept_event(1, "1.0", "50000")).unwrap();
        engine
            .apply_event(&Event::OrderFilled {
                order_id: OrderId::new(1),
                trade_id: TradeId::new(100),
                user_id: UserId::new(42),
                symbol: SymbolId::new(1),
                side: Side::Buy,
                fill_price: dec("50000"),
                fill_qty: dec("0.4"),
                is_maker: true,
                remaining_qty: dec("0.6"),
                timestamp: ts(),
            })
            .unwrap();
        let order = engine.orderbook().get_order(OrderId::new(1)).unwrap();
        assert_eq!(order.remaining_qty, dec("0.6"));
    }

    #[test]
    fn apply_order_filled_zero_removes_book_order() {
        let mut engine = test_engine();
        engine.apply_event(&accept_event(1, "1.0", "50000")).unwrap();
        engine
            .apply_event(&Event::OrderFilled {
                order_id: OrderId::new(1),
                trade_id: TradeId::new(100),
                user_id: UserId::new(42),
                symbol: SymbolId::new(1),
                side: Side::Buy,
                fill_price: dec("50000"),
                fill_qty: dec("1.0"),
                is_maker: false,
                remaining_qty: Decimal128::ZERO,
                timestamp: ts(),
            })
            .unwrap();
        assert!(engine.orderbook().get_order(OrderId::new(1)).is_none());
    }

    #[test]
    fn apply_trade_executed_is_noop_on_book() {
        let mut engine = test_engine();
        engine.apply_event(&accept_event(1, "1.0", "50000")).unwrap();
        engine
            .apply_event(&Event::TradeExecuted {
                trade_id: TradeId::new(100),
                symbol: SymbolId::new(1),
                price: dec("50000"),
                qty: dec("0.5"),
                buyer_order_id: OrderId::new(1),
                seller_order_id: OrderId::new(2),
                buyer_user_id: UserId::new(42),
                seller_user_id: UserId::new(43),
                buyer_fee: dec("0.005"),
                seller_fee: dec("0.0125"),
                timestamp: ts(),
            })
            .unwrap();
        // book unchanged
        assert_eq!(
            engine.orderbook().get_order(OrderId::new(1)).unwrap().remaining_qty,
            dec("1.0")
        );
    }

    #[test]
    fn apply_order_rejected_is_noop() {
        let mut engine = test_engine();
        engine
            .apply_event(&Event::OrderRejected {
                order_id: OrderId::new(9),
                user_id: UserId::new(42),
                reason: RejectReason::InsufficientMargin,
                timestamp: ts(),
            })
            .unwrap();
        assert_eq!(engine.orderbook().order_count(), 0);
    }

    #[test]
    fn apply_duplicate_order_accepted_returns_err() {
        let mut engine = test_engine();
        engine.apply_event(&accept_event(1, "1.0", "50000")).unwrap();
        let err = engine
            .apply_event(&accept_event(1, "2.0", "50001"))
            .unwrap_err();
        assert!(matches!(err, ApplyError::DuplicateOrder(_)));
    }

    #[test]
    fn apply_unknown_order_canceled_returns_err() {
        let mut engine = test_engine();
        let err = engine
            .apply_event(&Event::OrderCanceled {
                order_id: OrderId::new(99),
                user_id: UserId::new(42),
                symbol: SymbolId::new(1),
                remaining_qty: dec("0"),
                timestamp: ts(),
            })
            .unwrap_err();
        assert!(matches!(err, ApplyError::UnknownOrder(_)));
    }

    #[test]
    fn apply_over_fill_returns_err() {
        let mut engine = test_engine();
        engine.apply_event(&accept_event(1, "1.0", "50000")).unwrap();
        let err = engine
            .apply_event(&Event::OrderFilled {
                order_id: OrderId::new(1),
                trade_id: TradeId::new(100),
                user_id: UserId::new(42),
                symbol: SymbolId::new(1),
                side: Side::Buy,
                fill_price: dec("50000"),
                fill_qty: dec("2.0"),
                is_maker: true,
                remaining_qty: Decimal128::ZERO,
                timestamp: ts(),
            })
            .unwrap_err();
        assert!(matches!(err, ApplyError::OverFill { .. }));
    }

    #[test]
    fn apply_mark_price_update_returns_unexpected_variant() {
        let mut engine = test_engine();
        let err = engine
            .apply_event(&Event::MarkPriceUpdate {
                symbol: SymbolId::new(1),
                mark_price: dec("50000"),
                index_price: dec("50000"),
                timestamp: ts(),
            })
            .unwrap_err();
        assert!(matches!(
            err,
            ApplyError::UnexpectedVariant { variant: "MarkPriceUpdate" }
        ));
    }

    #[test]
    fn apply_unknown_order_filled_returns_err() {
        let mut engine = test_engine();
        // No prior OrderAccepted — OrderFilled must be rejected.
        let err = engine
            .apply_event(&Event::OrderFilled {
                order_id: OrderId::new(999),
                trade_id: TradeId::new(100),
                user_id: UserId::new(42),
                symbol: SymbolId::new(1),
                side: Side::Buy,
                fill_price: dec("50000"),
                fill_qty: dec("0.1"),
                is_maker: false,
                remaining_qty: dec("0.9"),
                timestamp: ts(),
            })
            .unwrap_err();
        assert!(matches!(err, ApplyError::UnknownOrder(_)));
    }

    #[test]
    fn replay_then_take_snapshot_round_trip() {
        use exg_common::SnowflakeGen;
        use exg_protocol::Command;
        // Live engine: process N NewOrder commands, collect events.
        let mut live = test_engine();
        live.set_mark_price(dec("60000"));
        let sf = SnowflakeGen::new(1);
        let mut events = Vec::new();
        for i in 0..5 {
            let cmd = Command::NewOrder {
                order_id: OrderId::new(sf.next_id()),
                user_id: UserId::new(42),
                symbol: SymbolId::new(1),
                side: Side::Buy,
                order_type: OrderType::Limit,
                time_in_force: TimeInForce::Gtc,
                price: Some(dec(&format!("{}", 55000 + i))),
                quantity: dec("0.001"),
                stop_price: None,
                trailing_delta: None,
                visible_quantity: None,
                reduce_only: false,
                timestamp: ts(),
                client_order_id: None,
            };
            events.extend(live.process_command(&cmd));
        }
        // Empty engine: apply the same events.
        let mut replayed = test_engine();
        for evt in &events {
            replayed.apply_event(evt).unwrap();
        }
        assert_eq!(
            live.orderbook().order_count(),
            replayed.orderbook().order_count(),
            "replayed engine must have same order count as live engine"
        );
    }
}
```

- [ ] **Step 6: Run the unit tests**

```bash
cargo test -p exg-matching-engine --lib replay 2>&1 | tail -15
```

Expected:

```
test replay::tests::apply_order_accepted_inserts_book_order ... ok
test replay::tests::apply_order_canceled_removes_book_order ... ok
test replay::tests::apply_order_filled_decrements_remaining_qty ... ok
test replay::tests::apply_order_filled_zero_removes_book_order ... ok
test replay::tests::apply_trade_executed_is_noop_on_book ... ok
test replay::tests::apply_order_rejected_is_noop ... ok
test replay::tests::apply_duplicate_order_accepted_returns_err ... ok
test replay::tests::apply_unknown_order_canceled_returns_err ... ok
test replay::tests::apply_unknown_order_filled_returns_err ... ok
test replay::tests::apply_over_fill_returns_err ... ok
test replay::tests::apply_mark_price_update_returns_unexpected_variant ... ok
test replay::tests::replay_then_take_snapshot_round_trip ... ok

test result: ok. 12 passed; 0 failed; ...
```

- [ ] **Step 7: Commit**

```bash
git add crates/exg-matching-engine/src/replay.rs \
        crates/exg-matching-engine/src/lib.rs \
        crates/exg-matching-engine/src/engine.rs \
        crates/exg-matching-engine/Cargo.toml
git commit -m "$(cat <<'EOF'
feat(matching-engine): add apply_event for WAL replay

Stage 1b lifecycle (Step 3.6) feeds each WAL event back through this
function to rebuild engine state from scratch. Dispatch covers:
- OrderAccepted: insert BookOrder into book or stop_orders by type
- OrderRejected: no-op
- OrderCanceled: remove from book (or stop_orders for conditionals)
- OrderFilled: decrement remaining_qty; remove when zero
- TradeExecuted: no-op (OrderFilled events on both sides cover state)
- MarkPriceUpdate / FundingRateUpdate / LiquidationOrder: rejected
  (Stage 0/1a never emit these; explicit error catches schema drift)

ApplyError enumerates: UnknownOrder, DuplicateOrder, OverFill,
UnexpectedVariant. 10 unit tests cover happy + error paths.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Boot lifecycle — remove invariant 3, add Step 3.6 + 3.8

**Files:**
- Modify: `crates/exg-server/src/lib.rs` — `validate_invariants` (~lines 95-119) and `run_with_config_with_pool` (after Step 3.5, before Step 3.7)

### Step 1: Remove invariant 3 (WAL must be empty)

In `crates/exg-server/src/lib.rs::validate_invariants`, delete the entire "Invariant 3" block (currently lines ~95-119). The block starts with `// Invariant 3: WAL directory must be empty or not yet created.` and ends after the `}` that closes the `if wal_dir.exists() { ... }` block. The `let wal_dir = ...` line is inside the same block — remove it too.

After deletion, `validate_invariants` flows directly from the previous invariant (mark price > 0) into Invariant 11 (jwt_secret length).

- [ ] **Step 2: Run `cargo check -p exg-server`**

```bash
cargo check -p exg-server 2>&1 | tail -5
```

Expected: clean. The `PathBuf` import may become unused — leave it for now (used elsewhere in the file).

- [ ] **Step 3: Add Step 3.6 (WAL replay) inside `run_with_config_with_pool` (CEO review A3 — single `ReplayError` enum)**

In `crates/exg-server/src/lib.rs::run_with_config_with_pool`, find the existing Step 3.7 (`init_dummy_argon2_hash`). Insert Step 3.6 immediately before it, using a single `Option<ReplayError>` for error propagation instead of three separate option captures:

```rust
    // ── Step 3.6 (Stage 1b): WAL replay ───────────────────────────────────
    // Boot may be picking up where a previous instance left off. Replay
    // every WAL record through engine.apply_event so the matching engine
    // resumes with the same orderbook state. Step 0 (validate_invariants)
    // has already passed; Step 3.5 (PG ping) confirms DB connectivity;
    // replay runs on the boot thread before the matching thread is spawned,
    // so no locking is needed.
    {
        use exg_wal::WalReader;

        // Local error enum keeps the closure body single-exit and the post-loop
        // check single-branch. Variants map 1:1 to the failure modes documented
        // in the spec §7.1 boot-panic table.
        enum ReplayError {
            SequenceGap { expected: u64, got: u64 },
            Decode { seq: u64, msg: String },
            Apply { seq: u64, msg: String },
        }
        impl std::fmt::Display for ReplayError {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match self {
                    ReplayError::SequenceGap { expected, got } => write!(
                        f,
                        "WAL replay failed: sequence gap at expected={expected}, got={got}"
                    ),
                    ReplayError::Decode { seq, msg } => {
                        write!(f, "WAL replay failed: rkyv decode at sequence {seq}: {msg}")
                    }
                    ReplayError::Apply { seq, msg } => {
                        write!(f, "WAL replay failed at sequence {seq}: {msg}")
                    }
                }
            }
        }

        let wal_dir = PathBuf::from(&cfg.wal.dir);
        let mut reader = WalReader::open(&wal_dir).context("WAL reader open")?;
        let mut expected_seq: u64 = 0;
        let mut replayed_count: u64 = 0;
        let mut replay_err: Option<ReplayError> = None;

        reader
            .read_from(0, |seq, payload| {
                if seq != expected_seq {
                    replay_err = Some(ReplayError::SequenceGap {
                        expected: expected_seq,
                        got: seq,
                    });
                    return false;
                }
                let event = match rkyv::from_bytes::<exg_protocol::Event, rkyv::rancor::Error>(
                    payload,
                ) {
                    Ok(e) => e,
                    Err(e) => {
                        replay_err = Some(ReplayError::Decode {
                            seq,
                            msg: format!("{e}"),
                        });
                        return false;
                    }
                };
                if let Err(e) = engine.apply_event(&event) {
                    replay_err = Some(ReplayError::Apply {
                        seq,
                        msg: format!("{e}"),
                    });
                    return false;
                }
                expected_seq = seq + 1;
                replayed_count += 1;
                true
            })
            .map_err(|e| anyhow::anyhow!("WAL replay failed: {e}"))?;

        if let Some(err) = replay_err {
            anyhow::bail!("{err}");
        }

        // ── Step 3.8 (Stage 1b): invariant 21 ─────────────────────────────
        let writer_next = wal.lock().current_sequence();
        if replayed_count != writer_next {
            anyhow::bail!(
                "invariant 21 violated: replayed_count={replayed_count}, wal_writer.current_seq={writer_next}"
            );
        }

        if replayed_count > 0 {
            tracing::info!(
                target: "boot",
                replayed_count,
                last_seq = expected_seq.saturating_sub(1),
                "WAL replay complete"
            );
        }
    }
```

The `engine` binding and `wal` binding are both in scope at this point in `run_with_config_with_pool` (engine is built in Step 3, wal in Step 1). `wal` is `Arc<Mutex<WalWriter>>`; `wal.lock().current_sequence()` returns the next-seq-to-assign.

`PathBuf` is already imported at the top of `lib.rs`. `exg_wal::WalReader` is added inside the block to keep its scope tight; alternatively add `use exg_wal::WalReader;` at the top.

- [ ] **Step 4: Run `cargo check --workspace`**

```bash
cargo check --workspace 2>&1 | tail -5
```

Expected: clean.

- [ ] **Step 5: Sanity-run existing tests (replay path is exercised even on empty WAL)**

```bash
DATABASE_URL=postgres://exg:exg_dev_password@localhost:5433/exg \
  cargo test -p exg-server --test stage1a_e2e 2>&1 | tail -8
```

Expected: 12/12 still passing. The replay path runs on every boot now; with empty WAL `replayed_count == 0 == writer.current_sequence()` (writer is fresh too), so the invariant holds.

- [ ] **Step 6: Commit**

```bash
git add crates/exg-server/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(server): WAL replay at boot (Stage 1b Step 3.6 + 3.8)

- Remove Stage 0 invariant 3 (WAL must be empty); non-empty WAL is now
  the expected state after restart
- Step 3.6: scan every WAL record from seq 0, decode via rkyv,
  apply_event onto the empty engine; boot panics with WAL replay
  message on sequence gap / rkyv decode / apply_event error
- Step 3.8: invariant 21 — replayed_count == wal_writer.current_sequence()
- WalWriter::open already handles tail truncation + mid-stream CRC
  panic from Stage 0; Stage 1b just adds the consumer side

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Boot panic tests — remove invariant 3 test, add corrupt-WAL + sequence-gap tests

**Files:**
- Modify: `crates/exg-server/tests/boot_panics.rs`

### Step 1: Remove the obsolete `boot_panics_on_nonempty_wal_dir` test

In `crates/exg-server/tests/boot_panics.rs`, delete the entire `boot_panics_on_nonempty_wal_dir` test function (its body asserts the WAL-non-empty panic that no longer happens).

- [ ] **Step 2: Add the new corrupt-WAL test**

Append to `crates/exg-server/tests/boot_panics.rs`:

```rust
#[actix_web::test]
async fn boot_panics_on_corrupt_wal_crc() {
    let tmp = TempDir::new().unwrap();
    let wal_dir = tmp.path().join("wal");
    std::fs::create_dir(&wal_dir).unwrap();

    // Hand-craft a WAL segment with a bogus CRC. Real layout: each record
    // is [4-byte len LE][8-byte seq LE][payload][4-byte CRC LE].
    // We write seq=0 with empty payload but the wrong CRC. The exact byte
    // layout is documented in `crates/exg-wal/src/segment.rs::encode_record`.
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&0u32.to_le_bytes()); // payload len = 0
    bytes.extend_from_slice(&0u64.to_le_bytes()); // seq = 0
    bytes.extend_from_slice(&0xDEADBEEFu32.to_le_bytes()); // wrong CRC
    std::fs::write(wal_dir.join("wal-00000000000000000000.log"), &bytes).unwrap();

    let mut cfg = base_cfg(tmp.path());
    cfg.wal.dir = wal_dir.to_string_lossy().into_owned();

    let result = exg_server::run_with_config(cfg).await;
    let err = result.err().expect("expected Err from corrupt WAL");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("CRC") || msg.contains("Corrupt") || msg.contains("corrupt"),
        "expected CRC/corruption message, got: {msg}"
    );
}
```

NOTE: this test exercises the `WalWriter::open` corruption-detect path (Stage 0's pre-existing `recover_state`), since writer-open runs before reader-replay. The replay step never sees the corrupt record — writer aborts first. Spec §7.1 documents this ordering explicitly.

- [ ] **Step 3: Add the new sequence-gap test**

This one needs Stage 1b's replay code to fire (not writer-open), so we must produce a WAL that writer-open accepts but reader-replay rejects. Approach: write a valid segment header with no records, but trick the reader by writing a record with seq=5 (skipping 0-4).

The simpler approach is to bypass writer-open entirely by manipulating segments after a clean shutdown. Append:

```rust
#[actix_web::test]
async fn boot_panics_on_sequence_gap() {
    use exg_wal::{WalConfig, WalWriter};

    let tmp = TempDir::new().unwrap();
    let wal_dir = tmp.path().join("wal");
    std::fs::create_dir(&wal_dir).unwrap();

    // Step 1: write three valid records (seq 0,1,2).
    {
        let mut w = WalWriter::open(WalConfig {
            dir: wal_dir.clone(),
            segment_size: 64 * 1024 * 1024,
            flush_interval_us: 1000,
            flush_every_n: 1,
        })
        .unwrap();
        for _ in 0..3 {
            w.append(b"hello").unwrap();
        }
        w.flush().unwrap();
    }

    // Step 2: surgically delete record at seq=1 by rewriting the segment.
    // Simpler approach: truncate the segment to one record (keeps seq=0
    // only), then append a fresh segment file claiming first_seq=5.
    // The list_segments scan returns both; reader reads seq=0 then jumps to
    // seq=5 — gap.
    let segments: Vec<_> = std::fs::read_dir(&wal_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    assert_eq!(segments.len(), 1, "expected exactly one segment");
    let seg0 = &segments[0];
    let raw = std::fs::read(seg0).unwrap();

    // First record ends at offset = 4 (len) + 8 (seq) + 5 (payload) + 4 (crc) = 21
    let truncated = raw[..21].to_vec();
    std::fs::write(seg0, &truncated).unwrap();

    // Create a second segment named for first_seq=5 with one record at seq=5.
    let mut second = Vec::new();
    second.extend_from_slice(&5u32.to_le_bytes()); // payload len = 5
    second.extend_from_slice(&5u64.to_le_bytes()); // seq = 5
    second.extend_from_slice(b"hello");
    let crc = crc32fast::hash(&{
        let mut h = Vec::new();
        h.extend_from_slice(&5u32.to_le_bytes());
        h.extend_from_slice(&5u64.to_le_bytes());
        h.extend_from_slice(b"hello");
        h
    });
    second.extend_from_slice(&crc.to_le_bytes());
    std::fs::write(wal_dir.join("wal-00000000000000000005.log"), &second).unwrap();

    let mut cfg = base_cfg(tmp.path());
    cfg.wal.dir = wal_dir.to_string_lossy().into_owned();

    let result = exg_server::run_with_config(cfg).await;
    let err = result.err().expect("expected Err from sequence gap");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("sequence gap") || msg.contains("gap"),
        "expected sequence-gap message, got: {msg}"
    );
}
```

If `crc32fast` is not yet a dev-dep of `exg-server`, add it. Check first:

```bash
grep -E "crc32fast" crates/exg-server/Cargo.toml
```

If absent, add to `[dev-dependencies]`:

```toml
crc32fast = { workspace = true }
```

(crc32fast is already in workspace deps via exg-wal.)

Also add `exg-wal = { workspace = true }` to `[dev-dependencies]` if not present.

- [ ] **Step 4: Update `base_cfg()` to point WAL at the wal subdirectory by default**

The existing `base_cfg` uses `wal_dir = tmp.path()`. The new tests put the WAL under `tmp.path().join("wal")` to keep the temp dir tidy and avoid name collisions with the new segment files. Keep base_cfg unchanged; the new tests override `cfg.wal.dir` themselves.

- [ ] **Step 5: Run the boot panic suite**

```bash
DATABASE_URL=postgres://exg:exg_dev_password@localhost:5433/exg \
  cargo test -p exg-server --test boot_panics 2>&1 | tail -10
```

Expected: **8 tests pass** (6 retained from Stage 0/1a — minus the removed nonempty_wal_dir = 6 — plus 2 new = **8 total**).

Wait — Stage 1a's count was 7/7 (4 Stage 0 + 3 Stage 1a). Remove `boot_panics_on_nonempty_wal_dir` → 6 retained. Add `boot_panics_on_corrupt_wal_crc` + `boot_panics_on_sequence_gap` → **8 total**.

- [ ] **Step 6: Commit**

```bash
git add crates/exg-server/tests/boot_panics.rs crates/exg-server/Cargo.toml
git commit -m "$(cat <<'EOF'
test(server): replace WAL-empty boot panic with replay-corruption tests

- Remove boot_panics_on_nonempty_wal_dir (Stage 0 invariant 3 retired)
- Add boot_panics_on_corrupt_wal_crc — exercises WalWriter::open
  recover_state path (CRC failure at writer open, before replay)
- Add boot_panics_on_sequence_gap — surgically craft a WAL with a
  missing sequence, replay rejects with 'sequence gap'

Net delta: 7 -> 8 boot panic tests.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Handler polish — `consume_user_bucket` helper + cancel/amend + login double-consume

**Files:**
- Modify: `crates/exg-api-gateway/src/handlers.rs`

### Step 1: Extract `consume_user_bucket` helper (CEO review A2)

The rate-limit consumption pattern would appear 3 times (place + cancel + amend) without a helper. Extract once. In `crates/exg-api-gateway/src/handlers.rs`, near the existing helper `extract_user_id_from_jwt`, add:

```rust
/// Charge one token from the per-user rate limit bucket (`user:{id}` key).
/// Shared by place / cancel / amend. Returns 429 + ERR_RATE_LIMITED_USER on miss.
fn consume_user_bucket(state: &AppState, user_id: UserId) -> Result<(), ApiError> {
    let now_ts = UnixMicros::now();
    let key = format!("user:{}", user_id.value());
    let mut limiter = state.rate_limiter.lock();
    if !limiter.consume(&key, now_ts) {
        return Err(ApiError::user_rate_limited("rate limit exceeded for user"));
    }
    Ok(())
}
```

`UserId` is already imported at the top of handlers.rs.

- [ ] **Step 2: Migrate `place_order` to use the helper**

Find the existing in-line rate-limit block in `place_order` (the one Stage 1a added). Replace the inline block with:

```rust
    consume_user_bucket(&state, user_id)?;
```

This removes ~7 lines from `place_order`.

- [ ] **Step 3: Add the helper call to `cancel_order`**

In `cancel_order`, immediately after `let user_id = extract_user_id_from_jwt(&req, ...)?;` and before `let symbol = ...`, insert:

```rust
    consume_user_bucket(&state, user_id)?;
```

- [ ] **Step 4: Add the helper call to `amend_order`**

In `amend_order`, same position (after `extract_user_id_from_jwt`, before `let symbol = ...`), insert:

```rust
    consume_user_bucket(&state, user_id)?;
```

All three handlers now share one bucket via one call site. Future endpoints (`cancel_all`, etc.) reuse the same helper.

- [ ] **Step 5: Fix login `||` short-circuit**

In `handlers.rs::login`, find the existing block (around line 246):

```rust
    {
        let mut limiter = state.rate_limiter.lock();
        if !limiter.consume(&email_key, now_ts) || !limiter.consume(&ip_key, now_ts) {
            return Err(ApiError::user_rate_limited(
                "login rate limit exceeded",
            ));
        }
    }
```

Replace with:

```rust
    {
        let mut limiter = state.rate_limiter.lock();
        // Charge BOTH buckets unconditionally — || would short-circuit and
        // skip the second consume if the first refused. An attacker cycling
        // emails from one IP must still consume the IP bucket.
        let email_ok = limiter.consume(&email_key, now_ts);
        let ip_ok = limiter.consume(&ip_key, now_ts);
        if !email_ok || !ip_ok {
            return Err(ApiError::user_rate_limited(
                "login rate limit exceeded",
            ));
        }
    }
```

- [ ] **Step 6: Run handler tests (lib tests)**

```bash
cargo test -p exg-api-gateway --lib 2>&1 | tail -5
```

Expected: 29/29 still passing (handler unit tests do not assert rate-limit on cancel/amend; e2e tests in Task 6 cover the new behavior).

- [ ] **Step 7: Commit**

```bash
git add crates/exg-api-gateway/src/handlers.rs
git commit -m "$(cat <<'EOF'
fix(api-gateway): close Stage 1a rate-limit gaps

- New helper consume_user_bucket(&state, user_id) shared by place /
  cancel / amend (DRY; CEO review A2). All three handlers route
  through one call site with key "user:{N}".
- login: stop short-circuiting the IP bucket consume when the email
  bucket has already been exhausted. Both buckets charged on every
  attempt; rotating emails from one IP still depletes the IP bucket.

Spec §9 invariant 17 reworded: 'per-user rate limit on authenticated
order endpoints (place / cancel / amend)'.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Stage 1b e2e suite — 16 integration tests

**Files:**
- Create: `crates/exg-server/tests/stage1b_e2e.rs`

This task is split into manageable chunks: shared setup, replay correctness (9 tests), polish (7 tests). Commit at the end.

### Step 1: Write file scaffold + shared helpers

Create `crates/exg-server/tests/stage1b_e2e.rs`:

```rust
//! Stage 1b end-to-end tests: WAL replay + Stage 1a polish coverage.
//! Every test gets its own throwaway PG database via #[sqlx::test].

use exg_config::ExgConfig;
use exg_matching_engine::MatchingEngine;
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
    cfg.auth.jwt_secret = "stage1b-test-secret-padding-32-bytes-ok".into();
    cfg
}

async fn boot_server(cfg: ExgConfig, pool: PgPool) -> (exg_server::ServerHandle, String) {
    let handle = exg_server::run_with_config_with_pool(cfg, Some(pool))
        .await
        .expect("server boot");
    let base = format!("http://127.0.0.1:{}", handle.bound_port);
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
    panic!("server not ready");
}

async fn register_and_login(
    client: &Client,
    base: &str,
    email: &str,
    password: &str,
) -> String {
    let _ = client
        .post(format!("{base}/api/v1/auth/register"))
        .json(&serde_json::json!({"email": email, "password": password}))
        .send()
        .await
        .unwrap();
    let resp: serde_json::Value = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&serde_json::json!({"email": email, "password": password}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    resp["accessToken"]
        .as_str()
        .unwrap_or_else(|| panic!("accessToken missing: {resp}"))
        .to_string()
}

/// Reboot the server against the same WAL + pool, exercising the replay path.
/// Returns the new ServerHandle + base URL.
async fn reboot(
    cfg: ExgConfig,
    pool: PgPool,
) -> (exg_server::ServerHandle, String) {
    boot_server(cfg, pool).await
}
```

- [ ] **Step 2: Write replay correctness tests 1–4**

Append to `stage1b_e2e.rs`:

```rust
#[sqlx::test(migrations = "../../migrations")]
async fn boot_replays_empty_wal_succeeds(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let (handle, base) = boot_server(cfg, pool).await;
    // Health endpoint reachable proves boot finished.
    let client = Client::new();
    let resp = client.get(format!("{base}/api/v1/health")).send().await.unwrap();
    assert!(resp.status().is_success());
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn boot_replays_single_order_restores_orderbook(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let wal_dir = std::path::PathBuf::from(tmp.path());
    let cfg1 = base_cfg(tmp.path());
    // Boot 1: place one order, kill.
    {
        let (handle, base) = boot_server(cfg1.clone(), pool.clone()).await;
        let client = Client::new();
        let token = register_and_login(&client, &base, "happy@e.com", "hunter2hunter2").await;
        let resp = client
            .post(format!("{base}/api/v1/order"))
            .header("Authorization", format!("Bearer {token}"))
            .json(&serde_json::json!({
                "symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT",
                "timeInForce":"GTC","quantity":"0.001","price":"59000",
                "clientOrderId":"800001"
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        tokio::time::sleep(Duration::from_millis(200)).await; // let WAL flush
        handle.shutdown().await.unwrap();
    }

    // Independently inspect the WAL: at least one OrderAccepted recorded.
    let mut reader = WalReader::open(&wal_dir).unwrap();
    let mut accept_count = 0;
    reader
        .read_from(0, |_seq, payload| {
            let e: Event =
                rkyv::from_bytes::<Event, rkyv::rancor::Error>(payload).unwrap();
            if matches!(e, Event::OrderAccepted { .. }) {
                accept_count += 1;
            }
            true
        })
        .unwrap();
    assert!(accept_count >= 1, "WAL must contain at least one OrderAccepted");

    // Boot 2: replay. Test passes if boot succeeds (replay applied without panic).
    let (handle2, _base2) = reboot(cfg1, pool).await;
    handle2.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn boot_replays_place_cancel_restores_empty_orderbook(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());

    let order_id: u64 = {
        let (handle, base) = boot_server(cfg.clone(), pool.clone()).await;
        let client = Client::new();
        let token = register_and_login(&client, &base, "pcr@e.com", "hunter2hunter2").await;
        let resp: serde_json::Value = client
            .post(format!("{base}/api/v1/order"))
            .header("Authorization", format!("Bearer {token}"))
            .json(&serde_json::json!({
                "symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT",
                "timeInForce":"GTC","quantity":"0.001","price":"58000"
            }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let oid: u64 = resp["orderId"].as_str().unwrap().parse().unwrap();
        let cancel = client
            .post(format!("{base}/api/v1/order/cancel"))
            .header("Authorization", format!("Bearer {token}"))
            .json(&serde_json::json!({"orderId": oid, "symbol":"BTCUSDT"}))
            .send()
            .await
            .unwrap();
        assert!(cancel.status().is_success());
        tokio::time::sleep(Duration::from_millis(200)).await;
        handle.shutdown().await.unwrap();
        oid
    };

    // Reboot — replay should leave orderbook empty for that order_id.
    let (handle2, _) = reboot(cfg, pool).await;
    // We cannot directly query the engine over HTTP yet (no /me/orders endpoint
    // in Stage 1b). Boot succeeding means apply_event handled the
    // OrderAccepted + OrderCanceled sequence without error.
    let _ = order_id; // suppress unused-binding; presence in scope documents intent
    handle2.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn boot_replays_place_amend_succeeds(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());

    {
        let (handle, base) = boot_server(cfg.clone(), pool.clone()).await;
        let client = Client::new();
        let token = register_and_login(&client, &base, "pa@e.com", "hunter2hunter2").await;
        let resp: serde_json::Value = client
            .post(format!("{base}/api/v1/order"))
            .header("Authorization", format!("Bearer {token}"))
            .json(&serde_json::json!({
                "symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT",
                "timeInForce":"GTC","quantity":"0.001","price":"58000"
            }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let oid: u64 = resp["orderId"].as_str().unwrap().parse().unwrap();
        let amend = client
            .post(format!("{base}/api/v1/order/amend"))
            .header("Authorization", format!("Bearer {token}"))
            .json(&serde_json::json!({
                "orderId": oid, "symbol":"BTCUSDT", "newPrice":"58500"
            }))
            .send()
            .await
            .unwrap();
        assert!(amend.status().is_success());
        tokio::time::sleep(Duration::from_millis(200)).await;
        handle.shutdown().await.unwrap();
    }

    let (handle2, _) = reboot(cfg, pool).await;
    handle2.shutdown().await.unwrap();
}
```

- [ ] **Step 3: Write replay correctness tests 5–9**

Append:

```rust
#[sqlx::test(migrations = "../../migrations")]
async fn boot_replays_matched_trade_succeeds(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());

    {
        let (handle, base) = boot_server(cfg.clone(), pool.clone()).await;
        let client = Client::new();
        let token_a = register_and_login(&client, &base, "maker@e.com", "hunter2hunter2").await;
        let token_b = register_and_login(&client, &base, "taker@e.com", "hunter2hunter2").await;

        // Maker resting bid at 60000.
        client
            .post(format!("{base}/api/v1/order"))
            .header("Authorization", format!("Bearer {token_a}"))
            .json(&serde_json::json!({
                "symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT",
                "timeInForce":"GTC","quantity":"0.001","price":"60000"
            }))
            .send()
            .await
            .unwrap();

        // Taker hits with a sell at 60000 — should match.
        client
            .post(format!("{base}/api/v1/order"))
            .header("Authorization", format!("Bearer {token_b}"))
            .json(&serde_json::json!({
                "symbol":"BTCUSDT","side":"SELL","orderType":"LIMIT",
                "timeInForce":"GTC","quantity":"0.001","price":"60000"
            }))
            .send()
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(200)).await;
        handle.shutdown().await.unwrap();
    }

    // Reboot — replay must handle OrderAccepted + OrderAccepted + OrderFilled
    // + OrderFilled + TradeExecuted without ApplyError.
    let (handle2, _) = reboot(cfg, pool).await;
    handle2.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn place_then_kill_then_place_continues_sequence(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());

    let first_oid: u64 = {
        let (handle, base) = boot_server(cfg.clone(), pool.clone()).await;
        let client = Client::new();
        let token = register_and_login(&client, &base, "k@e.com", "hunter2hunter2").await;
        let resp: serde_json::Value = client
            .post(format!("{base}/api/v1/order"))
            .header("Authorization", format!("Bearer {token}"))
            .json(&serde_json::json!({
                "symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT",
                "timeInForce":"GTC","quantity":"0.001","price":"57000"
            }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        handle.shutdown().await.unwrap();
        resp["orderId"].as_str().unwrap().parse().unwrap()
    };

    // Reboot, place a second order — must get a different order_id.
    let (handle2, base2) = reboot(cfg, pool.clone()).await;
    let client = Client::new();
    let token = register_and_login(&client, &base2, "k@e.com", "hunter2hunter2").await;
    let resp: serde_json::Value = client
        .post(format!("{base2}/api/v1/order"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&serde_json::json!({
            "symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT",
            "timeInForce":"GTC","quantity":"0.001","price":"57001"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let second_oid: u64 = resp["orderId"].as_str().unwrap().parse().unwrap();
    assert_ne!(first_oid, second_oid, "second boot must allocate fresh order_id");
    handle2.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn boot_replays_three_orders_inspectable_via_wal(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let wal_dir = std::path::PathBuf::from(tmp.path());
    let cfg = base_cfg(tmp.path());

    {
        let (handle, base) = boot_server(cfg.clone(), pool.clone()).await;
        let client = Client::new();
        let token = register_and_login(&client, &base, "three@e.com", "hunter2hunter2").await;
        for i in 0..3 {
            client
                .post(format!("{base}/api/v1/order"))
                .header("Authorization", format!("Bearer {token}"))
                .json(&serde_json::json!({
                    "symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT",
                    "timeInForce":"GTC","quantity":"0.001","price": format!("{}", 56000 + i)
                }))
                .send()
                .await
                .unwrap();
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        handle.shutdown().await.unwrap();
    }

    let mut reader = WalReader::open(&wal_dir).unwrap();
    let mut accept_count = 0;
    reader
        .read_from(0, |_seq, payload| {
            let e: Event = rkyv::from_bytes::<Event, rkyv::rancor::Error>(payload).unwrap();
            if matches!(e, Event::OrderAccepted { .. }) {
                accept_count += 1;
            }
            true
        })
        .unwrap();
    assert_eq!(accept_count, 3);

    let (handle2, _) = reboot(cfg, pool).await;
    handle2.shutdown().await.unwrap();
}
```

(That covers replay tests 1-7 of the 9. Tests 8 and 9 — `boot_panics_on_corrupt_wal_crc` and `boot_panics_on_sequence_gap` — already live in `boot_panics.rs` from Task 4, so we drop them from the e2e suite to avoid duplication.)

Spec listed 9 replay-correctness tests; we have 7 in this file plus the 2 corruption tests in boot_panics.rs = 9 total replay tests. Update the running count: stage1b_e2e.rs has 7 replay + 7 polish = **14 tests**, not 16. The spec's "16" is the e2e-only count; with the boot_panics relocation it lands at 14.

Add two more replay-correctness tests to keep stage1b_e2e.rs at 16 if you want exact spec parity:

```rust
#[sqlx::test(migrations = "../../migrations")]
async fn replay_engine_state_matches_pre_kill_snapshot(pool: PgPool) {
    // Sanity check that replay produces the same engine state as a
    // live engine that processed the same commands. Exercises apply_event
    // on a realistic event stream.
    use exg_common::SnowflakeGen;
    use exg_protocol::Command;
    use exg_risk_engine::{MarginTier, SymbolConfig};
    use exg_common::{Decimal128, OrderId, OrderType, Side, SymbolId, TimeInForce, UnixMicros, UserId};

    fn dec(s: &str) -> Decimal128 { s.parse().unwrap() }

    // Drop the unused pool argument; this test stays in-process.
    drop(pool);

    let cfg = SymbolConfig {
        symbol: SymbolId::new(1),
        tick_size: dec("0.01"),
        lot_size: dec("0.001"),
        min_notional: dec("10"),
        max_leverage: dec("125"),
        maker_fee: dec("0.0002"),
        taker_fee: dec("0.0005"),
        margin_tiers: vec![MarginTier {
            notional_floor: dec("0"),
            notional_cap: dec("50000"),
            maintenance_margin_rate: dec("0.004"),
            maintenance_amount: dec("0"),
        }],
        impact_notional: dec("200"),
    };

    let snowflake = SnowflakeGen::new(1);

    // Live engine: process 3 commands.
    let mut live = MatchingEngine::new(cfg.clone(), 1);
    live.set_mark_price(dec("60000"));
    let mut events_for_replay: Vec<Event> = Vec::new();
    for i in 0..3 {
        let cmd = Command::NewOrder {
            order_id: OrderId::new(snowflake.next_id()),
            user_id: UserId::new(42),
            symbol: SymbolId::new(1),
            side: Side::Buy,
            order_type: OrderType::Limit,
            time_in_force: TimeInForce::Gtc,
            price: Some(dec(&format!("{}", 55000 + i))),
            quantity: dec("0.001"),
            stop_price: None,
            trailing_delta: None,
            visible_quantity: None,
            reduce_only: false,
            timestamp: UnixMicros::now(),
            client_order_id: None,
        };
        events_for_replay.extend(live.process_command(&cmd));
    }

    // Replayed engine: apply the same events into an empty engine.
    let mut replayed = MatchingEngine::new(cfg, 1);
    for evt in &events_for_replay {
        replayed.apply_event(evt).unwrap();
    }

    assert_eq!(
        live.orderbook().order_count(),
        replayed.orderbook().order_count(),
        "replay order_count must match live"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn boot_with_only_rejected_events_succeeds(pool: PgPool) {
    // Exercises the OrderRejected no-op arm in apply_event. We can't easily
    // generate a pure-rejection event stream over HTTP without first
    // synthesizing rejection-causing requests. Easier: write events
    // directly into the WAL, then boot.
    use exg_wal::{WalConfig, WalWriter};

    let tmp = TempDir::new().unwrap();
    let wal_dir = tmp.path().join("wal");
    std::fs::create_dir(&wal_dir).unwrap();

    {
        let mut w = WalWriter::open(WalConfig {
            dir: wal_dir.clone(),
            segment_size: 64 * 1024 * 1024,
            flush_interval_us: 1000,
            flush_every_n: 1,
        })
        .unwrap();
        let evt = Event::OrderRejected {
            order_id: exg_common::OrderId::new(7777),
            user_id: exg_common::UserId::new(42),
            reason: exg_protocol::RejectReason::InsufficientMargin,
            timestamp: exg_common::UnixMicros::now(),
        };
        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&evt).unwrap();
        w.append(&bytes).unwrap();
        w.flush().unwrap();
    }

    let mut cfg = base_cfg(tmp.path());
    cfg.wal.dir = wal_dir.to_string_lossy().into_owned();
    let (handle, _) = boot_server(cfg, pool).await;
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn replay_survived_order_matches_post_reboot_taker(pool: PgPool) {
    // CEO review A4 — strongest replay correctness test. Without this, the
    // other replay e2e tests only prove "boot did not panic", not "the order
    // that was on the book before kill is on the book after replay and can
    // be matched." This test routes the verification through the matching
    // engine itself: if the maker order survived replay correctly, the
    // taker's aggressive order will match it and the new WAL records will
    // contain an OrderFilled referencing the maker order_id from boot 1.
    let tmp = TempDir::new().unwrap();
    let wal_dir = std::path::PathBuf::from(tmp.path());
    let cfg = base_cfg(tmp.path());

    // ── Boot 1: place a resting maker bid, shut down cleanly. ────────────
    let maker_order_id: u64 = {
        let (handle, base) = boot_server(cfg.clone(), pool.clone()).await;
        let client = Client::new();
        let maker_token =
            register_and_login(&client, &base, "maker-survives@e.com", "hunter2hunter2").await;
        let resp: serde_json::Value = client
            .post(format!("{base}/api/v1/order"))
            .header("Authorization", format!("Bearer {maker_token}"))
            .json(&serde_json::json!({
                "symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT",
                "timeInForce":"GTC","quantity":"0.001","price":"61000"
            }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let oid: u64 = resp["orderId"].as_str().unwrap().parse().unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await; // WAL flush
        handle.shutdown().await.unwrap();
        oid
    };

    // Snapshot the WAL record count from boot 1 so we can identify NEW
    // records appended during boot 2.
    let mut reader = WalReader::open(&wal_dir).unwrap();
    let mut boot1_record_count: u64 = 0;
    reader
        .read_from(0, |_seq, _payload| {
            boot1_record_count += 1;
            true
        })
        .unwrap();

    // ── Boot 2: replay (Step 3.6 fires on the maker's OrderAccepted),
    // then a taker hits the maker at the maker's price. ──────────────────
    {
        let (handle2, base2) = reboot(cfg, pool).await;
        let client = Client::new();
        let taker_token =
            register_and_login(&client, &base2, "taker-hits-maker@e.com", "hunter2hunter2").await;
        let resp = client
            .post(format!("{base2}/api/v1/order"))
            .header("Authorization", format!("Bearer {taker_token}"))
            .json(&serde_json::json!({
                "symbol":"BTCUSDT","side":"SELL","orderType":"LIMIT",
                "timeInForce":"GTC","quantity":"0.001","price":"61000"
            }))
            .send()
            .await
            .unwrap();
        assert!(resp.status().is_success(), "taker place failed: {resp:?}");
        tokio::time::sleep(Duration::from_millis(200)).await;
        handle2.shutdown().await.unwrap();
    }

    // ── Verify: boot 2's NEW WAL records contain an OrderFilled
    // referencing the maker_order_id from boot 1. ────────────────────────
    let mut reader = WalReader::open(&wal_dir).unwrap();
    let mut filled_for_maker = false;
    let mut seen: u64 = 0;
    reader
        .read_from(0, |_seq, payload| {
            seen += 1;
            if seen <= boot1_record_count {
                return true; // skip boot 1 records
            }
            let e: Event = rkyv::from_bytes::<Event, rkyv::rancor::Error>(payload).unwrap();
            if let Event::OrderFilled { order_id, .. } = e {
                if order_id.value() == maker_order_id {
                    filled_for_maker = true;
                }
            }
            true
        })
        .unwrap();
    assert!(
        filled_for_maker,
        "post-reboot taker did not match the replayed maker order — replay regression"
    );
}
```

Now stage1b_e2e.rs has **10 replay tests**.

- [ ] **Step 4: Write polish e2e tests (7 tests)**

Append:

```rust
#[sqlx::test(migrations = "../../migrations")]
async fn cancel_order_rate_limit(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let mut cfg = base_cfg(tmp.path());
    cfg.risk.max_orders_per_second = 2; // small bucket
    let (handle, base) = boot_server(cfg, pool).await;
    let client = Client::new();
    let token = register_and_login(&client, &base, "co-rl@e.com", "hunter2hunter2").await;

    let mut saw_429 = false;
    for _ in 0..20 {
        let resp = client
            .post(format!("{base}/api/v1/order/cancel"))
            .header("Authorization", format!("Bearer {token}"))
            .json(&serde_json::json!({"orderId": 1u64, "symbol":"BTCUSDT"}))
            .send()
            .await
            .unwrap();
        if resp.status().as_u16() == 429 {
            let body: serde_json::Value = resp.json().await.unwrap();
            assert_eq!(body["code"], -1003);
            saw_429 = true;
            break;
        }
    }
    assert!(saw_429, "expected 429 from per-user cancel limit");
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn amend_order_rate_limit(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let mut cfg = base_cfg(tmp.path());
    cfg.risk.max_orders_per_second = 2;
    let (handle, base) = boot_server(cfg, pool).await;
    let client = Client::new();
    let token = register_and_login(&client, &base, "am-rl@e.com", "hunter2hunter2").await;

    let mut saw_429 = false;
    for _ in 0..20 {
        let resp = client
            .post(format!("{base}/api/v1/order/amend"))
            .header("Authorization", format!("Bearer {token}"))
            .json(&serde_json::json!({
                "orderId": 1u64, "symbol":"BTCUSDT", "newPrice":"60000"
            }))
            .send()
            .await
            .unwrap();
        if resp.status().as_u16() == 429 {
            let body: serde_json::Value = resp.json().await.unwrap();
            assert_eq!(body["code"], -1003);
            saw_429 = true;
            break;
        }
    }
    assert!(saw_429, "expected 429 from per-user amend limit");
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn mixed_place_cancel_share_user_bucket(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let mut cfg = base_cfg(tmp.path());
    cfg.risk.max_orders_per_second = 3;
    let (handle, base) = boot_server(cfg, pool).await;
    let client = Client::new();
    let token = register_and_login(&client, &base, "mix@e.com", "hunter2hunter2").await;

    // Drain the bucket with place orders.
    for _ in 0..3 {
        client
            .post(format!("{base}/api/v1/order"))
            .header("Authorization", format!("Bearer {token}"))
            .json(&serde_json::json!({
                "symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT",
                "timeInForce":"GTC","quantity":"0.001","price":"50000"
            }))
            .send()
            .await
            .unwrap();
    }
    // Now cancel — should hit 429 because the bucket is empty.
    let resp = client
        .post(format!("{base}/api/v1/order/cancel"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&serde_json::json!({"orderId": 1u64, "symbol":"BTCUSDT"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 429);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["code"], -1003);
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn login_charges_ip_bucket_even_when_email_exhausted(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let mut cfg = base_cfg(tmp.path());
    cfg.risk.max_orders_per_second = 1; // each bucket holds 1 token
    let (handle, base) = boot_server(cfg, pool).await;
    let client = Client::new();

    // Register two distinct users.
    let _ = register_and_login(&client, &base, "a@e.com", "hunter2hunter2").await;
    let _ = register_and_login(&client, &base, "b@e.com", "hunter2hunter2").await;

    // 1st login on email A from this IP — consumes both email_A bucket and IP bucket.
    let r1 = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&serde_json::json!({"email":"a@e.com","password":"hunter2hunter2"}))
        .send()
        .await
        .unwrap();
    assert!(r1.status().is_success() || r1.status().as_u16() == 429);

    // 2nd login on email B from the same IP — IP bucket already empty,
    // so must 429 even though email_B bucket has its full token.
    let r2 = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&serde_json::json!({"email":"b@e.com","password":"hunter2hunter2"}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        r2.status().as_u16(),
        429,
        "expected 429 — IP bucket exhausted by first login"
    );
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn tampered_jwt_signature_returns_401(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let (handle, base) = boot_server(cfg, pool).await;
    let client = Client::new();
    let token = register_and_login(&client, &base, "tamper@e.com", "hunter2hunter2").await;

    // Flip a few characters in the signature segment (after the last `.`).
    let mut tampered = token.clone();
    let pos = tampered.rfind('.').unwrap() + 1;
    // Replace 4 chars after the dot. Pick chars that are valid base64url.
    let bad: String = tampered.chars().enumerate().map(|(i, c)| {
        if i >= pos && i < pos + 4 { 'A' } else { c }
    }).collect();
    tampered = bad;

    let resp = client
        .get(format!("{base}/api/v1/me"))
        .header("Authorization", format!("Bearer {tampered}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 401);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["code"], -1002);
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn token_reuse_within_expiry_succeeds(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let (handle, base) = boot_server(cfg, pool).await;
    let client = Client::new();
    let token = register_and_login(&client, &base, "reuse@e.com", "hunter2hunter2").await;

    // Same token, two consecutive /me calls — both must succeed.
    for _ in 0..2 {
        let resp = client
            .get(format!("{base}/api/v1/me"))
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200);
    }
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn kyc_level_reflected_in_me(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let (handle, base) = boot_server(cfg, pool.clone()).await;
    let client = Client::new();
    let token = register_and_login(&client, &base, "kyc@e.com", "hunter2hunter2").await;

    // Read the user_id via /me first, then update its kyc_level directly in PG.
    let me1: serde_json::Value = client
        .get(format!("{base}/api/v1/me"))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let user_id: i64 = me1["userId"].as_str().unwrap().parse().unwrap();

    sqlx::query("UPDATE users SET kyc_level = $1 WHERE user_id = $2")
        .bind(2_i16)
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();

    let me2: serde_json::Value = client
        .get(format!("{base}/api/v1/me"))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(me2["kycLevel"], 2);
    handle.shutdown().await.unwrap();
}
```

- [ ] **Step 5: Add any missing dev-dependencies**

```bash
grep -E "tempfile|exg-matching-engine|exg-wal|exg-protocol|exg-common|rkyv" crates/exg-server/Cargo.toml
```

If any of these is not yet a dev-dep, add it. Likely already present from Stage 0/1a. The exact dep names used in the test imports:

```toml
[dev-dependencies]
tempfile = { workspace = true }
exg-matching-engine = { workspace = true }
exg-wal = { workspace = true }
exg-protocol = { workspace = true }
exg-common = { workspace = true }
rkyv = { workspace = true }
```

- [ ] **Step 6: Run the suite**

```bash
DATABASE_URL=postgres://exg:exg_dev_password@localhost:5433/exg \
  cargo test -p exg-server --test stage1b_e2e 2>&1 | tail -25
```

Expected: **17 passed** (10 replay + 7 polish).

Common failure modes to debug if not green:
- `boot_replays_three_orders_inspectable_via_wal` count mismatch: the matching engine emits OrderAccepted both for the conditional-order path and the regular path; verify only one OrderAccepted per submitted order.
- `mixed_place_cancel_share_user_bucket`: time elapsed between requests may refill 1 token; if flaky, fire requests faster by removing `await` between them with `tokio::join!` or `JoinSet`.
- `login_charges_ip_bucket_even_when_email_exhausted`: if `r1` succeeds and `r2` also succeeds at status 200, refill timing has crossed the boundary. Reduce `max_orders_per_second` to e.g. `0` (would never allow any, breaking r1) — instead, structure r1 to deliberately fail (wrong password) so it still consumes both buckets.

If the IP-bucket test is flaky, replace its logic with:

```rust
    // 1st login with wrong password — still consumes both email_A + IP buckets.
    let _ = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&serde_json::json!({"email":"a@e.com","password":"WRONGPASS"}))
        .send()
        .await
        .unwrap();
    // 2nd login on a different email — IP bucket already empty.
    let r2 = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&serde_json::json!({"email":"b@e.com","password":"hunter2hunter2"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r2.status().as_u16(), 429);
```

- [ ] **Step 7: Commit**

```bash
git add crates/exg-server/tests/stage1b_e2e.rs crates/exg-server/Cargo.toml
git commit -m "$(cat <<'EOF'
test(server): add Stage 1b e2e suite (17 cases)

Replay correctness (10):
- boot on empty WAL succeeds
- boot replays single order
- boot replays place+cancel
- boot replays place+amend
- boot replays matched trade (2 sides)
- second boot allocates fresh order_id (Snowflake doesn't collide)
- boot replays N orders inspectable via wal-dump count
- replay engine state matches live engine for same command stream
- boot with OrderRejected-only WAL succeeds (no-op arm)
- replay-survived order matches post-reboot taker (CEO review A4)

Polish (7):
- cancel_order per-user rate limit
- amend_order per-user rate limit
- place + cancel share the user:N bucket
- login charges IP bucket even when email bucket exhausted
- tampered JWT signature returns 401
- token reuse within expiry succeeds
- kyc_level reflected in /me after direct DB UPDATE

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: Demo script + WAL-replay-failed runbook + final acceptance

**Files:**
- Create: `scripts/demo-stage1b.sh`
- Create: `docs/runbooks/wal-replay-failed.md`

### Step 0: Write `docs/runbooks/wal-replay-failed.md` (CEO review A6)

Operators need a one-page guide for the boot panics introduced in Step 3.6/3.8. Create `docs/runbooks/wal-replay-failed.md`:

```markdown
# Runbook: `WAL replay failed` at boot

A boot panic with message starting `WAL replay failed:` or `invariant 21
violated:` means Stage 1b's replay step (server lib.rs Step 3.6 / 3.8)
rejected the on-disk WAL. The server refuses to start to prevent silent
state divergence.

## Identify the failure mode

Grep the boot stderr for the exact panic line. Five variants:

| Panic message prefix                          | Root cause                                     |
| --------------------------------------------- | ---------------------------------------------- |
| `WAL writer open failed: corrupt at sequence` | CRC mismatch in mid-stream segment (writer-open level — see spec §7.1). |
| `WAL replay failed: sequence gap at expected` | A WAL record is missing between two existing records. |
| `WAL replay failed: rkyv decode at sequence`  | An event's bytes do not match the current rkyv layout. Usually a schema change without WAL clear. |
| `WAL replay failed at sequence`               | `apply_event` returned an `ApplyError` (UnknownOrder / DuplicateOrder / OverFill / UnexpectedVariant). |
| `invariant 21 violated`                       | Reader's replayed event count ≠ writer's recorded next sequence. WAL state internally inconsistent. |

## Recovery actions

**Dev / CI environment (Stage 1b ships into this).** The contract is:
no production data exists. Reset:

```bash
# Stop the failed server (it's already exited).
rm -rf data/wal

# (Optional) preserve the broken WAL for forensics:
mv data/wal data/wal-broken-$(date +%s)
mkdir data/wal

# Restart.
EXG_CONFIG=config/default.toml RUST_LOG=info ./target/release/exg-server
```

The server boots with an empty WAL, replays zero events, and proceeds
normally. All previously open orders are lost.

**Production environment (future, not Stage 1b).** Do not run the dev
recovery. Page on-call SRE. Steps will live in a separate Stage 5+
runbook covering snapshot fallback, partial-truncate repair, and
multi-replica reconciliation. The Stage 1b runbook is intentionally
narrow.

## Most common cause in dev: schema drift

After pulling a Stage 1b commit that bumped an event schema (the
Stage 1a → 1b cutover is the classic case), any WAL written by the
older binary is unreadable. The fix is always `rm -rf data/wal`.
See spec §9.5 "Migration from Stage 1a."

## What NOT to do

- Don't delete only "some" segments — the writer's `recover_state` will
  surface a CRC or sequence-gap panic.
- Don't manually edit a segment file — the CRC trailer makes byte edits
  detectable, but a coincidental CRC-valid edit (vanishingly rare) would
  produce a silently wrong replay.
- Don't disable invariant 21 to "see what happens" — it exists because
  reader/writer disagreement is a real-world symptom of a bug somewhere.
  File an issue instead.
```

### Step 1: Write the demo script

Create `scripts/demo-stage1b.sh`:

```bash
#!/usr/bin/env bash
# Stage 1b cold-boot demo: place → kill → reboot replays → wal-dump.
set -euo pipefail

WAL_DIR=$(mktemp -d /tmp/exg-stage1b.XXXXXX)
PORT=8080
SERVER_PID=""
TMP_CFG=$(mktemp /tmp/exg-stage1b-cfg.XXXXXX.toml)

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
        if curl -sf "http://127.0.0.1:${PORT}/api/v1/health" >/dev/null; then
            return 0
        fi
        sleep 1
    done
    echo "server did not become ready" >&2
    return 1
}

stop_server() {
    if [[ -n "${SERVER_PID}" ]]; then
        kill -INT "${SERVER_PID}"
        wait "${SERVER_PID}" 2>/dev/null || true
        SERVER_PID=""
    fi
}

echo "── stage 1b demo ──"
docker compose up -d postgres
sleep 2

echo "─ migrate ─"
scripts/migrate.sh reset

echo "─ build ─"
cargo build --release -p exg-server -p exg-wal-dump >/dev/null

echo "─ prepare config ─"
cp config/default.toml "$TMP_CFG"
python3 - <<PY
import re
with open('$TMP_CFG') as f: c = f.read()
c = re.sub(r'dir = "\\./data/wal"', f'dir = "$WAL_DIR"', c)
c = re.sub(r'jwt_secret = "CHANGE-ME-DEV-ONLY-MUST-BE-AT-LEAST-32-BYTES-OK"', 'jwt_secret = "demo-stage1b-secret-padding-32-bytes"', c)
with open('$TMP_CFG', 'w') as f: f.write(c)
PY

echo
echo "─ boot 1: register + login + place ─"
start_server

curl -s -X POST "http://127.0.0.1:${PORT}/api/v1/auth/register" \
    -H 'Content-Type: application/json' \
    -d '{"email":"demo@example.com","password":"hunter2hunter2"}' >/dev/null

LOGIN_RESP=$(curl -s -X POST "http://127.0.0.1:${PORT}/api/v1/auth/login" \
    -H 'Content-Type: application/json' \
    -d '{"email":"demo@example.com","password":"hunter2hunter2"}')
TOKEN=$(echo "${LOGIN_RESP}" | python3 -c 'import json,sys; print(json.load(sys.stdin)["accessToken"])')

curl -s -X POST "http://127.0.0.1:${PORT}/api/v1/order" \
    -H "Authorization: Bearer $TOKEN" \
    -H 'Content-Type: application/json' \
    -d '{"symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT","timeInForce":"GTC","quantity":"0.001","price":"59000","clientOrderId":"42"}'
echo

echo
echo "─ shutdown 1 ─"
stop_server

echo
echo "─ WAL contents after boot 1 ─"
./target/release/exg-wal-dump --wal-dir "${WAL_DIR}" | head -20
echo

echo
echo "─ boot 2: server replays from WAL ─"
start_server

echo "─ health check ─"
curl -sf "http://127.0.0.1:${PORT}/api/v1/health"
echo

echo
echo "─ shutdown 2 ─"
stop_server

echo
echo "─ WAL contents after boot 2 (no new events expected) ─"
./target/release/exg-wal-dump --wal-dir "${WAL_DIR}" | head -20
echo

echo "─ demo complete ─"
```

- [ ] **Step 2: Make the script executable**

```bash
chmod +x scripts/demo-stage1b.sh
```

- [ ] **Step 3: Run the full acceptance sequence**

PG must be up:

```bash
docker compose up -d postgres
```

Then:

```bash
DATABASE_URL=postgres://exg:exg_dev_password@localhost:5433/exg cargo check --workspace
DATABASE_URL=postgres://exg:exg_dev_password@localhost:5433/exg cargo clippy --workspace -- -D warnings
cargo fmt --check
DATABASE_URL=postgres://exg:exg_dev_password@localhost:5433/exg cargo test --workspace
DATABASE_URL=postgres://exg:exg_dev_password@localhost:5433/exg cargo test -p exg-server --test stage1b_e2e
DATABASE_URL=postgres://exg:exg_dev_password@localhost:5433/exg cargo test -p exg-server --test stage1a_e2e
DATABASE_URL=postgres://exg:exg_dev_password@localhost:5433/exg cargo test -p exg-server --test stage0_e2e
DATABASE_URL=postgres://exg:exg_dev_password@localhost:5433/exg cargo test -p exg-server --test boot_panics
cargo test -p exg-matching-engine --lib
scripts/demo-stage1b.sh
```

Expected pass counts:
- `cargo test --workspace`: all green (~415 tests)
- `stage1b_e2e`: 17/17
- `stage1a_e2e`: 12/12
- `stage0_e2e`: 7/7
- `boot_panics`: 8/8
- `exg-matching-engine --lib`: 12 new + existing
- `demo-stage1b.sh`: clean exit, second boot health-checks green

If `cargo fmt --check` flags issues, run `cargo fmt`, stage the modified `.rs` files, commit as a separate `style: cargo fmt` commit.

- [ ] **Step 4: Commit the demo script + runbook**

```bash
git add scripts/demo-stage1b.sh docs/runbooks/wal-replay-failed.md
git commit -m "$(cat <<'EOF'
feat(scripts): add stage 1b cold-boot demo + replay-failed runbook

Demo boots the server, places one order via the JWT flow, kills the
server, boots a second time (replays the WAL), and dumps WAL contents
before and after to make the replay path observable.

Runbook docs/runbooks/wal-replay-failed.md catalogs the five replay
boot panics and the dev recovery path (rm -rf data/wal). Per CEO
review A6 — operators need to see the recovery action, not just the
panic message.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 5: If `cargo fmt` needed changes during Step 3, commit them**

```bash
cargo fmt
# review what changed
git status
# stage only modified files
git add -u   # safe — only modified files, not untracked
git commit -m "$(cat <<'EOF'
style: cargo fmt across stage 1b new files

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Spec ↔ Plan Coverage Matrix

| Spec section                                | Task(s)                  |
| ------------------------------------------- | ------------------------ |
| §2 Scope item 1 (WAL replay)                | Task 2, Task 3           |
| §2 Scope item 2 (invariant 3 removed)       | Task 3, Task 4           |
| §2 Scope item 3 (OrderAccepted schema bump) | Task 1                   |
| §2 Scope item 4 (apply_event)               | Task 2                   |
| §2 Scope item 5 (cancel/amend bucket)       | Task 5                   |
| §2 Scope item 6 (login \|\| fix)            | Task 5                   |
| §2 Scope item 7 (JWT/kyc e2e)               | Task 6                   |
| §4.1 ApplyError + dispatch table            | Task 2                   |
| §4.2 OrderAccepted schema bump              | Task 1                   |
| §4.3 Boot replay loop                       | Task 3                   |
| §4.4 Cancel/amend per-user bucket           | Task 5                   |
| §4.5 Login `\|\|` fix                       | Task 5                   |
| §4.6 Invariant 3 removal                    | Task 3                   |
| §4.7 JWT/kyc e2e                            | Task 6                   |
| §6 Data flow                                | Task 3 (lifecycle)       |
| §7.1 New boot panics                        | Task 3 (impl), Task 4 (tests) |
| §8.1 Unit tests (12)                        | Task 2                   |
| §8.2 Integration tests (17)                 | Task 6                   |
| §9.5 Migration from Stage 1a                | Plan header (cutover)    |
| §9.6 Rollback to Stage 1a                   | Plan header (rollback)   |
| §8.3 Existing test deltas                   | Task 4 (boot_panics)     |
| §9 Invariants 21–23                         | Task 3                   |
| §10 Acceptance                              | Task 7                   |

All spec sections covered.

---

## GSTACK REVIEW REPORT

| Review | Trigger | Why | Runs | Status | Findings |
|--------|---------|-----|------|--------|----------|
| CEO Review | `/plan-ceo-review` | Scope & strategy | 1 | CLEAR (PLAN) | mode: HOLD_SCOPE, 0 critical gaps, 8 findings all accepted (A1–A8) |
| Codex Review | `/codex review` | Independent 2nd opinion | 0 | — | — |
| Eng Review | `/plan-eng-review` | Architecture & tests (required) | 0 | — | — |
| Design Review | `/plan-design-review` | UI/UX gaps | 0 | SKIPPED | no UI scope |
| DX Review | `/plan-devex-review` | Developer experience gaps | 0 | — | — |

**UNRESOLVED:** 0 across all reviews.

**VERDICT:** CEO CLEARED — proceed to `/plan-eng-review` (architecture / tests / edge cases under HOLD SCOPE rigor). Eng review is the required shipping gate.
