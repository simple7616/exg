# Stage 3 — Position Tracking + Ledger Wiring + Funding Settlement

## 1. Overview

Stage 0–2 built order matching, persistence/auth, WAL replay, and dynamic
mark-price + funding-**rate** computation. But `exg-clearing`
(`PositionManager`, `RiskMonitor`, `ClearingService`) and `exg-ledger`
(double-entry accounts, `transfer`, `settle_funding_checked`,
`close_position_settled`, `verify_all_invariants`) exist as standalone
crates **driven by nothing on the live event path**. No live position
state is tracked, `OrderFilled`/`TradeExecuted` update no downstream
state, the Stage 2 funding rate is computed but never applied to user
funds, and `LiquidationOrder` is emitted by nothing.

Stage 3 wires the post-trade pipeline: trades drive live position state,
and a funding tick **actually moves user funds** through the double-entry
ledger. This closes Stage 2's primary forward pointer ("funding
settlement: per-interval debit/credit; needs position tracking +
ledger"). Liquidation and margin are deliberately deferred to Stage 4.

## 2. Scope

In scope:

1. **`PostTradeProcessor`** — a new component owning `PositionManager` +
   `Ledger`, run in the matching OS thread immediately after
   `MatchingEngine::process_command`, consuming engine-emitted events,
   mutating clearing/ledger state, emitting post-trade events.
2. **Live position tracking** — `OrderFilled`/`TradeExecuted` →
   `PositionManager.open_or_increase` / `reduce_or_close`. Position
   quantity + weighted-average entry price only.
3. **Realized PnL on reduce/close** — settled through the ledger against
   a reserved system account; emitted as a `RealizedPnl` fact event.
4. **Funding settlement** — on a funding tick, every open position is
   settled `notional × funding_rate` (long pays, short receives) via the
   ledger against the system account; emitted as `FundingSettled` fact
   events. Atomic with the `FundingRateUpdate` that drove it.
5. **Minimal admin credit** — new `Command::AdminCredit` + admin endpoint
   `POST /api/v1/admin/credit` (reuses the Stage 2 admin server +
   `X-Admin-Secret`) so wallets have balances to settle against; emitted
   as an `AdminCredited` fact event.
6. **Replay extension** — `PostTradeProcessor::apply_event` re-projects
   positions from fills and re-applies recorded money facts; the boot
   replay loop drives both `engine.apply_event` and
   `post_trade.apply_event` from the one WAL.

Out of scope (forward pointers, Stage 4+):

- Initial-margin reservation/enforcement; order-acceptance margin checks.
- Liquidation (`RiskMonitor` drive, `LiquidationOrder` emission/replay),
  bankruptcy, ADL.
- Insurance-fund semantics; precise per-trade PnL counterparty.
- Periodic wall-clock funding timer (Stage 2 deferred this; Stage 3
  remains admin-tick driven for deterministic tests).
- Real deposit/withdrawal flow; multi-symbol.

## 3. Architecture

```
 matching OS thread (single, lock-free — Stage 0 invariant):

   consumer.try_pop(cmd)
        │
        ▼
   MatchingEngine.process_command(cmd) ──► [engine events: OrderAccepted,
        │                                    OrderFilled, TradeExecuted,
        │                                    MarkPriceUpdate,
        │                                    FundingRateUpdate, ...]
        ▼
   PostTradeProcessor.consume(&engine_events) ──► [post-trade events:
        │     ├─ OrderFilled/TradeExecuted → PositionManager open/reduce   AdminCredited,
        │     │     └─ on reduce/close: realized PnL → ledger vs system     RealizedPnl,
        │     │        → emit RealizedPnl                                   FundingSettled]
        │     ├─ FundingRateUpdate → for each open position:
        │     │     fee = notional × rate (long pays / short recv)
        │     │     → ledger.settle_funding_checked vs system
        │     │     → emit FundingSettled         (atomic w/ this tick)
        │     └─ (AdminCredit command handled at dispatch → emit AdminCredited)
        ▼
   WAL.append(engine_events ++ post_trade_events, in sequence order)

 boot replay (extends Stage 1b loop, lib.rs:307): for each WAL record
   engine.apply_event(e)        // engine-state events (unchanged)
   post_trade.apply_event(e)    // position projection + recorded money facts
   (each ignores events not in its domain; ordering preserved by the single WAL)
```

