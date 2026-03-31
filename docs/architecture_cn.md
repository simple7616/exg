# EXG 架构设计

## 系统概览

EXG 是一个中心化永续合约 + 现货交易所，采用 LMAX 交易所架构模式。核心设计原则是**单写者事件溯源模型**——所有状态变更均通过运行在独占 CPU 绑核线程上的确定性撮合引擎完成。

### 数据流

```
Client Request
     │
     ▼
API Gateway (Actix-web)
     │  校验、认证、限流
     ▼
Command 序列化 (rkyv 零拷贝)
     │
     ▼
SPSC Ring Buffer (mmap, 无锁)
     │
     ▼
Matching Engine (单写者线程, CPU 绑核)
     │  确定性状态转换
     ▼
WAL 追加写入 (CRC32 完整性校验)
     │
     ├──▶ Clearing Service    (仓位 + 结算 + 资金费率)
     ├──▶ Market Data Service  (K 线、深度、行情、成交)
     ├──▶ Order Service        (订单生命周期、用户通知)
     └──▶ NATS JetStream      (事件分发至下游消费者)
```

### 设计原则

1. **单写者确定性** -- 所有订单簿变更发生在同一线程内。无锁、无竞争、无竞态条件。给定相同的输入序列，引擎产生完全一致的输出。

2. **事件溯源** -- WAL 是唯一的事实来源（Source of Truth）。所有状态均可通过重放 WAL 中的事件重建。快照是性能优化手段，而非必要条件。

3. **热路径零拷贝** -- Command 通过 Ring Buffer 以 rkyv 序列化格式传输（零拷贝反序列化）。API 网关到撮合引擎的关键路径上无内存分配。

4. **纯函数风控计算** -- 所有风控引擎函数均为纯函数（无 I/O、无状态变更）。接收输入，返回结果。这使得风控计算天然可测试、可审计，且可在读路径上并行执行。

5. **复式记账** -- 每一笔余额变动均记录为带有借方和贷方的日记账分录。全局不变量在任意时点均可机械化验证。

---

## 撮合引擎

### 单写者线程模型

撮合引擎（`exg-matching-engine`）运行在独占 CPU 核心上，通过 `core_affinity::set_for_current()` 实现与操作系统调度器的隔离。引擎以 busy-spin 循环从输入 Ring Buffer 读取 Command，确定性地处理每条 Command，并将产生的 Event 写入 WAL。

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

### 订单簿结构

每个交易对拥有独立的 `OrderBook`：

- **买单（Bids）**：`BTreeMap<Reverse<Decimal128>, PriceLevel>` -- 价格降序排列
- **卖单（Asks）**：`BTreeMap<Decimal128, PriceLevel>` -- 价格升序排列
- **订单索引**：`FxHashMap<OrderId, BookOrder>` -- O(1) 按 ID 查找
- **用户订单**：`FxHashMap<UserId, Vec<OrderId>>` -- O(1) 全部撤单

每个 `PriceLevel` 包含一个 `Vec<OrderId>`，维护 FIFO 插入顺序以实现同价位时间优先。

### 撮合算法（价格-时间优先）

1. 买单与卖单（最低价优先）匹配
2. 卖单与买单（最高价优先）匹配
3. 同一价格层级内，按 FIFO 顺序匹配
4. 成交始终以 **Maker 价格**执行（Taker 获得价格改善）

### 订单类型

| 类型 | 行为 |
|------|------|
| Limit | 挂单，按指定价格挂在订单簿上 |
| Market | 以最优可得价格成交，剩余部分取消 |
| StopMarket | 触发价到达时触发，以市价单执行 |
| StopLimit | 触发价到达时触发，以指定价格挂限价单 |
| TakeProfitMarket | 与 StopMarket 触发方向相反 |
| TakeProfitLimit | 与 StopLimit 触发方向相反 |
| TrailingStop | 追踪峰值价格，回撤达到指定幅度时触发 |
| Iceberg | 大单拆分为可见切片，成交后自动补充 |

### 有效期策略

| 策略 | 行为 |
|------|------|
| GTC | Good-Till-Canceled -- 保持有效直到成交或手动撤单 |
| IOC | Immediate-Or-Cancel -- 尽可能成交，剩余立即取消 |
| FOK | Fill-Or-Kill -- 要么全部成交，要么整单拒绝（预校验） |
| GTD | Good-Till-Date -- 到达指定时间戳后自动过期 |
| PostOnly | 若会立即成交则拒绝（仅做 Maker） |

### 条件单流程

Stop、止盈和追踪止损订单排入独立的 `stop_orders` 列表。每次 `update_mark_price()` 调用时：

