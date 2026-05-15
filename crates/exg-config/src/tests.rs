use std::io::Write;

use super::*;

#[test]
fn test_default_config_validates() {
    let mut cfg = ExgConfig::default_config();
    cfg.auth.jwt_secret = "a".repeat(32);
    cfg.validate().expect("default config should validate");
}

#[test]
fn test_load_default_toml() {
    // Use the workspace-root config/default.toml
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let workspace_root = Path::new(manifest_dir).parent().unwrap().parent().unwrap();
    let config_path = workspace_root.join("config").join("default.toml");

    // Deserialize the TOML file directly via the config crate (no env overlay,
    // no validation) to verify field values parse correctly. Validation is tested
    // by other tests; this test's purpose is purely structural/parsing.
    let built = config::Config::builder()
        .add_source(config::File::from(config_path.as_path()).required(true))
        .build()
        .expect("config::Config::builder should succeed");
    let mut cfg: ExgConfig = built.try_deserialize().expect("should parse default.toml");

    // Override placeholder so any downstream validate call (if added later) works.
    cfg.auth.jwt_secret = "a".repeat(32);

    assert_eq!(cfg.server.host, "127.0.0.1");
    assert_eq!(cfg.server.port, 8080);
    assert!(!cfg.trading.symbols.is_empty());
    assert_eq!(cfg.trading.symbols[0].name, "BTCUSDT");
}

#[test]
fn test_env_override() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.toml");
    write_minimal_toml(&path);

    // Set an env var that should override the port.
    // Use a unique prefix to avoid collisions.
    // SAFETY: This test is single-threaded with respect to this env var name.
    unsafe { std::env::set_var("TESTCFG1_SERVER_PORT", "9999") };

    let cfg =
        ExgConfig::load_with_prefix(&path, "TESTCFG1").expect("should load with env override");
    assert_eq!(cfg.server.port, 9999);

    // SAFETY: Restoring env state; no concurrent readers of this var.
    unsafe { std::env::remove_var("TESTCFG1_SERVER_PORT") };
}

#[test]
fn test_invalid_node_id() {
    let mut cfg = ExgConfig::default_config();
    cfg.server.node_id = 1024;
    let err = cfg.validate().unwrap_err();
    assert!(err.to_string().contains("node_id"), "error: {err}");
}

#[test]
fn test_non_power_of_two_slot_count() {
    let mut cfg = ExgConfig::default_config();
    cfg.ringbuffer.slot_count = 100;
    let err = cfg.validate().unwrap_err();
    assert!(err.to_string().contains("power of 2"), "error: {err}");
}

#[test]
fn test_zero_slot_count() {
    let mut cfg = ExgConfig::default_config();
    cfg.ringbuffer.slot_count = 0;
    let err = cfg.validate().unwrap_err();
    assert!(err.to_string().contains("power of 2"), "error: {err}");
}

#[test]
fn test_invalid_decimal_string() {
    let mut cfg = ExgConfig::default_config();
    cfg.risk.price_band_pct = "not_a_number".into();
    let err = cfg.validate().unwrap_err();
    assert!(err.to_string().contains("invalid decimal"), "error: {err}");
}

#[test]
fn test_invalid_tick_size() {
    let mut cfg = ExgConfig::default_config();
    cfg.trading.symbols[0].tick_size = "0".into();
    let err = cfg.validate().unwrap_err();
    assert!(err.to_string().contains("tick_size"), "error: {err}");
}

#[test]
fn test_invalid_lot_size() {
    let mut cfg = ExgConfig::default_config();
    cfg.trading.symbols[0].lot_size = "-1".into();
    let err = cfg.validate().unwrap_err();
    assert!(err.to_string().contains("lot_size"), "error: {err}");
}

#[test]
fn test_negative_maker_fee() {
    let mut cfg = ExgConfig::default_config();
    cfg.trading.symbols[0].maker_fee = "-0.01".into();
    let err = cfg.validate().unwrap_err();
    assert!(err.to_string().contains("maker_fee"), "error: {err}");
}

#[test]
fn test_duplicate_symbol_ids() {
    let mut cfg = ExgConfig::default_config();
    let mut dup = cfg.trading.symbols[0].clone();
    dup.name = "ETHUSDT".into();
    // Same id=1
    cfg.trading.symbols.push(dup);
    let err = cfg.validate().unwrap_err();
    assert!(
        err.to_string().contains("duplicate symbol id"),
        "error: {err}"
    );
}

#[test]
fn test_overlapping_margin_tiers() {
    let mut cfg = ExgConfig::default_config();
    // Make tier 1 overlap with tier 0 by setting its floor below tier 0's cap.
    cfg.trading.symbols[0].margin_tiers[1].notional_floor = "10000".into();
    let err = cfg.validate().unwrap_err();
    assert!(err.to_string().contains("overlap"), "error: {err}");
}

