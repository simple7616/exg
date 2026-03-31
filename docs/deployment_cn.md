# EXG 部署指南

## 前置条件

| 工具 | 版本 | 用途 |
|------|------|------|
| Rust | 1.84+ | 后端编译（edition 2024） |
| Node.js | 20+ | 前端构建 |
| Docker | 24+ | 容器化 |
| docker-compose | 2.0+ | 本地基础设施 |
| kubectl | 1.28+ | Kubernetes 部署 |
| sqlx-cli | 0.8+ | 数据库迁移（可选） |

---

## 使用 docker-compose 进行本地开发

### 1. 启动基础设施

```bash
# 启动所有服务（PostgreSQL、Redis、NATS、Prometheus、Grafana）
scripts/dev.sh

# 或按需启动
docker compose up -d postgres redis nats
```

### 服务端口

| 服��� | 端口 | 凭据 |
|------|------|------|
| PostgreSQL (TimescaleDB) | 5432 | exg / exg_dev_password |
| Redis | 6379 | -- |
| NATS | 4222（客户端）、8222（监控） | -- |
| Prometheus | 9090 | -- |
| Grafana | 3100 | admin / admin |
| 交易所 API | 8080 | -- |
| 交易所 WS | 8081 | -- |
| 管理后台 API | 9090 | -- |
| 指标端点 | 9000 | -- |

### 2. 数据库配置

```bash
# 自动方式（需安装 sqlx-cli）
export DATABASE_URL="postgresql://exg:exg_dev_password@localhost:5432/exg"
sqlx migrate run --source migrations/

# 手动方式（通过 docker）
docker compose exec postgres psql -U exg -d exg -f /docker-entrypoint-initdb.d/migrations/20260101000001_users.up.sql
# 按顺序对每个迁移文件重复执行
```

迁移文件：

| 迁移 | 表 |
|------|---|
| `20260101000001_users` | users、api_keys、sub_accounts、login_history |
| `20260101000002_symbols` | symbols、margin tiers |
| `20260101000003_orders` | orders、order history |
| `20260101000004_positions_accounts` | positions、account balances |
| `20260101000005_deposits_withdrawals` | deposits、withdrawals、transfers |
| `20260101000006_timescaledb` | trades_ts、klines_1m（hypertable） |

### 3. 构建与运行

```bash
# 构建
cargo build --workspace

# 运行交易所服务
cargo run -p exg-server

# 运行前端
cd web/trading && npm run dev   # http://localhost:3000
cd web/admin && npm run dev     # http://localhost:3001
```

### 4. 停止所有服务

```bash
docker compose down
# 添加 -v 以同时删除数据卷
docker compose down -v
```

---

## 配置

### TOML 配置（`config/default.toml`）

交易所从 TOML 文件读取配置，支持通过 `EXG_` 前缀的环境变量覆盖。

```toml
[server]
host = "127.0.0.1"
port = 8080
ws_port = 8081
admin_port = 9090
node_id = 1                    # Snowflake 节点 ID（0-1023）

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
segment_size_mb = 64           # 段文件轮转阈值
flush_interval_us = 1000       # 刷盘间隔（1ms）
flush_every_n = 1000           # 每 N 条记录刷盘

[ringbuffer]
slot_count = 65536             # 必须为 2 的幂
slot_size = 4096               # 每个 Slot 的字节数

[risk]
max_orders_per_second = 300
max_cancels_per_second = 600
price_band_pct = "0.05"        # 5% 价格偏离带
max_position_notional = "10000000"
funding_interval_hours = 8
interest_rate = "0.0001"       # 0.01%
impact_notional = "200"        # 用于资金费率计算
```

### 环境变量覆盖

格式：`EXG_{SECTION}_{KEY}`（大小写不敏感）

示例：
```bash
EXG_DATABASE_URL=postgres://...
EXG_SERVER_PORT=8080
EXG_REDIS_URL=redis://...
EXG_NATS_URL=nats://...
```

### 交易对配置

交易对在 TOML 配置的 `[[trading.symbols]]` 数组中定义。每个交易对包含：
- ID、名称、基础资产 / 报价资产
- 交易对类型（perpetual_linear、perpetual_inverse、spot）
- Tick size、Lot size、最小名义价值
- 最大杠杆、Maker/Taker 费率
- 保证金梯度（兼容币安的阶梯费率表）

---

## Docker 镜像构建

### 服务端镜像

```bash
# 构建
docker build -t exg/server:latest -f Dockerfile .

# 多阶段构建：
# 第一阶段：rust:1.84-bookworm - 编译 workspace
# 第二阶段：gcr.io/distroless/cc-debian12 - 最小化运行时
```

### 前端镜像

```bash
docker build -t exg/trading:latest -f Dockerfile.trading .
docker build -t exg/admin:latest -f Dockerfile.admin .
```

### 构建脚本

```bash
scripts/docker-build.sh
```

---

## Kubernetes 部署

### 命名空间配置

```bash
kubectl apply -f deploy/k8s/namespace.yml
```

创建 `exg` 命名空间并设置应用标签。

### Secret 配置

部署服务前先创建 Secret：

```bash
kubectl -n exg create secret generic exg-secrets \
  --from-literal=database-url='postgres://exg:PROD_PASSWORD@postgres:5432/exg' \
  --from-literal=redis-url='redis://redis:6379' \
  --from-literal=jwt-secret='YOUR_JWT_SECRET'

kubectl -n exg create secret tls exg-tls-secret \
  --cert=path/to/tls.crt \
  --key=path/to/tls.key
```

