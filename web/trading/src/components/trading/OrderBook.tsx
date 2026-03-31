"use client";

import { useMarketStore } from "@/stores/useMarketStore";
import { formatPrice, formatQty } from "@/lib/utils";

export default function OrderBook() {
  const { orderBook } = useMarketStore();
  const maxTotal = Math.max(
    orderBook.asks.length > 0 ? orderBook.asks[orderBook.asks.length - 1].total : 0,
    orderBook.bids.length > 0 ? orderBook.bids[orderBook.bids.length - 1].total : 0
  );

  return (
    <div className="flex flex-col h-full bg-card rounded overflow-hidden">
      {/* Header */}
      <div className="flex items-center px-3 py-2 border-b border-border">
        <span className="text-xs text-secondary font-medium">Order Book</span>
      </div>

      {/* Column headers */}
      <div className="grid grid-cols-3 px-3 py-1 text-[10px] text-secondary">
        <span>Price(USDT)</span>
        <span className="text-right">Qty(BTC)</span>
        <span className="text-right">Total</span>
      </div>

      {/* Asks */}
      <div className="flex-1 overflow-hidden flex flex-col justify-end px-1">
        {orderBook.asks.map((entry, i) => (
          <div
            key={`ask-${i}`}
            className="relative grid grid-cols-3 px-2 py-[1px] text-[11px] font-mono hover:bg-white/5"
          >
            <div
              className="absolute inset-y-0 right-0 bg-red/10"
              style={{ width: `${(entry.total / maxTotal) * 100}%` }}
            />
            <span className="relative text-red">{formatPrice(entry.price)}</span>
            <span className="relative text-right text-primary">{formatQty(entry.qty)}</span>
            <span className="relative text-right text-secondary">{formatQty(entry.total)}</span>
          </div>
        ))}
      </div>

      {/* Spread */}
      <div className="flex items-center justify-between px-3 py-1.5 border-y border-border bg-card">
        <span className="font-mono text-sm text-primary font-semibold">
          {formatPrice(orderBook.bids[0]?.price ?? 0)}
        </span>
        <span className="text-[10px] text-secondary">
          Spread: {formatPrice(orderBook.spread)} ({orderBook.spreadPercent.toFixed(4)}%)
        </span>
      </div>

      {/* Bids */}
      <div className="flex-1 overflow-hidden px-1">
        {orderBook.bids.map((entry, i) => (
          <div
            key={`bid-${i}`}
            className="relative grid grid-cols-3 px-2 py-[1px] text-[11px] font-mono hover:bg-white/5"
          >
            <div
              className="absolute inset-y-0 right-0 bg-green/10"
              style={{ width: `${(entry.total / maxTotal) * 100}%` }}
            />
            <span className="relative text-green">{formatPrice(entry.price)}</span>
            <span className="relative text-right text-primary">{formatQty(entry.qty)}</span>
            <span className="relative text-right text-secondary">{formatQty(entry.total)}</span>
          </div>
        ))}
      </div>
    </div>
  );
}
