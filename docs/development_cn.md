# EXG 开发指南

## 代码仓库结构

```
exg/
├── crates/                    # Rust workspace crate（16 个 crate）
├── web/trading/               # 交易前端（Next.js 15）
├── web/admin/                 # 管理后台（Next.js 15）
├── config/                    # TOML 配置
├── deploy/                    # Docker、K8s、Prometheus、Grafana、Terraform
├── migrations/                # PostgreSQL + TimescaleDB SQL 迁移
├── scripts/                   # 开发/测试/构建/Lint 脚本
├── tests/e2e/                 # 端到端集成测试
├── tests/load/                # 压力测试
├── Cargo.toml                 # Workspace 根配置
├── Cargo.lock                 # 依赖锁定文件
├── rustfmt.toml               # 格式化配置：max_width=100
├── docker-compose.yml         # 本地基础设施
├── Dockerfile                 # 服务端镜像（多阶段构建）
├── Dockerfile.trading         # 交易前端镜像
└── Dockerfile.admin           # 管理后台镜像
```

## Workspace 配置

- **Rust edition**：2024
- **Resolver**：version 2
- **Release profile**：`opt-level=3`、`lto=fat`、`codegen-units=1`、`panic=abort`
- **Bench profile**：`opt-level=3`、`lto=thin`

所有依赖在 `[workspace.dependencies]` 中声明，各 crate 通过 `{ workspace = true }` 引用。

---

## 添加新 Crate

1. 创建 crate 目录：
```bash
cargo new crates/exg-my-crate --lib
```

2. 添加到 `Cargo.toml` workspace members：
```toml
[workspace]
members = [
    # ... 已有 crate
    "crates/exg-my-crate",
]
```

3. 在 crate 的 `Cargo.toml` 中添加 workspace 依赖：
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

4. 遵循现有模式：
   - `lib.rs` 重导出公共 API
   - 测试放在源文件中的 `#[cfg(test)] mod tests` 块内
   - 使用 `thiserror` 定义错误类型
   - 金融计算一律使用 `Decimal128`

---

## 测试规范

### 结构

测试以内联方式放在每个源文件的 `#[cfg(test)] mod tests` 块中。保持测试与被测代码的物理邻近。

### 命名

```rust
#[test]
fn test_{function_name}_{scenario}() {
    // ...
}
```

示例：
- `test_initial_margin_basic`
- `test_funding_rate_clamped_upper`
- `test_liquidation_deficit`

### Decimal 辅助函数

每个使用 `Decimal128` 的测试模块包含：
```rust
fn dec(s: &str) -> Decimal128 {
    s.parse().unwrap()
}
```

### 金融精度测试

金融计算必须使用**已知精确值**进行测试，不允许近似比较：

```rust
#[test]
fn test_maintenance_margin_tier2() {
    let tiers = binance_btc_tiers();
    // 100000 * 0.005 - 50 = 450
    let result = calc_maintenance_margin(dec("100000"), &tiers);
    assert_eq!(result, dec("450"));
}
```

### 不变量验证

账本测试在每组操作序列后调用 `verify_all_invariants()`，确保全局余额等式成立：

```rust
ledger.deposit(user, dec("1000"), "dep-1", ts(1)).unwrap();
ledger.verify_all_invariants().unwrap();
```

### 运行测试

```bash
# 全部测试
cargo test --workspace

# 指定 crate
cargo test -p exg-matching-engine

# 指定测试
cargo test -p exg-risk-engine test_funding_rate_clamped

# 显示输出
cargo test --workspace -- --nocapture

# 完整测试套件（含 Lint + 可选前端测试）
scripts/test.sh --all --verbose
```

---

## 代码风格

### 格式化

在 `rustfmt.toml` 中配置：
```toml
max_width = 100
use_field_init_shorthand = true
use_try_shorthand = true
```

运行：`cargo fmt`
检查：`cargo fmt --check`

### Clippy

零警告策略：`cargo clippy --workspace -- -D warnings`

### 命名规范

| 条目 | 规范 | 示例 |
|------|------|------|
| 类型 | PascalCase | `OrderBook`、`MatchingEngine` |
| 函数 | snake_case | `calc_initial_margin` |
| 常量 | SCREAMING_SNAKE | `RECORD_OVERHEAD`、`SCALE` |
| Newtype ID | PascalCase 包装 | `OrderId(u64)`、`UserId(u64)` |
| 模块文件 | snake_case | `pre_trade.rs`、`risk_monitor.rs` |

### 错误处理

- **库 crate**（`exg-common`、`exg-risk-engine` 等）：使用 `thiserror` 定义 `ExgError` 枚举
- **应用 crate**（`exg-server`）：使用 `anyhow` 进行顶层错误聚合
- **API 层**：使用 `ApiError` 结构体，携带兼容币安的错误码

### 序列化策略