### 服务部署顺序

按以下顺序部署以满足依赖关系：

```bash
# 1. 基础设施
kubectl apply -f deploy/k8s/postgres-statefulset.yml
kubectl apply -f deploy/k8s/redis-deployment.yml
kubectl apply -f deploy/k8s/nats-statefulset.yml

# 2. 等待基础设施就绪
kubectl -n exg wait --for=condition=ready pod -l app=postgres --timeout=120s
kubectl -n exg wait --for=condition=ready pod -l app=redis --timeout=60s
kubectl -n exg wait --for=condition=ready pod -l app=nats --timeout=60s

# 3. 运行数据库迁移（通过 Job 或手动）
# 4. 部署撮合引擎
kubectl apply -f deploy/k8s/matching-engine-daemonset.yml

# 5. 部署 API 服务
kubectl apply -f deploy/k8s/exg-server-deployment.yml
kubectl apply -f deploy/k8s/exg-server-service.yml

# 6. 配置 Ingress
kubectl apply -f deploy/k8s/ingress.yml
```

### 撮合引擎 DaemonSet

撮合引擎以 DaemonSet 方式运行在专用节点上：

```yaml
nodeSelector:
  node-role.exg.io/matching-engine: "true"
tolerations:
  - key: dedicated
    operator: Equal
    value: matching-engine
    effect: NoSchedule
```

关键配置：
- **hostNetwork: true** -- 消除容器网络开销
- **SYS_NICE capability** -- CPU 绑核（`core_affinity`）所需
- **Guaranteed QoS** -- requests == limits（4 CPU、8Gi 内存）
- **WAL 使用 hostPath** -- `/data/exg/wal`，直接磁盘访问

为撮合引擎节点打标签：
```bash
kubectl label node <node-name> node-role.exg.io/matching-engine=true
kubectl taint node <node-name> dedicated=matching-engine:NoSchedule
```

### Ingress + TLS

Ingress 配置处理：
- `api.exg.io` -- REST API + WebSocket 升级
- WebSocket 超时设置为 3600 秒
- 通过 `exg-tls-secret` 终止 TLS

### API 服务

- 副本数：1（可水平扩展只读接口）
- 存活探针：`GET /health`（10 秒间隔）
- 就绪探针：`GET /ready`（5 秒间隔）
- 配置从 ConfigMap 加载

---

## 监控配置

### Prometheus

Prometheus 采集以下目标的指标：

| 目标 | 端口 | 路径 |
|------|------|------|
| exg-server | 9000 | /metrics |
| PostgreSQL exporter | 9187 | /metrics |
| Redis exporter | 9121 | /metrics |
| NATS | 8222 | /varz |

核心交易所指标：

| 指标 | 类型 | 描述 |
|------|------|------|
| `exg_matching_engine_latency_seconds` | histogram | 订单处理延迟 |
| `exg_orders_total` | counter | 已处理订单总数 |
| `exg_active_positions` | gauge | 活跃仓位数 |
| `exg_insurance_fund_balance` | gauge | 保险基金余额 |
| `exg_api_requests_total` | counter | API 请求总数 |
| `exg_websocket_connections` | gauge | 活跃 WebSocket 连接数 |

### Grafana

仪表盘从 `deploy/grafana/dashboards/` 自动加载。Grafana 在开发环境运行于 3100 端口，启用匿名访问。

---

## 健康检查与就绪探针

| 接口 | 用途 |
|------|------|
| `GET /health` | 存活探针 -- 进程存活时返回 200 |
| `GET /ready` | 就绪探针 -- 所有依赖就绪时返回 200 |

就绪检查项：
1. 数据库连接池可用
2. Redis 连接可用
3. NATS 连接可用
4. WAL 目录可写
5. Ring Buffer 已初始化

---

## 备份与恢复

### WAL 快照

WAL 是所有交易所状态的权威来源。恢复流程：

1. **定位最新快照**：WAL 目录中的 `snapshot-{sequence}.snap`
2. **加载快照**：从快照文件反序列化引擎状态（CRC32 校验）
3. **重放 WAL**：从 `snapshot_sequence + 1` 开始重放所有事件至 WAL 末尾
4. **验证**：引擎状态完全重建

### 备份策略

```bash
# 1. 备份 WAL 目录（文件系统级原子性）
rsync -a /data/exg/wal/ /backup/wal/$(date +%Y%m%d)/

# 2. PostgreSQL 备份
pg_dump -h localhost -U exg exg > /backup/pg/exg_$(date +%Y%m%d).sql

# 3. 验证 WAL 完整性
# WalReader 会通过 CRC32 校验检测所有损坏的记录
```

### 快照保留策略

WAL 写入器自动仅保留最新的 3 个快照。每次保存新快照时自动删除旧快照。根据 WAL 重放时间容忍度配置快照频率。

### 灾难恢复

1. 搭建新的基础设施
2. 从备份恢复 PostgreSQL
3. 将 WAL 目录复制到新的撮合引擎节点
4. 启动撮合引擎——系统将自动：
   a. 加载最新快照
   b. 重放剩余 WAL 事件
   c. 恢复处理新的 Command
