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
    cfg.admin.admin_secret = "a".repeat(32);
    cfg.server.admin_port = 0;
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
    cfg.auth.jwt_secret = "CHANGE-ME-DEV-ONLY-MUST-BE-AT-LEAST-32-BYTES-OK".into();
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

// ── Stage 1b: WAL replay invariants ──────────────────────────────────────────

#[actix_web::test]
async fn boot_panics_on_corrupt_wal_crc() {
    let tmp = TempDir::new().unwrap();
    let wal_dir = tmp.path().join("wal");
    std::fs::create_dir(&wal_dir).unwrap();

    // recover_state only returns WalError::Corrupt for CRC errors in non-last
    // segments (last-segment corrupt records are truncated as partial writes).
    // Strategy: write one valid record in seg-0, then a corrupt record in seg-1
    // which becomes non-last once we add seg-2 with a valid CRC'd record.
    //
    // Actual record layout (from crates/exg-wal/src/segment.rs::encode_record):
    //   [seq: u64 LE][payload_len: u32 LE][payload][crc32: u32 LE]
    // CRC covers: seq_bytes ++ payload_len_bytes ++ payload
    use exg_wal::{WalConfig, WalWriter};

    // Step 1: write seq=0 via the real writer (produces seg-0).
    {
        let mut w = WalWriter::open(WalConfig {
            dir: wal_dir.clone(),
            segment_size: 64 * 1024 * 1024,
            flush_interval_us: 1000,
            flush_every_n: 1,
        })
        .unwrap();
        w.append(b"ok").unwrap();
        w.flush().unwrap();
    }

    // Step 2: hand-craft seg-1 (first_seq=1) with a corrupt CRC at seq=1.
    // This is a non-last segment once we add seg-2 below.
    let mut seg1_bytes = Vec::new();
    seg1_bytes.extend_from_slice(&1u64.to_le_bytes()); // seq = 1
    seg1_bytes.extend_from_slice(&0u32.to_le_bytes()); // payload_len = 0
    seg1_bytes.extend_from_slice(&0xDEADBEEFu32.to_le_bytes()); // wrong CRC
    std::fs::write(wal_dir.join("wal-00000000000000000001.log"), &seg1_bytes).unwrap();

    // Step 3: hand-craft seg-2 (first_seq=2) so seg-1 becomes non-last.
    // Compute a valid CRC for seq=2, payload=b"ok".
    let seq2: u64 = 2;
    let payload2 = b"ok";
    let crc2 = {
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(&seq2.to_le_bytes());
        hasher.update(&(payload2.len() as u32).to_le_bytes());
        hasher.update(payload2);
        hasher.finalize()
    };
    let mut seg2_bytes = Vec::new();
    seg2_bytes.extend_from_slice(&seq2.to_le_bytes());
    seg2_bytes.extend_from_slice(&(payload2.len() as u32).to_le_bytes());
    seg2_bytes.extend_from_slice(payload2);
    seg2_bytes.extend_from_slice(&crc2.to_le_bytes());
    std::fs::write(wal_dir.join("wal-00000000000000000002.log"), &seg2_bytes).unwrap();

    let mut cfg = base_cfg(tmp.path());
    cfg.wal.dir = wal_dir.to_string_lossy().into_owned();

    let result = exg_server::run_with_config(cfg).await;
    let err = result.err().expect("expected Err from corrupt WAL");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("CRC") || msg.contains("Corrupt") || msg.contains("corrupt"),
        "expected CRC/corruption message, got: {msg}"
    );
}

