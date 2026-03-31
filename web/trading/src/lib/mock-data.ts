import type {
  Kline,
  OrderBook,
  OrderBookEntry,
  Trade,
  Position,
  Order,
  Balance,
  Ticker,
  Symbol,
} from "./types";

// --- Seeded PRNG for deterministic mock data ---
function mulberry32(seed: number) {
  return function () {
    let t = (seed += 0x6d2b79f5);
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const rand = mulberry32(42);

// --- Symbol ---
export const BTCUSDT_SYMBOL: Symbol = {
  symbol: "BTCUSDT",
  baseAsset: "BTC",
  quoteAsset: "USDT",
  pricePrecision: 2,
  qtyPrecision: 3,
  tickSize: 0.01,
  lotSize: 0.001,
};

// --- Klines ---
export function generateKlines(count = 200): Kline[] {
  const klines: Kline[] = [];
  const now = Math.floor(Date.now() / 1000);
  const interval = 3600; // 1h candles
  let price = 65200;

  for (let i = 0; i < count; i++) {
    const time = now - (count - i) * interval;
    const change = (rand() - 0.48) * 400;
    const open = price;
    const close = open + change;
    const high = Math.max(open, close) + rand() * 200;
    const low = Math.min(open, close) - rand() * 200;
    const volume = 50 + rand() * 500;

    klines.push({
      time,
      open: Math.round(open * 100) / 100,
      high: Math.round(high * 100) / 100,
      low: Math.round(low * 100) / 100,
      close: Math.round(close * 100) / 100,
      volume: Math.round(volume * 1000) / 1000,
    });

    price = close;
  }

  return klines;
}

// --- Order Book ---
export function generateOrderBook(): OrderBook {
  const midPrice = 66432.5;
  const asks: OrderBookEntry[] = [];
  const bids: OrderBookEntry[] = [];

  let askTotal = 0;
  for (let i = 0; i < 20; i++) {
    const price = midPrice + 0.5 + i * (0.5 + rand() * 2);
    const qty = 0.01 + rand() * 2;
    askTotal += qty;
    asks.push({
      price: Math.round(price * 100) / 100,
      qty: Math.round(qty * 1000) / 1000,
      total: Math.round(askTotal * 1000) / 1000,
    });
  }

  let bidTotal = 0;
  for (let i = 0; i < 20; i++) {
    const price = midPrice - 0.5 - i * (0.5 + rand() * 2);
    const qty = 0.01 + rand() * 2;
    bidTotal += qty;
    bids.push({
      price: Math.round(price * 100) / 100,
      qty: Math.round(qty * 1000) / 1000,
      total: Math.round(bidTotal * 1000) / 1000,
    });
  }

  const spread = asks[0].price - bids[0].price;
  const spreadPercent = (spread / midPrice) * 100;

  return {
    asks: asks.reverse(),
    bids,
    spread: Math.round(spread * 100) / 100,
    spreadPercent: Math.round(spreadPercent * 10000) / 10000,
  };
}

// --- Recent Trades ---
export function generateTrades(count = 50): Trade[] {
  const trades: Trade[] = [];
  const now = Date.now();
  let price = 66432.5;

  for (let i = 0; i < count; i++) {
    const change = (rand() - 0.5) * 5;
    price = price + change;
    const qty = 0.001 + rand() * 1.5;
    const isBuyerMaker = rand() > 0.5;

    trades.push({
      id: `t${now - i}`,
      price: Math.round(price * 100) / 100,
      qty: Math.round(qty * 1000) / 1000,
      quoteQty: Math.round(price * qty * 100) / 100,
      time: now - i * (1000 + Math.floor(rand() * 5000)),
      isBuyerMaker,
    });
  }

  return trades;
}

// --- Positions ---
export function generatePositions(): Position[] {
  return [
    {
      symbol: "BTCUSDT",
      side: "LONG",
      size: 0.5,
      entryPrice: 65800.0,
      markPrice: 66432.5,
      liquidationPrice: 58200.0,
      leverage: 10,
      margin: 3290.0,
      unrealizedPnl: 316.25,
      unrealizedPnlPercent: 9.61,
    },
    {
      symbol: "ETHUSDT",
      side: "SHORT",
      size: 5.0,
      entryPrice: 3520.0,
      markPrice: 3485.6,
      liquidationPrice: 3960.0,
      leverage: 20,
      margin: 880.0,
      unrealizedPnl: 172.0,
      unrealizedPnlPercent: 19.55,
    },
    {
      symbol: "SOLUSDT",
      side: "LONG",
      size: 100.0,
      entryPrice: 145.2,
      markPrice: 142.8,
      liquidationPrice: 120.5,
      leverage: 5,
      margin: 2904.0,
      unrealizedPnl: -240.0,
      unrealizedPnlPercent: -8.26,
    },
  ];
}

// --- Orders ---
export function generateOrders(): { active: Order[]; history: Order[] } {
  const now = Date.now();

  const active: Order[] = [
    {
      id: "o1001",
      symbol: "BTCUSDT",
      side: "BUY",
      type: "LIMIT",
      status: "NEW",
      price: 65000.0,
      qty: 0.1,
      filledQty: 0,
      avgPrice: 0,
      leverage: 10,
      time: now - 3600000,
      updateTime: now - 3600000,
    },
    {
      id: "o1002",
      symbol: "BTCUSDT",
      side: "SELL",
      type: "LIMIT",
      status: "NEW",
      price: 68000.0,
      qty: 0.2,
      filledQty: 0,
      avgPrice: 0,
      leverage: 10,
      time: now - 7200000,
      updateTime: now - 7200000,
    },
    {
      id: "o1003",
      symbol: "ETHUSDT",
      side: "BUY",
      type: "STOP_LIMIT",
      status: "NEW",
      price: 3400.0,
      stopPrice: 3420.0,
      qty: 2.0,
      filledQty: 0,
      avgPrice: 0,
      leverage: 20,
      time: now - 1800000,
      updateTime: now - 1800000,
    },
    {
      id: "o1004",
      symbol: "BTCUSDT",
      side: "BUY",
      type: "LIMIT",
      status: "PARTIALLY_FILLED",
      price: 66200.0,
      qty: 0.5,
      filledQty: 0.15,
      avgPrice: 66200.0,
      leverage: 10,
      time: now - 900000,
      updateTime: now - 600000,
    },
    {
      id: "o1005",
      symbol: "SOLUSDT",
      side: "SELL",
      type: "LIMIT",
      status: "NEW",
      price: 155.0,
      qty: 50.0,
      filledQty: 0,
      avgPrice: 0,
      leverage: 5,
      time: now - 300000,
      updateTime: now - 300000,
    },
  ];

  const history: Order[] = [
    {
      id: "o0991",
      symbol: "BTCUSDT",
      side: "BUY",
      type: "MARKET",
      status: "FILLED",
      price: 0,
      qty: 0.5,
      filledQty: 0.5,
      avgPrice: 65800.0,
      leverage: 10,
      time: now - 86400000,
      updateTime: now - 86400000,
    },
    {
      id: "o0992",
      symbol: "ETHUSDT",
      side: "SELL",
      type: "LIMIT",
      status: "FILLED",
      price: 3520.0,
      qty: 5.0,
      filledQty: 5.0,
      avgPrice: 3520.0,
      leverage: 20,
      time: now - 86400000 * 2,
      updateTime: now - 86400000 * 2,
    },
    {
      id: "o0993",
      symbol: "BTCUSDT",
      side: "SELL",
      type: "LIMIT",
      status: "CANCELED",
      price: 70000.0,
      qty: 0.3,
      filledQty: 0,
      avgPrice: 0,
      leverage: 10,
      time: now - 86400000 * 3,
      updateTime: now - 86400000 * 2.5,
    },
    {
      id: "o0994",
      symbol: "SOLUSDT",
      side: "BUY",
      type: "MARKET",
      status: "FILLED",
      price: 0,
      qty: 100.0,
      filledQty: 100.0,
      avgPrice: 145.2,
      leverage: 5,
      time: now - 86400000 * 4,
      updateTime: now - 86400000 * 4,
    },
    {
      id: "o0995",
      symbol: "BTCUSDT",
      side: "BUY",
      type: "LIMIT",
      status: "FILLED",
      price: 64500.0,
      qty: 0.2,
      filledQty: 0.2,
      avgPrice: 64500.0,
      leverage: 10,
      time: now - 86400000 * 5,
      updateTime: now - 86400000 * 5,
    },
    {
      id: "o0996",
      symbol: "ETHUSDT",
      side: "BUY",
      type: "STOP_LIMIT",
      status: "EXPIRED",
      price: 3200.0,
      stopPrice: 3210.0,
      qty: 3.0,
      filledQty: 0,
      avgPrice: 0,
      leverage: 15,
      time: now - 86400000 * 6,
      updateTime: now - 86400000 * 5.5,
    },
    {
      id: "o0997",
      symbol: "BTCUSDT",
      side: "SELL",
      type: "MARKET",
      status: "FILLED",
      price: 0,
      qty: 0.1,
      filledQty: 0.1,
      avgPrice: 67200.0,
      leverage: 10,
      time: now - 86400000 * 7,
      updateTime: now - 86400000 * 7,
    },
    {
      id: "o0998",
      symbol: "SOLUSDT",
      side: "SELL",
      type: "LIMIT",
      status: "CANCELED",
      price: 160.0,
      qty: 30.0,
      filledQty: 0,
      avgPrice: 0,
      leverage: 5,
      time: now - 86400000 * 8,
      updateTime: now - 86400000 * 7.5,
    },
    {
      id: "o0999",
      symbol: "BTCUSDT",
      side: "BUY",
      type: "LIMIT",
      status: "FILLED",
      price: 63000.0,
      qty: 0.4,
      filledQty: 0.4,
      avgPrice: 63000.0,
      leverage: 10,
      time: now - 86400000 * 9,
      updateTime: now - 86400000 * 9,
    },
    {
      id: "o1000",
      symbol: "ETHUSDT",
      side: "SELL",
      type: "MARKET",
      status: "FILLED",
      price: 0,
      qty: 2.0,
      filledQty: 2.0,
      avgPrice: 3580.0,
      leverage: 20,
      time: now - 86400000 * 10,
      updateTime: now - 86400000 * 10,
    },
  ];

  return { active, history };
}

// --- Balances ---
export function generateBalances(): Balance[] {
  return [
    {
      asset: "USDT",
      walletBalance: 52340.8,
      availableBalance: 45266.8,
      unrealizedPnl: 248.25,
      marginBalance: 52589.05,
    },
    {
      asset: "BTC",
      walletBalance: 1.245,
      availableBalance: 0.745,
      unrealizedPnl: 0.00476,
      marginBalance: 1.24976,
    },
    {
      asset: "ETH",
      walletBalance: 15.8,
      availableBalance: 10.8,
      unrealizedPnl: 0.0493,
      marginBalance: 15.8493,
    },
  ];
}

// --- Ticker ---
export function generateTicker(): Ticker {
  return {
    symbol: "BTCUSDT",
    lastPrice: 66432.5,
    priceChange: 1132.5,
    priceChangePercent: 1.73,
    high24h: 67180.0,
    low24h: 64890.0,
    volume24h: 28456.32,
    quoteVolume24h: 1889432100.0,
    markPrice: 66435.2,
    indexPrice: 66430.8,
    fundingRate: 0.0001,
    nextFundingTime: Date.now() + 3600000 * 4,
  };
}
