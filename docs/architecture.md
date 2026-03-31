# EXG Architecture

## System Overview

EXG is a centralized perpetual futures + spot exchange following the LMAX exchange architecture pattern. The core design principle is a **single-writer event-sourcing model** where all state mutations flow through a deterministic matching engine running on a dedicated, CPU-pinned thread.

### Data Flow

```
Client Request
     │
     ▼
API Gateway (Actix-web)
     │  validation, auth, rate limiting
     ▼
Command serialization (rkyv zero-copy)
     │
     ▼
SPSC Ring Buffer (mmap, lock-free)
     │
     ▼
Matching Engine (single writer thread, CPU-pinned)
     │  deterministic state transitions
     ▼
WAL append (CRC32 integrity)
     │
     ├──▶ Clearing Service    (position + settlement + funding)
     ├──▶ Market Data Service  (klines, depth, ticker, trades)
     ├──▶ Order Service        (order lifecycle, user notifications)
     └──▶ NATS JetStream      (event distribution to downstream consumers)
```

### Design Principles

1. **Single-writer determinism** -- All order book mutations happen in one thread. No locks, no contention, no race conditions. Given the same input sequence, the engine produces identical output.

2. **Event sourcing** -- The WAL is the source of truth. All state can be reconstructed by replaying events from the WAL. Snapshots are an optimization, not a requirement.

3. **Zero-copy hot path** -- Commands flow through the ring buffer serialized with rkyv (zero-copy deserialization). No allocation on the critical path between API gateway and matching engine.

4. **Pure risk calculations** -- All risk engine functions are pure (no I/O, no state mutation). They take inputs and return results. This makes them trivially testable, auditable, and parallelizable on read paths.

5. **Double-entry bookkeeping** -- Every balance change is recorded as a journal entry with debit and credit sides. Global invariants are mechanically verifiable at any point.

---

## Matching Engine

### Single-Writer Thread Model

The matching engine (`exg-matching-engine`) runs on a dedicated CPU core, isolated from the OS scheduler via `core_affinity::set_for_current()`. It reads commands from the input ring buffer in a busy-spin loop, processes each command deterministically, and emits events to the WAL.

```
┌──────────────────────────────────────────────────┐
│                Matching Engine Thread             │
│                                                  │
│  loop {                                          │
│      cmd = ringbuffer.consumer.pop()             │
│      events = engine.process_command(cmd)        │
│      wal.append(events)                          │
│      event_bus.publish(events)                   │
│  }                                               │
└──────────────────────────────────────────────────┘
```

### Order Book Structure

Each symbol has an independent `OrderBook`:

- **Bids**: `BTreeMap<Reverse<Decimal128>, PriceLevel>` -- descending price order
- **Asks**: `BTreeMap<Decimal128, PriceLevel>` -- ascending price order
- **Order lookup**: `FxHashMap<OrderId, BookOrder>` -- O(1) by ID
- **User orders**: `FxHashMap<UserId, Vec<OrderId>>` -- O(1) cancel-all

Each `PriceLevel` contains a `Vec<OrderId>` maintaining FIFO insertion order for time priority within a price level.

### Matching Algorithm (Price-Time Priority)

1. Buy order matches against asks (lowest price first)
2. Sell order matches against bids (highest price first)
3. Within each price level, orders are matched in FIFO order
4. Fill always executes at the **maker's price** (price improvement for taker)

### Order Types

| Type | Behavior |
|------|----------|
| Limit | Rests on book at specified price |
| Market | Fills at best available, remaining canceled |
| StopMarket | Triggers at stop_price, executes as market |
| StopLimit | Triggers at stop_price, places limit at price |
| TakeProfitMarket | Inverse trigger direction vs StopMarket |
| TakeProfitLimit | Inverse trigger direction vs StopLimit |
| TrailingStop | Tracks peak price, triggers on reversal by delta |
| Iceberg | Large order split into visible slices; refills on fill |

### Time-in-Force Policies

| Policy | Behavior |
|--------|----------|
| GTC | Good-Till-Canceled -- remains until filled or canceled |
| IOC | Immediate-Or-Cancel -- fill what you can, cancel rest |
| FOK | Fill-Or-Kill -- fill entirely or reject entirely (pre-validated) |
| GTD | Good-Till-Date -- auto-expires at specified timestamp |
| PostOnly | Rejected if it would take liquidity (maker-only) |

### Conditional Order Flow

Stop, take-profit, and trailing-stop orders are queued in a separate `stop_orders` list. On each `update_mark_price()` call:

1. Trailing peak prices are updated (high watermark for sell, low watermark for buy)
2. All stop orders are checked against current mark price for trigger conditions
3. Triggered orders are converted to Market/Limit and processed through the book

### Snapshot and Recovery

The engine supports `take_snapshot()` and `restore_from_snapshot()`. Snapshots capture:
- All resting orders on the book
- All pending conditional orders
- Mark/index prices
- WAL sequence number
- GTD expiry heap entries

Recovery: load latest snapshot, then replay WAL events from snapshot sequence onward.

---

## Ring Buffer Protocol

### SPSC mmap Ring Buffer (`exg-ringbuffer`)

The ring buffer provides lock-free, single-producer single-consumer communication between the API gateway and the matching engine via anonymous mmap.

### Memory Layout

```
Offset 0:    [head: AtomicU64]  [padding to 128 bytes]    ← consumer writes
Offset 128:  [tail: AtomicU64]  [padding to 128 bytes]    ← producer writes
Offset 256:  [slot_count: u64]  [slot_size: u64]          ← metadata
Offset 512:  [Slot 0] [Slot 1] ... [Slot N-1]             ← data region

Each slot:
  [msg_len: u32 LE] [payload bytes] [padding to slot_size]
```

Key properties:
- **128-byte cache line separation** between head and tail pointers eliminates false sharing
- `slot_count` must be a power of 2 for bitmask-based indexing (`index = seq & (count - 1)`)
- Producer uses `Ordering::Release` on tail write; consumer uses `Ordering::Acquire` on tail read
- Backpressure: when `tail - head >= slot_count`, producer receives `WouldBlock`

### Configuration

Default: 65,536 slots x 4,096 bytes = 256 MB ring buffer.

---

## WAL Format and Crash Recovery

### Write-Ahead Log (`exg-wal`)

The WAL provides durable, sequential persistence of all engine events with CRC32 integrity checking.

### Record Format

```
┌─────────┬──────────────┬─────────────────┬──────────┐
│ seq: u64│ len: u32 LE  │ payload: [u8]   │ crc: u32 │
│ (8 B)   │ (4 B)        │ (variable)      │ (4 B)    │
└─────────┴──────────────┴─────────────────┴──────────┘
         12 bytes header          4 bytes trailer
```

- **Sequence number**: monotonically increasing, globally unique
- **CRC32**: computed over `[seq_bytes | len_bytes | payload_bytes]`

### Segment Files

WAL data is split into segment files named `wal-{first_sequence:020}.log`. Rotation occurs when a segment exceeds the configured size (default: 64 MB). Segments are immutable once rotated.

### Crash Recovery Procedure

1. **Scan all segments** in order, validating every record's CRC32
2. **Detect partial writes**: if the last record in the last segment has an incomplete header or CRC mismatch, truncate the file to the last valid record boundary
3. **Sequence continuity check**: any gap in sequence numbers in non-trailing position is a fatal `SequenceGap` error
4. **CRC errors in sealed segments**: fatal `Corrupt` error (confirmed data loss)
5. **Resume**: writer reopens at `next_sequence` after the last valid record

### Snapshot Management

Snapshots are stored as `snapshot-{sequence:020}.snap` with a CRC32 trailer. Atomic write via temp file + `fsync` + rename + directory `fsync`. Only the latest 3 snapshots are retained; older ones are automatically cleaned up.

---

## Decimal128 Fixed-Point Arithmetic

### Why Not f64?

IEEE 754 double-precision cannot represent `0.1 + 0.2 == 0.3`. In a financial system, accumulated rounding errors in fee calculations, margin computations, and PnL settlements would lead to balance discrepancies that violate double-entry invariants. This is not acceptable for a system that handles real money.

### Why Not rust_decimal?

`rust_decimal` is backed by a 96-bit mantissa with a variable scale factor (0-28). While correct, it has several drawbacks for this use case:

1. **Variable scale** -- operations may change the scale, requiring normalization after arithmetic
2. **No rkyv support** -- zero-copy serialization on the hot path requires `rkyv::Archive` derivation
3. **Performance** -- the variable-scale division algorithm is slower than fixed-scale division

### Decimal128 Design

`Decimal128` uses a fixed 18-digit fractional scale backed by `i128`:

```
raw_value = real_value * 10^18
Range: approximately +/- 1.7 * 10^20 (integer part)
```

