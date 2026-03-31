"use client";

import dynamic from "next/dynamic";
import OrderBook from "@/components/trading/OrderBook";
import TradePanel from "@/components/trading/TradePanel";
import RecentTrades from "@/components/trading/RecentTrades";
import OrderList from "@/components/trading/OrderList";
import { useMarketStore } from "@/stores/useMarketStore";
import { useEffect } from "react";

const Chart = dynamic(() => import("@/components/trading/Chart"), { ssr: false });

export default function SpotPage() {
  const init = useMarketStore((s) => s.init);
  useEffect(() => { init(); }, [init]);

  return (
    <div className="flex flex-col h-full p-1 gap-1">
      <div className="flex gap-1 flex-1 min-h-0" style={{ flex: "3 1 0%" }}>
        <div className="flex-[3] min-w-0">
          <Chart />
        </div>
        <div className="flex-[1] min-w-0 max-w-[280px]">
          <OrderBook />
        </div>
        <div className="flex-[1] min-w-0 max-w-[280px]">
          <RecentTrades />
        </div>
      </div>
      <div className="max-w-sm">
        <TradePanel />
      </div>
      <div className="bg-card rounded flex-1 min-h-0" style={{ flex: "1.2 1 0%" }}>
        <OrderList />
      </div>
    </div>
  );
}