#[test]
fn test_missing_file() {
    let result = ExgConfig::load(Path::new("/nonexistent/path/config.toml"));
    assert!(result.is_err());
}

#[test]
fn test_toml_parsing_all_types() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("full.toml");
    write_minimal_toml(&path);

    let cfg = ExgConfig::load_with_prefix(&path, "TESTCFG_PARSE_UNLIKELY")
        .expect("should parse all types");

    assert_eq!(cfg.server.port, 8080);
    assert_eq!(cfg.database.max_connections, 20);
    assert_eq!(cfg.wal.flush_interval_us, 1000);
    assert_eq!(cfg.ringbuffer.slot_count, 65536);
    assert_eq!(cfg.risk.funding_interval_hours, 8);
    assert_eq!(cfg.trading.symbols[0].margin_tiers.len(), 2);
}

#[test]
fn test_symbol_mark_price_field_parses() {
    let mut cfg = ExgConfig::default_config();
    cfg.auth.jwt_secret = "a".repeat(32);
    cfg.trading.symbols[0].mark_price = "60000".into();
    assert!(cfg.validate().is_ok());
}

#[test]
fn test_symbol_mark_price_must_be_positive() {
    let mut cfg = ExgConfig::default_config();
    cfg.trading.symbols[0].mark_price = "0".into();
    let err = cfg.validate().unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("mark_price"), "msg: {msg}");
}

#[test]
fn test_symbol_mark_price_must_parse_as_decimal() {
    let mut cfg = ExgConfig::default_config();
    cfg.trading.symbols[0].mark_price = "not-a-number".into();
    assert!(cfg.validate().is_err());
}

#[test]
fn test_auth_jwt_secret_too_short_rejected() {
    let mut cfg = ExgConfig::default_config();
    cfg.auth.jwt_secret = "short".into();
    let err = cfg.validate().unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("jwt_secret"), "msg: {msg}");
}

#[test]
fn test_auth_jwt_secret_placeholder_rejected() {
    let mut cfg = ExgConfig::default_config();
    cfg.auth.jwt_secret = "CHANGE-ME-DEV-ONLY-MUST-BE-AT-LEAST-32-BYTES-OK".into();
    let err = cfg.validate().unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("jwt_secret") || msg.contains("placeholder"));
}

#[test]
fn test_auth_jwt_secret_valid_32_bytes_ok() {
    let mut cfg = ExgConfig::default_config();
    cfg.auth.jwt_secret = "a".repeat(32);
    assert!(cfg.validate().is_ok());
}

#[test]
fn test_auth_jwt_expiry_zero_rejected() {
    let mut cfg = ExgConfig::default_config();
    cfg.auth.jwt_secret = "a".repeat(32); // override placeholder so we hit the expiry check
    cfg.auth.jwt_expiry_secs = 0;
    let err = cfg.validate().unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("jwt_expiry"), "msg: {msg}");
}

#[test]
fn test_database_url_format_sanity() {
    let cfg = ExgConfig::default_config();
    assert!(
        cfg.database.url.starts_with("postgres://"),
        "default url: {}",
        cfg.database.url
    );
}

/// Write a minimal valid TOML config for testing.
fn write_minimal_toml(path: &Path) {
    let toml = r#"
[server]
host = "127.0.0.1"
port = 8080
ws_port = 8081
admin_port = 9090
node_id = 1

[database]
url = "postgres://exg:exg@localhost:5432/exg"
max_connections = 20
min_connections = 2

[redis]
url = "redis://localhost:6379"
pool_size = 8

[nats]
url = "nats://localhost:4222"

[wal]
dir = "./data/wal"
segment_size_mb = 64
flush_interval_us = 1000
flush_every_n = 1000

[ringbuffer]
slot_count = 65536
slot_size = 4096

[risk]
max_orders_per_second = 300
max_cancels_per_second = 600
price_band_pct = "0.05"
max_position_notional = "10000000"
funding_interval_hours = 8
interest_rate = "0.0001"
impact_notional = "200"

[[trading.symbols]]
id = 1
name = "BTCUSDT"
base_asset = "BTC"
quote_asset = "USDT"
symbol_type = "perpetual_linear"
status = "trading"
tick_size = "0.01"
lot_size = "0.001"
min_notional = "10"
max_leverage = "125"
maker_fee = "0.0002"
taker_fee = "0.0005"
mark_price = "60000"

[[trading.symbols.margin_tiers]]
notional_floor = "0"
notional_cap = "50000"
maintenance_margin_rate = "0.004"
maintenance_amount = "0"

[[trading.symbols.margin_tiers]]
notional_floor = "50000"
notional_cap = "250000"
maintenance_margin_rate = "0.005"
maintenance_amount = "50"

[auth]
jwt_secret = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
jwt_expiry_secs = 86400
"#;
    let mut f = std::fs::File::create(path).unwrap();
    f.write_all(toml.as_bytes()).unwrap();
}