**Bounded contexts (CLAUDE.md §10.1).** `MatchingEngine` = matching
domain; `PostTradeProcessor` (positions + ledger) = clearing domain. They
communicate only through the event stream — no shared mutable state, no
cross-calls. Single-thread / single-WAL / single-replay invariants
(Stage 0 / Stage 1b) preserved: `PostTradeProcessor` runs in the same
matching thread, its events go through the same WAL, and replay is one
pass over that WAL.

**Why a same-thread post-trade processor (not an async consumer).** An
async event-bus consumer would create a second source of truth with an
eventual-consistency window — directly contradicting Stage 1b's "WAL is
the single source of truth, replay reconstructs all state". A funding /
PnL settlement subsystem must be exactly consistent with the matching
state that produced it; same-thread synchronous processing guarantees
that without locks.

## 4. Detailed Design

### 4.1 `Command::AdminCredit` (exg-protocol/src/command.rs)

Appended **at the end** of the `Command` enum (after `ComputeFunding`) to
keep existing rkyv discriminants stable (Stage 1b lesson — see §8.5):

```rust
AdminCredit {
    user_id: UserId,
    amount: Decimal128,
    idempotency_key: String,
    timestamp: UnixMicros,
},
```

`process_command` dispatches it to the post-trade path (the matching
engine ignores it — it is a clearing-domain command). It produces no
engine events; the post-trade handler performs the ledger credit and
emits `AdminCredited`.

### 4.2 New `Event` variants (exg-protocol/src/event.rs)

Appended **at the end** of the `Event` enum (after `LiquidationOrder`),
discriminants of existing variants unchanged so Stage 2-era WALs still
decode (§8.5):

```rust
AdminCredited  { user_id: UserId, amount: Decimal128, timestamp: UnixMicros },
RealizedPnl    { user_id: UserId, symbol: SymbolId, amount: Decimal128,
                 timestamp: UnixMicros },         // signed: +profit / -loss
FundingSettled { user_id: UserId, symbol: SymbolId, funding_period_id: u64,
                 amount: Decimal128, timestamp: UnixMicros }, // signed
```

These are **facts** (Q6): they record the exact money movement that
already happened on the live path; replay re-applies the recorded amount,
never recomputes it (Stage 2 P1 discipline).

### 4.3 System account (Q4 / Q7) — resolved against source

`exg-ledger` **already** defines a private `SYSTEM_USER_ID: UserId =
UserId(0)` (`operations.rs:10`) and performs **all** system-side
double-entry **internally**: `deposit`, `withdraw`, and `settle_funding`
each append journal entries against `SYSTEM_USER_ID`. `PostTradeProcessor`
therefore **never references a system account directly** and **no new
`UserId::SYSTEM` is added** — it calls the high-level ledger ops and the
ledger does the counterparty bookkeeping. (Snowflake `next_id()` is
`ts_ms << 22 | …` so it never yields 0; `UserId(0)` is a safe sentinel —
no real user collides.)

The funding counterparty pool is `WalletType::Funding`.
`verify_all_invariants()` enforces non-negativity only for
`NON_NEGATIVE_SYSTEM_WALLETS`, which **explicitly excludes `Funding`**
("Funding pool can legitimately be negative (receives before pays)").
So an imbalanced book (longs notional ≠ shorts notional) nets into the
Funding pool **without** violating any invariant — the Q7 "system absorbs
the imbalance" requirement is satisfied by the existing ledger model with
no extra code. Precise insurance-fund accounting (which *does* constrain
`WalletType::InsuranceFund`) remains Stage 4.

### 4.4 `PostTradeProcessor` (new — `crates/exg-clearing/src/post_trade.rs`)

```rust
pub struct PostTradeProcessor {
    positions: PositionManager,
    ledger: Ledger,
    mark_price: Decimal128,        // tracked from MarkPriceUpdate (for notional)
    funding_period_id: u64,        // monotonic; ++ per FundingRateUpdate consumed
}
```

