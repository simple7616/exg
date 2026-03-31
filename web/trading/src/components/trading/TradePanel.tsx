"use client";

import { useState } from "react";
import { cn } from "@/lib/utils";

type OrderTab = "Limit" | "Market" | "Stop";
type Side = "BUY" | "SELL";
const LEVERAGE_OPTIONS = [1, 2, 3, 5, 10, 20, 50, 75, 100, 125];

export default function TradePanel() {
  const [tab, setTab] = useState<OrderTab>("Limit");
  const [side, setSide] = useState<Side>("BUY");
  const [price, setPrice] = useState("66432.50");
  const [qty, setQty] = useState("");
  const [stopPrice, setStopPrice] = useState("");
  const [leverage, setLeverage] = useState(10);
  const [sliderValue, setSliderValue] = useState(0);
  const [showLeverage, setShowLeverage] = useState(false);

  const isBuy = side === "BUY";

  const handleSlider = (pct: number) => {
    setSliderValue(pct);
    // Mock: available balance ~45266 USDT, leverage 10
    const available = 45266.8 * leverage;
    const p = parseFloat(price) || 66432.5;
    const maxQty = available / p;
    setQty((maxQty * (pct / 100)).toFixed(3));
  };

  return (
    <div className="bg-card rounded p-4 flex flex-col gap-3">
      {/* Tabs */}
      <div className="flex items-center gap-1 border-b border-border pb-2">
        {(["Limit", "Market", "Stop"] as const).map((t) => (
          <button
            key={t}
            onClick={() => setTab(t)}
            className={cn(
              "px-3 py-1 text-xs rounded transition-colors",
              tab === t ? "bg-white/10 text-primary" : "text-secondary hover:text-primary"
            )}
          >
            {t}
          </button>
        ))}
      </div>

      {/* Side toggle */}
      <div className="grid grid-cols-2 gap-1">
        <button
          onClick={() => setSide("BUY")}
          className={cn(
            "py-2 text-sm font-semibold rounded transition-colors",
            isBuy ? "bg-green text-white" : "bg-white/5 text-secondary hover:text-primary"
          )}
        >
          Buy / Long
        </button>
        <button
          onClick={() => setSide("SELL")}
          className={cn(
            "py-2 text-sm font-semibold rounded transition-colors",
            !isBuy ? "bg-red text-white" : "bg-white/5 text-secondary hover:text-primary"
          )}
        >
          Sell / Short
        </button>
      </div>

      {/* Leverage */}
      <div className="relative">
        <button
          onClick={() => setShowLeverage(!showLeverage)}
          className="w-full flex items-center justify-between px-3 py-2 bg-white/5 rounded text-xs"
        >
          <span className="text-secondary">Leverage</span>
          <span className="text-accent font-mono font-semibold">{leverage}x</span>
        </button>
        {showLeverage && (
          <div className="absolute top-full left-0 right-0 mt-1 bg-card border border-border rounded p-2 grid grid-cols-5 gap-1 z-10">
            {LEVERAGE_OPTIONS.map((lev) => (
              <button
                key={lev}
                onClick={() => {
                  setLeverage(lev);
                  setShowLeverage(false);
                }}
                className={cn(
                  "py-1 text-xs rounded transition-colors",
                  leverage === lev
                    ? "bg-accent text-black font-semibold"
                    : "bg-white/5 text-secondary hover:text-primary"
                )}
              >
                {lev}x
              </button>
            ))}
          </div>
        )}
      </div>

      {/* Stop Price (only for Stop orders) */}
      {tab === "Stop" && (
        <div>
          <label className="text-[10px] text-secondary mb-1 block">Stop Price</label>
          <input
            type="text"
            value={stopPrice}
            onChange={(e) => setStopPrice(e.target.value)}
            placeholder="Stop Price"
            className="w-full bg-white/5 border border-border rounded px-3 py-2 text-sm font-mono text-primary outline-none focus:border-accent"
          />
        </div>
      )}

      {/* Price */}
      {tab !== "Market" && (
        <div>
          <label className="text-[10px] text-secondary mb-1 block">Price (USDT)</label>
          <input
            type="text"
            value={price}
            onChange={(e) => setPrice(e.target.value)}
            className="w-full bg-white/5 border border-border rounded px-3 py-2 text-sm font-mono text-primary outline-none focus:border-accent"
          />
        </div>
      )}

      {/* Quantity */}
      <div>
        <label className="text-[10px] text-secondary mb-1 block">Quantity (BTC)</label>
        <input
          type="text"
          value={qty}
          onChange={(e) => setQty(e.target.value)}
          placeholder="0.000"
          className="w-full bg-white/5 border border-border rounded px-3 py-2 text-sm font-mono text-primary outline-none focus:border-accent"
        />
      </div>

      {/* Percentage slider */}
      <div className="grid grid-cols-4 gap-1">
        {[25, 50, 75, 100].map((pct) => (
          <button
            key={pct}
            onClick={() => handleSlider(pct)}
            className={cn(
              "py-1 text-xs rounded transition-colors",
              sliderValue === pct
                ? isBuy
                  ? "bg-green/20 text-green"
                  : "bg-red/20 text-red"
                : "bg-white/5 text-secondary hover:text-primary"
            )}
          >
            {pct}%
          </button>
        ))}
      </div>

      {/* Cost / Available */}
      <div className="flex justify-between text-[10px] text-secondary">
        <span>Available</span>
        <span className="font-mono">45,266.80 USDT</span>
      </div>

      {/* Submit */}
      <button
        className={cn(
          "w-full py-3 rounded font-semibold text-sm text-white transition-colors",
          isBuy ? "bg-green hover:bg-green/80" : "bg-red hover:bg-red/80"
        )}
      >
        {isBuy ? "Buy / Long" : "Sell / Short"} BTCUSDT
      </button>
    </div>
  );
}
