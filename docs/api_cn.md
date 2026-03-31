# EXG REST API 参考

基础 URL：`https://api.exg.io/api/v1`

所有接口返回 JSON。Decimal 类型值以字符串编码，以保证精度。

错误响应遵循兼容币安的格式：

```json
{
  "code": -1100,
  "msg": "Invalid parameter"
}
```

## 错误码

| 错误码 | 描述 |
|--------|------|
| -1000 | 未知错误 |
| -1002 | 未授权 |
| -1015 | 请求过于频繁（限流） |
| -1100 | 参数无效 |
| -2010 | 余额不足 |
| -2013 | 订单不存在 |

## 认证

### JWT（浏览器 / 会话）

在请求头中包含：`Authorization: Bearer {token}`

### API Key HMAC（程序化访问）

必需的请求头：

| 请求头 | 描述 |
|--------|------|
| `X-EXG-APIKEY` | 你的 API Key ID |
| `X-EXG-SIGNATURE` | 使用 Secret Key 对 `{timestamp}{method}{path}{body}` 进行 HMAC-SHA256 签名 |
| `X-EXG-TIMESTAMP` | 当前时间戳（毫秒） |

时间戳必须在服务器时间的 10 秒以内。

---

## 交易接口

### POST /api/v1/order -- 下单

创建新订单。

**请求体：**

```json
{
  "symbol": "BTCUSDT",
  "side": "BUY",
  "order_type": "LIMIT",
  "time_in_force": "GTC",
  "quantity": "1.5",
  "price": "50000",
  "stop_price": null,
  "reduce_only": false,
  "client_order_id": "12345"
}
```

| 参数 | 类型 | 是否必需 | 描述 |
|------|------|----------|------|
| symbol | string | 是 | 交易对（如 "BTCUSDT"） |
| side | string | 是 | "BUY" 或 "SELL" |
| order_type | string | 是 | "LIMIT"、"MARKET"、"STOP_MARKET"、"STOP_LIMIT"、"TAKE_PROFIT_MARKET"、"TAKE_PROFIT_LIMIT"、"TRAILING_STOP"、"ICEBERG" |
| time_in_force | string | 否 | "GTC"（默认）、"IOC"、"FOK"、"GTD"、"POST_ONLY"。市价单默认为 "IOC" |
| quantity | string | 是 | 订单数量，decimal 字符串 |
| price | string | 条件必需 | 限价类订单必填 |
| stop_price | string | 否 | 条件单的触发价格 |
| reduce_only | bool | 否 | 为 true 时仅减少现有仓位 |
| client_order_id | string | 否 | 用户自定义订单 ID（数字字符串） |

**响应（200）：**

```json
{
  "order_id": "1234567890123456",
  "client_order_id": "12345",
  "symbol": "BTCUSDT",
  "side": "BUY",
  "order_type": "LIMIT",
  "quantity": "1.5",
  "price": "50000",
  "status": "NEW",
  "timestamp": 1700000000000
}
```

### DELETE /api/v1/order -- 撤单

取消现有订单。

**请求体：**

```json
{
  "symbol": "BTCUSDT",
  "order_id": "1234567890123456",
  "client_order_id": null
}
```

| 参数 | 类型 | 是否必需 | 描述 |
|------|------|----------|------|
| symbol | string | 是 | 交易对 |
| order_id | string | 条件必需 | 服务端分配的订单 ID |
| client_order_id | string | 条件必需 | 用户自定义订单 ID。order_id 和 client_order_id 二选一 |

**响应（200）：**

```json
{
  "order_id": "1234567890123456",
  "symbol": "BTCUSDT",
  "status": "CANCELED",
  "timestamp": 1700000000001
}
```

### GET /api/v1/openOrders -- 查询活跃订单

获取当前认证用户的所有活跃订单。

**查询参数：**

| 参数 | 类型 | 是否必需 | 描述 |
|------|------|----------|------|
| symbol | string | 否 | 按交易对筛选 |

**响应（200）：**

```json
[
  {
    "order_id": "1234567890123456",
    "client_order_id": "12345",
    "symbol": "BTCUSDT",
    "side": "BUY",
    "order_type": "LIMIT",
    "quantity": "1.5",
    "price": "50000",
    "status": "NEW",
    "timestamp": 1700000000000
  }
]
```

