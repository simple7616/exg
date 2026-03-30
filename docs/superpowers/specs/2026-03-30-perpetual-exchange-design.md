# EXG - 生产级中心化永续合约交易所设计规格

## Context

构建一个对标 Binance/OKX 的生产级中心化交易所，支持永续合约和现货交易。使用 Rust 前沿技术栈实现后端核心引擎，Next.js 实现 Web 前端。

**性能目标**：
- 单 Symbol 撮合延迟 < 100μs (p99)
- 单 Symbol 吞吐 > 500K orders/sec（基于 LMAX 在现代 CPU 上的实测基线）
- 平台聚合吞吐通过 Per-Symbol 水平扩展：N symbols × 500K = 总 TPS
- 初始部署支持 50+ 交易对，单机 32 核可承载 50 个独立撮合实例
- **降级策略**：当单 Symbol 订单流超过阈值（如极端行情 BTC/USDT），启用批量撮合模式（micro-batch per 10μs），牺牲延迟换吞吐
- **Benchmark 验证**：P0 阶段完成后必须通过 criterion benchmark 验证单 Symbol 延迟和吞吐指标

## 1. 整体架构

### 1.1 架构模式

**LMAX-Style 单写者事件溯源架构**：
- 撮合引擎：单线程无锁，事件溯源，Disruptor 模式
- 微服务：其他组件独立部署，通过消息队列通信
- 混合部署：撮合引擎裸金属，其余 K8s 容器化

### 1.2 系统拓扑

```
Load Balancer (Nginx/HAProxy)
    │
    ├── REST API (Actix-web)
    ├── WebSocket API (Actix-web)
    └── Admin API (Actix-web)
         │
    API Gateway Layer (Auth · Rate Limit · Validation · Routing)
         │ NATS JetStream
         ├── Order Router Service
         ├── Market Data Service
         └── User/Account Service
              │
    Shared Memory Ring Buffer (mmap + CAS)
         │
         │
         ├── Matching Engine (Bare Metal, per-symbol, CPU pinned)
         │    └── [内嵌] Pre-Trade Risk Gate (同步, < 5μs, 撮合引擎单线程内模块)
         │
              │ Events
    Event Journal (WAL, append-only, sequenced, durable)
         │
         ├── Clearing Service
         ├── Market Data Aggregator
         ├── Risk Monitor Service (异步, 独立部署, 持续监控保证金/强平/ADL)
         └── Notification Service
```

### 1.3 通信模式

| 路径 | 协议 | 延迟目标 |
|------|------|----------|
| API → 撮合引擎 | Shared Memory Ring Buffer (mmap + CAS) | < 1μs |
| 撮合引擎 → 下游 | Event Journal (mmap WAL) | < 5μs |
| 跨服务通信 | NATS JetStream | < 100μs |
| API → 客户端 | REST/WebSocket over TCP | < 5ms |

### 1.4 技术栈

| 层级 | 技术 | 理由 |
|------|------|------|
| 语言(后端) | Rust 2024 Edition | 零成本抽象、无 GC、内存安全 |
| 异步运行时 | Tokio | 生态最成熟 |
| Web 框架 | Actix-web | Actor 模型适合 WS 管理、性能最强 |
| 数据库(OLTP) | PostgreSQL 16 | 成熟度、事务保证 |
| 时序数据 | TimescaleDB | K线聚合、hypertable 自动分区 |
| 缓存 | Redis 7+ / DragonflyDB | 热数据缓存、会话管理 |
| 消息队列 | NATS JetStream | 低延迟、At-Least-Once |
| 事件存储 | 自研 mmap WAL | 比任何通用方案都快 |
| 序列化 | rkyv (zero-copy) + serde | 热路径 zero-copy，冷路径 JSON |
| 前端框架 | Next.js 15 + React 19 | SSR + 组件生态 |
| 前端图表 | TradingView Lightweight Charts | 行业标准 |
| 前端样式 | TailwindCSS + shadcn/ui | 快速开发、一致性 |
| 状态管理 | Zustand | 轻量、TypeScript 友好 |
| 容器化 | Docker + K8s | 非核心路径弹性伸缩 |
| 可观测 | Prometheus + Grafana + OpenTelemetry | 行业标准 |

