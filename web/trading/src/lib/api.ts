const API_BASE = process.env.NEXT_PUBLIC_API_URL || 'http://localhost:8080/api/v1';

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

export const api = {
  // Market data
  getDepth: (symbol: string, limit = 20) =>
    request(`/depth?symbol=${symbol}&limit=${limit}`),
  getTrades: (symbol: string, limit = 50) =>
    request(`/trades?symbol=${symbol}&limit=${limit}`),
  getKlines: (symbol: string, interval: string, limit = 200) =>
    request(`/klines?symbol=${symbol}&interval=${interval}&limit=${limit}`),
  getTicker: (symbol: string) =>
    request(`/ticker?symbol=${symbol}`),

  // Trading
  placeOrder: (params: Record<string, unknown>) =>
    request('/order', { method: 'POST', body: JSON.stringify(params) }),
  cancelOrder: (symbol: string, orderId: string) =>
    request('/order', { method: 'DELETE', body: JSON.stringify({ symbol, orderId }) }),
  getOpenOrders: (symbol: string) =>
    request(`/openOrders?symbol=${symbol}`),
  getAllOrders: (symbol: string) =>
    request(`/allOrders?symbol=${symbol}`),

  // Account
  getAccount: () => request('/account'),
  getPositions: () => request('/position'),
  transfer: (params: Record<string, unknown>) =>
    request('/transfer', { method: 'POST', body: JSON.stringify(params) }),
  setLeverage: (symbol: string, leverage: number) =>
    request('/leverage', { method: 'POST', body: JSON.stringify({ symbol, leverage }) }),
};