### GET /api/v1/allOrders -- 查询历史订单

获取订单历史（包括已成交、已取消、已拒绝）。

**查询参数：**

| 参数 | 类型 | 是否必需 | 描述 |
|------|------|----------|------|
| symbol | string | 是 | 交易对 |
| limit | int | 否 | 返回数量（默认 500，最大 1000） |
| start_time | int | 否 | 起始时间戳（毫秒） |
| end_time | int | 否 | 结束时间戳（毫秒） |

**响应（200）：** 与 openOrders 相同的 schema，包含所有状态类型。

---

## 账户接口

### GET /api/v1/account -- 查询账户信息

**响应（200）：**

```json
{
  "user_id": "42",
  "balances": [
    {
      "wallet_type": "FUTURES",
      "available": "10000.5",
      "frozen": "500",
      "margin": "2000"
    },
    {
      "wallet_type": "FUNDING",
      "available": "5000",
      "frozen": "0",
      "margin": "0"
    }
  ]
}
```

### GET /api/v1/position -- 查询仓位

**查询参数：**

| 参数 | 类型 | 是否必需 | 描述 |
|------|------|----------|------|
| symbol | string | 否 | 按交易对筛选 |

**响应（200）：**

```json
[
  {
    "symbol": "BTCUSDT",
    "side": "LONG",
    "size": "1.5",
    "entry_price": "50000",
    "mark_price": "51000",
    "unrealized_pnl": "1500",
    "leverage": "10",
    "margin_mode": "CROSS"
  }
]
```

### POST /api/v1/leverage -- 设置杠杆

**请求体：**

```json
{
  "symbol": "BTCUSDT",
  "leverage": 20
}
```

**响应（200）：**

```json
{
  "symbol": "BTCUSDT",
  "leverage": 20
}
```

### POST /api/v1/transfer -- 内部划转

在钱包之间划转（如从资金账户到合约账户）。

**请求体：**

```json
{
  "from_wallet": "FUNDING",
  "to_wallet": "FUTURES",
  "amount": "1000"
}
```

| 参数 | 类型 | 是否必需 | 描述 |
|------|------|----------|------|
| from_wallet | string | 是 | "SPOT"、"FUTURES"、"FUNDING" |
| to_wallet | string | 是 | "SPOT"、"FUTURES"、"FUNDING" |
| amount | string | 是 | 划转金额，decimal 字符串 |

**响应（200）：**

```json
{
  "status": "ok",
  "timestamp": 1700000000000
}
```

---

## 行情数据接口

### GET /api/v1/depth -- 订单簿深度

**查询参数：**

| 参数 | 类型 | 是否必需 | 描述 |
|------|------|----------|------|
| symbol | string | 是 | 交易对 |
| limit | int | 否 | 深度档位：5、10、20（默认）、50、100 |

**响应（200）：**

```json
{
  "symbol": "BTCUSDT",
  "bids": [
    ["50000.00", "1.5"],
    ["49999.50", "3.2"]
  ],
  "asks": [
    ["50000.50", "0.8"],
    ["50001.00", "2.1"]
  ],
  "timestamp": 1700000000000
}
```

每条记录为 `[price, quantity]` 字符串数组。

### GET /api/v1/trades -- 最近成交

**查询参数：**

| 参数 | 类型 | 是否必需 | 描述 |
|------|------|----------|------|
| symbol | string | 是 | 交易对 |
| limit | int | 否 | 返回成交数量（默认 100，最大 1000） |

**响应（200）：**

```json
[
  {
    "trade_id": "9876543210",
    "symbol": "BTCUSDT",
    "price": "50000.50",
    "qty": "0.5",
    "side": "BUY",
    "timestamp": 1700000000000
  }
]
```

### GET /api/v1/klines -- K 线数据

**查询参数：**

| 参数 | 类型 | 是否必需 | 描述 |
|------|------|----------|------|
| symbol | string | 是 | 交易对 |
| interval | string | 是 | "1m"、"5m"、"15m"、"30m"、"1h"、"4h"、"1d"、"1w" |
| limit | int | 否 | K 线数量（默认 500，最大 1500） |
| start_time | int | 否 | 起始时间戳（毫秒） |
| end_time | int | 否 | 结束时间戳（毫秒） |