## 2. 撮合引擎（Matching Engine）

### 2.1 架构

LMAX Disruptor 模式 Rust 实现：
- **Input Ring Buffer** (mmap + CAS)：接收 NewOrder/Cancel/Amend 命令
- **Matching Core** (Single Thread)：Sequencer → Pre-Risk → OrderBook Match → Post-Trade Risk
- **Output Ring Buffer** (mmap WAL)：输出 OrderAccepted/Filled/Canceled/TradeExec 事件
- **Consumers**：Clearing, MarketData, Risk Monitor, Notifier

### 2.2 OrderBook 数据结构

```rust
struct PriceLevel {
    price: Decimal128,       // 128-bit 定点数
    total_qty: Decimal128,
    orders: VecDeque<Order>, // FIFO 时间优先
}

struct OrderBook {
    symbol: SymbolId,
    bids: BTreeMap<Decimal128, PriceLevel>,  // 买盘，降序
    asks: BTreeMap<Decimal128, PriceLevel>,  // 卖盘，升序
    order_index: FxHashMap<OrderId, OrderLocation>, // O(1) 查找
}
```

### 2.3 定点数精度

自研 `Decimal128`：18 位整数 + 18 位小数，所有算术用整数指令，避免浮点非确定性。乘除法使用 i128 中间结果防溢出。

### 2.4 订单类型

| 类型 | 说明 |
|------|------|
| LIMIT | 限价单 |
| MARKET | 市价单 |
| STOP_LIMIT | 止损限价 |
| STOP_MARKET | 止损市价 |
| TAKE_PROFIT | 止盈 |
| TRAILING_STOP | 追踪止损 |
| POST_ONLY | 只做 Maker |
| IOC | Immediate-or-Cancel |
| FOK | Fill-or-Kill |
| GTC | Good-Till-Cancel |
| GTD | Good-Till-Date |
| ICEBERG | 冰山单 |

### 2.5 撮合算法

Price-Time Priority：
1. 新订单 → Pre-risk check（保证金、仓位限制、价格带）
2. 与对手盘逐级撮合
3. 未成交部分挂入 OrderBook
4. 生成 TradeExecution 事件写入 WAL
5. Post-trade risk check

### 2.6 性能设计

- 单线程无锁，消除并发开销
- Per-Symbol 实例，绑定独立 CPU 核（core_affinity）
- 启动时预分配 OrderBook 内存池，运行时零分配
- WAL 写入使用 io_uring 异步 I/O
- NUMA 感知内存分配

### 2.7 崩溃恢复协议

**WAL 持久化保证**：
- 每个事件写入 mmap 后，定期 `msync` / `fdatasync` 刷盘（可配置：每 N 个事件或每 T 微秒）
- 每个事件携带单调递增的 `sequence_number`，用于确定最后一个 durable 事件

**Snapshot 策略**：
- 每 100,000 个事件或每 5 分钟（以先到者为准）创建 OrderBook 内存快照
- 快照格式：`(snapshot_sequence, serialized_orderbook)` 写入独立文件
- 保留最近 3 个快照，旧快照自动清理

**撮合引擎重启恢复**：
1. 加载最新 Snapshot → 获得 `snapshot_sequence`
2. 从 WAL 的 `snapshot_sequence + 1` 开始 Replay 到末尾
3. 恢复完成后开始接受新命令

**下游消费者恢复**：
- 每个消费者在本地持久化 `last_processed_sequence`（写入 PostgreSQL/Redis）
- 重启后从 `last_processed_sequence + 1` 开始重放
- 所有消费者必须实现**幂等处理**（通过 `idempotency_key` 或 `sequence_number` 去重）

### 2.8 GTD 过期管理

- 撮合引擎内维护一个按过期时间排序的 **Min-Heap**（`BinaryHeap<(expiry_time, order_id)>`）
- 每次撮合循环开始时检查 heap 顶部，批量取出所有已过期订单
- 生成 `CancelOrder` 命令注入处理流程（与正常撤单相同路径）
- 过期取消事件写入 WAL，下游消费者可感知

### 2.9 特殊订单类型详细机制

