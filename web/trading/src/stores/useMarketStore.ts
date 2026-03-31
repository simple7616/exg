import { create } from "zustand";
import type { Ticker, Symbol as ExgSymbol, Kline, OrderBook, Trade, KlineInterval } from "@/lib/types";
import {
  generateTicker,
  generateKlines,
  generateOrderBook,
  generateTrades,
  BTCUSDT_SYMBOL,
} from "@/lib/mock-data";

interface MarketState {
  symbol: ExgSymbol;
  ticker: Ticker;
  klines: Kline[];
  orderBook: OrderBook;
  trades: Trade[];
  interval: KlineInterval;
  setInterval: (interval: KlineInterval) => void;
  init: () => void;
}

export const useMarketStore = create<MarketState>((set) => ({
  symbol: BTCUSDT_SYMBOL,
  ticker: generateTicker(),
  klines: [],
  orderBook: generateOrderBook(),
  trades: [],
  interval: "1h",
  setInterval: (interval) => set({ interval }),
  init: () =>
    set({
      klines: generateKlines(200),
      orderBook: generateOrderBook(),
      trades: generateTrades(50),
    }),
}));
