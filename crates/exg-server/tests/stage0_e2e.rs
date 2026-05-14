//! Stage 0 end-to-end integration tests.
//! Boots the server in-process via `run_with_config`, fires reqwest calls,
//! then walks the WAL to assert the expected event sequence.

use exg_config::ExgConfig;
use exg_protocol::Event;
use exg_wal::WalReader;
use reqwest::Client;
use std::time::Duration;
use tempfile::TempDir;

fn base_cfg(wal_dir: &std::path::Path) -> ExgConfig {
    let mut cfg = ExgConfig::default_config();
    cfg.wal.dir = wal_dir.to_string_lossy().into_owned();
    cfg.server.host = "127.0.0.1".into();
    cfg.server.port = 0; // ephemeral
    cfg
}

async fn boot_server(cfg: ExgConfig) -> (exg_server::ServerHandle, String) {
    let handle = exg_server::run_with_config(cfg).await.expect("server boot");
    let base = format!("http://127.0.0.1:{}", handle.bound_port);
    // Health-poll until ready (max 5s).
    let client = Client::new();
    for _ in 0..50 {
        if client
            .get(format!("{base}/api/v1/health"))
            .timeout(Duration::from_millis(100))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
        {
            return (handle, base);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("server did not become ready");
}

fn read_events(wal_dir: &std::path::Path) -> Vec<Event> {
    let mut reader = WalReader::open(wal_dir).unwrap();
    let mut events = Vec::new();
    reader
        .read_from(0, |_seq, payload| {
            // rkyv requires 16-byte aligned input; the mmap slice may be
            // misaligned. Copy to owned Vec first (same pattern as wal-dump).
            let owned: Vec<u8> = payload.to_vec();
            let e: Event = rkyv::from_bytes::<Event, rkyv::rancor::Error>(&owned)
                .expect("rkyv decode WAL event");
            events.push(e);
            true
        })
        .unwrap();
    events
}

#[actix_web::test]
async fn place_cancel_amend_happy_path() {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let wal_dir = std::path::PathBuf::from(&cfg.wal.dir);
    let (handle, base) = boot_server(cfg).await;
    let client = Client::new();

    // Place
    let place: serde_json::Value = client
        .post(format!("{base}/api/v1/order"))
        .header("X-User-Id", "42")
        .json(&serde_json::json!({
            "symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT",
            "timeInForce":"GTC","quantity":"0.001","price":"59000"
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .expect("place 200")
        .json()
        .await
        .unwrap();
    let order_id: u64 = place["orderId"].as_str().unwrap().parse().unwrap();
    assert_eq!(place["status"], "ACCEPTED");

    // Amend
    let amend = client
        .post(format!("{base}/api/v1/order/amend"))
        .header("X-User-Id", "42")
        .json(&serde_json::json!({
            "orderId": order_id, "symbol":"BTCUSDT", "newPrice":"59500"
        }))
        .send()
        .await
        .unwrap();
    assert!(amend.status().is_success(), "amend status {}", amend.status());

    // Cancel
    let cancel = client
        .post(format!("{base}/api/v1/order/cancel"))
        .header("X-User-Id", "42")
        .json(&serde_json::json!({"orderId": order_id, "symbol":"BTCUSDT"}))
        .send()
        .await
        .unwrap();
    assert!(cancel.status().is_success(), "cancel status {}", cancel.status());

    // Give the matching thread a moment to drain.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Shutdown then inspect WAL.
    handle.shutdown().await.unwrap();
    let events = read_events(&wal_dir);
    assert!(!events.is_empty(), "WAL should contain events");
    // At minimum we expect OrderAccepted from the place call.
    assert!(
        events.iter().any(|e| matches!(e, Event::OrderAccepted { .. })),
        "expected at least one OrderAccepted, got: {events:?}"
    );
}

#[actix_web::test]
async fn missing_x_user_id_returns_401() {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let (handle, base) = boot_server(cfg).await;
    let client = Client::new();

    let resp = client
        .post(format!("{base}/api/v1/order"))
        .json(&serde_json::json!({
            "symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT",
            "timeInForce":"GTC","quantity":"0.001","price":"59000"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 401);
    // Unauthorized maps to ERR_UNAUTHORIZED = -1002 per error.rs
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["code"], -1002);

    handle.shutdown().await.unwrap();
}

#[actix_web::test]
async fn backpressure_returns_429() {
    let tmp = TempDir::new().unwrap();
    let mut cfg = base_cfg(tmp.path());
    cfg.ringbuffer.slot_count = 4; // force tiny buffer
    let (handle, base) = boot_server(cfg).await;
    let client = Client::new();

    // Fire many concurrent requests, expect at least one 429.
    let mut joinset = tokio::task::JoinSet::new();
    for _ in 0..64 {
        let c = client.clone();
        let url = format!("{base}/api/v1/order");
        joinset.spawn(async move {
            c.post(url)
                .header("X-User-Id", "42")
                .json(&serde_json::json!({
                    "symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT",
                    "timeInForce":"GTC","quantity":"0.001","price":"59000"
                }))
                .send()
                .await
                .map(|r| r.status().as_u16())
                .unwrap_or(0)
        });
    }
    let mut saw_backpressure = false;
    while let Some(res) = joinset.join_next().await {
        if res.unwrap_or(0) == 429 {
            saw_backpressure = true;
        }
    }
    assert!(saw_backpressure, "expected at least one 429 under backpressure");
    handle.shutdown().await.unwrap();
}

#[actix_web::test]
async fn idor_cancel_with_wrong_user_id_is_rejected() {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let wal_dir = std::path::PathBuf::from(&cfg.wal.dir);
    let (handle, base) = boot_server(cfg).await;
    let client = Client::new();

    // Place as user 42.
    let place: serde_json::Value = client
        .post(format!("{base}/api/v1/order"))
        .header("X-User-Id", "42")
        .json(&serde_json::json!({
            "symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT",
            "timeInForce":"GTC","quantity":"0.001","price":"59000"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let order_id: u64 = place["orderId"].as_str().unwrap().parse().unwrap();

    // Cancel as user 999 — engine rejects with OrderNotFound (per engine.rs:442).
    let _ = client
        .post(format!("{base}/api/v1/order/cancel"))
        .header("X-User-Id", "999")
        .json(&serde_json::json!({"orderId": order_id, "symbol":"BTCUSDT"}))
        .send()
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(200)).await;
    handle.shutdown().await.unwrap();

    let events = read_events(&wal_dir);
    let has_rejected = events.iter().any(|e| {
        matches!(
            e,
            Event::OrderRejected {
                reason: exg_protocol::RejectReason::OrderNotFound,
                ..
            }
        )
    });
    assert!(
        has_rejected,
        "expected OrderRejected/OrderNotFound for cross-user cancel, got: {events:?}"
    );

    // The original order must not be in a canceled state.
    let canceled_for_original = events.iter().any(|e| {
        matches!(
            e,
            Event::OrderCanceled { order_id: oid, .. } if oid.value() == order_id
        )
    });
    assert!(
        !canceled_for_original,
        "user 999 must not cancel user 42's order"
    );
}

#[actix_web::test]
async fn shutdown_drains_pending_commands() {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let wal_dir = std::path::PathBuf::from(&cfg.wal.dir);
    let (handle, base) = boot_server(cfg).await;
    let client = Client::new();

    // Fire 20 concurrent orders; all that get 200 must end up in WAL.
    let mut joinset = tokio::task::JoinSet::new();
    for _ in 0..20 {
        let c = client.clone();
        let url = format!("{base}/api/v1/order");
        joinset.spawn(async move {
            c.post(url)
                .header("X-User-Id", "42")
                .json(&serde_json::json!({
                    "symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT",
                    "timeInForce":"GTC","quantity":"0.001","price":"59000"
                }))
                .send()
                .await
                .map(|r| r.status().as_u16())
                .unwrap_or(0)
        });
    }
    let mut accepted = 0;
    while let Some(res) = joinset.join_next().await {
        if res.unwrap_or(0) == 200 {
            accepted += 1;
        }
    }

    // Immediately shut down.
    handle.shutdown().await.unwrap();

    let events = read_events(&wal_dir);
    let accepted_in_wal = events
        .iter()
        .filter(|e| matches!(e, Event::OrderAccepted { .. }))
        .count();
    assert_eq!(
        accepted_in_wal, accepted,
        "every 200 ACCEPTED response must produce an OrderAccepted event in WAL"
    );
}

#[actix_web::test]
async fn duplicate_client_order_id_creates_two_orders() {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let wal_dir = std::path::PathBuf::from(&cfg.wal.dir);
    let (handle, base) = boot_server(cfg).await;
    let client = Client::new();

    let body = serde_json::json!({
        "symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT",
        "timeInForce":"GTC","quantity":"0.001","price":"59000",
        "clientOrderId": "12345"
    });

    let r1: serde_json::Value = client
        .post(format!("{base}/api/v1/order"))
        .header("X-User-Id", "42")
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let r2: serde_json::Value = client
        .post(format!("{base}/api/v1/order"))
        .header("X-User-Id", "42")
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(r1["status"], "ACCEPTED");
    assert_eq!(r2["status"], "ACCEPTED");
    assert_ne!(
        r1["orderId"], r2["orderId"],
        "stage 0 must NOT deduplicate client_order_id (spec §9 invariant 9)"
    );

    tokio::time::sleep(Duration::from_millis(200)).await;
    handle.shutdown().await.unwrap();

    let events = read_events(&wal_dir);
    let accepted = events
        .iter()
        .filter(|e| matches!(e, Event::OrderAccepted { .. }))
        .count();
    assert!(accepted >= 2, "expected ≥ 2 OrderAccepted, got {accepted}");
}

#[actix_web::test]
async fn malformed_json_returns_400() {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let (handle, base) = boot_server(cfg).await;
    let client = Client::new();

    let resp = client
        .post(format!("{base}/api/v1/order"))
        .header("X-User-Id", "42")
        .header("Content-Type", "application/json")
        .body("not json")
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_client_error(), "got {}", resp.status());

    handle.shutdown().await.unwrap();
}
