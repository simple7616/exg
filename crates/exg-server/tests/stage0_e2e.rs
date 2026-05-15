//! Stage 0 end-to-end integration tests — rewritten for Stage 1a JWT auth.
//!
//! Each test uses `#[sqlx::test(migrations = "../../migrations")]` for an
//! isolated per-test PG database and `run_with_config_with_pool` to inject
//! that pool into the server. Auth is via JWT Bearer tokens obtained through
//! the register+login HTTP endpoints.

use exg_config::ExgConfig;
use exg_protocol::Event;
use exg_wal::WalReader;
use reqwest::Client;
use sqlx::PgPool;
use std::time::Duration;
use tempfile::TempDir;

fn base_cfg(wal_dir: &std::path::Path) -> ExgConfig {
    let mut cfg = ExgConfig::default_config();
    cfg.wal.dir = wal_dir.to_string_lossy().into_owned();
    cfg.server.host = "127.0.0.1".into();
    cfg.server.port = 0; // ephemeral
    cfg.auth.jwt_secret = "a".repeat(32);
    cfg
}

async fn boot_server_with_pool(cfg: ExgConfig, pool: PgPool) -> (exg_server::ServerHandle, String) {
    let handle = exg_server::run_with_config_with_pool(cfg, Some(pool))
        .await
        .expect("server boot");
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

/// Register a user (via the HTTP API) and return (access_token, user_id).
/// Ignores 409 on register (email already registered in same test DB).
async fn login_helper(client: &Client, base: &str, email: &str, password: &str) -> (String, u64) {
    // Register — ignore 409 if email already exists from a prior call.
    let _ = client
        .post(format!("{base}/api/v1/auth/register"))
        .json(&serde_json::json!({"email": email, "password": password}))
        .send()
        .await
        .unwrap();
    // Login.
    let resp: serde_json::Value = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&serde_json::json!({"email": email, "password": password}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = resp["accessToken"]
        .as_str()
        .unwrap_or_else(|| panic!("accessToken missing in login response: {resp}"))
        .to_string();
    let user_id: u64 = resp["userId"]
        .as_str()
        .unwrap_or_else(|| panic!("userId missing: {resp}"))
        .parse()
        .unwrap();
    (token, user_id)
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

#[sqlx::test(migrations = "../../migrations")]
async fn place_cancel_amend_happy_path(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let wal_dir = std::path::PathBuf::from(&cfg.wal.dir);
    let (handle, base) = boot_server_with_pool(cfg, pool).await;
    let client = Client::new();
    let (token, _user_id) =
        login_helper(&client, &base, "alice@example.com", "hunter2hunter2").await;

    // Place
    let place: serde_json::Value = client
        .post(format!("{base}/api/v1/order"))
        .header("Authorization", format!("Bearer {token}"))
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
        .header("Authorization", format!("Bearer {token}"))
        .json(&serde_json::json!({
            "orderId": order_id, "symbol":"BTCUSDT", "newPrice":"59500"
        }))
        .send()
        .await
        .unwrap();
    assert!(
        amend.status().is_success(),
        "amend status {}",
        amend.status()
    );

    // Cancel
    let cancel = client
        .post(format!("{base}/api/v1/order/cancel"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&serde_json::json!({"orderId": order_id, "symbol":"BTCUSDT"}))
        .send()
        .await
        .unwrap();
    assert!(
        cancel.status().is_success(),
        "cancel status {}",
        cancel.status()
    );

    // Give the matching thread a moment to drain.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Shutdown then inspect WAL.
    handle.shutdown().await.unwrap();
    let events = read_events(&wal_dir);
    assert!(!events.is_empty(), "WAL should contain events");
    // At minimum we expect OrderAccepted from the place call.
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::OrderAccepted { .. })),
        "expected at least one OrderAccepted, got: {events:?}"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn missing_authorization_returns_401(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let (handle, base) = boot_server_with_pool(cfg, pool).await;
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

#[sqlx::test(migrations = "../../migrations")]
async fn backpressure_returns_429(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let mut cfg = base_cfg(tmp.path());
    // Smallest legal slot_count (must be power of 2, >=2). Combined with
    // many concurrent requests this reliably observes ring-buffer-full even
    // when other workspace tests are running and the matching thread drains
    // fast.
    cfg.ringbuffer.slot_count = 2;
    // Raise per-user rate limit far above request count so the 429 comes from
    // ring-buffer-full rather than the per-user token-bucket gate.
    cfg.risk.max_orders_per_second = 100_000;
    let (handle, base) = boot_server_with_pool(cfg, pool).await;
    let client = Client::new();
    let (token, _user_id) =
        login_helper(&client, &base, "backpressure@example.com", "hunter2hunter2").await;

    // Fire MANY concurrent requests, expect at least one 429.
    let mut joinset = tokio::task::JoinSet::new();
    for _ in 0..512 {
        let c = client.clone();
        let url = format!("{base}/api/v1/order");
        let tok = token.clone();
        joinset.spawn(async move {
            c.post(url)
                .header("Authorization", format!("Bearer {tok}"))
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
    assert!(
        saw_backpressure,
        "expected at least one 429 under backpressure"
    );
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn idor_cancel_with_wrong_user_id_is_rejected(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let wal_dir = std::path::PathBuf::from(&cfg.wal.dir);
    let (handle, base) = boot_server_with_pool(cfg, pool).await;
    let client = Client::new();

    // Two distinct users.
    let (token_a, _) =
        login_helper(&client, &base, "alice-idor@example.com", "hunter2hunter2").await;
    let (token_b, _) = login_helper(&client, &base, "bob-idor@example.com", "hunter2hunter2").await;

    // Place as user A.
    let place: serde_json::Value = client
        .post(format!("{base}/api/v1/order"))
        .header("Authorization", format!("Bearer {token_a}"))
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

    // Cancel as user B — engine rejects with OrderNotFound.
    let _ = client
        .post(format!("{base}/api/v1/order/cancel"))
        .header("Authorization", format!("Bearer {token_b}"))
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
        "user B must not cancel user A's order"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn shutdown_drains_pending_commands(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let mut cfg = base_cfg(tmp.path());
    // Raise rate limit to avoid per-user throttle masking drain semantics.
    cfg.risk.max_orders_per_second = 100_000;
    let wal_dir = std::path::PathBuf::from(&cfg.wal.dir);
    let (handle, base) = boot_server_with_pool(cfg, pool).await;
    let client = Client::new();
    let (token, _user_id) =
        login_helper(&client, &base, "drain@example.com", "hunter2hunter2").await;

    // Fire 20 concurrent orders; all that get 200 must end up in WAL.
    let mut joinset = tokio::task::JoinSet::new();
    for _ in 0..20 {
        let c = client.clone();
        let url = format!("{base}/api/v1/order");
        let tok = token.clone();
        joinset.spawn(async move {
            c.post(url)
                .header("Authorization", format!("Bearer {tok}"))
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

#[sqlx::test(migrations = "../../migrations")]
async fn duplicate_client_order_id_returns_409(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let wal_dir = std::path::PathBuf::from(&cfg.wal.dir);
    let (handle, base) = boot_server_with_pool(cfg, pool).await;
    let client = Client::new();
    let (token, _user_id) =
        login_helper(&client, &base, "dedup@example.com", "hunter2hunter2").await;

    let body = serde_json::json!({
        "symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT",
        "timeInForce":"GTC","quantity":"0.001","price":"59000",
        "clientOrderId": "12345"
    });

    // First call: accepted.
    let r1 = client
        .post(format!("{base}/api/v1/order"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(r1.status().as_u16(), 200);
    let r1_body: serde_json::Value = r1.json().await.unwrap();
    assert_eq!(r1_body["status"], "ACCEPTED");

    // Second call with identical clientOrderId: Stage 1a dedup gate returns 409.
    let r2 = client
        .post(format!("{base}/api/v1/order"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(r2.status().as_u16(), 409);
    let body2: serde_json::Value = r2.json().await.unwrap();
    assert_eq!(body2["code"], -1014, "expected ERR_DUPLICATE_RESOURCE");

    // WAL: exactly 1 OrderAccepted (dedup gate prevented second from enqueuing).
    tokio::time::sleep(Duration::from_millis(200)).await;
    handle.shutdown().await.unwrap();
    let events = read_events(&wal_dir);
    let accepted = events
        .iter()
        .filter(|e| matches!(e, Event::OrderAccepted { .. }))
        .count();
    assert_eq!(
        accepted, 1,
        "dedup must prevent second order; got {accepted}"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn malformed_json_returns_400(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let (handle, base) = boot_server_with_pool(cfg, pool).await;
    let client = Client::new();
    // Login first to get past JWT extraction.
    let (token, _user_id) =
        login_helper(&client, &base, "malformed@example.com", "hunter2hunter2").await;

    let resp = client
        .post(format!("{base}/api/v1/order"))
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .body("not json")
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_client_error(), "got {}", resp.status());

    handle.shutdown().await.unwrap();
}
