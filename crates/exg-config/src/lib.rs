mod validation;

use std::path::Path;

use serde::{Deserialize, Serialize};

// ── Error ──────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("config error: {0}")]
    Config(#[from] config::ConfigError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("validation error: {0}")]
    Validation(String),
}

// ── Top-level config ──────────────────────────────────────────���────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    /// Must be at least 32 bytes (256 bits) for HS256 security. Boot validates.
    pub jwt_secret: String,
    /// JWT access token lifetime in seconds. Stage 1a defaults to 86400 (24h).
    pub jwt_expiry_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminConfig {
    /// Must be at least 32 bytes for admin HTTP auth. Boot validates.
    pub admin_secret: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExgConfig {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub redis: RedisConfig,
    pub nats: NatsConfig,
    pub wal: WalConfig,
    pub ringbuffer: RingBufferConfig,
    pub trading: TradingConfig,
    pub risk: RiskConfig,
    pub auth: AuthConfig,
    pub admin: AdminConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub ws_port: u16,
    pub admin_port: u16,
    /// Snowflake node ID, must be in 0..=1023.
    pub node_id: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    pub url: String,
    pub max_connections: u32,
    pub min_connections: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedisConfig {
    pub url: String,
    pub pool_size: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NatsConfig {
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalConfig {
    pub dir: String,
    pub segment_size_mb: usize,
    pub flush_interval_us: u64,
    pub flush_every_n: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RingBufferConfig {
    /// Must be a power of 2.
    pub slot_count: usize,
    pub slot_size: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradingConfig {
    pub symbols: Vec<SymbolConfigEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolConfigEntry {
    pub id: u16,
    pub name: String,
    pub base_asset: String,
    pub quote_asset: String,
    /// One of: "perpetual_linear", "perpetual_inverse", "spot".
    pub symbol_type: String,
    /// One of: "trading", "halted", etc.
    pub status: String,
    /// Decimal as string, e.g. "0.01".
    pub tick_size: String,
    /// Decimal as string, e.g. "0.001".
    pub lot_size: String,
    /// Decimal as string, e.g. "10".
    pub min_notional: String,
    /// Decimal as string, e.g. "125".
    pub max_leverage: String,
    /// Decimal as string, e.g. "0.0002".
    pub maker_fee: String,
    /// Decimal as string, e.g. "0.0005".
    pub taker_fee: String,
    /// Static mark price for Stage 0; replaced by oracle/mark service in Stage 2.
    /// Decimal as string, e.g. "60000".
    pub mark_price: String,
    pub margin_tiers: Vec<MarginTierEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarginTierEntry {
    pub notional_floor: String,
    pub notional_cap: String,
    pub maintenance_margin_rate: String,
    pub maintenance_amount: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskConfig {
    pub max_orders_per_second: u32,
    pub max_cancels_per_second: u32,
    /// E.g. "0.05" for 5%.
    pub price_band_pct: String,
    pub max_position_notional: String,
    /// Typically 8.
    pub funding_interval_hours: u32,
    /// E.g. "0.0001".
    pub interest_rate: String,
    /// For funding rate calculation.
    pub impact_notional: String,
}

// ── Loading ────────────────────────────────────────────────────────────────

impl ExgConfig {
    /// Load config from a TOML file, with environment variable overrides.
    ///
    /// Environment variables use the pattern `EXG_{SECTION}_{KEY}`
    /// (e.g. `EXG_DATABASE_URL`, `EXG_SERVER_PORT`).
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        Self::load_with_prefix(path, "EXG")
    }

    /// Load with an explicit environment prefix override.
    pub fn load_with_prefix(path: &Path, prefix: &str) -> Result<Self, ConfigError> {
        let cfg = config::Config::builder()
            .add_source(config::File::from(path).required(true))
            .add_source(
                config::Environment::with_prefix(prefix)
                    .separator("_")
                    .try_parsing(true),
            )
            .build()?;

        let parsed: Self = cfg.try_deserialize()?;
        parsed.validate()?;
        Ok(parsed)
    }

    /// Default config suitable for local development / testing.
    pub fn default_config() -> Self {
        Self {
            server: ServerConfig {
                host: "127.0.0.1".into(),
                port: 8080,
                ws_port: 8081,
                admin_port: 9090,
                node_id: 1,
            },
            database: DatabaseConfig {
                url: "postgres://exg:exg@localhost:5432/exg".into(),
                max_connections: 20,
                min_connections: 2,
            },
            redis: RedisConfig {
                url: "redis://localhost:6379".into(),
                pool_size: 8,
            },
            nats: NatsConfig {
                url: "nats://localhost:4222".into(),
            },
            wal: WalConfig {
                dir: "./data/wal".into(),
                segment_size_mb: 64,
                flush_interval_us: 1000,
                flush_every_n: 1000,
            },
            ringbuffer: RingBufferConfig {
                slot_count: 65536,
                slot_size: 4096,
            },
            trading: TradingConfig {
                symbols: vec![Self::default_btcusdt()],
            },
            risk: RiskConfig {
                max_orders_per_second: 300,
                max_cancels_per_second: 600,
                price_band_pct: "0.05".into(),
                max_position_notional: "10000000".into(),
                funding_interval_hours: 8,
                interest_rate: "0.0001".into(),
                impact_notional: "200".into(),
            },
            auth: AuthConfig {
                jwt_secret: "CHANGE-ME-DEV-ONLY-MUST-BE-AT-LEAST-32-BYTES-OK".into(),
                jwt_expiry_secs: 86400,
            },
            admin: AdminConfig {
                admin_secret: "CHANGE-ME-ADMIN-DEV-ONLY-MUST-BE-32-BYTES".into(),
            },
        }
    }

    fn default_btcusdt() -> SymbolConfigEntry {
        SymbolConfigEntry {
            id: 1,
            name: "BTCUSDT".into(),
            base_asset: "BTC".into(),
            quote_asset: "USDT".into(),
            symbol_type: "perpetual_linear".into(),
            status: "trading".into(),
            tick_size: "0.01".into(),
            lot_size: "0.001".into(),
            min_notional: "10".into(),
            max_leverage: "125".into(),
            maker_fee: "0.0002".into(),
            taker_fee: "0.0005".into(),
            mark_price: "60000".into(),
            margin_tiers: vec![
                MarginTierEntry {
                    notional_floor: "0".into(),
                    notional_cap: "50000".into(),
                    maintenance_margin_rate: "0.004".into(),
                    maintenance_amount: "0".into(),
                },
                MarginTierEntry {
                    notional_floor: "50000".into(),
                    notional_cap: "250000".into(),
                    maintenance_margin_rate: "0.005".into(),
                    maintenance_amount: "50".into(),
                },
                MarginTierEntry {
                    notional_floor: "250000".into(),
                    notional_cap: "1000000".into(),
                    maintenance_margin_rate: "0.01".into(),
                    maintenance_amount: "1300".into(),
                },
                MarginTierEntry {
                    notional_floor: "1000000".into(),
                    notional_cap: "5000000".into(),
                    maintenance_margin_rate: "0.025".into(),
                    maintenance_amount: "16300".into(),
                },
                MarginTierEntry {
                    notional_floor: "5000000".into(),
                    notional_cap: "20000000".into(),
                    maintenance_margin_rate: "0.05".into(),
                    maintenance_amount: "141300".into(),
                },
                MarginTierEntry {
                    notional_floor: "20000000".into(),
                    notional_cap: "100000000".into(),
                    maintenance_margin_rate: "0.1".into(),
                    maintenance_amount: "1141300".into(),
                },
                MarginTierEntry {
                    notional_floor: "100000000".into(),
                    notional_cap: "200000000".into(),
                    maintenance_margin_rate: "0.125".into(),
                    maintenance_amount: "3641300".into(),
                },
                MarginTierEntry {
                    notional_floor: "200000000".into(),
                    notional_cap: "500000000".into(),
                    maintenance_margin_rate: "0.25".into(),
                    maintenance_amount: "28641300".into(),
                },
            ],
        }
    }

    /// Validate all configuration invariants.
    pub fn validate(&self) -> Result<(), ConfigError> {
        validation::validate(self)
    }
}

#[cfg(test)]
mod tests;
