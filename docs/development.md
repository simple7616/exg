# EXG Developer Guide

## Repository Structure

```
exg/
├── crates/                    # Rust workspace crates (16 crates)
├── web/trading/               # Trading frontend (Next.js 15)
├── web/admin/                 # Admin dashboard (Next.js 15)
├── config/                    # TOML configuration
├── deploy/                    # Docker, K8s, Prometheus, Grafana, Terraform
├── migrations/                # PostgreSQL + TimescaleDB SQL migrations
├── scripts/                   # Shell scripts for dev/test/build/lint
├── tests/e2e/                 # End-to-end integration tests
├── tests/load/                # Load/stress tests
├── Cargo.toml                 # Workspace root
├── Cargo.lock                 # Locked dependencies
├── rustfmt.toml               # Formatting: max_width=100
├── docker-compose.yml         # Local infrastructure
├── Dockerfile                 # Server image (multi-stage)
├── Dockerfile.trading         # Trading frontend image
└── Dockerfile.admin           # Admin frontend image
```

## Workspace Configuration

- **Rust edition**: 2024
- **Resolver**: version 2
- **Release profile**: `opt-level=3`, `lto=fat`, `codegen-units=1`, `panic=abort`
- **Bench profile**: `opt-level=3`, `lto=thin`

All dependencies are declared in `[workspace.dependencies]` and referenced by crates via `{ workspace = true }`.

---

## Adding a New Crate

1. Create the crate directory:
```bash
cargo new crates/exg-my-crate --lib
```

2. Add to `Cargo.toml` workspace members:
```toml
[workspace]
members = [
    # ... existing crates
    "crates/exg-my-crate",
]
```

3. Add workspace dependencies in the crate's `Cargo.toml`:
```toml
[package]
name = "exg-my-crate"
edition.workspace = true
version.workspace = true
license.workspace = true

[dependencies]
exg-common = { workspace = true }
serde = { workspace = true }
```

4. Follow existing patterns:
   - `lib.rs` re-exports public API
   - Tests go in `#[cfg(test)] mod tests` blocks within source files
   - Use `thiserror` for error types
   - Financial calculations use `Decimal128` exclusively

---

## Testing Conventions

### Structure

Tests live inline in each source file under `#[cfg(test)] mod tests`. This keeps tests close to the code they verify.

### Naming

```rust
#[test]
fn test_{function_name}_{scenario}() {
    // ...
}
```

Examples:
- `test_initial_margin_basic`
- `test_funding_rate_clamped_upper`
- `test_liquidation_deficit`

### Decimal Helper

Every test module that uses `Decimal128` includes:
```rust
fn dec(s: &str) -> Decimal128 {
    s.parse().unwrap()
}
```

### Financial Precision Tests

Financial calculations must be tested against **known exact values**, not approximate comparisons:

```rust
#[test]
fn test_maintenance_margin_tier2() {
    let tiers = binance_btc_tiers();
    // 100000 * 0.005 - 50 = 450
    let result = calc_maintenance_margin(dec("100000"), &tiers);
    assert_eq!(result, dec("450"));
}
```

### Invariant Verification

Ledger tests call `verify_all_invariants()` after every operation sequence to ensure the global balance equation holds:

```rust
ledger.deposit(user, dec("1000"), "dep-1", ts(1)).unwrap();
ledger.verify_all_invariants().unwrap();
```

### Running Tests

```bash
# All tests
cargo test --workspace

# Specific crate
cargo test -p exg-matching-engine

# Specific test
cargo test -p exg-risk-engine test_funding_rate_clamped

# With output
cargo test --workspace -- --nocapture

# Full test suite with lint + optional frontend
scripts/test.sh --all --verbose
```

---

## Code Style

### Formatting

Configured in `rustfmt.toml`:
```toml
max_width = 100
use_field_init_shorthand = true
use_try_shorthand = true
```

Run: `cargo fmt`
Check: `cargo fmt --check`

### Clippy

Zero-warnings policy: `cargo clippy --workspace -- -D warnings`

### Naming Conventions

| Item | Convention | Example |
|------|-----------|---------|
| Types | PascalCase | `OrderBook`, `MatchingEngine` |
| Functions | snake_case | `calc_initial_margin` |
| Constants | SCREAMING_SNAKE | `RECORD_OVERHEAD`, `SCALE` |
| Newtype IDs | PascalCase wrapper | `OrderId(u64)`, `UserId(u64)` |
| Module files | snake_case | `pre_trade.rs`, `risk_monitor.rs` |

### Error Handling

- **Library crates** (`exg-common`, `exg-risk-engine`, etc.): use `thiserror` with `ExgError` enum
- **Application crates** (`exg-server`): use `anyhow` for top-level error aggregation
- **API layer**: use `ApiError` struct with Binance-compatible error codes