Key properties:
- **Exact representation**: `0.1 + 0.2 == 0.3` always holds
- **Fixed scale**: no scale tracking or normalization needed
- **18 digits**: sufficient for any realistic price (BTC at $100,000 with 8 decimal places = 14 digits)
- **Wide arithmetic**: when `a * b` overflows i128, a 256-bit intermediate multiplication via four 64-bit half-products is used, followed by 256-bit / 128-bit division
- **rkyv zero-copy**: derives `rkyv::Archive, rkyv::Serialize, rkyv::Deserialize`
- **serde as string**: serialized as `"123.456"` in JSON (no floating-point ambiguity)

### Checked vs Unchecked

Operator traits (`+`, `-`, `*`, `/`) panic on overflow in debug builds and wrap in release. For code that must never panic, use `checked_add`, `checked_sub`, `checked_mul`, `checked_div` which return `Option<Decimal128>`.

---

## Risk Engine

### Pure Function Design (`exg-risk-engine`)

All risk engine functions are stateless and pure. They take position data, configuration, and prices as inputs, and return computed values. No I/O, no database access, no mutable state. This design:

- Makes every calculation **unit-testable** against known values
- Enables **parallel risk checks** on read paths
- Guarantees **deterministic results** when called from the matching engine thread
- Simplifies **auditing** -- each function's behavior is self-contained

### Margin Calculation

**Initial margin**: `notional / leverage`

**Maintenance margin** (tiered):
```
For tier where notional_floor <= notional < notional_cap:
    maintenance_margin = notional * mmr - maintenance_amount
```

Tiers are configured per-symbol (Binance-compatible bracket system). The `maintenance_amount` field is a cumulative adjustment that makes the tiered rate continuous.

**Liquidation price** (linear perpetual):
```
Long:  liq_price = entry * (1 - 1/leverage + mmr) - accumulated_funding / size
Short: liq_price = entry * (1 + 1/leverage - mmr) + accumulated_funding / size
```

**Margin ratio**: `total_maintenance_margin / equity`, where `equity = wallet_balance + sum(unrealized_pnl)`. Returns `Decimal128::MAX` if equity is zero or negative.

### Pre-Trade Checks

Executed before order placement:

1. **Position limit**: total notional (existing + new order) must not exceed `max_position_notional`
2. **Price band**: `|order_price - mark_price| / mark_price <= band_pct` (default 5%)
3. **Self-trade prevention**: rejects if user has an existing order on the opposite side for the same symbol
4. **Rate limiting**: per-user order and cancel rates checked against configurable thresholds

---

## Funding Rate Calculation

### Impact Price Model

The funding rate uses the **impact mid price** model (Binance-compatible):

1. **Impact bid price**: VWAP to fill `impact_notional` worth on the bid side
2. **Impact ask price**: VWAP to fill `impact_notional` worth on the ask side
3. **Impact mid price**: `(impact_bid + impact_ask) / 2`
4. **Premium index**: `(impact_mid - index_price) / index_price`
5. **Funding rate**: `clamp(premium_index + interest_rate, -0.75%, +0.75%)`

Default interest rate: 0.01% (0.0001). Funding interval: 8 hours.

### Funding Fee

```
funding_fee = position_size * mark_price * funding_rate
```

- Positive funding rate: longs pay, shorts receive
- Negative funding rate: shorts pay, longs receive
- Convention: positive return from `calc_funding_fee` means user pays

### Funding Settlement in Ledger

The `settle_funding_checked` method:
1. Deducts from available balance first
2. If available is insufficient, taps margin (indicating liquidation risk)
3. Returns `true` if margin was tapped (caller must trigger liquidation check)
4. Uses structured idempotency keys: `funding_{period}_{user}_{symbol}`

---

## Liquidation, Insurance Fund, ADL Cascade

### Liquidation Trigger

When `margin_ratio >= 1.0` (maintenance margin equals or exceeds equity), the position is liquidated.

### Cascade

```
1. Liquidation triggered
   │
   ├── surplus > 0 → margin remainder goes to Insurance Fund
   │
   └── surplus < 0 (deficit) → Insurance Fund covers the gap
       │
       └── Insurance Fund depleted → ADL (Auto-Deleverage) activated
```

### ADL Ranking

ADL priority score: `(unrealized_pnl / margin) * (notional / margin)`

This product of PnL percentage and leverage factor ensures that the most profitable, highest-leveraged counterparties are deleveraged first. Scores are sorted descending; the highest-score user is deleveraged first.

### Insurance Fund Accounting