**TRAILING_STOP（追踪止损）**：
- 参数：`callback_rate`（回调比例，如 1%）或 `callback_amount`（绝对值）
- 驱动价格：Mark Price（非 Last Price，防止插针触发）
- 多仓追踪止损：追踪最高 Mark Price，触发价 = highest_mark × (1 - callback_rate)
- 空仓追踪止损：追踪最低 Mark Price，触发价 = lowest_mark × (1 + callback_rate)
- 激活条件：可选设置 `activation_price`，Mark Price 达到激活价后才开始追踪

**ICEBERG（冰山单）**：
- 参数：`total_quantity`（总数量）、`visible_quantity`（每次显示数量）
- 当可见部分完全成交后，自动生成新的可见订单（原价格，新时间优先级）
- 直到 `total_quantity` 全部成交或用户取消
- OrderBook 中仅显示 `visible_quantity`

## 3. 风控引擎（Risk Engine）

### 3.1 双层架构

- **Pre-Trade Risk Gate**（同步，撮合前，< 5μs）：**内嵌于撮合引擎单线程内**，作为 `exg-matching-engine` crate 的内部模块调用 `exg-risk-engine` 的纯函数。检查项：保证金充足、仓位限制、价格带、自成交防护、频率限制。
- **Risk Monitor Service**（异步，独立部署的微服务）：从 WAL 消费 Trade 事件，持续监控所有仓位的保证金率，触发强平/ADL。通过 NATS 向撮合引擎发送强平命令。

### 3.2 保证金模型

**全仓（Cross Margin）**：
```
Available Balance = Wallet Balance + Unrealized PnL - Maintenance Margin
Initial Margin = Σ (|Position Notional| / Leverage)
Maintenance Margin = Σ (|Position Notional| × MMR)
```

**逐仓（Isolated Margin）**：
```
Position Margin = Initial Margin + 追加保证金

Liquidation Price (Long) = Entry Price × (1 - 1/Leverage + MMR)
    + Accumulated Funding Fee / Position Size

Liquidation Price (Short) = Entry Price × (1 + 1/Leverage - MMR)
    - Accumulated Funding Fee / Position Size

注：Accumulated Funding Fee 为该仓位自开仓以来累计支付（正值）或收取（负值）的资金费用，
   会持续侵蚀或增加仓位保证金，从而影响实际强平价格。
```

### 3.3 维持保证金率分档

| 档位 | 名义价值(USDT) | 最大杠杆 | 维持保证金率 |
|------|---------------|----------|-------------|
| 1 | 0 - 50,000 | 125x | 0.40% |
| 2 | 50,000 - 250,000 | 100x | 0.50% |
| 3 | 250,000 - 1,000,000 | 50x | 1.00% |
| ... | 逐档递增 | 递减 | 递增 |

### 3.4 强制平仓流程

1. Risk Monitor Service 持续监控 Mark Price（每 100ms 刷新）
2. Margin Ratio ≥ 100% → 通过 NATS 发送 CancelAllOrders 命令到撮合引擎
3. 撮合引擎执行取消后，Risk Monitor 重算 Margin Ratio
4. 仍 ≥ 100% → 发起 Liquidation Order（以破产价格为限价）
5. 定义：`liquidation_surplus = liquidation_proceeds - bankruptcy_value`
   - `liquidation_proceeds`：强平委托实际成交总额
   - `bankruptcy_value`：破产价格 × 仓位数量（即用户保证金归零时的理论成交额）
6. `liquidation_surplus > 0`（成交价优于破产价）→ 盈余注入保险基金
7. `liquidation_surplus < 0`（穿仓，成交价劣于破产价）→ 由保险基金弥补亏损
8. 保险基金不足以弥补 → 触发 ADL

### 3.4.1 ADL（自动减仓）机制

- **排名指标**：盈利率 = (Mark Price - Entry Price) / Entry Price × Leverage × Side
- **执行顺序**：按盈利率从高到低选择对手方
- **执行价格**：破产价格（Bankruptcy Price）
- **通知**：ADL 事件实时推送给被减仓用户（WebSocket user data stream + 站内信）
- **前端标识**：仓位面板显示 ADL 排名指示灯（5 档）

### 3.5 Mark Price

```
Mark Price = Median(Price_1, Price_2, Price_3)
Price_1 = Index Price × (1 + Funding Basis)
Price_2 = Index Price + MA(Basis, 5min)
Price_3 = Index Price
Index Price = 加权平均(多交易所现货价)
```

