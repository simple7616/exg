import { assetSummary, depositsWithdrawals } from "@/lib/mock-data";

function formatUsd(n: number): string {
  if (n >= 1_000_000_000) return `$${(n / 1_000_000_000).toFixed(2)}B`;
  if (n >= 1_000_000) return `$${(n / 1_000_000).toFixed(2)}M`;
  if (n >= 1_000) return `$${(n / 1_000).toFixed(1)}K`;
  return `$${n.toFixed(2)}`;
}

function TxStatusBadge({ status }: { status: string }) {
  const colors: Record<string, string> = {
    completed: "bg-green-100 text-green-700",
    pending: "bg-yellow-100 text-yellow-700",
    failed: "bg-red-100 text-red-700",
  };
  return (
    <span className={`inline-flex items-center px-2 py-0.5 rounded text-xs font-medium ${colors[status] ?? "bg-gray-100 text-gray-700"}`}>
      {status.charAt(0).toUpperCase() + status.slice(1)}
    </span>
  );
}

export default function AssetsPage() {
  const cards = [
    { label: "Total Deposits", value: formatUsd(assetSummary.totalDeposits), color: "text-green-600" },
    { label: "Total Withdrawals", value: formatUsd(assetSummary.totalWithdrawals), color: "text-red-600" },
    { label: "Net Flow", value: formatUsd(assetSummary.netFlow), color: "text-blue-600" },
    { label: "Fee Revenue", value: formatUsd(assetSummary.feeRevenue), color: "text-purple-600" },
  ];

  return (
    <div className="space-y-6">
      <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-4">
        {cards.map((c) => (
          <div key={c.label} className="bg-white rounded-lg border border-gray-200 shadow-sm p-5">
            <p className="text-sm text-gray-500">{c.label}</p>
            <p className={`mt-1 text-2xl font-semibold ${c.color}`}>{c.value}</p>
          </div>
        ))}
      </div>

      <div className="bg-white rounded-lg border border-gray-200 shadow-sm p-5">
        <h2 className="text-sm font-medium text-gray-700 mb-4">Deposits & Withdrawals</h2>
        <div className="overflow-x-auto">
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-gray-200 text-left text-gray-500 bg-gray-50">
                <th className="px-4 py-3 font-medium">ID</th>
                <th className="px-4 py-3 font-medium">User</th>
                <th className="px-4 py-3 font-medium">Type</th>
                <th className="px-4 py-3 font-medium">Asset</th>
                <th className="px-4 py-3 font-medium text-right">Amount</th>
                <th className="px-4 py-3 font-medium">Status</th>
                <th className="px-4 py-3 font-medium">Time</th>
                <th className="px-4 py-3 font-medium">Tx Hash</th>
              </tr>
            </thead>
            <tbody>
              {depositsWithdrawals.map((tx) => (
                <tr key={tx.id} className="border-b border-gray-100 hover:bg-gray-50">
                  <td className="px-4 py-3 font-mono text-xs">{tx.id}</td>
                  <td className="px-4 py-3 font-mono text-xs">{tx.userId}</td>
                  <td className="px-4 py-3">
                    <span className={tx.type === "deposit" ? "text-green-600" : "text-red-600"}>
                      {tx.type === "deposit" ? "Deposit" : "Withdrawal"}
                    </span>
                  </td>
                  <td className="px-4 py-3">{tx.asset}</td>
                  <td className="px-4 py-3 text-right">${tx.amount.toLocaleString()}</td>
                  <td className="px-4 py-3">
                    <TxStatusBadge status={tx.status} />
                  </td>
                  <td className="px-4 py-3 text-gray-600">
                    {new Date(tx.timestamp).toLocaleString()}
                  </td>
                  <td className="px-4 py-3 font-mono text-xs text-gray-500">
                    {tx.txHash ?? "-"}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>
    </div>
  );
}