- `consume(&mut self, events: &[Event], ts: UnixMicros) -> Vec<Event>` —
  live path. Iterates engine events in order:
  - `OrderFilled { user_id, symbol, side, fill_qty, fill_price, is_maker, .. }`
    — positions are projected from `OrderFilled` **only** (the engine emits
    one `OrderFilled` per side of a trade — maker and taker — each scoped
    to one `user_id`). `TradeExecuted` is **NOT** used for position
    projection (it describes the same trade a second time → would
    double-count); it is consumed only as an audit fact. Map order `Side`
    → `PositionSide` (`Buy → Long`, `Sell → Short` for opens; a fill whose
    side is opposite the resting position reduces/closes it, flipping if
    `fill_qty` exceeds current size). Call
    `positions.open_or_increase(..)` or `positions.reduce_or_close(..)`.
    `reduce_or_close(user, symbol, qty, exit_price) -> ExgResult<(pnl,
    Option<&Position>)>` returns a **signed** `realized_pnl`; if it is
    non-zero, settle vs the system pool with idempotency key
    `pnl_{seq}_{user}_{symbol}`: profit → `ledger.deposit(user,
    pnl.abs(), key, ts)` (SYSTEM→user, `WalletType::Funding`); loss →
    `ledger.withdraw(user, pnl.abs(), key, ts)` (user→SYSTEM). Push
    `Event::RealizedPnl { .. , amount: realized_pnl /* signed */, .. }`.
    Note: `withdraw` errors `InsufficientBalance` if the user's Funding
    available cannot cover the loss → live fail-fast (Stage 3 has no
    margin/liquidation to prevent it; documented limitation, §5.4 / §9 →
    Stage 4). Tests/demo admin-credit users before any losing close.
  - `MarkPriceUpdate { mark_price, .. }` → `self.mark_price = mark_price`
    (needed for funding notional). No money move.
  - `FundingRateUpdate { funding_rate, .. }` → `funding_period_id += 1`;
    for each open position: `notional = position.size * self.mark_price`;
    `payment = notional * funding_rate` (Long pays when rate > 0 → debit
    user; Short receives → credit user; signs handled by
    `settle_funding_checked`'s signed `payment`). Call
    `ledger.settle_funding_checked(user, symbol, funding_period_id,
    payment, ts)` (its idempotency key is
    `funding_{period}_{user}_{symbol}` — deterministic, replay-safe).
    Push `Event::FundingSettled { .., funding_period_id, amount: payment,
    .. }` per position. The whole loop is one atomic batch with this tick
    (Invariant 33). After the batch, call
    `ledger.verify_all_invariants()` (Invariant 32) — failure ⇒ fail-fast
    (matching thread aborts; WAL is truth).
- `apply_event(&mut self, e: &Event)` — replay path:
  - `OrderFilled`/`TradeExecuted` → **re-project position qty/avg-entry
    only** (same `open_or_increase`/`reduce_or_close` arithmetic, which
    is pure additive). Do **NOT** emit or recompute money here.
  - `RealizedPnl { user_id, amount, .. }` → apply the **recorded** signed
    amount via the same `deposit`(>0)/`withdraw`(<0) rule + same
    `pnl_{seq}_{user}_{symbol}` idempotency key. No PnL recomputation
    (the sign→direction mapping is a fixed deterministic rule applied to
    the recorded number — not a recompute).
  - `FundingSettled { user_id, funding_period_id, amount, .. }` → apply
    the **recorded** signed `amount` via
    `ledger.settle_funding_checked(.., funding_period_id, amount, ..)`;
    its deterministic idempotency key makes a duplicate apply a no-op.
    Also advance `self.funding_period_id` to `max(self, period)`.
  - `AdminCredited { user_id, amount, .. }` → `ledger.deposit(user,
    amount, "admincredit_{user}_{seq}", ts)` (the ledger journals
    SYSTEM→user `WalletType::Funding`; idempotent on the key).
  - `MarkPriceUpdate` → `self.mark_price = mark_price` (so a post-replay
    funding tick has the right notional).
  - `FundingRateUpdate` on replay → **NO-OP for settlement** (only
    `funding_period_id += 1` to stay aligned). Settlement state is
    reconstructed solely from the recorded `FundingSettled` facts — this
    is the explicit Stage 2-P1 guard: replay never re-derives money.
  - All other events → ignored (engine domain).

`AdminCredit` command (not an engine event) is handled where commands are
dispatched: `PostTradeProcessor` exposes
`handle_admin_credit(user, amount, idempotency_key, ts) -> Vec<Event>`
returning `[AdminCredited{..}]` after the ledger credit; on replay the
`AdminCredited` event arm re-applies it idempotently.

### 4.5 Ledger usage notes — resolved against source

Verified by reading `crates/exg-ledger/src/operations.rs`:

- **Funding sign convention (resolved — was a flagged unknown).**
  `settle_funding_checked(user, symbol, funding_period_id, payment, ts)
  -> ExgResult<bool>` builds the idempotency key
  `funding_{period}_{user}_{symbol}` internally (deterministic across
  live/replay — Invariant 31) and delegates to `settle_funding`, which
  **handles a signed `payment`**: `payment > 0` ⇒ **user pays** (debit
  user `WalletType::Futures` available→margin, credit `SYSTEM_USER_ID`
  Funding pool); `payment < 0` ⇒ **user receives** (credit user, debit
  the Funding pool); `payment == 0` ⇒ early `Ok`. So
  `payment = position_notional × funding_rate` is passed **signed,
  unchanged** — long with `rate > 0` yields `payment > 0` (pays), short
  yields `payment < 0` (receives). The brainstorming intent matches the
  ledger exactly; no manual debit/credit selection is needed. The
  returned `bool` (margin tapped) is ignored in Stage 3 (no liquidation
  → Stage 4).
- **Realized PnL** does **not** use `close_position_settled`
  (`margin_released > 0` required — margin-coupled, Stage 4) and **cannot**
  use `transfer` (`transfer` moves between two wallets of the *same*
  user, not user↔system). It uses `deposit` (profit; SYSTEM→user Funding)
  / `withdraw` (loss; user→SYSTEM Funding), the only margin-free
  user↔SYSTEM primitives. Both require a **positive** `amount` → pass
  `pnl.abs()` and choose the op by `pnl`'s sign.
- **Admin credit** uses `deposit(user, amount, idempotency_key, ts)` —
  its journal is exactly SYSTEM→user `WalletType::Funding`; `amount` must
  be positive (handler rejects `≤ 0` at 400 before the command).
- **Idempotency.** `deposit`, `withdraw`, `settle_funding` all call
  `check_idempotency(key)` and early-return `Ok` on a duplicate key
  (CLAUDE.md "duplicates silently accepted"). This is exactly what makes
  hybrid replay safe: re-applying a recorded `AdminCredited`/
  `RealizedPnl`/`FundingSettled` with its deterministic key is a no-op,
  so double-entry stays balanced (Invariant 31/36).
- **`verify_all_invariants()`** checks every user account invariant plus
  non-negativity of `NON_NEGATIVE_SYSTEM_WALLETS` only — `Funding` is
  **excluded** by design, so an imbalanced funding book is invariant-safe
  (§4.3).

### 4.6 Boot lifecycle (exg-server/src/lib.rs)

- **Dispatch rule (explicit, no per-variant routing in the boot loop).**
  `engine.apply_event` gains a single new arm
  `Event::AdminCredited { .. } | Event::RealizedPnl { .. } |
  Event::FundingSettled { .. } => Ok(())` — the matching engine ignores
  post-trade fact events (mirrors its existing `OrderRejected => Ok(())`
  / `TradeExecuted => Ok(())` no-op arms; it does **not** make them
  `UnexpectedVariant`). The Stage 1b replay loop (lib.rs:307) gains a
  `PostTradeProcessor` constructed before replay and calls **both**
  `engine.apply_event(e)?` **and** `post_trade.apply_event(e)?` for
  **every** decoded WAL record, uniformly (each side Ok-noops events
  outside its domain). A `post_trade.apply_event` error becomes a
  `ReplayError::Apply` → boot abort (same fail-fast as Stage 1b).
- After replay, before spawning the matching thread,
  `post_trade.ledger.verify_all_invariants()` must hold (Invariant 32) —
  failure ⇒ boot panic.
- The matching thread loop (lib.rs:399) gains the
  `engine.process_command → PostTradeProcessor.consume` stage before WAL
  append; `AdminCredit` commands route to
  `PostTradeProcessor.handle_admin_credit`. All engine + post-trade events
  are appended to the WAL in order.
- `PostTradeProcessor` is owned by the matching thread (moved in like the
  engine/consumer); no `Arc<Mutex<>>` (single-thread invariant).

### 4.7 Admin credit endpoint (exg-api-gateway/src/admin.rs)

`POST /api/v1/admin/credit` (Stage 2 admin server, `X-Admin-Secret`
constant-time gate, invariant 26 reused). Body
`AdminCreditRequest { user_id: u64, amount: String }` (camelCase,
stringified decimal — Stage 2 shape). Reject `amount <= 0` → 400 / -1100
(symmetric with Stage 2 markPrice guard). Emits the
`tracing::info!(target:"admin", ..)` audit line before enqueue (invariant
30 reuse). Builds `Command::AdminCredit { user_id, amount,
idempotency_key: format!("admincredit_{user_id}_{}", UnixMicros::now()),
timestamp }` and pushes to the shared ring buffer.

## 5. Data Flow & Error Handling

### 5.1 Live admin credit

```
POST /admin/credit
  ├─ X-Admin-Secret missing/wrong ──► 401 (-1002)        (Stage 2 inv 26)
  ├─ amount not Decimal128 / <= 0 ──► 400 (-1100)
  ├─ ring buffer full ──► 429 (-1015)
  └─ ok ──► 200 {status:"ACCEPTED"}  → Command::AdminCredit
            → PostTradeProcessor.handle_admin_credit → ledger credit
            → Event::AdminCredited (WAL'd)
```

### 5.2 Fill → position → realized PnL

`OrderFilled` (engine; `TradeExecuted` not used for projection) →
position open/increase (no money) or reduce/close → signed `realized_pnl`
from `PositionManager.reduce_or_close` → `ledger.deposit` (profit) /
`ledger.withdraw` (loss) vs SYSTEM Funding → `Event::RealizedPnl{amount}`
WAL'd.

### 5.3 Funding tick (atomic)

`Command::ComputeFunding` (Stage 2, admin funding-tick) → engine emits
`FundingRateUpdate{rate}` → `PostTradeProcessor` consumes it →
`funding_period_id++` → for each open position: notional × rate →
`settle_funding_checked` → `Event::FundingSettled{period,amount}` per
position. `verify_all_invariants()` after the batch. The
`FundingRateUpdate` and all its `FundingSettled` events are appended
contiguously in WAL order (one tick = rate + settlement; Invariant 33).

### 5.4 Error handling (fail-fast, no fallback — CLAUDE.md §8.2)

| Condition | Behavior |
|-----------|----------|
| admin credit amount ≤ 0 | 400 at handler; no Command produced |
| ledger op fails live (e.g. account-not-found) | matching thread aborts (fail-fast; WAL is truth) — no silent skip |
| realized **loss** exceeds user Funding available (`withdraw` → `InsufficientBalance`) | live fail-fast (no margin/liquidation in Stage 3 to prevent it); documented limitation → Stage 4. Tests/demo admin-credit users before any losing close |
| `verify_all_invariants()` fails live | fail-fast abort |
| replay `apply_event` ledger error | `ReplayError::Apply` → boot panic |
| replay end `verify_all_invariants()` fails | boot panic |
| duplicate idempotency key (replay re-apply) | ledger no-op `Ok` (by design — Invariant 31) |

### 5.5 Replay flow (boot)

Each WAL record → `engine.apply_event` (engine state, unchanged) **and**
`post_trade.apply_event`. Positions re-projected from `OrderFilled`/
`TradeExecuted` (pure additive). `AdminCredited`/`RealizedPnl`/
`FundingSettled` re-apply **recorded** amounts via idempotent ledger ops.
`FundingRateUpdate` is a settlement no-op on replay (period counter only).
`LiquidationOrder` still `UnexpectedVariant` for the engine (Stage 4).

## 6. Invariants

Numbering continues from Stage 2's #30. Retained: Stage 0 #1–#10, Stage
1a #11–#20, Stage 1b #21–#23, Stage 2 #24–#30 (all unaffected; verified
by regression baselines).

