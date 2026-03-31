# EXG -- 永续合约交易所

基于 Rust 后端和 Next.js 前端构建的生产级中心化永续合约 + 现货交易所。

## 架构

LMAX 风格的单写者事件溯源架构：

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
                    │  Matching    │ ← 单写者线程
                    │   Engine     │   (CPU 绑核, 无锁)
                    └──────┬──────┘
                           │ Events
                    ┌──────▼──────┐
                    │    WAL       │ (仅追加, CRC32)
                    └──────┬──────┘
                           │
              ┌────────────┼────────────┐
              │            │            │
       ┌──────▼──┐  ┌─────▼────┐ ┌────▼─────┐
       │ Clearing │  │  Market  │ │  Order   │
       │ Service  │  │   Data   │ │ Service  │
       └─────────┘  └──────────┘ └──────────┘
```

## 技术栈

| 层级 | 技术 |
|------|------|
| 撮合引擎 | Rust（单线程、无锁） |
| 序列化 | rkyv（零拷贝）+ serde |
| 进程间通信 | mmap SPSC Ring Buffer |
| 持久化 | 自研 WAL + 快照 |
| API | Actix-web（REST + WebSocket） |
| 数据库 | PostgreSQL + TimescaleDB |
| 缓存 | Redis |
| 消息队列 | NATS（JetStream） |
| 前端 | Next.js 15 + TypeScript + TailwindCSS |
| 图表 | TradingView Lightweight Charts |
| 监控 | Prometheus + Grafana |
| 部署 | Docker + Kubernetes |

## 项目结构

```
exg/
├── crates/
│   ├── exg-common/            # Decimal128、ID、类型、错误定义
│   ├── exg-protocol/          # Command/Event 消息定义
│   ├── exg-ringbuffer/        # SPSC mmap 环形缓冲区
│   ├── exg-wal/               # 预写日志 + 快照
│   ├── exg-risk-engine/       # 保证金、资金费率、ADL 计算
│   ├── exg-matching-engine/   # 订单簿 + 价格-时间优先撮合
│   ├── exg-ledger/            # 复式记账
│   ├── exg-config/            # TOML 配置加载 + 校验
│   ├── exg-order-service/     # 订单生命周期管理
│   ├── exg-clearing/          # 仓位 + 结算 + 资金费率
│   ├── exg-market-data/       # K 线、行情、深度、成交
│   ├── exg-user-service/      # 认证、JWT、2FA、API Key 管理
│   ├── exg-api-gateway/       # API 类型、中间件、限流
│   ├── exg-wallet-service/    # 充提管理
│   ├── exg-admin-service/     # 管理后台操作 + 报表
│   └── exg-server/            # 服务端二进制入口
├── web/
│   ├── trading/               # 交易前端（Next.js）
│   └── admin/                 # 管理后台（Next.js）
├── config/                    # TOML 配置文件
├── deploy/
│   ├── k8s/                   # Kubernetes 清单
│   ├── docker/                # Docker 配置
│   ├── prometheus/            # Prometheus 配置
│   ├── grafana/               # Grafana 仪表盘
│   └── terraform/             # 基础设施即代码
├── migrations/                # PostgreSQL + TimescaleDB 数据库迁移
├── scripts/                   # 开发/构建/测试脚本
├── tests/
│   ├── e2e/                   # 端到端测试
│   └── load/                  # 压力测试
├── Cargo.toml                 # Workspace 根配置
├── docker-compose.yml         # 本地基础设施
├── Dockerfile                 # 服务端镜像
├── Dockerfile.trading         # 交易前端镜像
└── Dockerfile.admin           # 管理后台镜像
```

## Crate 依赖图

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

## Crate 概览

| Crate | 功能描述 | 测试数 |
|-------|----------|--------|
| exg-common | Decimal128、Snowflake ID、领域类型、错误定义 | 70 |
| exg-protocol | Command/Event 消息定义（serde + rkyv） | 14 |
| exg-ringbuffer | SPSC mmap 环形缓冲区，用于进程间通信 | 8 |
| exg-wal | 预写日志 + CRC32 校验 + 快照 | 9 |
| exg-risk-engine | 保证金、资金费率、ADL、交易前置检查 | 45 |
| exg-matching-engine | 订单簿 + 价格-时间优先撮合 | 41 |
| exg-ledger | 带不变量检查的复式记账 | 19 |
| exg-config | TOML 配置加载 + 校验 | 14 |
| exg-order-service | 订单生命周期管理 | 21 |
| exg-clearing | 仓位管理 + 结算 + 资金费率 | 25 |
| exg-market-data | K 线、行情、深度快照、最近成交 | 27 |
| exg-user-service | 认证、JWT、TOTP 2FA、API Key 管理 | 16 |
| exg-api-gateway | REST/WS 类型、限流、认证中间件 | 19 |
| exg-wallet-service | 充提 + 热钱包管理 | 24 |
| exg-admin-service | 管理操作、用户/交易对管理、报表 | 12 |
| exg-server | 服务端二进制入口 | -- |
| **合计** | | **364** |

## 快速开始

```bash
# 前置条件：Rust (1.84+)、Node.js (20+)、Docker

# 1. 初始化开发环境
scripts/setup.sh

# 2. 启动基础设施（PostgreSQL、Redis、NATS、Prometheus、Grafana）
scripts/dev.sh

# 3. 运行全部测试
cargo test --workspace

# 4. 启动交易所服务
cargo run -p exg-server
```

## 开发

```bash
# 类型检查
cargo check --workspace

# 运行全部测试
cargo test --workspace

# 代码检查（零警告策略）
cargo clippy --workspace -- -D warnings

# 格式检查
cargo fmt --check

# 运行完整测试套件（含可选的前端测试 + 基准测试）
scripts/test.sh --all

# 运行特定基准测试
scripts/bench.sh matching
scripts/bench.sh decimal
scripts/bench.sh ringbuffer
scripts/bench.sh wal

# 全量 Lint（Rust + TypeScript）
scripts/lint.sh

# 构建交易前端
cd web/trading && npm run build

# 构建管理后台前端
cd web/admin && npm run build
```

## 部署

```bash
# 构建 Docker 镜像
scripts/docker-build.sh

# Kubernetes 部署
kubectl apply -f deploy/k8s/namespace.yml
kubectl apply -f deploy/k8s/

# 完整部署指南请参阅 docs/zh/deployment.md
```

## 文档

- [架构设计](architecture.md) -- 系统设计、数据流、组件详解
- [API 参考](api.md) -- REST + WebSocket API 文档
- [部署指南](deployment.md) -- Docker、Kubernetes、监控配置
- [开发指南](development.md) -- 代码仓库结构、开发规范、工作流

## 许可证

UNLICENSED -- 专有软件