The insurance fund is a system account in the ledger. Operations:
- **Surplus**: `escrow -> insurance_fund` (liquidation with remaining margin)
- **Deficit**: `insurance_fund -> escrow` (liquidation loss exceeds margin)
- **Depleted**: returns `ExgError::InsuranceFundDepleted`, triggering ADL

---

## Mark Price Model

Mark price uses a **median of three sources** approach (configurable):

1. Exchange last price
2. External index price (weighted average of major exchanges)
3. Moving average of impact mid price

### Staleness Handling

If mark price data is older than the configured threshold, the engine rejects new orders with `RejectReason::MarkPriceStale` and halts liquidation processing until fresh data arrives. This prevents liquidations based on stale or manipulated prices.

---

## Ledger Double-Entry Bookkeeping

### Model (`exg-ledger`)

Every balance change is recorded as a `JournalEntry` with:
- Debit side: `(user, wallet, field)` -- balance decreases
- Credit side: `(user, wallet, field)` -- balance increases
- Amount (always positive)
- Idempotency key (prevents duplicate processing)
- Entry type classification

### Wallet Types

| Wallet | Purpose |
|--------|---------|
| Spot | Spot trading balances |
| Futures | Perpetual contract margin and available |
| Funding | Deposit/withdrawal holding wallet |
| InsuranceFund | System: liquidation surplus pool |
| FeeCollection | System: accumulated trading fees |
| Escrow | System: interim settlement holding |

### Balance Sub-Fields

Each wallet has three sub-fields:
- **available**: freely usable balance
- **frozen**: locked for pending orders
- **margin**: locked as position collateral

### Invariants

1. **No negative sub-fields**: `available >= 0 && frozen >= 0 && margin >= 0` for all user wallets
2. **Non-negative system accounts**: InsuranceFund, FeeCollection, Escrow must be >= 0
3. **Global balance**: `sum(all_user_balances) + sum(all_system_balances) == net_deposits - net_withdrawals`
4. **Idempotency**: duplicate keys are silently accepted (no double-processing)
5. **Failed operations are retryable**: on failure, the idempotency key is removed so the same key can be reused

### Operation Flow Example (Trade Settlement)

```
1. freeze_for_order:   available -> frozen         (order placed)
2. open_position:      frozen -> margin + fee      (trade executed)
3. close_position:     margin -> available + pnl   (position closed)
   - If profit: counterparty.margin debited
   - If loss: counterparty.margin credited
   - Fee: deducted from available, credited to FeeCollection
```

---

## API Authentication Flow

### JWT Authentication

1. User registers with email + password (Argon2 hash)
2. Login returns a JWT token with configurable expiry
3. Optional TOTP 2FA via `totp-rs` (QR code provisioning)
4. JWT token included in `Authorization: Bearer {token}` header

### API Key HMAC Authentication

For programmatic access:

1. User creates API key via authenticated endpoint
2. Each key has permissions: `can_trade`, `can_withdraw`, `can_read`
3. Optional IP whitelist per key
4. Request signing: HMAC-SHA256 over `timestamp + method + path + body`
5. Headers: `X-EXG-APIKEY`, `X-EXG-SIGNATURE`, `X-EXG-TIMESTAMP`
6. Timestamp validation: request must be within configurable window (default 10s)

### Rate Limiting

Token bucket algorithm per API key:
- Configurable max tokens and refill rate
- Separate buckets per key
- Returns Binance-compatible error code `-1015` when exceeded

---

## WebSocket Subscription Model

### Connection

```
wss://api.exg.io/ws/stream          # Public market data
wss://api.exg.io/ws/{listenKey}     # Private user stream
```

### Subscribe/Unsubscribe

```json
{"method": "SUBSCRIBE", "params": ["btcusdt@depth20", "ethusdt@trade"], "id": 1}
{"method": "UNSUBSCRIBE", "params": ["btcusdt@depth20"], "id": 2}
```

### Stream Names

Format: `{symbol}@{channel}`

| Channel | Data |
|---------|------|
| `@depth{N}` | Order book snapshot (top N levels) |
| `@trade` | Real-time trade stream |
| `@kline_{interval}` | Candlestick updates (1m, 5m, 15m, 1h, 4h, 1d) |
| `@ticker` | 24h ticker statistics |

### Subscription Manager

Bidirectional mapping maintained in memory:
- `client_id -> Set<stream_name>` (what streams a client subscribes to)
- `stream_name -> Set<client_id>` (which clients need a given stream)

On client disconnect, all subscriptions are cleaned up via `remove_client()`.