- **#31** Every Stage 3 ledger mutation carries an idempotency key whose
  derivation is identical on the live and replay paths; replay
  re-application is a no-op and double-entry stays balanced.
- **#32** `Ledger::verify_all_invariants()` holds after every funding
  settlement batch on the live path and at the end of boot replay.
- **#33** Funding settlement is atomic with the `FundingRateUpdate` tick
  that drove it — all `FundingSettled` events for a tick are produced and
  WAL-appended contiguously with that `FundingRateUpdate`; a per-position
  ledger failure aborts the process (no partial batch persists).
- **#34** Position quantity / weighted-average entry price is a pure
  projection of `OrderFilled` **only** (`TradeExecuted` is never used for
  projection — anti-double-count); never separately evented; live and
  replayed position state are identical (round-trip equivalence test —
  the Stage 2 C10 observable-assertion discipline).
- **#35** The ledger-internal `SYSTEM_USER_ID` (`UserId(0)`) is the sole
  Stage 3 counterparty; every Stage 3 money op is a balanced ledger
  journal (Σ debits = Σ credits) — `verify_all_invariants` enforces
  per-account invariants + non-negativity of
  `NON_NEGATIVE_SYSTEM_WALLETS` (Funding pool excluded by design).
- **#36** On replay, `PostTradeProcessor` never recomputes a money amount
  — `FundingRateUpdate` is a settlement no-op; all money state comes from
  recorded `AdminCredited`/`RealizedPnl`/`FundingSettled` facts (explicit
  Stage 2-P1 regression guard).

