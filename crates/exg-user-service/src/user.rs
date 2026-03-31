use exg_common::{AccountId, UnixMicros, UserId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub user_id: UserId,
    pub email: String,
    pub password_hash: String,
    pub totp_secret: Option<String>,
    pub kyc_level: KycLevel,
    pub is_active: bool,
    pub created_at: UnixMicros,
    pub updated_at: UnixMicros,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KycLevel {
    L0,
    L1,
    L2,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKey {
    pub key_id: String,
    pub user_id: UserId,
    /// Raw secret stored in memory. In production, use HSM.
    pub secret_key: String,
    pub label: String,
    pub permissions: ApiPermissions,
    pub is_active: bool,
    pub created_at: UnixMicros,
    pub ip_whitelist: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiPermissions {
    pub can_trade: bool,
    pub can_withdraw: bool,
    pub can_read: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAccount {
    pub account_id: AccountId,
    pub user_id: UserId,
    pub label: String,
    pub is_active: bool,
    pub created_at: UnixMicros,
}
