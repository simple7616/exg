import { services, systemMetrics } from "@/lib/mock-data";

function StatusDot({ status }: { status: string }) {
  const colors: Record<string, string> = {
    healthy: "bg-green-500",
    degraded: "bg-yellow-500",
    down: "bg-red-500",
  };
  return (
    <span className={`inline-block w-2.5 h-2.5 rounded-full ${colors[status] ?? "bg-gray-400"}`} />
  );
}

export default function SystemPage() {
  return (
    <div className="space-y-6">
      {/* Service status cards */}
      <div>
        <h2 className="text-sm font-medium text-gray-700 mb-3">Service Status</h2>
        <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-4">
          {services.map((svc) => (
            <div key={svc.name} className="bg-white rounded-lg border border-gray-200 shadow-sm p-4">
              <div className="flex items-center justify-between mb-3">
                <span className="text-sm font-medium text-gray-900">{svc.name}</span>
                <StatusDot status={svc.status} />
              </div>
              <div className="space-y-1 text-xs text-gray-500">
                <div className="flex justify-between">
                  <span>Uptime</span>
                  <span className="text-gray-700">{svc.uptime}%</span>
                </div>
                <div className="flex justify-between">
                  <span>p50 Latency</span>
                  <span className="text-gray-700">{svc.latencyP50}ms</span>
                </div>
                <div className="flex justify-between">
                  <span>p99 Latency</span>
                  <span className="text-gray-700">{svc.latencyP99}ms</span>
                </div>
                <div className="flex justify-between">
                  <span>Version</span>
                  <span className="text-gray-700">{svc.version}</span>
                </div>
              </div>
            </div>
          ))}
        </div>
      </div>

      {/* Performance metrics */}
      <div>
        <h2 className="text-sm font-medium text-gray-700 mb-3">Performance Metrics</h2>
        <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-4">
          {[
            { label: "Matching Latency p50", value: `${systemMetrics.matchingLatencyP50}ms` },
            { label: "Matching Latency p99", value: `${systemMetrics.matchingLatencyP99}ms` },
            { label: "Orders/sec", value: systemMetrics.ordersPerSecond.toLocaleString() },
            { label: "Active Connections", value: systemMetrics.activeConnections.toLocaleString() },
            { label: "Active WebSockets", value: systemMetrics.activeWebsockets.toLocaleString() },
            { label: "Pending Orders", value: systemMetrics.pendingOrders.toLocaleString() },
          ].map((m) => (
            <div key={m.label} className="bg-white rounded-lg border border-gray-200 shadow-sm p-5">
              <p className="text-sm text-gray-500">{m.label}</p>
              <p className="mt-1 text-2xl font-semibold text-gray-900">{m.value}</p>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
