"use client";

import { useState } from "react";
import { cn, formatPrice, formatQty } from "@/lib/utils";
import type { OrderSide, OrderType, OrderStatus } from "@/lib/types";

interface OrderRow {
  id: string;
  symbol: string;
  side: OrderSide;
  type: OrderType;
  status: OrderStatus;
  price: number;
  qty: number;
  filledQty: number;
  avgPrice: number;
  time: number;
}

const now = Date.now();

const mockOrders: OrderRow[] = [
  { id: "o1001", symbol: "BTCUSDT", side: "BUY", type: "LIMIT", status: "FILLED", price: 65000, qty: 0.1, filledQty: 0.1, avgPrice: 65000, time: now - 86400000 },
  { id: "o1002", symbol: "BTCUSDT", side: "SELL", type: "LIMIT", status: "CANCELED", price: 68000, qty: 0.2, filledQty: 0, avgPrice: 0, time: now - 86400000 * 2 },
  { id: "o1003", symbol: "ETHUSDT", side: "BUY", type: "MARKET", status: "FILLED", price: 0, qty: 5.0, filledQty: 5.0, avgPrice: 3520, time: now - 86400000 * 3 },
  { id: "o1004", symbol: "BTCUSDT", side: "BUY", type: "LIMIT", status: "PARTIALLY_FILLED", price: 66200, qty: 0.5, filledQty: 0.15, avgPrice: 66200, time: now - 86400000 * 4 },
  { id: "o1005", symbol: "SOLUSDT", side: "SELL", type: "LIMIT", status: "FILLED", price: 155, qty: 50, filledQty: 50, avgPrice: 155, time: now - 86400000 * 5 },
  { id: "o1006", symbol: "BTCUSDT", side: "BUY", type: "STOP_LIMIT", status: "EXPIRED", price: 63000, qty: 0.3, filledQty: 0, avgPrice: 0, time: now - 86400000 * 6 },
  { id: "o1007", symbol: "ETHUSDT", side: "SELL", type: "MARKET", status: "FILLED", price: 0, qty: 2.0, filledQty: 2.0, avgPrice: 3580, time: now - 86400000 * 7 },
  { id: "o1008", symbol: "SOLUSDT", side: "BUY", type: "LIMIT", status: "NEW", price: 140, qty: 100, filledQty: 0, avgPrice: 0, time: now - 86400000 * 8 },
];

interface TradeRow {
  id: string;
  symbol: string;
  side: OrderSide;
  price: number;
  qty: number;
  fee: number;
  time: number;
}

const mockTrades: TradeRow[] = [
  { id: "t1", symbol: "BTCUSDT", side: "BUY", price: 65000, qty: 0.1, fee: 3.25, time: now - 86400000 },
  { id: "t2", symbol: "ETHUSDT", side: "BUY", price: 3520, qty: 5.0, fee: 8.80, time: now - 86400000 * 3 },
  { id: "t3", symbol: "BTCUSDT", side: "BUY", price: 66200, qty: 0.15, fee: 4.97, time: now - 86400000 * 4 },
  { id: "t4", symbol: "SOLUSDT", side: "SELL", price: 155, qty: 50, fee: 3.88, time: now - 86400000 * 5 },
  { id: "t5", symbol: "ETHUSDT", side: "SELL", price: 3580, qty: 2.0, fee: 3.58, time: now - 86400000 * 7 },
];

type Tab = "orders" | "trades";
const allSymbols = ["All", "BTCUSDT", "ETHUSDT", "SOLUSDT"];
const allStatuses: ("All" | OrderStatus)[] = ["All", "NEW", "PARTIALLY_FILLED", "FILLED", "CANCELED", "EXPIRED"];

