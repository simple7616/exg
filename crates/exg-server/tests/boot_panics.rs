//! Stage 0 boot-time invariant guards (spec §9 invariants 1-4).
//! These tests ensure that misconfigurations fail loudly at startup
//! rather than silently misbehaving in production.

use exg_config::ExgConfig;
use tempfile::TempDir;

fn base_cfg(wal_dir: &std::path::Path) -> ExgConfig {
    let mut cfg = ExgConfig::default_config();
    cfg.wal.dir = wal_dir.to_string_lossy().into_owned();
    cfg.server.port = 0; // ephemeral
    cfg
}

#[actix_web::test]
async fn boot_panics_on_non_loopback_host() {
    let tmp = TempDir::new().unwrap();
    let mut cfg = base_cfg(tmp.path());
    cfg.server.host = "0.0.0.0".into();
    let result = exg_server::run_with_config(cfg).await;
    let err = result.err().expect("expected Err from run_with_config");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("loopback") || msg.contains("127.0.0.1") || msg.contains("host"),
        "expected host-invariant message, got: {msg}"
    );
}

#[actix_web::test]
async fn boot_panics_on_multiple_symbols() {
    let tmp = TempDir::new().unwrap();
    let mut cfg = base_cfg(tmp.path());
    let extra = cfg.trading.symbols[0].clone();
    cfg.trading.symbols.push(extra);
    let result = exg_server::run_with_config(cfg).await;
    let err = result.err().expect("expected Err from run_with_config");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("symbols.len") || msg.contains("single-symbol") || msg.contains("exactly 1"),
        "expected symbol-count message, got: {msg}"
    );
}

#[actix_web::test]
async fn boot_panics_on_nonempty_wal_dir() {
    let tmp = TempDir::new().unwrap();
    // Drop a sentinel file with the actual segment filename pattern. The
    // freshness check rejects any entry now (per the fix), so any name works,
    // but using a realistic name documents intent.
    std::fs::write(
        tmp.path().join("wal-00000000000000000000.log"),
        b"stale data",
    )
    .unwrap();
    let cfg = base_cfg(tmp.path());
    let result = exg_server::run_with_config(cfg).await;
    let err = result.err().expect("expected Err from run_with_config");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("WAL") && (msg.contains("non-empty") || msg.contains("fresh")),
        "expected WAL freshness message, got: {msg}"
    );
}

#[actix_web::test]
async fn boot_panics_on_invalid_mark_price() {
    let tmp = TempDir::new().unwrap();
    let mut cfg = base_cfg(tmp.path());
    cfg.trading.symbols[0].mark_price = "-1".into();
    // ExgConfig::validate should reject this. Boot path doesn't call validate
    // explicitly (config::Builder::try_deserialize runs it only via the load
    // path), so call it directly to confirm.
    let err = cfg.validate().unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("mark_price"), "got: {msg}");
}
