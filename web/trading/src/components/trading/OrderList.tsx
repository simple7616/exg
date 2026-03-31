"use client";

import { useState } from "react";
import { useAccountStore } from "@/stores/useAccountStore";
import { formatPrice, formatQty, cn } from "@/lib/utils";
import type { Order } from "@/lib/types";

function OrderRow({ order }: { order: Order }) {
  const time = new Date(order.time);
  const dateStr = `${time.getMonth() + 1}/${time.getDate()} ${time
    .getHours()
    .toString()
    .padStart(2, "0")}:${time.getMinutes().toString().padStart(2, "0")}`;

  return (
    <tr className="border-b border-border/50 hover:bg-white/[0.02] text-xs">
      <td className="px-3 py-2 text-secondary">{dateStr}</td>
      <td className="px-3 py-2 font-medium text-primary">{order.symbol}</td>
      <td className="px-3 py-2">
        <span className={cn(order.side === "BUY" ? "text-green" : "text-red", "font-medium")}>
          {order.side}
        </span>
      </td>
      <td className="px-3 py-2 text-secondary">{order.type.replace("_", " ")}</td>
      <td className="px-3 py-2 text-right font-mono text-primary">
        {order.price > 0 ? formatPrice(order.price) : "Market"}
      </td>
      <td className="px-3 py-2 text-right font-mono text-primary">{formatQty(order.qty)}</td>
      <td className="px-3 py-2 text-right font-mono text-primary">
        {formatQty(order.filledQty)}/{formatQty(order.qty)}
      </td>
      <td className="px-3 py-2">
        <span
          className={cn(
            "text-[10px] px-1.5 py-0.5 rounded",
            order.status === "FILLED" && "bg-green/10 text-green",
            order.status === "NEW" && "bg-accent/10 text-accent",
            order.status === "PARTIALLY_FILLED" && "bg-accent/10 text-accent",
            order.status === "CANCELED" && "bg-white/10 text-secondary",
            order.status === "EXPIRED" && "bg-white/10 text-secondary"
          )}
        >
          {order.status.replace("_", " ")}
        </span>
      </td>
      <td className="px-3 py-2 text-right">
        {(order.status === "NEW" || order.status === "PARTIALLY_FILLED") && (
          <button className="text-secondary hover:text-red text-[10px] border border-border rounded px-2 py-0.5">
            Cancel
          </button>
        )}
      </td>
    </tr>
  );
}

export default function OrderList() {
  const [tab, setTab] = useState<"active" | "history">("active");
  const { activeOrders, orderHistory } = useAccountStore();
  const orders = tab === "active" ? activeOrders : orderHistory;

  return (
    <div>
      <div className="flex items-center gap-4 px-3 border-b border-border">
        <button
          onClick={() => setTab("active")}
          className={cn(
            "py-2 text-xs font-medium border-b-2 transition-colors",
            tab === "active" ? "border-accent text-primary" : "border-transparent text-secondary hover:text-primary"
          )}
        >
          Active Orders ({activeOrders.length})
        </button>
        <button
          onClick={() => setTab("history")}
          className={cn(
            "py-2 text-xs font-medium border-b-2 transition-colors",
            tab === "history" ? "border-accent text-primary" : "border-transparent text-secondary hover:text-primary"
          )}
        >
          Order History
        </button>
      </div>

      {orders.length === 0 ? (
        <div className="flex items-center justify-center h-24 text-secondary text-sm">
          No orders
        </div>
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full">
            <thead>
              <tr className="text-secondary text-xs border-b border-border">
                <th className="text-left px-3 py-2 font-medium">Time</th>
                <th className="text-left px-3 py-2 font-medium">Symbol</th>
                <th className="text-left px-3 py-2 font-medium">Side</th>
                <th className="text-left px-3 py-2 font-medium">Type</th>
                <th className="text-right px-3 py-2 font-medium">Price</th>
                <th className="text-right px-3 py-2 font-medium">Qty</th>
                <th className="text-right px-3 py-2 font-medium">Filled</th>
                <th className="text-left px-3 py-2 font-medium">Status</th>
                <th className="text-right px-3 py-2 font-medium">Action</th>
              </tr>
            </thead>
            <tbody>
              {orders.map((order) => (
                <OrderRow key={order.id} order={order} />
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
