import { riskSnapshot, positions, liquidations } from "@/lib/mock-data";

function formatUsd(n: number): string {
  if (n >= 1_000_000_000) return `$${(n / 1_000_000_000).toFixed(2)}B`;
  if (n >= 1_000_000) return `$${(n / 1_000_000).toFixed(2)}M`;
  if (n >= 1_000) return `$${(n / 1_000).toFixed(1)}K`;
  return `$${n.toFixed(2)}`;
}

const longShortRatio = (
  riskSnapshot.longOpenInterest / riskSnapshot.shortOpenInterest
).toFixed(2);

const sortedPositions = [...positions].sort((a, b) => b.notional - a.notional);

export default function RiskPage() {
  return (
    <div className="space-y-6">
      {/* Risk stats */}
      <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-4">
        {[
          { label: "Total Open Interest", value: formatUsd(riskSnapshot.totalOpenInterest) },
          { label: "Long/Short Ratio", value: longShortRatio },
          { label: "Insurance Fund", value: formatUsd(riskSnapshot.insuranceFund) },
          { label: "Total Margin Used", value: formatUsd(riskSnapshot.totalMarginUsed) },
        ].map((s) => (
          <div key={s.label} className="bg-white rounded-lg border border-gray-200 shadow-sm p-5">
            <p className="text-sm text-gray-500">{s.label}</p>
            <p className="mt-1 text-2xl font-semibold text-gray-900">{s.value}</p>
          </div>
        ))}
      </div>

      {/* Top positions */}
      <div className="bg-white rounded-lg border border-gray-200 shadow-sm p-5">
        <h2 className="text-sm font-medium text-gray-700 mb-4">Top Positions by Notional</h2>
        <div className="overflow-x-auto">
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-gray-200 text-left text-gray-500">
                <th className="pb-2 pr-4 font-medium">User</th>
                <th className="pb-2 pr-4 font-medium">Symbol</th>
                <th className="pb-2 pr-4 font-medium">Side</th>
                <th className="pb-2 pr-4 font-medium text-right">Size</th>
                <th className="pb-2 pr-4 font-medium text-right">Notional</th>
                <th className="pb-2 pr-4 font-medium text-right">Leverage</th>
                <th className="pb-2 pr-4 font-medium text-right">uPnL</th>
                <th className="pb-2 font-medium text-right">Margin Ratio</th>
              </tr>
            </thead>
            <tbody>
              {sortedPositions.map((p) => (
                <tr key={p.id} className="border-b border-gray-100">
                  <td className="py-2.5 pr-4 font-mono text-xs">{p.userId}</td>
                  <td className="py-2.5 pr-4">{p.symbol}</td>
                  <td className="py-2.5 pr-4">
                    <span className={p.side === "long" ? "text-green-600" : "text-red-600"}>
                      {p.side.toUpperCase()}
                    </span>
                  </td>
                  <td className="py-2.5 pr-4 text-right">{p.size}</td>
                  <td className="py-2.5 pr-4 text-right">{formatUsd(p.notional)}</td>
                  <td className="py-2.5 pr-4 text-right">{p.leverage}x</td>
                  <td className={`py-2.5 pr-4 text-right ${p.unrealizedPnl >= 0 ? "text-green-600" : "text-red-600"}`}>
                    {p.unrealizedPnl >= 0 ? "+" : ""}
                    {formatUsd(Math.abs(p.unrealizedPnl))}
                  </td>
                  <td className={`py-2.5 text-right ${p.marginRatio < 0.05 ? "text-red-600 font-medium" : ""}`}>
                    {(p.marginRatio * 100).toFixed(1)}%
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>

      {/* Recent liquidations */}
      <div className="bg-white rounded-lg border border-gray-200 shadow-sm p-5">
        <h2 className="text-sm font-medium text-gray-700 mb-4">Recent Liquidations</h2>
        <div className="overflow-x-auto">
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-gray-200 text-left text-gray-500">
                <th className="pb-2 pr-4 font-medium">Time</th>
                <th className="pb-2 pr-4 font-medium">User</th>
                <th className="pb-2 pr-4 font-medium">Symbol</th>
                <th className="pb-2 pr-4 font-medium">Side</th>
                <th className="pb-2 pr-4 font-medium text-right">Size</th>
                <th className="pb-2 pr-4 font-medium text-right">Bankruptcy</th>
                <th className="pb-2 font-medium text-right">Liq. Price</th>
              </tr>
            </thead>
            <tbody>
              {liquidations.map((l) => (
                <tr key={l.id} className="border-b border-gray-100">
                  <td className="py-2.5 pr-4 text-gray-600">
                    {new Date(l.timestamp).toLocaleString()}
                  </td>
                  <td className="py-2.5 pr-4 font-mono text-xs">{l.userId}</td>
                  <td className="py-2.5 pr-4">{l.symbol}</td>
                  <td className="py-2.5 pr-4">
                    <span className={l.side === "long" ? "text-green-600" : "text-red-600"}>
                      {l.side.toUpperCase()}
                    </span>
                  </td>
                  <td className="py-2.5 pr-4 text-right">{l.size}</td>
                  <td className="py-2.5 pr-4 text-right">${l.bankruptcyPrice.toLocaleString()}</td>
                  <td className="py-2.5 text-right">${l.liquidationPrice.toLocaleString()}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>
    </div>
  );
}
