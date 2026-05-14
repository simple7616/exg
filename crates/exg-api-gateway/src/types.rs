use serde::{Deserialize, Serialize};

// ── Order endpoints ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaceOrderRequest {
    pub symbol: String,
    pub side: String,
    pub order_type: String,
    pub time_in_force: Option<String>,
    pub quantity: String,
    pub price: Option<String>,
    pub stop_price: Option<String>,
    pub reduce_only: Option<bool>,
    pub client_order_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaceOrderResponse {
    /// Stringified u64 per Binance convention (avoids JS 53-bit precision loss).
    pub order_id: String,
    pub client_order_id: Option<u64>,
    pub status: &'static str,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelOrderRequest {
    /// Server-generated order ID returned by the place call.
    pub order_id: u64,
    pub symbol: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AmendOrderRequest {
    pub order_id: u64,
    pub symbol: String,
    /// Decimal as string, e.g. "59500".
    pub new_price: Option<String>,
    pub new_quantity: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelOrderResponse {
    pub order_id: String,
    pub symbol: String,
    pub status: String,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AckResponse {
    pub order_id: String,
    pub status: &'static str,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: &'static str,
}

// ── Account endpoints ────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct AccountInfo {
    pub user_id: String,
    pub balances: Vec<WalletInfo>,
}

#[derive(Debug, Serialize)]
pub struct WalletInfo {
    pub wallet_type: String,
    pub available: String,
    pub frozen: String,
    pub margin: String,
}

#[derive(Debug, Serialize)]
pub struct PositionInfo {
    pub symbol: String,
    pub side: String,
    pub size: String,
    pub entry_price: String,
    pub mark_price: String,
    pub unrealized_pnl: String,
    pub leverage: String,
    pub margin_mode: String,
}

// ── Market data endpoints ────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct DepthResponse {
    pub symbol: String,
    pub bids: Vec<[String; 2]>,
    pub asks: Vec<[String; 2]>,
    pub timestamp: u64,
}

#[derive(Debug, Serialize)]
pub struct TradeResponse {
    pub trade_id: String,
    pub symbol: String,
    pub price: String,
    pub qty: String,
    pub side: String,
    pub timestamp: u64,
}

#[derive(Debug, Serialize)]
pub struct KlineResponse {
    pub open_time: u64,
    pub open: String,
    pub high: String,
    pub low: String,
    pub close: String,
    pub volume: String,
    pub close_time: u64,
}

#[derive(Debug, Serialize)]
pub struct TickerResponse {
    pub symbol: String,
    pub last_price: String,
    pub mark_price: String,
    pub index_price: String,
    pub high_24h: String,
    pub low_24h: String,
    pub volume_24h: String,
    pub price_change_pct: String,
}

// ── Auth endpoints ───────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
    pub totp_code: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct LoginResponse {
    pub token: String,
    pub expires_at: u64,
}

#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub email: String,
    pub password: String,
}

// ── Transfer/Withdraw ────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct TransferRequest {
    pub from_wallet: String,
    pub to_wallet: String,
    pub amount: String,
}

#[derive(Debug, Deserialize)]
pub struct SetLeverageRequest {
    pub symbol: String,
    pub leverage: u32,
}
