"use client";

import { useState } from "react";
import { cn, formatPrice } from "@/lib/utils";

interface WalletBalance {
  asset: string;
  spot: number;
  futures: number;
  funding: number;
}

const mockBalances: WalletBalance[] = [
  { asset: "USDT", spot: 25420.50, futures: 52340.80, funding: 8200.00 },
  { asset: "BTC", spot: 0.852, futures: 1.245, funding: 0.100 },
  { asset: "ETH", spot: 12.50, futures: 15.80, funding: 2.00 },
  { asset: "SOL", spot: 340.00, futures: 0, funding: 50.00 },
];

const mockHistory = [
  { id: "1", type: "deposit" as const, asset: "USDT", amount: 50000, time: "2026-03-31 14:00", status: "completed" as const },
  { id: "2", type: "withdrawal" as const, asset: "USDT", amount: 10000, time: "2026-03-30 10:30", status: "completed" as const },
  { id: "3", type: "deposit" as const, asset: "BTC", amount: 0.5, time: "2026-03-29 08:15", status: "completed" as const },
  { id: "4", type: "withdrawal" as const, asset: "ETH", amount: 5.0, time: "2026-03-28 16:45", status: "pending" as const },
  { id: "5", type: "deposit" as const, asset: "USDT", amount: 25000, time: "2026-03-27 12:00", status: "completed" as const },
];

// Approximate USD prices for total equity calc
const usdPrices: Record<string, number> = { USDT: 1, BTC: 66432.5, ETH: 3485.6, SOL: 142.8 };