## 7. Testing

### 7.1 Unit (`exg-clearing` post_trade + ledger)

1. `open_increases_position_no_money` — fill opens/increases position;
   no ledger movement; weighted-avg entry correct.
2. `reduce_emits_realized_pnl_long_profit` / `..._short_loss` — signed
   PnL via ledger user↔system; `RealizedPnl` event amount matches.
3. `funding_long_pays_short_receives` — rate>0: long debited, short
   credited, system nets the imbalance; `FundingSettled` signs correct.
4. `funding_zero_rate_noop` — rate 0 → no ledger move, no event.
5. `admin_credit_double_entry` — user.Futures ↑, system.Futures ↓;
   `verify_all_invariants` ok.
6. `verify_all_invariants_after_settlement_batch`.
7. `settle_funding_checked_idempotent` — same `funding_period_id`
   re-applied → no double charge.

### 7.2 Replay round-trip equivalence (mandatory — Stage 2 C10 discipline)

Drive a live `MatchingEngine` + `PostTradeProcessor`, collect **all**
WAL'd events, replay them into a fresh pair, assert **identical**:
positions (size + entry per user/symbol), every user wallet balance, the
system Funding-pool balance (`ledger.system_balance(WalletType::Funding)`),
and the full journal length/entries.

1. `replay_admin_credit_then_open_then_funding_tick` — credit → open both
   sides → funding tick → reboot → balances + positions identical; **no**
   double settlement (assert system + user balances exactly equal pre-
   reboot; the Stage 2-P1-class guard).
