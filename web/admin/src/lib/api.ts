const API_BASE = process.env.NEXT_PUBLIC_ADMIN_API_URL || 'http://localhost:8081/api/v1/admin';

async function request<T>(path: string, options?: RequestInit): Promise<T> {
  const res = await fetch(`${API_BASE}${path}`, {
    headers: { 'Content-Type': 'application/json', ...options?.headers },
    ...options,
  });
  if (!res.ok) {
    const err = await res.json().catch(() => ({ msg: res.statusText }));
    throw new Error(err.msg || res.statusText);
  }
  return res.json();
}

export const adminApi = {
  // Users
  getUsers: (params?: { page?: number; limit?: number }) =>
    request(`/users?page=${params?.page ?? 1}&limit=${params?.limit ?? 50}`),
  getUser: (id: string) =>
    request(`/users/${id}`),
  freezeUser: (id: string) =>
    request(`/users/${id}/freeze`, { method: 'POST' }),
  unfreezeUser: (id: string) =>
    request(`/users/${id}/unfreeze`, { method: 'POST' }),
  setKycLevel: (id: string, level: number) =>
    request(`/users/${id}/kyc`, { method: 'PUT', body: JSON.stringify({ level }) }),

  // Risk
  getRiskSnapshot: () => request('/risk/snapshot'),
  getPositions: () => request('/risk/positions'),
  getLiquidations: () => request('/risk/liquidations'),

  // Symbols
  getSymbols: () => request('/symbols'),
  updateSymbol: (id: string, data: Record<string, unknown>) =>
    request(`/symbols/${id}`, { method: 'PUT', body: JSON.stringify(data) }),

  // Assets
  getDepositsWithdrawals: () => request('/assets/transactions'),
  getFeeReports: () => request('/assets/fees'),
  getAssetSummary: () => request('/assets/summary'),

  // System
  getServices: () => request('/system/services'),
  getMetrics: () => request('/system/metrics'),
};
