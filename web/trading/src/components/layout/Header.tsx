"use client";

import { useMarketStore } from "@/stores/useMarketStore";
import { formatPrice, formatPercent, formatVolume, cn } from "@/lib/utils";
import Link from "next/link";

export default function Header() {
  const { ticker, symbol } = useMarketStore();
  const isPositive = ticker.priceChange >= 0;

  return (
    <header className="flex items-center h-14 px-4 border-b border-border bg-card shrink-0">
      {/* Logo */}
      <Link href="/futures" className="text-accent font-bold text-xl tracking-wider mr-8">
        EXG
      </Link>

      {/* Nav */}
      <nav className="flex items-center gap-6 mr-8 text-sm">
        <Link href="/futures" className="text-primary hover:text-accent transition-colors font-medium">
          Futures
        </Link>
        <Link href="/spot" className="text-primary hover:text-accent transition-colors font-medium">
          Spot
        </Link>
        <Link href="/account" className="text-primary hover:text-accent transition-colors font-medium">
          Account
        </Link>
      </nav>

      {/* Divider */}
      <div className="w-px h-6 bg-border mr-6" />

      {/* Symbol Info */}
      <div className="flex items-center gap-6">
        <div className="flex items-center gap-2">
          <span className="text-primary font-semibold text-base">
            {symbol.baseAsset}/{symbol.quoteAsset}
          </span>
          <span className="text-xs bg-accent/20 text-accent px-1.5 py-0.5 rounded font-medium">
            Perp
          </span>
        </div>

        <div className="flex items-center gap-1">
          <span className={cn("font-mono text-lg font-semibold", isPositive ? "text-green" : "text-red")}>
            {formatPrice(ticker.lastPrice)}
          </span>
        </div>

        <div className="flex items-center gap-4 text-xs">
          <div>
            <span className="text-secondary mr-1">24h Change</span>
            <span className={cn("font-mono", isPositive ? "text-green" : "text-red")}>
              {formatPercent(ticker.priceChangePercent)}
            </span>
          </div>
          <div>
            <span className="text-secondary mr-1">24h High</span>
            <span className="font-mono text-primary">{formatPrice(ticker.high24h)}</span>
          </div>
          <div>
            <span className="text-secondary mr-1">24h Low</span>
            <span className="font-mono text-primary">{formatPrice(ticker.low24h)}</span>
          </div>
          <div>
            <span className="text-secondary mr-1">24h Vol</span>
            <span className="font-mono text-primary">{formatVolume(ticker.quoteVolume24h)}</span>
          </div>
          <div>
            <span className="text-secondary mr-1">Funding</span>
            <span className="font-mono text-primary">{(ticker.fundingRate * 100).toFixed(4)}%</span>
          </div>
        </div>
      </div>
    </header>
  );
}