export default function AccountOverviewPage() {
  const [showTransfer, setShowTransfer] = useState(false);
  const [transferFrom, setTransferFrom] = useState("spot");
  const [transferTo, setTransferTo] = useState("futures");
  const [transferAsset, setTransferAsset] = useState("USDT");
  const [transferAmount, setTransferAmount] = useState("");

  const totalEquity = mockBalances.reduce((sum, b) => {
    const price = usdPrices[b.asset] ?? 0;
    return sum + (b.spot + b.futures + b.funding) * price;
  }, 0);

  return (
    <div className="space-y-6 max-w-5xl">
      {/* Total Equity */}
      <div className="bg-card rounded p-6 border border-border">
        <p className="text-secondary text-sm mb-1">Total Equity (USD)</p>
        <p className="text-3xl font-mono font-bold text-primary">
          ${formatPrice(totalEquity)}
        </p>
      </div>

      {/* Transfer Modal */}
      {showTransfer && (
        <div className="bg-card rounded p-4 border border-accent/30 space-y-3">
          <div className="flex items-center justify-between">
            <h3 className="text-sm font-semibold text-primary">Transfer Between Wallets</h3>
            <button onClick={() => setShowTransfer(false)} className="text-secondary hover:text-primary text-xs">Close</button>
          </div>
          <div className="grid grid-cols-3 gap-3">
            <div>
              <label className="text-[10px] text-secondary block mb-1">From</label>
              <select value={transferFrom} onChange={(e) => setTransferFrom(e.target.value)} className="w-full bg-white/5 border border-border rounded px-2 py-1.5 text-sm text-primary">
                <option value="spot">Spot</option>
                <option value="futures">Futures</option>
                <option value="funding">Funding</option>
              </select>
            </div>
            <div>
              <label className="text-[10px] text-secondary block mb-1">To</label>
              <select value={transferTo} onChange={(e) => setTransferTo(e.target.value)} className="w-full bg-white/5 border border-border rounded px-2 py-1.5 text-sm text-primary">
                <option value="spot">Spot</option>
                <option value="futures">Futures</option>
                <option value="funding">Funding</option>
              </select>
            </div>
            <div>
              <label className="text-[10px] text-secondary block mb-1">Asset</label>
              <select value={transferAsset} onChange={(e) => setTransferAsset(e.target.value)} className="w-full bg-white/5 border border-border rounded px-2 py-1.5 text-sm text-primary">
                {mockBalances.map((b) => <option key={b.asset} value={b.asset}>{b.asset}</option>)}
              </select>
            </div>
          </div>
          <div>
            <label className="text-[10px] text-secondary block mb-1">Amount</label>
            <input
              type="text"
              value={transferAmount}
              onChange={(e) => setTransferAmount(e.target.value)}
              placeholder="0.00"
              className="w-full bg-white/5 border border-border rounded px-3 py-2 text-sm font-mono text-primary outline-none focus:border-accent"
            />
          </div>
          <button className="bg-accent text-black px-4 py-2 rounded text-sm font-semibold hover:bg-accent/80 transition-colors">
            Confirm Transfer
          </button>
        </div>
      )}

      {/* Wallet Balances */}
      <div className="bg-card rounded border border-border">
        <div className="flex items-center justify-between px-4 py-3 border-b border-border">
          <h2 className="text-sm font-semibold text-primary">Wallet Balances</h2>
          <button
            onClick={() => setShowTransfer(!showTransfer)}
            className="text-xs text-accent hover:text-accent/80 font-medium"
          >
            Transfer
          </button>
        </div>
        <table className="w-full text-sm">
          <thead>
            <tr className="border-b border-border text-secondary text-xs">
              <th className="text-left px-4 py-2 font-medium">Asset</th>
              <th className="text-right px-4 py-2 font-medium">Spot</th>
              <th className="text-right px-4 py-2 font-medium">Futures</th>
              <th className="text-right px-4 py-2 font-medium">Funding</th>
              <th className="text-right px-4 py-2 font-medium">Total</th>
            </tr>
          </thead>
          <tbody>
            {mockBalances.map((b) => (
              <tr key={b.asset} className="border-b border-border/50 hover:bg-white/[0.02]">
                <td className="px-4 py-2 font-medium text-primary">{b.asset}</td>
                <td className="px-4 py-2 text-right font-mono text-primary">{b.spot.toLocaleString()}</td>
                <td className="px-4 py-2 text-right font-mono text-primary">{b.futures.toLocaleString()}</td>
                <td className="px-4 py-2 text-right font-mono text-primary">{b.funding.toLocaleString()}</td>
                <td className="px-4 py-2 text-right font-mono text-primary font-semibold">
                  {(b.spot + b.futures + b.funding).toLocaleString()}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      {/* Recent Deposit/Withdrawal History */}
      <div className="bg-card rounded border border-border">
        <div className="px-4 py-3 border-b border-border">
          <h2 className="text-sm font-semibold text-primary">Recent Deposits & Withdrawals</h2>
        </div>
        <table className="w-full text-sm">
          <thead>
            <tr className="border-b border-border text-secondary text-xs">
              <th className="text-left px-4 py-2 font-medium">Type</th>
              <th className="text-left px-4 py-2 font-medium">Asset</th>
              <th className="text-right px-4 py-2 font-medium">Amount</th>
              <th className="text-left px-4 py-2 font-medium">Time</th>
              <th className="text-left px-4 py-2 font-medium">Status</th>
            </tr>
          </thead>
          <tbody>
            {mockHistory.map((h) => (
              <tr key={h.id} className="border-b border-border/50 hover:bg-white/[0.02]">
                <td className={cn("px-4 py-2", h.type === "deposit" ? "text-green" : "text-red")}>
                  {h.type === "deposit" ? "Deposit" : "Withdrawal"}
                </td>
                <td className="px-4 py-2 text-primary">{h.asset}</td>
                <td className="px-4 py-2 text-right font-mono text-primary">{h.amount.toLocaleString()}</td>
                <td className="px-4 py-2 text-secondary">{h.time}</td>
                <td className="px-4 py-2">
                  <span className={cn(
                    "text-xs px-2 py-0.5 rounded",
                    h.status === "completed" ? "bg-green/10 text-green" : "bg-yellow-500/10 text-yellow-500"
                  )}>
                    {h.status}
                  </span>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}