#[actix_web::test]
async fn boot_panics_on_sequence_gap() {
    use exg_wal::{WalConfig, WalWriter};

    let tmp = TempDir::new().unwrap();
    let wal_dir = tmp.path().join("wal");
    std::fs::create_dir(&wal_dir).unwrap();

    // Write three valid records (seq 0, 1, 2) via the real writer.
    {
        let mut w = WalWriter::open(WalConfig {
            dir: wal_dir.clone(),
            segment_size: 64 * 1024 * 1024,
            flush_interval_us: 1000,
            flush_every_n: 1,
        })
        .unwrap();
        for _ in 0..3 {
            w.append(b"hello").unwrap();
        }
        w.flush().unwrap();
    }

    // Truncate the segment to keep only the first record (seq=0).
    // Record layout: [seq: u64 LE (8)][payload_len: u32 LE (4)][payload (5)][crc: u32 LE (4)] = 21 bytes.
    let segments: Vec<_> = std::fs::read_dir(&wal_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    assert_eq!(segments.len(), 1, "expected exactly one segment");
    let seg0 = &segments[0];
    let raw = std::fs::read(seg0).unwrap();
    // First record: 8 (seq) + 4 (payload_len) + 5 (payload "hello") + 4 (crc) = 21 bytes.
    let first_record_size: usize = 8 + 4 + 5 + 4;
    let truncated = raw[..first_record_size].to_vec();
    std::fs::write(seg0, &truncated).unwrap();

    // Construct a second segment at first_seq=5 with one valid record at seq=5.
    // CRC input: seq.to_le_bytes() ++ payload_len.to_le_bytes() ++ payload
    let seq: u64 = 5;
    let payload = b"hello";
    let payload_len = payload.len() as u32;
    let crc = {
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(&seq.to_le_bytes());
        hasher.update(&payload_len.to_le_bytes());
        hasher.update(payload);
        hasher.finalize()
    };
    let mut second = Vec::new();
    second.extend_from_slice(&seq.to_le_bytes());
    second.extend_from_slice(&payload_len.to_le_bytes());
    second.extend_from_slice(payload);
    second.extend_from_slice(&crc.to_le_bytes());
    std::fs::write(wal_dir.join("wal-00000000000000000005.log"), &second).unwrap();

    let mut cfg = base_cfg(tmp.path());
    cfg.wal.dir = wal_dir.to_string_lossy().into_owned();

    let result = exg_server::run_with_config(cfg).await;
    let err = result.err().expect("expected Err from sequence gap");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("sequence gap") || msg.contains("gap"),
        "expected sequence-gap message, got: {msg}"
    );
}

#[actix_web::test]
async fn boot_panics_on_unknown_order_filled() {
    use exg_protocol::Event;
    use exg_wal::{WalConfig, WalWriter};

    let tmp = TempDir::new().unwrap();
    let wal_dir = tmp.path().join("wal");
    std::fs::create_dir(&wal_dir).unwrap();

    // Write a single OrderFilled event for an order that was never accepted.
    {
        let mut w = WalWriter::open(WalConfig {
            dir: wal_dir.clone(),
            segment_size: 64 * 1024 * 1024,
            flush_interval_us: 1000,
            flush_every_n: 1,
        })
        .unwrap();
        let evt = Event::OrderFilled {
            order_id: exg_common::OrderId::new(9999),
            trade_id: exg_common::TradeId::new(1),
            user_id: exg_common::UserId::new(42),
            symbol: exg_common::SymbolId::new(1),
            side: exg_common::Side::Buy,
            fill_price: "50000".parse().unwrap(),
            fill_qty: "1.0".parse().unwrap(),
            is_maker: false,
            remaining_qty: exg_common::Decimal128::ZERO,
            timestamp: exg_common::UnixMicros::now(),
        };
        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&evt).unwrap();
        w.append(&bytes).unwrap();
        w.flush().unwrap();
    }

    let mut cfg = base_cfg(tmp.path());
    cfg.wal.dir = wal_dir.to_string_lossy().into_owned();
    // WAL replay happens after the PG connectivity check. Override the DB URL
    // from DATABASE_URL env var (set by the test runner) so the PG step passes
    // and boot proceeds to replay, where the unknown-order error surfaces.
    if let Ok(url) = std::env::var("DATABASE_URL") {
        cfg.database.url = url;
    }

    let result = exg_server::run_with_config(cfg).await;
    let err = result
        .err()
        .expect("expected Err from unknown-order replay");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("WAL replay failed at sequence")
            && (msg.contains("UnknownOrder")
                || msg.contains("unknown order")
                || msg.contains("unknown order_id")),
        "expected replay-apply-error message containing UnknownOrder, got: {msg}"
    );
}

#[actix_web::test]
async fn boot_panics_on_short_admin_secret() {
    let tmp = TempDir::new().unwrap();
    let mut cfg = base_cfg(tmp.path());
    cfg.admin.admin_secret = "short".into();
    let result = exg_server::run_with_config(cfg).await;
    let err = result.err().expect("expected Err");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("admin.admin_secret") && msg.contains("32 bytes"),
        "expected admin secret length panic, got: {msg}"
    );
}

#[actix_web::test]
async fn boot_panics_on_placeholder_admin_secret() {
    let tmp = TempDir::new().unwrap();
    let mut cfg = base_cfg(tmp.path());
    cfg.admin.admin_secret = "CHANGE-ME-ADMIN-DEV-ONLY-MUST-BE-32-BYTES".into();
    let result = exg_server::run_with_config(cfg).await;
    let err = result.err().expect("expected Err");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("admin.admin_secret") && msg.contains("placeholder"),
        "expected admin secret placeholder panic, got: {msg}"
    );
}
