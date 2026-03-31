"use client";

import { useAccountStore } from "@/stores/useAccountStore";
import { formatPrice, formatPnl, formatPercent, pnlColor, cn } from "@/lib/utils";

export default function PositionList() {
  const { positions } = useAccountStore();

  if (positions.length === 0) {
    return (
      <div className="flex items-center justify-center h-full text-secondary text-sm">
        No open positions
      </div>
    );
  }

  return (
    <div className="overflow-x-auto">
      <table className="w-full text-xs">
        <thead>
          <tr className="text-secondary border-b border-border">
            <th className="text-left px-3 py-2 font-medium">Symbol</th>
            <th className="text-left px-3 py-2 font-medium">Side</th>
            <th className="text-right px-3 py-2 font-medium">Size</th>
            <th className="text-right px-3 py-2 font-medium">Entry Price</th>
            <th className="text-right px-3 py-2 font-medium">Mark Price</th>
            <th className="text-right px-3 py-2 font-medium">Liq. Price</th>
            <th className="text-right px-3 py-2 font-medium">Leverage</th>
            <th className="text-right px-3 py-2 font-medium">Margin</th>
            <th className="text-right px-3 py-2 font-medium">PnL (ROE%)</th>
            <th className="text-right px-3 py-2 font-medium">Actions</th>
          </tr>
        </thead>
        <tbody>
          {positions.map((pos) => (
            <tr key={`${pos.symbol}-${pos.side}`} className="border-b border-border/50 hover:bg-white/[0.02]">
              <td className="px-3 py-2 font-medium text-primary">{pos.symbol}</td>
              <td className="px-3 py-2">
                <span className={cn(pos.side === "LONG" ? "text-green" : "text-red", "font-medium")}>
                  {pos.side}
                </span>
              </td>
              <td className="px-3 py-2 text-right font-mono text-primary">{pos.size}</td>
              <td className="px-3 py-2 text-right font-mono text-primary">{formatPrice(pos.entryPrice)}</td>
              <td className="px-3 py-2 text-right font-mono text-primary">{formatPrice(pos.markPrice)}</td>
              <td className="px-3 py-2 text-right font-mono text-secondary">{formatPrice(pos.liquidationPrice)}</td>
              <td className="px-3 py-2 text-right font-mono text-accent">{pos.leverage}x</td>
              <td className="px-3 py-2 text-right font-mono text-primary">{formatPrice(pos.margin)}</td>
              <td className={cn("px-3 py-2 text-right font-mono", pnlColor(pos.unrealizedPnl))}>
                {formatPnl(pos.unrealizedPnl)} ({formatPercent(pos.unrealizedPnlPercent)})
              </td>
              <td className="px-3 py-2 text-right">
                <button className="text-secondary hover:text-primary text-[10px] border border-border rounded px-2 py-0.5 mr-1">
                  TP/SL
                </button>
                <button className="text-secondary hover:text-red text-[10px] border border-border rounded px-2 py-0.5">
                  Close
                </button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