### Serialization Strategy

| Context | Library | Reason |
|---------|---------|--------|
| Ring buffer (hot path) | rkyv | Zero-copy deserialization, no allocation |
| WAL storage | raw bytes | Direct payload append, CRC32 wrapping |
| REST API / JSON | serde + serde_json | Standard JSON serialization |
| Database | sqlx | Native PostgreSQL types |
| Configuration | serde + TOML | Human-readable config files |

### Hash Maps

- **`FxHashMap`** (from `rustc-hash`): non-cryptographic, used for internal lookups (order book, user orders, rate limiter buckets)
- **`HashMap`** (std): used for external-facing or persisted data (ledger accounts, config)

### ID Types

Always use newtype wrappers, never raw integers:

```rust
// Correct
fn get_order(&self, order_id: OrderId) -> Option<&BookOrder>

// Wrong -- ambiguous, no type safety
fn get_order(&self, order_id: u64) -> Option<&BookOrder>
```

---

## Benchmark Guide

Benchmarks use the `criterion` crate. Run with:

```bash
# All benchmarks
scripts/bench.sh

# Specific suite
scripts/bench.sh decimal    # Decimal128 arithmetic
scripts/bench.sh matching   # Matching engine throughput
scripts/bench.sh ringbuffer # Ring buffer push/pop
scripts/bench.sh wal        # WAL append/read

# Direct cargo
cargo bench -p exg-matching-engine --bench matching
```

Results are saved in `target/criterion/` with HTML reports.

---

## Common Development Workflows

### Adding a New Order Type

1. Add variant to `OrderType` in `exg-common/src/types.rs`
2. Update `is_conditional()` and `is_limit()` methods
3. Add handling in `exg-matching-engine/src/engine.rs` (`handle_new_order`)
4. Add serde test coverage in `exg-protocol/src/lib.rs`
5. Add API conversion in `exg-api-gateway/src/conversion.rs`

### Adding a New API Endpoint

1. Define request/response types in `exg-api-gateway/src/types.rs`
2. Add conversion functions in `exg-api-gateway/src/conversion.rs`
3. Add error cases to `exg-api-gateway/src/error.rs` if needed
4. Document in `docs/api.md`
5. Add tests covering validation, happy path, and error cases

### Adding a New Risk Check

1. Add pure function in appropriate `exg-risk-engine` module
2. Wire into the pre-trade check chain in the matching engine
3. Add corresponding `RejectReason` variant in `exg-protocol/src/event.rs`
4. Test against known values with realistic market data

### Adding a New Ledger Operation

1. Add method to `Ledger` in `exg-ledger/src/operations.rs`
2. Create appropriate `JournalEntry` records (debit + credit)
3. Handle idempotency key
4. Test with `verify_all_invariants()` call after every operation
5. Verify failed operations are retryable

---

## Debugging Tips

### WAL Inspection

The WAL reader can dump all records:
```rust
let mut reader = WalReader::open(Path::new("./data/wal")).unwrap();
reader.read_from(0, |seq, payload| {
    println!("seq={seq} len={}", payload.len());
    true
}).unwrap();
```

### Ring Buffer Monitoring

The ring buffer exposes head/tail positions via atomic reads. Monitor for backpressure by checking `tail - head` approaching `slot_count`.

### Decimal128 Precision

When debugging financial calculations, use `Decimal128::raw()` to inspect the internal i128 representation:
```rust
let val = dec("0.1") + dec("0.2");
println!("raw={} display={}", val.raw(), val);
// raw=300000000000000000 display=0.3
```

### Order Book State

The matching engine exposes `orderbook()` for inspection:
```rust
let (bids, asks) = engine.orderbook().depth(10);
println!("Best bid: {:?}", engine.orderbook().best_bid());
println!("Best ask: {:?}", engine.orderbook().best_ask());
println!("Orders on book: {}", engine.orderbook().order_count());
```

---

## Edition 2024 Notes

Rust edition 2024 reserves `gen` as a keyword. Never use `gen` as a variable or function name. Use alternatives:

- `sf` or `id_gen` for Snowflake generators
- `generator` for generic generators
- `rng` for random number generators

---

## Contributing Guidelines

1. **Branch from main**: create feature branches named `feature/{description}` or `fix/{description}`
2. **Test everything**: `cargo test --workspace` must pass with zero failures
3. **Lint clean**: `cargo clippy --workspace -- -D warnings` must produce zero warnings
4. **Format**: `cargo fmt --check` must pass
5. **Document public APIs**: all `pub` functions need doc comments
6. **Financial precision**: decimal calculations must have precision tests against known values
7. **Invariant tests**: ledger operations must verify invariants after every mutation
8. **No f64 for money**: all financial values use `Decimal128`