### 3.6 资金费率

每 8 小时结算（00:00, 08:00, 16:00 UTC）：
```
Funding Rate = clamp(Premium Index + clamp(IR - Premium Index, -0.05%, 0.05%), -0.75%, 0.75%)
Premium Index = (Impact Mid Price - Index Price) / Index Price

Impact Mid Price = (Impact Bid Price + Impact Ask Price) / 2
Impact Bid Price = 以 Impact Notional (10,000 USDT) 的市价卖单可成交的均价
Impact Ask Price = 以 Impact Notional (10,000 USDT) 的市价买单可成交的均价
注：Impact Notional 可按交易对配置，深度不足时降级为 Best Bid/Ask
Funding Fee = Position Notional × Funding Rate
```

## 4. 内部账本系统（Ledger）

### 4.1 双式记账

```rust
struct JournalEntry {
    id: EntryId,
    tx_id: TransactionId,
    debit_account: AccountId,
    credit_account: AccountId,
    amount: Decimal128,
    currency: CurrencyId,
    entry_type: EntryType, // Trade, Transfer, Fee, Funding, Liquidation, Deposit, Withdraw
    timestamp: UnixMicros,
    idempotency_key: Uuid,
}
```

### 4.2 账户体系

```
User Account
 ├── Spot Wallet (Available, Frozen)
 ├── Futures Wallet / Cross (Available, Margin, Unrealized PnL)
 ├── Isolated Margin[N] (Margin, Unrealized PnL)
 └── Funding Account (充提)

System Accounts:
 ├── Fee Collection
 ├── Insurance Fund
 ├── Funding Fee Pool
 └── Liquidation Revenue
```

### 4.3 关键资金流

**开仓**：
```
① 冻结保证金:  DR Futures.Available    → CR Futures.Margin        (amount: initial_margin)
② 手续费:     DR Futures.Available    → CR System.FeeCollection  (amount: fee)
```

**平仓盈利**（用户 A 盈利，对手方 B 亏损）：
```
① 释放 A 保证金: DR A.Futures.Margin     → CR A.Futures.Available  (amount: A.margin)
② 释放 B 保证金: DR B.Futures.Margin     → CR B.Futures.Available  (amount: B.margin - loss)
③ 盈亏转移:     DR B.Futures.Available  → CR A.Futures.Available  (amount: A.profit = B.loss)
④ 手续费:      DR A.Futures.Available  → CR System.FeeCollection (amount: A.fee)
⑤ 手续费:      DR B.Futures.Available  → CR System.FeeCollection (amount: B.fee)
注：每笔分录借贷平衡，全局 invariant 始终成立。盈利来自对手方保证金，不涉及保险基金。
```

**强平**：
```
① 冻结仓位保证金:    DR Position.Margin       → CR Liquidation.Escrow     (amount: position_margin)
② 强平成交对手方收款: DR Liquidation.Escrow     → CR Counterparty.Available (amount: bankruptcy_value)
   注：bankruptcy_value = 破产价格 × 仓位数量，即对手方正常盈利部分
③ 计算 surplus = position_margin - bankruptcy_value（即 Escrow 剩余）
   surplus > 0:      DR Liquidation.Escrow    → CR System.InsuranceFund   (盈余注入保险基金)
   surplus == 0:     Escrow 自然归零，无额外分录
   surplus < 0:      DR System.InsuranceFund  → CR Counterparty.Available (弥补穿仓差额)
④ 保险基金不足:       触发 ADL，从高盈利对手方仓位强制减仓弥补
   Escrow 在所有分录完成后必须归零，否则触发系统告警。
```

**资金费率结算**：
```
正费率（多头付空头）:
  DR Long.Futures.Available  → CR System.FundingPool   (收取)
  DR System.FundingPool      → CR Short.Futures.Available (发放)
```

### 4.4 Invariant

- 全局：sum(debits) == sum(credits)
- 用户：available >= 0
- 仓位：(size != 0) == (margin > 0)

## 5. 数据模型

### 5.1 核心实体

