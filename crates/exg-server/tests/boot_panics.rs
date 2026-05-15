//! Stage 0 boot-time invariant guards (spec §9 invariants 1-4) and
//! Stage 1a boot-time invariant guards (spec §9 invariants 11-13).
//! These tests ensure that misconfigurations fail loudly at startup
//! rather than silently misbehaving in production.

use exg_config::ExgConfig;
use tempfile::TempDir;

fn base_cfg(wal_dir: &std::path::Path) -> ExgConfig {
    let mut cfg = ExgConfig::default_config();
    cfg.wal.dir = wal_dir.to_string_lossy().into_owned();
    cfg.server.port = 0; // ephemeral
    // Stage 1a: override the placeholder to a valid 32-byte secret so the
    // 4 non-auth invariant tests don't trip on the JWT placeholder check.
    cfg.auth.jwt_secret = "a".repeat(32);
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
    // Give the duplicate a distinct id + name so ExgConfig::validate (now
    // called from inside run_with_config) doesn't trip on its duplicate-id
    // check before the symbol-count check fires. This test verifies the
    // count check specifically.
    let mut extra = cfg.trading.symbols[0].clone();
    extra.id = 2;
    extra.name = "ETHUSDT".into();
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
    // run_with_config now calls ExgConfig::validate(), so the boot path
    // itself rejects an invalid mark price — covering the in-process /
    // programmatic-mutation path as well as the file-load path.
    let result = exg_server::run_with_config(cfg).await;
    let err = result.err().expect("expected Err from run_with_config");
    let msg = format!("{err:#}");
    assert!(msg.contains("mark_price"), "got: {msg}");
}

// ── Stage 1a: invariants 11-13 ────────────────────────────────────────────

#[actix_web::test]
async fn boot_panics_on_short_jwt_secret() {
    let tmp = TempDir::new().unwrap();
    let mut cfg = base_cfg(tmp.path());
    cfg.auth.jwt_secret = "short".into();
    let result = exg_server::run_with_config(cfg).await;
    let err = result.err().expect("expected Err");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("jwt_secret"),
        "expected jwt_secret-length message, got: {msg}"
    );
}

#[actix_web::test]
async fn boot_panics_on_default_jwt_secret() {
    let tmp = TempDir::new().unwrap();
    let mut cfg = base_cfg(tmp.path());
    cfg.auth.jwt_secret =
        "CHANGE-ME-DEV-ONLY-MUST-BE-AT-LEAST-32-BYTES-OK".into();
    let result = exg_server::run_with_config(cfg).await;
    let err = result.err().expect("expected Err");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("jwt_secret") || msg.contains("placeholder"),
        "expected placeholder-rejection message, got: {msg}"
    );
}

#[actix_web::test]
async fn boot_panics_on_db_unreachable() {
    let tmp = TempDir::new().unwrap();
    let mut cfg = base_cfg(tmp.path());
    // Point at a definitely-unreachable port to force connect failure.
    cfg.database.url = "postgres://exg:exg_dev_password@127.0.0.1:1/exg".into();
    let result = exg_server::run_with_config(cfg).await;
    let err = result.err().expect("expected Err");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("PG") || msg.contains("connect") || msg.contains("SELECT 1"),
        "expected PG-connect-failure message, got: {msg}"
    );
}
