"use client";

import { useEffect, useState } from "react";
import Chart from "@/components/trading/Chart";
import OrderBook from "@/components/trading/OrderBook";
import RecentTrades from "@/components/trading/RecentTrades";
import TradePanel from "@/components/trading/TradePanel";
import PositionList from "@/components/trading/PositionList";
import OrderList from "@/components/trading/OrderList";
import { useMarketStore } from "@/stores/useMarketStore";
import { useAccountStore } from "@/stores/useAccountStore";
import { cn } from "@/lib/utils";

type BottomTab = "positions" | "orders" | "history";

export default function FuturesPage() {
  const initMarket = useMarketStore((s) => s.init);
  const initAccount = useAccountStore((s) => s.init);
  const [bottomTab, setBottomTab] = useState<BottomTab>("positions");

  useEffect(() => {
    initMarket();
    initAccount();
  }, [initMarket, initAccount]);

  return (
    <div className="flex flex-col h-full p-1 gap-1">
      {/* Top row: Chart | OrderBook | RecentTrades */}
      <div className="flex gap-1 flex-1 min-h-0" style={{ flex: "3 1 0%" }}>
        {/* Chart - 60% */}
        <div className="flex-[3] min-w-0">
          <Chart />
        </div>
        {/* OrderBook - 20% */}
        <div className="flex-[1] min-w-0 max-w-[280px]">
          <OrderBook />
        </div>
        {/* RecentTrades - 20% */}
        <div className="flex-[1] min-w-0 max-w-[280px]">
          <RecentTrades />
        </div>
      </div>

      {/* Middle row: TradePanel */}
      <div className="max-w-sm">
        <TradePanel />
      </div>

      {/* Bottom row: Positions / Orders / History */}
      <div className="bg-card rounded flex-1 min-h-0 flex flex-col" style={{ flex: "1.2 1 0%" }}>
        {/* Tabs */}
        <div className="flex items-center gap-4 px-3 border-b border-border">
          {([
            ["positions", "Positions"],
            ["orders", "Active Orders"],
            ["history", "Order History"],
          ] as const).map(([key, label]) => (
            <button
              key={key}
              onClick={() => setBottomTab(key)}
              className={cn(
                "py-2 text-xs font-medium border-b-2 transition-colors",
                bottomTab === key
                  ? "border-accent text-primary"
                  : "border-transparent text-secondary hover:text-primary"
              )}
            >
              {label}
            </button>
          ))}
        </div>

        <div className="flex-1 overflow-auto">
          {bottomTab === "positions" && <PositionList />}
          {(bottomTab === "orders" || bottomTab === "history") && <OrderList />}
        </div>
      </div>
    </div>
  );
}
