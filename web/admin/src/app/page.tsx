"use client";

import {
  BarChart,
  Bar,
  XAxis,
  YAxis,
  CartesianGrid,
  Tooltip,
  ResponsiveContainer,
} from "recharts";
import { volumeByHour, liquidations, riskSnapshot } from "@/lib/mock-data";
import { users } from "@/lib/mock-data";

function formatUsd(n: number): string {
  if (n >= 1_000_000_000) return `$${(n / 1_000_000_000).toFixed(2)}B`;
  if (n >= 1_000_000) return `$${(n / 1_000_000).toFixed(2)}M`;
  if (n >= 1_000) return `$${(n / 1_000).toFixed(1)}K`;
  return `$${n.toFixed(2)}`;
}

const totalVolume = volumeByHour.reduce((s, h) => s + h.volume, 0);

const stats = [
  { label: "Total Users", value: users.length.toLocaleString() },
  { label: "24h Volume", value: formatUsd(totalVolume) },
  { label: "Open Interest", value: formatUsd(riskSnapshot.totalOpenInterest) },
  { label: "Insurance Fund", value: formatUsd(riskSnapshot.insuranceFund) },
];

export default function DashboardPage() {
  return (
    <div className="space-y-6">
      {/* Stat cards */}
      <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-4">
        {stats.map((s) => (
          <div
            key={s.label}
            className="bg-white rounded-lg border border-gray-200 shadow-sm p-5"
          >
            <p className="text-sm text-gray-500">{s.label}</p>
            <p className="mt-1 text-2xl font-semibold text-gray-900">
              {s.value}
            </p>
          </div>
        ))}
      </div>

      {/* Volume chart */}
      <div className="bg-white rounded-lg border border-gray-200 shadow-sm p-5">
        <h2 className="text-sm font-medium text-gray-700 mb-4">
          24h Volume by Hour (USD)
        </h2>
        <div className="h-64">
          <ResponsiveContainer width="100%" height="100%">
            <BarChart data={volumeByHour}>
              <CartesianGrid strokeDasharray="3 3" stroke="#e5e7eb" />
              <XAxis
                dataKey="hour"
                tick={{ fontSize: 11, fill: "#6b7280" }}
                interval={2}
              />
              <YAxis
                tick={{ fontSize: 11, fill: "#6b7280" }}
                tickFormatter={(v: number) => formatUsd(v)}
              />
              <Tooltip
                formatter={(value) => [formatUsd(Number(value)), "Volume"]}
                contentStyle={{
                  fontSize: 12,
                  borderRadius: 6,
                  border: "1px solid #e5e7eb",
                }}
              />
              <Bar dataKey="volume" fill="#3b82f6" radius={[2, 2, 0, 0]} />
            </BarChart>
          </ResponsiveContainer>
        </div>
      </div>

      {/* Recent liquidations */}
      <div className="bg-white rounded-lg border border-gray-200 shadow-sm p-5">
        <h2 className="text-sm font-medium text-gray-700 mb-4">
          Recent Liquidations
        </h2>
        <div className="overflow-x-auto">
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-gray-200 text-left text-gray-500">
                <th className="pb-2 pr-4 font-medium">Time</th>
                <th className="pb-2 pr-4 font-medium">User</th>
                <th className="pb-2 pr-4 font-medium">Symbol</th>
                <th className="pb-2 pr-4 font-medium">Side</th>
                <th className="pb-2 pr-4 font-medium text-right">Size</th>
                <th className="pb-2 font-medium text-right">Liq. Price</th>
              </tr>
            </thead>
            <tbody>
              {liquidations.slice(0, 5).map((l) => (
                <tr key={l.id} className="border-b border-gray-100">
                  <td className="py-2.5 pr-4 text-gray-600">
                    {new Date(l.timestamp).toLocaleTimeString()}
                  </td>
                  <td className="py-2.5 pr-4 font-mono text-xs">
                    {l.userId}
                  </td>
                  <td className="py-2.5 pr-4">{l.symbol}</td>
                  <td className="py-2.5 pr-4">
                    <span
                      className={
                        l.side === "long" ? "text-green-600" : "text-red-600"
                      }
                    >
                      {l.side.toUpperCase()}
                    </span>
                  </td>
                  <td className="py-2.5 pr-4 text-right">{l.size}</td>
                  <td className="py-2.5 text-right">
                    ${l.liquidationPrice.toLocaleString()}
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