1. 更新追踪峰值价格（卖单取高水位，买单取低水位）
2. 检查所有 Stop 订单是否满足当前标记价格的触发条件
3. 已触发的订单转换为 Market/Limit 并送入订单簿处理

### 快照与恢复

引擎支持 `take_snapshot()` 和 `restore_from_snapshot()`。快照捕获以下内容：
- 订单簿上所有挂单
- 所有待执行条件单
- 标记价格 / 指数价格
- WAL 序列号
- GTD 过期堆条目

恢复流程：加载最新快照，然后从快照序列号开始重放后续 WAL 事件。

---

## Ring Buffer 协议

### SPSC mmap 环形缓冲区（`exg-ringbuffer`）

Ring Buffer 通过匿名 mmap 提供无锁的单生产者-单消费者通信，连接 API 网关与撮合引擎。

### 内存布局

```
Offset 0:    [head: AtomicU64]  [padding to 128 bytes]    ← 消费者写入
Offset 128:  [tail: AtomicU64]  [padding to 128 bytes]    ← 生产者写入
Offset 256:  [slot_count: u64]  [slot_size: u64]          ← 元数据
Offset 512:  [Slot 0] [Slot 1] ... [Slot N-1]             ← 数据区域

每个 Slot：
  [msg_len: u32 LE] [payload bytes] [padding to slot_size]
```

关键特性：
- head 和 tail 指针之间 **128 字节缓存行隔离**，消除伪共享（False Sharing）
- `slot_count` 必须为 2 的幂，以实现位掩码索引（`index = seq & (count - 1)`）
- 生产者对 tail 写入使用 `Ordering::Release`；消费者对 tail 读取使用 `Ordering::Acquire`
- 背压机制：当 `tail - head >= slot_count` 时，生产者收到 `WouldBlock`

### 配置

默认值：65,536 个 Slot x 4,096 字节 = 256 MB Ring Buffer。

---

## WAL 格式与崩溃恢复

### 预写日志（`exg-wal`）

WAL 为所有引擎事件提供持久化的顺序存储，并具备 CRC32 完整性校验。

### 记录格式

```
┌─────────┬──────────────┬─────────────────┬──────────┐
│ seq: u64│ len: u32 LE  │ payload: [u8]   │ crc: u32 │
│ (8 B)   │ (4 B)        │ (variable)      │ (4 B)    │
└─────────┴──────────────┴─────────────────┴──────────┘
         12 字节头部              4 字节尾部
```

- **序列号**：单调递增，全局唯一
- **CRC32**：基于 `[seq_bytes | len_bytes | payload_bytes]` 计算

### 段文件

WAL 数据拆分为段文件，命名格式为 `wal-{first_sequence:020}.log`。当段文件超过配置的大小阈值时触发轮转（默认：64 MB）。轮转后的段文件不可变。

### 崩溃恢复流程

1. **按顺序扫描所有段文件**，验证每条记录的 CRC32
2. **检测部分写入**：如果最后一个段文件的末尾记录存在不完整的头部或 CRC 不匹配，则将文件截断到最后一条有效记录的边界
3. **序列号连续性检查**：非尾部位置的序列号间断为致命错误（`SequenceGap`）
4. **已封存段中的 CRC 错误**：致命错误（`Corrupt`），确认数据丢失
5. **恢复写入**：写入器从最后一条有效记录之后的 `next_sequence` 重新打开

### 快照管理

快照存储为 `snapshot-{sequence:020}.snap`，附带 CRC32 尾部校验。通过临时文件 + `fsync` + 重命名 + 目录 `fsync` 实现原子写入。仅保留最新的 3 个快照；旧快照自动清理。

---

## Decimal128 定点数算术

### 为什么不用 f64？

IEEE 754 双精度浮点数无法准确表示 `0.1 + 0.2 == 0.3`。在金融系统中，手续费计算、保证金计算和盈亏结算中的累积舍入误差会导致余额差异，违反复式记账不变量。对于处理真实资金的系统，这是不可接受的。

### 为什么不用 rust_decimal？

`rust_decimal` 底层是 96 位尾数配合可变精度因子（0-28）。虽然计算正确，但对本系统存在以下不足：

1. **可变精度** -- 运算可能改变精度，算术运算后需要归一化处理
2. **不支持 rkyv** -- 热路径上的零拷贝序列化要求 `rkyv::Archive` 派生
3. **性能** -- 可变精度的除法算法慢于定点精度除法

### Decimal128 设计

`Decimal128` 使用固定 18 位小数精度，底层为 `i128`：

```
raw_value = real_value * 10^18
范围：整数部分约 +/- 1.7 * 10^20
```