- **Symbol**：交易对配置（tick_size, lot_size, min_notional, max_leverage, fee, margin_tiers, status）
- **Order**：订单（id, user_id, client_order_id, symbol, side, type, time_in_force, price, stop_price, qty, filled_qty, status, reduce_only, post_only, leverage）
- **Position**：仓位（user_id, symbol, side, size, entry_price, mark_price, margin, unrealized_pnl, leverage, margin_mode, liquidation_price, adl_quantile）
- **Trade**：成交（id, symbol, price, qty, buyer/seller_order_id, buyer/seller_user_id, fees, is_buyer_maker）

ID 生成：Snowflake 算法，全局唯一。

### 5.2 数据库分层

| 数据类型 | 存储 |
|----------|------|
| 用户/账户/KYC | PostgreSQL |
| 订单/仓位 | PostgreSQL (热) + 归档 |
| 成交记录 | TimescaleDB |
| 账本流水 | PostgreSQL (事件表) |
| K线/Ticker | TimescaleDB + Redis |
| 深度快照 | Redis |
| 会话/Token | Redis |

## 6. API 层

### 6.1 REST API

对标 Binance API 风格：
- 交易：POST/DELETE/PUT /api/v1/order, GET /api/v1/openOrders, /allOrders
- 账户：GET /api/v1/account, /position
- 合约设置：POST /api/v1/leverage, /marginType, /positionMargin
- 现货：POST /api/v1/spot/order, GET /api/v1/spot/depth
- 行情：GET /api/v1/depth, /trades, /klines, /ticker/24hr, /premiumIndex, /fundingRate
- 资产：POST /api/v1/transfer, /withdraw, GET /api/v1/deposit/history

### 6.2 认证

- API Key + HMAC-SHA256 签名（与 Binance 相同）
- JWT Token（Web 前端）
- Timestamp 窗口 < 5s
- IP 白名单（可选）

### 6.3 限流

Token Bucket per API Key，不同接口不同权重，HTTP 429 + Retry-After。

### 6.4 WebSocket

```
ws://host/ws/stream
订阅频道：depth@100ms, trade, kline_1m, ticker, markPrice, forceOrder
用户数据流：ws://host/ws/{listenKey} → 余额/订单/仓位/保证金变动
```

## 7. 行情系统

Trade Events → K线聚合器(1s~1M) + Ticker聚合器(滑动24h) + 深度快照(增量diff+全量) + 最近成交缓存(1000笔)

K 线聚合策略：
- **1s K 线**：内存实时聚合（不经过 TimescaleDB），直接推送 WebSocket
- **1m 及以上**：TimescaleDB `continuous_aggregate` + `real-time aggregation` 模式（`materialized_only = false`），查询时自动合并物化数据与未物化的最新数据
- **刷新策略**：`refresh_lag = 1 minute`，接受分钟级物化延迟，实时查询由 real-time aggregation 补偿

## 8. 用户系统

- 注册/登录：Email + Argon2id
- 2FA：TOTP (Google Authenticator)
- KYC：L0(浏览) → L1(基础交易) → L2(完整功能)
- API Key：Ed25519，权限粒度控制
- 子账户：最多 200 个
- 安全：登录异常检测、设备指纹、提币地址白名单

### 8.1 子账户隔离模型

- **完全隔离**：每个子账户拥有独立的 Spot Wallet、Futures Wallet，保证金计算完全独立
- 子账户之间不共享保证金池
- 母账户可向子账户划转资金（内部 Transfer），子账户不可反向划转（需母账户授权）
- 子账户可独立设置 API Key、杠杆、保证金模式
- 用途：量化策略隔离、风控隔离、多策略并行

## 9. 钱包系统

### 9.1 充值

链上扫描器(per chain) → 确认数达标 → 入账(CR User.Funding, DR System.HotWallet)

**各链确认数标准**：

| 链 | 确认数 | 理由 |
|----|--------|------|
| Ethereum | 12 blocks (~3min) | Finality after The Merge |
| BSC | 15 blocks (~45s) | 较短出块时间 |
| Arbitrum | 12 L1 blocks | 依赖 L1 finality |
| Optimism | 12 L1 blocks | 依赖 L1 finality |
| Tron | 19 blocks (~57s) | SR 共识确认 |

