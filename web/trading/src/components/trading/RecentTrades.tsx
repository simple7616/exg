"use client";

import { useMarketStore } from "@/stores/useMarketStore";
import { formatPrice, formatQty, cn } from "@/lib/utils";

export default function RecentTrades() {
  const { trades } = useMarketStore();

  return (
    <div className="flex flex-col h-full bg-card rounded overflow-hidden">
      <div className="flex items-center px-3 py-2 border-b border-border">
        <span className="text-xs text-secondary font-medium">Recent Trades</span>
      </div>

      <div className="grid grid-cols-3 px-3 py-1 text-[10px] text-secondary">
        <span>Price(USDT)</span>
        <span className="text-right">Qty(BTC)</span>
        <span className="text-right">Time</span>
      </div>

      <div className="flex-1 overflow-y-auto">
        {trades.map((trade) => {
          const time = new Date(trade.time);
          const ts = `${time.getHours().toString().padStart(2, "0")}:${time
            .getMinutes()
            .toString()
            .padStart(2, "0")}:${time.getSeconds().toString().padStart(2, "0")}`;

          return (
            <div
              key={trade.id}
              className="grid grid-cols-3 px-3 py-[1px] text-[11px] font-mono hover:bg-white/5"
            >
              <span className={cn(trade.isBuyerMaker ? "text-red" : "text-green")}>
                {formatPrice(trade.price)}
              </span>
              <span className="text-right text-primary">{formatQty(trade.qty)}</span>
              <span className="text-right text-secondary">{ts}</span>
            </div>
          );
        })}
      </div>
    </div>
  );
}