关键特性：
- **精确表示**：`0.1 + 0.2 == 0.3` 始终成立
- **固定精度**：无需精度追踪或归一化
- **18 位小数**：足以表示任何现实价格（BTC 在 $100,000 时保留 8 位小数 = 14 位数字）
- **宽位算术**：当 `a * b` 溢出 i128 时，使用四个 64 位半乘积进行 256 位中间乘法，再执行 256 位 / 128 位除法
- **rkyv 零拷贝**：派生 `rkyv::Archive, rkyv::Serialize, rkyv::Deserialize`
- **serde 字符串序列化**：在 JSON 中序列化为 `"123.456"`（无浮点歧义）

### Checked 与 Unchecked

运算符 trait（`+`、`-`、`*`、`/`）在 debug 构建中溢出时 panic，release 构建中回绕。对于绝不允许 panic 的代码，使用 `checked_add`、`checked_sub`、`checked_mul`、`checked_div`，返回 `Option<Decimal128>`。

---

## 风控引擎

### 纯函数设计（`exg-risk-engine`）

所有风控引擎函数均为无状态纯函数。接收仓位数据、配置和价格作为输入，返回计算结果。无 I/O，无数据库访问，无可变状态。此设计：

- 使每一项计算均可通过已知值进行**单元测试**
- 支持读路径上的**并行风控检查**
- 保证从撮合引擎线程调用时的**确定性结果**
- 简化**审计**——每个函数的行为自包含

### 保证金计算

**初始保证金**：`notional / leverage`

**维持保证金**（阶梯式）：
```
对于满足 notional_floor <= notional < notional_cap 的梯度：
    maintenance_margin = notional * mmr - maintenance_amount
```

梯度按交易对配置（兼容币安的梯度保证金体系）。`maintenance_amount` 字段是累积调整值，使阶梯费率保持连续。

**强平价格**（线性永续合约）：
```
多头：liq_price = entry * (1 - 1/leverage + mmr) - accumulated_funding / size
空头：liq_price = entry * (1 + 1/leverage - mmr) + accumulated_funding / size
```

**保证金率**：`total_maintenance_margin / equity`，其中 `equity = wallet_balance + sum(unrealized_pnl)`。当 equity 为零或负值时返回 `Decimal128::MAX`。

### 交易前置检查

在下单前执行：

1. **仓位限额**：总名义价值（现有仓位 + 新订单）不得超过 `max_position_notional`
2. **价格偏离带**：`|order_price - mark_price| / mark_price <= band_pct`（默认 5%）
3. **自成交防范**：如果用户在同一交易对的对手方已有挂单，则拒绝
4. **频率限制**：按用户的下单和撤单频率，对照可配置阈值进行检查

---

## 资金费率计算

### 冲击价格模型

资金费率使用**冲击中间价**模型（兼容币安）：

1. **冲击买价**：在买方深度上成交 `impact_notional` 金额的 VWAP
2. **冲击卖价**：在卖方深度上成交 `impact_notional` 金额的 VWAP
3. **冲击中间价**：`(impact_bid + impact_ask) / 2`
4. **溢价指数**：`(impact_mid - index_price) / index_price`
5. **资金费率**：`clamp(premium_index + interest_rate, -0.75%, +0.75%)`

默认利率：0.01%（0.0001）。资金费率结算周期：8 小时。

### 资金费用

```
funding_fee = position_size * mark_price * funding_rate
```

- 正资金费率：多头支付，空头收取
- 负资金费率：空头支付，多头收取
- 约定：`calc_funding_fee` 返回正值表示用户需支付

### 资金费率的账本结算

`settle_funding_checked` 方法：
1. 优先从可用余额扣除
2. 可用余额不足时，从保证金中扣除（提示强平风险）
3. 如果动用了保证金，返回 `true`（调用方须触发强平检查）
4. 使用结构化幂等键：`funding_{period}_{user}_{symbol}`

---

## 强平、保险基金与 ADL 级联

### 强平触发条件

当 `margin_ratio >= 1.0`（维持保证金达到或超过净值）时，触发仓位强平。

### 级联流程

```
1. 触发强平
   │
   ├── surplus > 0 → 保证金剩余部分划入保险基金
   │
   └── surplus < 0（亏损） → 保险基金弥补缺口
       │
       └── 保险基金耗尽 → 激活 ADL（自动减仓）
```

### ADL 排序

ADL 优先级分数：`(unrealized_pnl / margin) * (notional / margin)`

此公式将盈亏百分比与杠杆因子相乘，确保盈利最多、杠杆最高的对手方优先被减仓。按分数降序排列；最高分用户最先被减仓。

### 保险基金记账