| 上下文 | 库 | 原因 |
|--------|---|------|
| Ring Buffer（热路径） | rkyv | 零拷贝反序列化，无内存分配 |
| WAL 存储 | 原始字节 | 直接载荷追加，CRC32 包装 |
| REST API / JSON | serde + serde_json | 标准 JSON 序列化 |
| 数据库 | sqlx | 原生 PostgreSQL 类型 |
| 配置 | serde + TOML | 人类可读的配置文件 |

### Hash Map 选型

- **`FxHashMap`**（来自 `rustc-hash`）：非加密哈希，用于内部查找（订单簿、用户订单、限流桶）
- **`HashMap`**（标准库）：用于对外暴露或需持久化的数据（账本账户、配置）

### ID 类型

始终使用 Newtype 包装，禁止裸整数：

```rust
// 正确
fn get_order(&self, order_id: OrderId) -> Option<&BookOrder>

// 错误——语义不明确，无类型安全
fn get_order(&self, order_id: u64) -> Option<&BookOrder>
```

---

## 基准测试指南

基准测试使用 `criterion` crate。运行方式：

```bash
# 全部基准测试
scripts/bench.sh

# 指定套件
scripts/bench.sh decimal    # Decimal128 算术
scripts/bench.sh matching   # 撮合引擎吞吐量
scripts/bench.sh ringbuffer # Ring Buffer push/pop
scripts/bench.sh wal        # WAL 追加/读取

# 直接使用 cargo
cargo bench -p exg-matching-engine --bench matching
```

结果保存在 `target/criterion/`，包含 HTML 报告。

---

## 常见开发工作流

### 添加新订单类型

1. 在 `exg-common/src/types.rs` 中添加 `OrderType` 变体
2. 更新 `is_conditional()` 和 `is_limit()` 方法
3. 在 `exg-matching-engine/src/engine.rs`（`handle_new_order`）中添加处理逻辑
4. 在 `exg-protocol/src/lib.rs` 中添加 serde 测试覆盖
5. 在 `exg-api-gateway/src/conversion.rs` 中添加 API 转换

### 添加新 API 接口

1. 在 `exg-api-gateway/src/types.rs` 中定义请求/响应类型
2. 在 `exg-api-gateway/src/conversion.rs` 中添加转换函数
3. 如需要，在 `exg-api-gateway/src/error.rs` 中添加错误类型
4. 更新 `docs/api.md` 文档
5. 添加覆盖校验、正常路径和错误场景的测试

### 添加新风控检查

1. 在 `exg-risk-engine` 的相应模块中添加纯函数
2. 在撮合引擎中接入交易前置检查链
3. 在 `exg-protocol/src/event.rs` 中添加对应的 `RejectReason` 变体
4. 使用真实市场数据和已知值进行测试

### 添加新账本操作

1. 在 `exg-ledger/src/operations.rs` 的 `Ledger` 中添加方法
2. 创建相应的 `JournalEntry` 记录（借方 + 贷方）
3. 处理幂等键
4. 每次操作后调用 `verify_all_invariants()` 进行测试
5. 验证失败操作可重试

---

## 调试技巧

### WAL 检查

WAL reader 可以导出所有记录：
```rust
let mut reader = WalReader::open(Path::new("./data/wal")).unwrap();
reader.read_from(0, |seq, payload| {
    println!("seq={seq} len={}", payload.len());
    true
}).unwrap();
```

### Ring Buffer 监控

Ring Buffer 通过原子读取暴露 head/tail 位置。监控 `tail - head` 是否接近 `slot_count` 以检测背压。

### Decimal128 精度调试

调试金融计算时，使用 `Decimal128::raw()` 检查内部 i128 表示：
```rust
let val = dec("0.1") + dec("0.2");
println!("raw={} display={}", val.raw(), val);
// raw=300000000000000000 display=0.3
```

### 订单簿状态

撮合引擎暴露 `orderbook()` 用于检查：
```rust
let (bids, asks) = engine.orderbook().depth(10);
println!("Best bid: {:?}", engine.orderbook().best_bid());
println!("Best ask: {:?}", engine.orderbook().best_ask());
println!("Orders on book: {}", engine.orderbook().order_count());
```

---

## Edition 2024 注意事项

Rust edition 2024 将 `gen` 保留为关键字。禁止使用 `gen` 作为变量或函数名。替代方案：

- `sf` 或 `id_gen` 用于 Snowflake 生成器
- `generator` 用于通用生成器
- `rng` 用于随机数生成器

---

## 贡献指南

1. **从 main 分支创建分支**：功能分支命名为 `feature/{description}`，修复分支命名为 `fix/{description}`
2. **测试全通过**：`cargo test --workspace` 必须零失败
3. **Lint 无警告**：`cargo clippy --workspace -- -D warnings` 必须零警告
4. **格式化通过**：`cargo fmt --check` 必须通过
5. **公共 API 文档化**：所有 `pub` 函数需要 doc comment
6. **金融精度**：decimal 计算必须有已知精确值的精度测试
7. **不变量测试**：账本操作每次变更后必须验证不变量
8. **禁止 f64 表示金额**：所有金融数值使用 `Decimal128`