export default function OrdersPage() {
  const [tab, setTab] = useState<Tab>("orders");
  const [symbolFilter, setSymbolFilter] = useState("All");
  const [statusFilter, setStatusFilter] = useState<"All" | OrderStatus>("All");

  const filteredOrders = mockOrders.filter((o) => {
    if (symbolFilter !== "All" && o.symbol !== symbolFilter) return false;
    if (statusFilter !== "All" && o.status !== statusFilter) return false;
    return true;
  });

  const filteredTrades = mockTrades.filter((t) => {
    if (symbolFilter !== "All" && t.symbol !== symbolFilter) return false;
    return true;
  });

  function formatTime(ts: number) {
    return new Date(ts).toLocaleString("en-US", { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
  }

  function statusBadge(status: OrderStatus) {
    const colors: Record<OrderStatus, string> = {
      NEW: "bg-blue-500/10 text-blue-400",
      PARTIALLY_FILLED: "bg-yellow-500/10 text-yellow-400",
      FILLED: "bg-green/10 text-green",
      CANCELED: "bg-white/5 text-secondary",
      EXPIRED: "bg-white/5 text-secondary",
    };
    return <span className={cn("text-xs px-2 py-0.5 rounded", colors[status])}>{status}</span>;
  }

  return (
    <div className="space-y-4 max-w-6xl">
      {/* Tabs */}
      <div className="flex items-center gap-4 border-b border-border">
        {(["orders", "trades"] as const).map((t) => (
          <button
            key={t}
            onClick={() => setTab(t)}
            className={cn(
              "py-2 text-sm font-medium border-b-2 transition-colors capitalize",
              tab === t ? "border-accent text-primary" : "border-transparent text-secondary hover:text-primary"
            )}
          >
            {t === "orders" ? "Order History" : "Trade History"}
          </button>
        ))}
      </div>

      {/* Filters */}
      <div className="flex items-center gap-3">
        <select
          value={symbolFilter}
          onChange={(e) => setSymbolFilter(e.target.value)}
          className="bg-white/5 border border-border rounded px-2 py-1.5 text-sm text-primary"
        >
          {allSymbols.map((s) => <option key={s} value={s}>{s}</option>)}
        </select>
        {tab === "orders" && (
          <select
            value={statusFilter}
            onChange={(e) => setStatusFilter(e.target.value as "All" | OrderStatus)}
            className="bg-white/5 border border-border rounded px-2 py-1.5 text-sm text-primary"
          >
            {allStatuses.map((s) => <option key={s} value={s}>{s}</option>)}
          </select>
        )}
      </div>

      {/* Order History Table */}
      {tab === "orders" && (
        <div className="bg-card rounded border border-border overflow-x-auto">
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-border text-secondary text-xs">
                <th className="text-left px-4 py-2 font-medium">Time</th>
                <th className="text-left px-4 py-2 font-medium">Symbol</th>
                <th className="text-left px-4 py-2 font-medium">Side</th>
                <th className="text-left px-4 py-2 font-medium">Type</th>
                <th className="text-right px-4 py-2 font-medium">Price</th>
                <th className="text-right px-4 py-2 font-medium">Qty</th>
                <th className="text-right px-4 py-2 font-medium">Filled</th>
                <th className="text-right px-4 py-2 font-medium">Avg Price</th>
                <th className="text-left px-4 py-2 font-medium">Status</th>
              </tr>
            </thead>
            <tbody>
              {filteredOrders.map((o) => (
                <tr key={o.id} className="border-b border-border/50 hover:bg-white/[0.02]">
                  <td className="px-4 py-2 text-secondary text-xs">{formatTime(o.time)}</td>
                  <td className="px-4 py-2 text-primary font-medium">{o.symbol}</td>
                  <td className={cn("px-4 py-2", o.side === "BUY" ? "text-green" : "text-red")}>{o.side}</td>
                  <td className="px-4 py-2 text-primary">{o.type}</td>
                  <td className="px-4 py-2 text-right font-mono text-primary">{o.price > 0 ? formatPrice(o.price) : "Market"}</td>
                  <td className="px-4 py-2 text-right font-mono text-primary">{formatQty(o.qty)}</td>
                  <td className="px-4 py-2 text-right font-mono text-primary">{formatQty(o.filledQty)}</td>
                  <td className="px-4 py-2 text-right font-mono text-primary">{o.avgPrice > 0 ? formatPrice(o.avgPrice) : "-"}</td>
                  <td className="px-4 py-2">{statusBadge(o.status)}</td>
                </tr>
              ))}
              {filteredOrders.length === 0 && (
                <tr><td colSpan={9} className="px-4 py-8 text-center text-secondary">No orders found</td></tr>
              )}
            </tbody>
          </table>
        </div>
      )}

      {/* Trade History Table */}
      {tab === "trades" && (
        <div className="bg-card rounded border border-border overflow-x-auto">
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-border text-secondary text-xs">
                <th className="text-left px-4 py-2 font-medium">Time</th>
                <th className="text-left px-4 py-2 font-medium">Symbol</th>
                <th className="text-left px-4 py-2 font-medium">Side</th>
                <th className="text-right px-4 py-2 font-medium">Price</th>
                <th className="text-right px-4 py-2 font-medium">Qty</th>
                <th className="text-right px-4 py-2 font-medium">Fee</th>
              </tr>
            </thead>
            <tbody>
              {filteredTrades.map((t) => (
                <tr key={t.id} className="border-b border-border/50 hover:bg-white/[0.02]">
                  <td className="px-4 py-2 text-secondary text-xs">{formatTime(t.time)}</td>
                  <td className="px-4 py-2 text-primary font-medium">{t.symbol}</td>
                  <td className={cn("px-4 py-2", t.side === "BUY" ? "text-green" : "text-red")}>{t.side}</td>
                  <td className="px-4 py-2 text-right font-mono text-primary">{formatPrice(t.price)}</td>
                  <td className="px-4 py-2 text-right font-mono text-primary">{formatQty(t.qty)}</td>
                  <td className="px-4 py-2 text-right font-mono text-secondary">{t.fee.toFixed(2)} USDT</td>
                </tr>
              ))}
              {filteredTrades.length === 0 && (
                <tr><td colSpan={6} className="px-4 py-8 text-center text-secondary">No trades found</td></tr>
              )}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
