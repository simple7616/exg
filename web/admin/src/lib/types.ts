export interface UserSummary {
  id: string;
  email: string;
  kycLevel: 0 | 1 | 2 | 3;
  status: "active" | "frozen";
  createdAt: string;
  totalVolume: number;
  balance: number;
}

export interface SymbolEntry {
  id: string;
  name: string;
  type: "perpetual" | "spot";
  status: "Trading" | "Halted" | "Settling";
  tickSize: number;
  lotSize: number;
  makerFee: number;
  takerFee: number;
  lastPrice: number;
  volume24h: number;
}

export interface RiskSnapshot {
  totalOpenInterest: number;
  longOpenInterest: number;
  shortOpenInterest: number;
  insuranceFund: number;
  totalMarginUsed: number;
}

export interface Position {
  id: string;
  userId: string;
  symbol: string;
  side: "long" | "short";
  size: number;
  entryPrice: number;
  markPrice: number;
  notional: number;
  unrealizedPnl: number;
  leverage: number;
  marginRatio: number;
}

export interface LiquidationLog {
  id: string;
  userId: string;
  symbol: string;
  side: "long" | "short";
  size: number;
  bankruptcyPrice: number;
  liquidationPrice: number;
  timestamp: string;
}

export interface FeeReport {
  date: string;
  makerFees: number;
  takerFees: number;
  totalFees: number;
  liquidationFees: number;
}

export interface DepositWithdrawal {
  id: string;
  userId: string;
  type: "deposit" | "withdrawal";
  asset: string;
  amount: number;
  status: "completed" | "pending" | "failed";
  timestamp: string;
  txHash?: string;
}

export interface ServiceStatus {
  name: string;
  status: "healthy" | "degraded" | "down";
  uptime: number;
  latencyP50: number;
  latencyP99: number;
  version: string;
}

export interface VolumeByHour {
  hour: string;
  volume: number;
}

export interface AssetSummary {
  totalDeposits: number;
  totalWithdrawals: number;
  netFlow: number;
  feeRevenue: number;
}