保险基金是账本中的系统账户。操作如下：
- **盈余**：`escrow -> insurance_fund`（强平后有剩余保证金）
- **亏损**：`insurance_fund -> escrow`（强平亏损超过保证金）
- **耗尽**：返回 `ExgError::InsuranceFundDepleted`，触发 ADL

---

## 标记价格模型

标记价格使用**三源中位数**方法（可配置）：

1. 交易所最新成交价
2. 外部指数价格（主要交易所的加权平均价）
3. 冲击中间价的移动平均

### 陈旧数据处理

如果标记价格数据超过配置的陈旧阈值，引擎将以 `RejectReason::MarkPriceStale` 拒绝新订单，并暂停强平处理，直到收到新鲜数据。这可以防止基于陈旧或被操纵价格的强平。

---

## 账本复式记账

### 模型（`exg-ledger`）

每一笔余额变动均记录为 `JournalEntry`，包含：
- 借方：`(user, wallet, field)` -- 余额减少
- 贷方：`(user, wallet, field)` -- 余额增加
- 金额（始终为正）
- 幂等键（防止重复处理）
- 分录类型分类

### 钱包类型

| 钱包 | 用途 |
|------|------|
| Spot | 现货交易余额 |
| Futures | 永续合约保证金与可用余额 |
| Funding | 充提中转钱包 |
| InsuranceFund | 系统：强平盈余资金池 |
| FeeCollection | 系统：累积手续费 |
| Escrow | 系统：过渡结算中转 |

### 余额子字段

每个钱包包含三个子字段：
- **available**：可自由使用的余额
- **frozen**：为挂单锁定的余额
- **margin**：作为仓位抵押品锁定的余额

### 不变量

1. **子字段非负**：所有用户钱包的 `available >= 0 && frozen >= 0 && margin >= 0`
2. **系统账户非负**：InsuranceFund、FeeCollection、Escrow 必须 >= 0
3. **全局余额等式**：`sum(all_user_balances) + sum(all_system_balances) == net_deposits - net_withdrawals`
4. **幂等性**：重复的幂等键静默接受（不会重复处理）
5. **失败操作可重试**：操作失败时移除幂等键，使相同键可以重新使用

### 操作流程示例（交易结算）

```
1. freeze_for_order:   available -> frozen         （下单）
2. open_position:      frozen -> margin + fee      （成交）
3. close_position:     margin -> available + pnl   （平仓）
   - 盈利：对手方 margin 借记
   - 亏损：对手方 margin 贷记
   - 手续费：从 available 扣除，贷记至 FeeCollection
```

---

## API 认证流程

### JWT 认证

1. 用户使用邮箱 + 密码注册（Argon2 哈希）
2. 登录返回可配置过期时间的 JWT Token
3. 可选的 TOTP 2FA，通过 `totp-rs`（二维码配置）
4. JWT Token 通过 `Authorization: Bearer {token}` 请求头传递

### API Key HMAC 认证

供程序化访问使用：

1. 用户通过已认证的接口创建 API Key
2. 每个 Key 具有权限控制：`can_trade`、`can_withdraw`、`can_read`
3. 可选的按 Key 设置 IP 白名单
4. 请求签名：对 `timestamp + method + path + body` 进行 HMAC-SHA256 签名
5. 请求头：`X-EXG-APIKEY`、`X-EXG-SIGNATURE`、`X-EXG-TIMESTAMP`
6. 时间戳校验：请求必须在可配置的时间窗口内（默认 10 秒）

### 限流

基于令牌桶算法，按 API Key 限流：
- 可配置的最大令牌数和补充速率
- 每个 Key 独立桶
- 超限时返回兼容币安的错误码 `-1015`

---

## WebSocket 订阅模型

### 连接

```
wss://api.exg.io/ws/stream          # 公共行情数据
wss://api.exg.io/ws/{listenKey}     # 私有用户数据流
```

### 订阅 / 取消订阅

```json
{"method": "SUBSCRIBE", "params": ["btcusdt@depth20", "ethusdt@trade"], "id": 1}
{"method": "UNSUBSCRIBE", "params": ["btcusdt@depth20"], "id": 2}
```

### 数据流名称

格式：`{symbol}@{channel}`

| 频道 | 数据内容 |
|------|----------|
| `@depth{N}` | 订单簿快照（前 N 档） |
| `@trade` | 实时成交流 |
| `@kline_{interval}` | K 线更新（1m、5m、15m、1h、4h、1d） |
| `@ticker` | 24 小时行情统计 |

### 订阅管理器

内存中维护双向映射：
- `client_id -> Set<stream_name>`（客户端订阅了哪些数据流）
- `stream_name -> Set<client_id>`（数据流需要推送给哪些客户端）

客户端断开连接时，通过 `remove_client()` 清理所有订阅。