**响应（200）：**

```json
[
  {
    "open_time": 1700000000000,
    "open": "50000",
    "high": "50500",
    "low": "49800",
    "close": "50200",
    "volume": "1234.5",
    "close_time": 1700000060000
  }
]
```

### GET /api/v1/ticker -- 24 小时行情

**查询参数：**

| 参数 | 类型 | 是否必需 | 描述 |
|------|------|----------|------|
| symbol | string | 否 | 不指定则返回所有交易对 |

**响应（200）：**

```json
{
  "symbol": "BTCUSDT",
  "last_price": "50200",
  "mark_price": "50195.5",
  "index_price": "50190",
  "high_24h": "51000",
  "low_24h": "49000",
  "volume_24h": "12345.67",
  "price_change_pct": "1.23"
}
```

---

## 认证接口

### POST /api/v1/auth/register -- 注册

**请求体：**

```json
{
  "email": "user@example.com",
  "password": "secure_password_here"
}
```

**响应（201）：**

```json
{
  "user_id": "42",
  "email": "user@example.com"
}
```

### POST /api/v1/auth/login -- 登录

**请求体：**

```json
{
  "email": "user@example.com",
  "password": "secure_password_here",
  "totp_code": "123456"
}
```

`totp_code` 仅在启用 2FA 时必填。

**响��（200）：**

```json
{
  "token": "eyJhbGciOiJIUzI1NiIs...",
  "expires_at": 1700086400000
}
```

---

## WebSocket 数据流

### 连接

```
公共数据流：wss://api.exg.io/ws/stream
私有数据流：wss://api.exg.io/ws/{listenKey}
```

### 订阅

```json
{
  "method": "SUBSCRIBE",
  "params": ["btcusdt@depth20", "btcusdt@trade"],
  "id": 1
}
```

**响应：**

```json
{
  "result": null,
  "id": 1
}
```

### 取消订阅

```json
{
  "method": "UNSUBSCRIBE",
  "params": ["btcusdt@depth20"],
  "id": 2
}
```

### 数据流：@depth{N}

订单簿快照，前 N 档有变动时推送。

```json
{
  "stream": "btcusdt@depth20",
  "data": {
    "symbol": "BTCUSDT",
    "bids": [["50000.00", "1.5"], ["49999.50", "3.2"]],
    "asks": [["50000.50", "0.8"], ["50001.00", "2.1"]],
    "timestamp": 1700000000000
  }
}
```

### 数据流：@trade

实时成交推送。

```json
{
  "stream": "btcusdt@trade",
  "data": {
    "trade_id": "9876543210",
    "symbol": "BTCUSDT",
    "price": "50000.50",
    "qty": "0.5",
    "side": "BUY",
    "timestamp": 1700000000000
  }
}
```

### 数据流：@kline_{interval}

K 线更新（如 `btcusdt@kline_1m`）。

```json
{
  "stream": "btcusdt@kline_1m",
  "data": {
    "open_time": 1700000000000,
    "open": "50000",
    "high": "50500",
    "low": "49800",
    "close": "50200",
    "volume": "1234.5",
    "close_time": 1700000060000,
    "is_closed": false
  }
}
```

### 数据流：@ticker

24 小时滚动窗口行情统计。

```json
{
  "stream": "btcusdt@ticker",
  "data": {
    "symbol": "BTCUSDT",
    "last_price": "50200",
    "mark_price": "50195.5",
    "index_price": "50190",
    "high_24h": "51000",
    "low_24h": "49000",
    "volume_24h": "12345.67",
    "price_change_pct": "1.23"
  }
}
```

### 私有用户数据流

通过 `POST /api/v1/listenKey` 获取 listen key 后连接。接收以下推送：

- 订单更新（已接受、已成交、已取消、已拒绝、已过期）
- 仓位更新（开仓、平仓、强平）
- 余额更新（充值、提现、结算）

```json
{
  "event": "ORDER_UPDATE",
  "data": {
    "order_id": "1234567890123456",
    "symbol": "BTCUSDT",
    "side": "BUY",
    "status": "FILLED",
    "fill_price": "50000",
    "fill_qty": "1.5",
    "timestamp": 1700000000000
  }
}
```
