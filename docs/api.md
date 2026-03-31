# EXG REST API Reference

Base URL: `https://api.exg.io/api/v1`

All endpoints return JSON. Decimal values are encoded as strings to preserve precision.

Error responses follow Binance-compatible format:

```json
{
  "code": -1100,
  "msg": "Invalid parameter"
}
```

## Error Codes

| Code | Description |
|------|-------------|
| -1000 | Unknown error |
| -1002 | Unauthorized |
| -1015 | Too many requests (rate limited) |
| -1100 | Invalid parameter |
| -2010 | Insufficient balance |
| -2013 | Order not found |

## Authentication

### JWT (Browser/Session)

Include in header: `Authorization: Bearer {token}`

### API Key HMAC (Programmatic)

Required headers:

| Header | Description |
|--------|-------------|
| `X-EXG-APIKEY` | Your API key ID |
| `X-EXG-SIGNATURE` | HMAC-SHA256 of `{timestamp}{method}{path}{body}` using your secret key |
| `X-EXG-TIMESTAMP` | Current timestamp in milliseconds |

Timestamp must be within 10 seconds of server time.

---

## Trading Endpoints

### POST /api/v1/order -- Place Order

Place a new order.

**Request Body:**

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

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| symbol | string | Yes | Trading pair (e.g. "BTCUSDT") |
| side | string | Yes | "BUY" or "SELL" |
| order_type | string | Yes | "LIMIT", "MARKET", "STOP_MARKET", "STOP_LIMIT", "TAKE_PROFIT_MARKET", "TAKE_PROFIT_LIMIT", "TRAILING_STOP", "ICEBERG" |
| time_in_force | string | No | "GTC" (default), "IOC", "FOK", "GTD", "POST_ONLY". Market orders default to "IOC" |
| quantity | string | Yes | Order quantity as decimal string |
| price | string | Conditional | Required for LIMIT-type orders |
| stop_price | string | No | Trigger price for conditional orders |
| reduce_only | bool | No | If true, only reduces existing position |
| client_order_id | string | No | User-defined order ID (numeric string) |

**Response (200):**

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

### DELETE /api/v1/order -- Cancel Order

Cancel an existing order.

**Request Body:**

```json
{
  "symbol": "BTCUSDT",
  "order_id": "1234567890123456",
  "client_order_id": null
}
```

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| symbol | string | Yes | Trading pair |
| order_id | string | Conditional | Server-assigned order ID |
| client_order_id | string | Conditional | User-defined order ID. One of order_id or client_order_id required |

**Response (200):**

```json
{
  "order_id": "1234567890123456",
  "symbol": "BTCUSDT",
  "status": "CANCELED",
  "timestamp": 1700000000001
}
```

### GET /api/v1/openOrders -- Get Open Orders

Retrieve all active orders for the authenticated user.

**Query Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| symbol | string | No | Filter by symbol |

**Response (200):**

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

### GET /api/v1/allOrders -- Get All Orders

Retrieve order history (including filled, canceled, rejected).

**Query Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| symbol | string | Yes | Trading pair |
| limit | int | No | Number of results (default 500, max 1000) |
| start_time | int | No | Start timestamp in milliseconds |
| end_time | int | No | End timestamp in milliseconds |

**Response (200):** Same schema as openOrders, with all status types included.

---

## Account Endpoints

### GET /api/v1/account -- Get Account Info

**Response (200):**

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

### GET /api/v1/position -- Get Positions

**Query Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| symbol | string | No | Filter by symbol |

**Response (200):**

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

### POST /api/v1/leverage -- Set Leverage

**Request Body:**

```json
{
  "symbol": "BTCUSDT",
  "leverage": 20
}
```

**Response (200):**

```json
{
  "symbol": "BTCUSDT",
  "leverage": 20
}
```

### POST /api/v1/transfer -- Internal Transfer

Transfer between wallets (e.g. Funding to Futures).

**Request Body:**

```json
{
  "from_wallet": "FUNDING",
  "to_wallet": "FUTURES",
  "amount": "1000"
}
```

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| from_wallet | string | Yes | "SPOT", "FUTURES", "FUNDING" |
| to_wallet | string | Yes | "SPOT", "FUTURES", "FUNDING" |
| amount | string | Yes | Transfer amount as decimal string |

**Response (200):**

```json
{
  "status": "ok",
  "timestamp": 1700000000000
}
```

---

## Market Data Endpoints

### GET /api/v1/depth -- Order Book

**Query Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| symbol | string | Yes | Trading pair |
| limit | int | No | Depth levels: 5, 10, 20 (default), 50, 100 |

**Response (200):**

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

Each entry is `[price, quantity]` as string arrays.

### GET /api/v1/trades -- Recent Trades

**Query Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| symbol | string | Yes | Trading pair |
| limit | int | No | Number of trades (default 100, max 1000) |

**Response (200):**

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

### GET /api/v1/klines -- Kline/Candlestick Data

**Query Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| symbol | string | Yes | Trading pair |
| interval | string | Yes | "1m", "5m", "15m", "30m", "1h", "4h", "1d", "1w" |
| limit | int | No | Number of klines (default 500, max 1500) |
| start_time | int | No | Start timestamp in milliseconds |
| end_time | int | No | End timestamp in milliseconds |

**Response (200):**

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

### GET /api/v1/ticker -- 24h Ticker

**Query Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| symbol | string | No | If omitted, returns all symbols |

**Response (200):**

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

## Auth Endpoints

### POST /api/v1/auth/register

**Request Body:**

```json
{
  "email": "user@example.com",
  "password": "secure_password_here"
}
```

**Response (201):**

```json
{
  "user_id": "42",
  "email": "user@example.com"
}
```

### POST /api/v1/auth/login

**Request Body:**

```json
{
  "email": "user@example.com",
  "password": "secure_password_here",
  "totp_code": "123456"
}
```

`totp_code` is required only if 2FA is enabled.

**Response (200):**

```json
{
  "token": "eyJhbGciOiJIUzI1NiIs...",
  "expires_at": 1700086400000
}
```

---

## WebSocket Streams

### Connection

```
Public:  wss://api.exg.io/ws/stream
Private: wss://api.exg.io/ws/{listenKey}
```

### Subscribe

```json
{
  "method": "SUBSCRIBE",
  "params": ["btcusdt@depth20", "btcusdt@trade"],
  "id": 1
}
```

**Response:**

```json
{
  "result": null,
  "id": 1
}
```

### Unsubscribe

```json
{
  "method": "UNSUBSCRIBE",
  "params": ["btcusdt@depth20"],
  "id": 2
}
```

### Stream: @depth{N}

Order book snapshot, pushed on every change to the top N levels.

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

### Stream: @trade

Real-time trade feed.

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

### Stream: @kline_{interval}

Candlestick updates (e.g. `btcusdt@kline_1m`).

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

### Stream: @ticker

24h rolling window ticker statistics.

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

### Private User Stream

Connect with listen key obtained from `POST /api/v1/listenKey`. Receives:

- Order updates (accepted, filled, canceled, rejected, expired)
- Position updates (open, close, liquidation)
- Balance updates (deposit, withdrawal, settlement)

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
