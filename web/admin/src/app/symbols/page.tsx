import { symbols } from "@/lib/mock-data";

function SymbolStatusBadge({ status }: { status: string }) {
  const colors: Record<string, string> = {
    Trading: "bg-green-100 text-green-700",
    Halted: "bg-red-100 text-red-700",
    Settling: "bg-yellow-100 text-yellow-700",
  };
  return (
    <span className={`inline-flex items-center px-2 py-0.5 rounded text-xs font-medium ${colors[status] ?? "bg-gray-100 text-gray-700"}`}>
      {status}
    </span>
  );
}

function formatUsd(n: number): string {
  if (n >= 1_000_000_000) return `$${(n / 1_000_000_000).toFixed(2)}B`;
  if (n >= 1_000_000) return `$${(n / 1_000_000).toFixed(2)}M`;
  if (n >= 1_000) return `$${(n / 1_000).toFixed(1)}K`;
  return `$${n.toFixed(2)}`;
}

export default function SymbolsPage() {
  return (
    <div className="space-y-4">
      <div className="bg-white rounded-lg border border-gray-200 shadow-sm overflow-x-auto">
        <table className="w-full text-sm">
          <thead>
            <tr className="border-b border-gray-200 text-left text-gray-500 bg-gray-50">
              <th className="px-4 py-3 font-medium">ID</th>
              <th className="px-4 py-3 font-medium">Name</th>
              <th className="px-4 py-3 font-medium">Type</th>
              <th className="px-4 py-3 font-medium">Status</th>
              <th className="px-4 py-3 font-medium text-right">Last Price</th>
              <th className="px-4 py-3 font-medium text-right">24h Volume</th>
              <th className="px-4 py-3 font-medium text-right">Tick Size</th>
              <th className="px-4 py-3 font-medium text-right">Lot Size</th>
              <th className="px-4 py-3 font-medium text-right">Maker Fee</th>
              <th className="px-4 py-3 font-medium text-right">Taker Fee</th>
            </tr>
          </thead>
          <tbody>
            {symbols.map((s) => (
              <tr key={s.id} className="border-b border-gray-100 hover:bg-gray-50">
                <td className="px-4 py-3 font-mono text-xs font-medium">{s.id}</td>
                <td className="px-4 py-3">{s.name}</td>
                <td className="px-4 py-3 capitalize">{s.type}</td>
                <td className="px-4 py-3">
                  <SymbolStatusBadge status={s.status} />
                </td>
                <td className="px-4 py-3 text-right">${s.lastPrice.toLocaleString()}</td>
                <td className="px-4 py-3 text-right">{formatUsd(s.volume24h)}</td>
                <td className="px-4 py-3 text-right">{s.tickSize}</td>
                <td className="px-4 py-3 text-right">{s.lotSize}</td>
                <td className="px-4 py-3 text-right">{(s.makerFee * 100).toFixed(2)}%</td>
                <td className="px-4 py-3 text-right">{(s.takerFee * 100).toFixed(2)}%</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}
