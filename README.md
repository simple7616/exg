# EXG -- Perpetual Futures Exchange

Production-grade centralized perpetual contract + spot exchange built with Rust backend and Next.js frontend.

## Architecture

LMAX-style single-writer event-sourcing architecture:

```
                    ┌─────────────┐
                    │  API Gateway │
                    │  (REST + WS) │
                    └──────┬──────┘
                           │ Commands
                    ┌──────▼──────┐
                    │ Ring Buffer  │ (SPSC, mmap)
                    │   (Input)    │
                    └──────┬──────┘
                           │
                    ┌──────▼──────┐
                    │  Matching    │ ← Single writer thread
                    │   Engine     │   (CPU-pinned, lock-free)
                    └──────┬──────┘
                           │ Events
                    ┌──────▼──────┐
                    │    WAL       │ (append-only, CRC32)
                    └──────┬──────┘
                           │
              ┌────────────┼────────────┐
              │            │            │
       ┌──────▼──┐  ┌─────▼────┐ ┌────▼─────┐
       │ Clearing │  │  Market  │ │  Order   │
       │ Service  │  │   Data   │ │ Service  │
       └─────────┘  └──────────┘ └──────────┘
```

## Tech Stack

| Layer | Technology |
|-------|-----------|
| Matching Engine | Rust (single-threaded, lock-free) |
| Serialization | rkyv (zero-copy) + serde |
| IPC | mmap SPSC Ring Buffer |
| Persistence | Custom WAL + Snapshots |
| API | Actix-web (REST + WebSocket) |
| Database | PostgreSQL + TimescaleDB |
| Cache | Redis |
| Messaging | NATS (JetStream) |
| Frontend | Next.js 15 + TypeScript + TailwindCSS |
| Charts | TradingView Lightweight Charts |
| Monitoring | Prometheus + Grafana |
| Deployment | Docker + Kubernetes |

## Project Structure

```
exg/
├── crates/
│   ├── exg-common/            # Decimal128, IDs, types, errors
│   ├── exg-protocol/          # Command/Event message definitions
│   ├── exg-ringbuffer/        # SPSC mmap ring buffer
│   ├── exg-wal/               # Write-ahead log + snapshots
│   ├── exg-risk-engine/       # Margin, funding, ADL calculations
│   ├── exg-matching-engine/   # Order book + price-time matching
│   ├── exg-ledger/            # Double-entry bookkeeping
│   ├── exg-config/            # TOML configuration + validation
│   ├── exg-order-service/     # Order lifecycle management
│   ├── exg-clearing/          # Position + settlement + funding
│   ├── exg-market-data/       # Klines, tickers, depth, trades
│   ├── exg-user-service/      # Auth, JWT, 2FA, API keys
│   ├── exg-api-gateway/       # API types, middleware, rate limit
│   ├── exg-wallet-service/    # Deposit/withdrawal management
│   ├── exg-admin-service/     # Admin operations + reporting
│   └── exg-server/            # Server binary entry point
├── web/
│   ├── trading/               # Trading frontend (Next.js)
│   └── admin/                 # Admin dashboard (Next.js)
├── config/                    # TOML configuration files
├── deploy/
│   ├── k8s/                   # Kubernetes manifests
│   ├── docker/                # Docker configs
│   ├── prometheus/            # Prometheus config
│   ├── grafana/               # Grafana dashboards
│   └── terraform/             # Infrastructure as code
├── migrations/                # PostgreSQL + TimescaleDB migrations
├── scripts/                   # Dev/build/test scripts
├── tests/
│   ├── e2e/                   # End-to-end tests
│   └── load/                  # Load tests
├── Cargo.toml                 # Workspace root
├── docker-compose.yml         # Local infrastructure
├── Dockerfile                 # Server image
├── Dockerfile.trading         # Trading frontend image
└── Dockerfile.admin           # Admin frontend image
```

## Crate Dependency Graph

```
exg-server
├── exg-api-gateway
│   ├── exg-protocol
│   │   └── exg-common
│   └── exg-common
├── exg-matching-engine
│   ├── exg-protocol
│   ├── exg-risk-engine
│   │   └── exg-common
│   └── exg-common
├── exg-clearing
│   ├── exg-risk-engine
│   ├── exg-ledger
│   │   └── exg-common
│   └── exg-common
├── exg-order-service
│   ├── exg-protocol
│   └── exg-common
├── exg-market-data
│   └── exg-common
├── exg-user-service
│   └── exg-common
├── exg-wallet-service
│   ├── exg-ledger
│   └── exg-common
├── exg-admin-service
│   └── exg-common
├── exg-ringbuffer
├── exg-wal
└── exg-config
```

## Crate Overview

| Crate | Purpose | Tests |
|-------|---------|-------|
| exg-common | Decimal128, Snowflake IDs, domain types, errors | 70 |
| exg-protocol | Command/Event message definitions (serde + rkyv) | 14 |
| exg-ringbuffer | SPSC mmap ring buffer for IPC | 8 |
| exg-wal | Write-ahead log + CRC32 + snapshots | 9 |
| exg-risk-engine | Margin, funding rate, ADL, pre-trade checks | 45 |
| exg-matching-engine | Order book + price-time priority matching | 41 |
| exg-ledger | Double-entry bookkeeping with invariant checks | 19 |
| exg-config | TOML configuration loading + validation | 14 |
| exg-order-service | Order lifecycle management | 21 |
| exg-clearing | Position management + settlement + funding | 25 |
| exg-market-data | Klines, tickers, depth snapshots, recent trades | 27 |
| exg-user-service | Auth, JWT, TOTP 2FA, API key management | 16 |
| exg-api-gateway | REST/WS types, rate limiting, auth middleware | 19 |
| exg-wallet-service | Deposit/withdrawal + hot wallet management | 24 |
| exg-admin-service | Admin operations, user/symbol mgmt, reporting | 12 |
| exg-server | Server binary entry point | -- |
| **Total** | | **364** |

## Quick Start

```bash
# Prerequisites: Rust (1.84+), Node.js (20+), Docker

# 1. Setup development environment
scripts/setup.sh

# 2. Start infrastructure (PostgreSQL, Redis, NATS, Prometheus, Grafana)
scripts/dev.sh

# 3. Run all tests
cargo test --workspace

# 4. Start the exchange server
cargo run -p exg-server
```

## Development

```bash
# Type check
cargo check --workspace

# Run all tests
cargo test --workspace

# Lint (zero warnings policy)
cargo clippy --workspace -- -D warnings

# Format check
cargo fmt --check

# Run full test suite with optional frontend + benchmarks
scripts/test.sh --all

# Run only specific benchmark suite
scripts/bench.sh matching
scripts/bench.sh decimal
scripts/bench.sh ringbuffer
scripts/bench.sh wal

# Lint everything (Rust + TypeScript)
scripts/lint.sh

# Build trading frontend
cd web/trading && npm run build

# Build admin frontend
cd web/admin && npm run build
```

## Deployment

```bash
# Build Docker image
scripts/docker-build.sh

# Kubernetes deployment
kubectl apply -f deploy/k8s/namespace.yml
kubectl apply -f deploy/k8s/

# See docs/deployment.md for full guide
```

## Documentation

- [Architecture](docs/architecture.md) -- System design, data flow, component details
- [API Reference](docs/api.md) -- REST + WebSocket API documentation
- [Deployment Guide](docs/deployment.md) -- Docker, Kubernetes, monitoring setup
- [Developer Guide](docs/development.md) -- Repo structure, conventions, workflows

## License

UNLICENSED -- Proprietary