2. `replay_partial_close_realized_pnl` — open → partial close (realized
   PnL) → reboot → identical.
3. `replay_imbalanced_book_funding_net` — longs notional ≠ shorts
   notional → system absorbs net → reboot → identical.
4. `replay_funding_rate_update_is_settlement_noop` — assert replaying
   `FundingRateUpdate` alone moves no funds (only recorded
   `FundingSettled` does).

### 7.3 Integration (`exg-server/tests/stage3_e2e.rs`, `#[sqlx::test]`)

1. `admin_credit_then_funding_tick_moves_funds` — admin-credit two users
   → place crossing orders (open long+short) → admin funding-tick →
   assert wallets debited/credited + `FundingSettled` in WAL.
2. `funding_settlement_survives_reboot` — as above → reboot → balances +
   positions intact, no double settlement (observable WAL assertion,
   Stage 2 C10 style).
3. `admin_credit_missing_secret_401` / `wrong_secret_401` /
   `correct_secret_200`.
4. `admin_credit_negative_amount_400`.
5. `admin_credit_route_not_on_main_port` (Stage 2 port-isolation parity).

### 7.4 Boot panics (`exg-server/tests/boot_panics.rs`, net +1..2)

- `boot_panics_on_corrupt_funding_settled` (tampered amount / unknown
  user → replay `verify_all_invariants` / apply error → abort).

### 7.5 Regression baselines (must stay green, source unchanged)

stage0_e2e 7, stage1a_e2e 12, stage1b_e2e 16, stage2_e2e 11,
boot_panics (prior count), exg-matching-engine `--lib`,
exg-user-service 30. The matching thread now also runs
`PostTradeProcessor.consume`; baselines that place orders but never
admin-credit must still pass (a position with a zero-balance user can be
opened — no margin check, Q3; funding on it just drives the wallet/system
balance, still double-entry valid).

## 8. Acceptance

PR passes when:

1. `cargo check --workspace --all-targets` clean.
2. `cargo clippy --workspace -- -D warnings` clean.
3. `cargo fmt --check` clean.
4. `cargo test --workspace` all green.
5. New: stage3_e2e (5+), post_trade unit (7+), replay round-trip (4),
   boot_panics (+1..2).