**双花防护**：
- 入账幂等键：`(chain_id, tx_hash, log_index)` 三元组
- 写入前通过数据库唯一约束检查，防止重复入账
- 链重组处理：扫描器监控 reorg，已入账但被 reorg 的交易标记为 pending 重新等待确认

### 9.2 提币

User Request → 风控审核(大额人审/小额自动) → 签名队列 → 广播 → 确认

### 9.3 安全

- 热钱包余额上限，超出归集至冷钱包
- 提币限额分级（KYC 等级）
- 大额提币人工审核
- 地址白名单 + 24h 生效窗口

### 9.4 支持链

初始：EVM (ETH/BSC/ARB/OP) + Tron (TRC20-USDT)

## 10. Web 前端

### 10.1 架构

Next.js 15 App Router + React 19 + TailwindCSS + shadcn/ui + Zustand + TradingView Lightweight Charts

### 10.2 页面结构

- 交易主界面（永续/现货）：OrderBook + TradePanel + Chart + Positions + OrderList
- 账户管理：资产总览、订单历史、安全设置
- 管理后台：用户管理、风控监控、资产报表、交易对配置

### 10.3 技术要点

- WebSocket 自封装重连/心跳/订阅管理
- Canvas 渲染深度图
- TanStack Table 虚拟滚动
- 响应式设计

## 11. 可观测性

| 维度 | 工具 | 关键指标 |
|------|------|----------|
| 指标 | Prometheus + Grafana | 撮合延迟 p50/p99、TPS、深度变化率 |
| 日志 | tracing + JSON 结构化 | 订单生命周期、异常事件 |
| 链路追踪 | OpenTelemetry + Jaeger | 订单全链路 |
| 告警 | AlertManager | 撮合延迟飙升、保险基金告急、穿仓 |

## 12. 项目结构

```
exg/
├── Cargo.toml                    # Workspace root
├── crates/
│   ├── exg-common/               # 公共类型、Decimal128、错误定义
│   ├── exg-matching-engine/      # 撮合引擎核心
│   ├── exg-risk-engine/          # 风控引擎
│   ├── exg-ledger/               # 内部账本
│   ├── exg-order-service/        # 订单管理服务
│   ├── exg-clearing/             # 清算结算
│   ├── exg-market-data/          # 行情聚合
│   ├── exg-api-gateway/          # REST + WS API
│   ├── exg-user-service/         # 用户/认证/KYC
│   ├── exg-wallet-service/       # 钱包/充提
│   ├── exg-admin-service/        # 管理后台 API
│   ├── exg-ringbuffer/           # 自研无锁 Ring Buffer
│   ├── exg-wal/                  # Write-Ahead Log
│   ├── exg-protocol/             # 内部协议/消息定义(protobuf)
│   └── exg-config/               # 配置管理
├── web/
│   ├── trading/                  # 交易终端 (Next.js)
│   └── admin/                    # 管理后台 (Next.js)
├── migrations/                   # 数据库迁移
├── deploy/                       # Docker, K8s, Terraform
├── tests/                        # 集成测试 + 压测
└── docs/
```

## 13. 分阶段交付

| 阶段 | 子项目 | 核心交付物 |
|------|--------|-----------|
| P0 | 核心引擎 | 撮合引擎 + 风控引擎 + 内部账本 + Ring Buffer + WAL |
| P1 | 交易基础 | 订单管理 + 清算结算 + 行情系统 |
| P2 | 接入层 | API Gateway + 用户系统 + 认证 |
| P3 | 资产 | 钱包系统 + 资金管理 |
| P4 | 前端 | Web 交易终端 + 管理后台 |
| P5 | 运维 | 可观测性 + 部署 + 压测 |

## 14. 验证策略

### 14.1 单元测试

- 撮合引擎：OrderBook 操作、各订单类型、边界条件
- 风控引擎：保证金计算、强平价格、ADL 排名
- 账本：借贷平衡 invariant、并发安全

### 14.2 集成测试

- 完整下单→撮合→清算→结算流程
- 强平→ADL 全链路
- 资金费率结算

### 14.3 压测

- 撮合引擎延迟 benchmark（criterion）
- 全链路 TPS 压测
- WebSocket 万级连接推送

### 14.4 安全审计

- 资金相关代码 100% 覆盖
- 整数溢出/精度损失检查
- Race condition 检查
