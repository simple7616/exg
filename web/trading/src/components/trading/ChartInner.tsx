"use client";

import { useEffect, useRef } from "react";
import { useMarketStore } from "@/stores/useMarketStore";
import type { KlineInterval } from "@/lib/types";

const INTERVALS: KlineInterval[] = ["1m", "5m", "15m", "1h", "4h", "1d", "1w"];

function ChartInner() {
  const containerRef = useRef<HTMLDivElement>(null);
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const chartRef = useRef<any>(null);
  const { klines, interval, setInterval: setKlineInterval, init } = useMarketStore();

  useEffect(() => {
    init();
  }, [init]);

  useEffect(() => {
    if (!containerRef.current || klines.length === 0) return;

    let disposed = false;

    (async () => {
      const { createChart, CandlestickSeries, HistogramSeries } = await import("lightweight-charts");
      if (disposed || !containerRef.current) return;

      // Clear previous chart
      if (chartRef.current) {
        chartRef.current.remove();
        chartRef.current = null;
      }

      const chart = createChart(containerRef.current, {
        autoSize: true,
        layout: {
          background: { color: "#1E2329" },
          textColor: "#848E9C",
          fontSize: 12,
        },
        grid: {
          vertLines: { color: "#2B3139" },
          horzLines: { color: "#2B3139" },
        },
        crosshair: {
          mode: 0,
        },
        rightPriceScale: {
          borderColor: "#2B3139",
        },
        timeScale: {
          borderColor: "#2B3139",
          timeVisible: true,
        },
      });

      chartRef.current = chart;

      const candleSeries = chart.addSeries(CandlestickSeries, {
        upColor: "#0ECB81",
        downColor: "#F6465D",
        borderDownColor: "#F6465D",
        borderUpColor: "#0ECB81",
        wickDownColor: "#F6465D",
        wickUpColor: "#0ECB81",
      });

      candleSeries.setData(
        klines.map((k) => ({
          time: k.time as import("lightweight-charts").UTCTimestamp,
          open: k.open,
          high: k.high,
          low: k.low,
          close: k.close,
        }))
      );

      const volumeSeries = chart.addSeries(HistogramSeries, {
        priceFormat: { type: "volume" },
        priceScaleId: "volume",
      });

      chart.priceScale("volume").applyOptions({
        scaleMargins: { top: 0.8, bottom: 0 },
      });

      volumeSeries.setData(
        klines.map((k) => ({
          time: k.time as import("lightweight-charts").UTCTimestamp,
          value: k.volume,
          color: k.close >= k.open ? "rgba(14,203,129,0.3)" : "rgba(246,70,93,0.3)",
        }))
      );

      chart.timeScale().fitContent();
    })();

    return () => {
      disposed = true;
      if (chartRef.current) {
        chartRef.current.remove();
        chartRef.current = null;
      }
    };
  }, [klines]);

  return (
    <div className="flex flex-col h-full bg-card rounded">
      {/* Interval selector */}
      <div className="flex items-center gap-1 px-3 py-2 border-b border-border">
        {INTERVALS.map((iv) => (
          <button
            key={iv}
            onClick={() => setKlineInterval(iv)}
            className={`px-2.5 py-1 text-xs rounded transition-colors ${
              interval === iv
                ? "bg-accent/20 text-accent"
                : "text-secondary hover:text-primary"
            }`}
          >
            {iv}
          </button>
        ))}
      </div>
      {/* Chart container */}
      <div ref={containerRef} className="flex-1 min-h-0" />
    </div>
  );
}

export default ChartInner;
