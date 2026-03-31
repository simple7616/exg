# EXG Project Instructions

## Build & Test
- `cargo check --workspace` -- type check
- `cargo test --workspace` -- run all tests (364 tests across 16 crates)
- `cargo clippy --workspace -- -D warnings` -- lint (zero warnings policy)
- `cargo fmt --check` -- format check
- `cd web/trading && npm run build` -- build trading frontend
- `cd web/admin && npm run build` -- build admin frontend
- `scripts/test.sh` -- full test suite (check + clippy + tests)
- `scripts/test.sh --all` -- includes frontend builds and benchmarks
- `scripts/bench.sh` -- run all benchmarks
- `scripts/bench.sh matching` -- matching engine benchmarks only

## Architecture Rules
- Matching engine is single-threaded, lock-free -- no async, no locks, no mutexes
- All financial calculations use Decimal128 (18-digit fixed-point precision) -- never f64
- Risk engine functions are pure (no I/O, no mutable state) -- input in, result out
- Ledger enforces double-entry invariants -- every debit has a matching credit
- All balance operations require idempotency keys -- duplicates silently accepted
- WAL is the source of truth -- snapshots are optimization only
- Ring buffer uses rkyv zero-copy serialization on the hot path
- Commands flow API -> Ring Buffer -> Matching Engine -> WAL -> Event Bus

## Rust Edition 2024
- `gen` is a reserved keyword -- never use as variable name
- Use `sf`, `id_gen`, `generator`, `rng` etc. instead

## Code Conventions
- Error types: use `thiserror` for library errors, `anyhow` for application errors
- Serialization: rkyv for hot-path (ring buffer), serde for storage/API
- Hash maps: `FxHashMap` (rustc-hash) for non-cryptographic internal lookups, std `HashMap` for external-facing
- IDs: always use newtype wrappers (`OrderId`, `UserId`, `SymbolId`, `TradeId`, `AccountId`)
- Decimal helper in tests: `fn dec(s: &str) -> Decimal128 { s.parse().unwrap() }`
- Formatting: `max_width = 100`, `use_field_init_shorthand = true`
- API errors: Binance-compatible codes (-1000, -1002, -1015, -1100, -2010, -2013)

## Testing
- Every public function needs tests
- Financial calculations need precision tests against known exact values
- Use realistic mock data (BTC prices ~50000-65000, typical leverage 10-125x)
- Ledger tests must call `verify_all_invariants()` after every operation sequence
- Failed ledger operations must be retryable with the same idempotency key

## Key Crate Dependencies
- `exg-common` -- shared types, Decimal128, IDs, errors (depended on by almost everything)
- `exg-protocol` -- Command/Event enums (depends on exg-common)
- `exg-matching-engine` -- depends on exg-protocol, exg-risk-engine, exg-common
- `exg-ledger` -- depends on exg-common
- `exg-clearing` -- depends on exg-risk-engine, exg-ledger, exg-common

## Infrastructure (docker-compose)
- PostgreSQL (TimescaleDB): localhost:5432 (exg/exg_dev_password)
- Redis: localhost:6379
- NATS: localhost:4222
- Prometheus: localhost:9090
- Grafana: localhost:3100 (admin/admin)

## Release Profile
- `opt-level = 3`, `lto = "fat"`, `codegen-units = 1`, `panic = "abort"`, `strip = "symbols"`
- Bench profile: `lto = "thin"`