6. Regression: stage0 7, stage1a 12, stage1b 16, stage2 11 — all green.
7. `scripts/demo-stage3.sh`: docker postgres → migrate reset → boot →
   admin-credit user A and B → A places long, B places short (they cross)
   → admin funding-tick → `wal-dump` shows `FundingRateUpdate` +
   `FundingSettled` × N + `RealizedPnl` (if any close) → ^C → reboot →
   server logs replay complete → `verify_all_invariants` holds → balances
   queryable and unchanged by replay.

## 8.5 Rollback to Stage 2

Stage 3 **appends** `Command::AdminCredit` and three `Event` variants at
the end of their enums; existing discriminants are unchanged, so a Stage
3 binary replays a Stage 2-era WAL fine. The reverse does **not** hold: a
Stage 2 binary's `apply_event` rejects the new variants
(`UnexpectedVariant`) → boot panic. Once Stage 3 has written any
`AdminCredited`/`RealizedPnl`/`FundingSettled` (or an `AdminCredit`
command) to the WAL, reverting to Stage 2 requires:

1. Clean-shutdown the Stage 3 server (both HTTP servers drain, matching
   thread joins).
2. `git revert <merge-commit>` to restore Stage 2 code.
3. `rm -rf data/wal` — Stage 2 cannot replay Stage 3 events. **All open
   orders/positions lost**; acceptable in dev (no production data).
4. Restart.

Symmetric to Stage 2 spec §8.5. A forward-compatible `apply_event` that
skips unknown variants for one major version (no WAL wipe on rollback)
remains deferred to Stage 5+ (tracked below).

## 9. Forward pointers (Stage 4+)

- **Liquidation** (Stage 4): drive `RiskMonitor.scan_positions` from the
  post-trade pipeline on `MarkPriceUpdate`; emit `LiquidationOrder`;
  matcher executes forced close; `close_position_settled` with margin;
  replay arm for `LiquidationOrder` (currently `UnexpectedVariant`).
- **Initial margin + realized-loss solvency** (Stage 4): reserve
  user Funding→margin on position open, release on close; order-acceptance
  rejects on insufficient margin; this also removes the Stage 3
  "realized loss exceeds Funding available → `withdraw` fail-fast"
  limitation (§5.4) — margin/liquidation guarantees a closing user can
  cover the loss.
- **Insurance fund** (Stage 4): replace the SYSTEM Funding-pool net-sink
  with a real `WalletType::InsuranceFund` account + bankruptcy/ADL
  waterfall; precise per-trade PnL counterparty accounting.
- **Periodic funding timer**: production `funding_interval_hours` cadence
  replacing admin-triggered ticks.
- **Real deposits/withdrawals**: replace admin-credit with a funded
  deposit flow (chain/PSP); admin-credit was the testable seam (as admin
  mark-price was for the oracle in Stage 2).
- **Forward-compatible replay for rollback** (CEO/Eng Stage 2 C3): an
  `apply_event` that skips unknown event variants for one major version so
  Stage N→N-1 rollback does not require a WAL wipe. Deferred until
  production traffic makes WAL preservation mandatory.
- **Multi-symbol**: per-symbol positions/funding when the single-symbol
  invariant (Stage 0 #1) is lifted.

## Appendix A — Decisions log (from brainstorming)

| # | Decision | Choice |
|---|----------|--------|
| 1 | Stage 3 scope | substrate (live position tracking + ledger wiring) + funding settlement; liquidation → Stage 4 |
| 2 | Architecture placement | same-thread post-trade pipeline; separate `PostTradeProcessor`; one WAL / one replay; matching vs clearing bounded contexts |
| 3 | Margin depth | position qty/avg-entry + realized PnL on close + funding settlement only; no margin reservation/enforcement (→ Stage 4) |
| 4 | Balance bootstrap | minimal `Command::AdminCredit` via ring buffer + `X-Admin-Secret` admin endpoint; double-entry vs system account |
| 5 | Settlement trigger | `PostTradeProcessor` reacts to `FundingRateUpdate`, atomic settle same tick; `FundingSettled` WAL'd; replay applies recorded amount |
| 6 | Replay model | hybrid — position qty/entry projected from fills; all money movements explicit WAL'd fact events, replay applies recorded amounts (no money recompute) |
| 7 | Double-entry counterparty | ledger-internal `SYSTEM_USER_ID`/Funding pool as unified counterparty (deposit/withdraw/settle_funding handle it internally); precise per-trade PnL + insurance fund → Stage 4 |
