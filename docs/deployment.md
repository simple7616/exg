# EXG Deployment Guide

## Prerequisites

| Tool | Version | Purpose |
|------|---------|---------|
| Rust | 1.84+ | Backend build (edition 2024) |
| Node.js | 20+ | Frontend build |
| Docker | 24+ | Containerization |
| docker-compose | 2.0+ | Local infrastructure |
| kubectl | 1.28+ | Kubernetes deployment |
| sqlx-cli | 0.8+ | Database migrations (optional) |

---

## Local Development with docker-compose

### 1. Start Infrastructure

```bash
# Start all services (PostgreSQL, Redis, NATS, Prometheus, Grafana)
scripts/dev.sh

# Or selectively
docker compose up -d postgres redis nats
```

### Service Ports

| Service | Port | Credentials |
|---------|------|-------------|
| PostgreSQL (TimescaleDB) | 5432 | exg / exg_dev_password |
| Redis | 6379 | -- |
| NATS | 4222 (client), 8222 (monitoring) | -- |
| Prometheus | 9090 | -- |
| Grafana | 3100 | admin / admin |
| Exchange API | 8080 | -- |
| Exchange WS | 8081 | -- |
| Admin API | 9090 | -- |
| Metrics | 9000 | -- |

### 2. Database Setup

```bash
# Automatic (if sqlx-cli installed)
export DATABASE_URL="postgresql://exg:exg_dev_password@localhost:5432/exg"
sqlx migrate run --source migrations/

# Manual (via docker)
docker compose exec postgres psql -U exg -d exg -f /docker-entrypoint-initdb.d/migrations/20260101000001_users.up.sql
# Repeat for each migration file in order
```

Migration files:

| Migration | Tables |
|-----------|--------|
| `20260101000001_users` | users, api_keys, sub_accounts, login_history |
| `20260101000002_symbols` | symbols, margin tiers |
| `20260101000003_orders` | orders, order history |
| `20260101000004_positions_accounts` | positions, account balances |
| `20260101000005_deposits_withdrawals` | deposits, withdrawals, transfers |
| `20260101000006_timescaledb` | trades_ts, klines_1m (hypertables) |

### 3. Build and Run

```bash
# Build
cargo build --workspace

# Run exchange server
cargo run -p exg-server

# Run frontends
cd web/trading && npm run dev   # http://localhost:3000
cd web/admin && npm run dev     # http://localhost:3001
```

### 4. Stop Everything

```bash
docker compose down
# Add -v to also remove data volumes
docker compose down -v
```

---

## Configuration

### TOML Configuration (`config/default.toml`)

The exchange reads configuration from TOML with environment variable overrides using the `EXG_` prefix.

```toml
[server]
host = "127.0.0.1"
port = 8080
ws_port = 8081
admin_port = 9090
node_id = 1                    # Snowflake node ID (0-1023)

[database]
url = "postgres://exg:exg@localhost:5432/exg"
max_connections = 20
min_connections = 2

[redis]
url = "redis://localhost:6379"
pool_size = 8

[nats]
url = "nats://localhost:4222"

[wal]
dir = "./data/wal"
segment_size_mb = 64           # Segment rotation threshold
flush_interval_us = 1000       # Flush interval (1ms)
flush_every_n = 1000           # Flush after N records

[ringbuffer]
slot_count = 65536             # Must be power of 2
slot_size = 4096               # Bytes per slot

[risk]
max_orders_per_second = 300
max_cancels_per_second = 600
price_band_pct = "0.05"        # 5% price band
max_position_notional = "10000000"
funding_interval_hours = 8
interest_rate = "0.0001"       # 0.01%
impact_notional = "200"        # For funding rate calculation
```

### Environment Variable Overrides

Pattern: `EXG_{SECTION}_{KEY}` (case-insensitive)

Examples:
```bash
EXG_DATABASE_URL=postgres://...
EXG_SERVER_PORT=8080
EXG_REDIS_URL=redis://...
EXG_NATS_URL=nats://...
```

### Symbol Configuration

Symbols are defined in the `[[trading.symbols]]` array in the TOML config. Each symbol includes:
- ID, name, base/quote assets
- Symbol type (perpetual_linear, perpetual_inverse, spot)
- Tick size, lot size, min notional
- Max leverage, maker/taker fees
- Margin tiers (Binance-compatible tiered rate schedule)

---

## Docker Image Builds

### Server Image

```bash
# Build
docker build -t exg/server:latest -f Dockerfile .

# Multi-stage build:
# Stage 1: rust:1.84-bookworm - compile workspace
# Stage 2: gcr.io/distroless/cc-debian12 - minimal runtime
```

### Frontend Images

```bash
docker build -t exg/trading:latest -f Dockerfile.trading .
docker build -t exg/admin:latest -f Dockerfile.admin .
```

### Build Script

```bash
scripts/docker-build.sh
```

---

## Kubernetes Deployment

### Namespace Setup

```bash
kubectl apply -f deploy/k8s/namespace.yml
```

Creates the `exg` namespace with labels for the application.

### Secret Configuration

Create secrets before deploying services:

```bash
kubectl -n exg create secret generic exg-secrets \
  --from-literal=database-url='postgres://exg:PROD_PASSWORD@postgres:5432/exg' \
  --from-literal=redis-url='redis://redis:6379' \
  --from-literal=jwt-secret='YOUR_JWT_SECRET'

kubectl -n exg create secret tls exg-tls-secret \
  --cert=path/to/tls.crt \
  --key=path/to/tls.key
```

### Service Deployment Order

Deploy in this order to respect dependencies:

```bash
# 1. Infrastructure
kubectl apply -f deploy/k8s/postgres-statefulset.yml
kubectl apply -f deploy/k8s/redis-deployment.yml
kubectl apply -f deploy/k8s/nats-statefulset.yml

# 2. Wait for infrastructure readiness
kubectl -n exg wait --for=condition=ready pod -l app=postgres --timeout=120s
kubectl -n exg wait --for=condition=ready pod -l app=redis --timeout=60s
kubectl -n exg wait --for=condition=ready pod -l app=nats --timeout=60s

# 3. Run migrations (via Job or manually)
# 4. Deploy matching engine
kubectl apply -f deploy/k8s/matching-engine-daemonset.yml

# 5. Deploy API server
kubectl apply -f deploy/k8s/exg-server-deployment.yml
kubectl apply -f deploy/k8s/exg-server-service.yml

# 6. Configure ingress
kubectl apply -f deploy/k8s/ingress.yml
```

### Matching Engine DaemonSet

The matching engine runs as a DaemonSet on dedicated nodes:

```yaml
nodeSelector:
  node-role.exg.io/matching-engine: "true"
tolerations:
  - key: dedicated
    operator: Equal
    value: matching-engine
    effect: NoSchedule
```

Key configuration:
- **hostNetwork: true** -- eliminates container network overhead
- **SYS_NICE capability** -- required for CPU affinity (`core_affinity`)
- **Guaranteed QoS** -- requests == limits (4 CPU, 8Gi memory)
- **WAL on hostPath** -- `/data/exg/wal` for direct disk access

Label your matching engine nodes:
```bash
kubectl label node <node-name> node-role.exg.io/matching-engine=true
kubectl taint node <node-name> dedicated=matching-engine:NoSchedule
```

### Ingress + TLS

The ingress configuration handles:
- `api.exg.io` -- REST API + WebSocket upgrade
- WebSocket timeouts set to 3600s
- TLS termination via `exg-tls-secret`

### API Server

- Replicas: 1 (can scale horizontally for read-only endpoints)
- Liveness probe: `GET /health` (10s interval)
- Readiness probe: `GET /ready` (5s interval)
- Config loaded from ConfigMap

---

## Monitoring Setup

### Prometheus

Prometheus scrapes metrics from:

| Target | Port | Path |
|--------|------|------|
| exg-server | 9000 | /metrics |
| PostgreSQL exporter | 9187 | /metrics |
| Redis exporter | 9121 | /metrics |
| NATS | 8222 | /varz |

Key exchange metrics:

| Metric | Type | Description |
|--------|------|-------------|
| `exg_matching_engine_latency_seconds` | histogram | Order processing latency |
| `exg_orders_total` | counter | Total orders processed |
| `exg_active_positions` | gauge | Active position count |
| `exg_insurance_fund_balance` | gauge | Insurance fund balance |
| `exg_api_requests_total` | counter | API request count |
| `exg_websocket_connections` | gauge | Active WS connections |

### Grafana

Dashboards are provisioned from `deploy/grafana/dashboards/`. Grafana runs on port 3100 (local) with anonymous access enabled for development.

---

## Health Checks and Readiness Probes

| Endpoint | Purpose |
|----------|---------|
| `GET /health` | Liveness -- returns 200 if process is alive |
| `GET /ready` | Readiness -- returns 200 when all dependencies connected |

Readiness checks:
1. Database connection pool active
2. Redis connection active
3. NATS connection active
4. WAL directory writable
5. Ring buffer initialized

---

## Backup and Recovery

### WAL Snapshots

The WAL is the authoritative source of all exchange state. Recovery procedure:

1. **Identify latest snapshot**: `snapshot-{sequence}.snap` in the WAL directory
2. **Load snapshot**: deserialize engine state from the snapshot file (CRC32 verified)
3. **Replay WAL**: replay all events from `snapshot_sequence + 1` to the end of the WAL
4. **Verify**: engine state is fully reconstructed

### Backup Strategy

```bash
# 1. Snapshot the WAL directory (atomic at filesystem level)
rsync -a /data/exg/wal/ /backup/wal/$(date +%Y%m%d)/

# 2. PostgreSQL backup
pg_dump -h localhost -U exg exg > /backup/pg/exg_$(date +%Y%m%d).sql

# 3. Verify WAL integrity
# The WalReader will detect any corrupt records via CRC32 check
```

### Snapshot Retention

The WAL writer automatically keeps only the latest 3 snapshots. Older snapshots are deleted on each new snapshot save. Configure snapshot frequency based on WAL replay time tolerance.

### Disaster Recovery

1. Provision new infrastructure
2. Restore PostgreSQL from backup
3. Copy WAL directory to new matching engine node
4. Start matching engine -- it will automatically:
   a. Load latest snapshot
   b. Replay remaining WAL events
   c. Resume processing new commands
