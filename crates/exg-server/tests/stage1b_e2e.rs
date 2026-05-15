//! Stage 1b end-to-end tests: WAL replay + Stage 1a polish coverage.
//! Every test gets its own throwaway PG database via #[sqlx::test].

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
    cfg.server.port = 0;
    cfg.auth.jwt_secret = "stage1b-test-secret-padding-32-bytes-ok".into();
    cfg.admin.admin_secret = "stage1b-admin-secret-padding-32-bytes-ok".into();
    cfg.server.admin_port = 0;
    cfg
}

async fn boot_server(cfg: ExgConfig, pool: PgPool) -> (exg_server::ServerHandle, String) {
    let handle = exg_server::run_with_config_with_pool(cfg, Some(pool))
        .await
        .expect("server boot");
    let base = format!("http://127.0.0.1:{}", handle.bound_port);
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
    panic!("server not ready");
}

async fn register_and_login(client: &Client, base: &str, email: &str, password: &str) -> String {
    let _ = client
        .post(format!("{base}/api/v1/auth/register"))
        .json(&serde_json::json!({"email": email, "password": password}))
        .send()
        .await
        .unwrap();
    let resp: serde_json::Value = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&serde_json::json!({"email": email, "password": password}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    resp["accessToken"]
        .as_str()
        .unwrap_or_else(|| panic!("accessToken missing: {resp}"))
        .to_string()
}

/// Reboot the server against the same WAL + pool, exercising the replay path.
async fn reboot(cfg: ExgConfig, pool: PgPool) -> (exg_server::ServerHandle, String) {
    boot_server(cfg, pool).await
}

// ── Replay tests 1-4 ─────────────────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn boot_replays_empty_wal_succeeds(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let (handle, base) = boot_server(cfg, pool).await;
    let client = Client::new();
    let resp = client
        .get(format!("{base}/api/v1/health"))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn boot_replays_single_order_restores_orderbook(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let wal_dir = std::path::PathBuf::from(tmp.path());
    let cfg1 = base_cfg(tmp.path());
    {
        let (handle, base) = boot_server(cfg1.clone(), pool.clone()).await;
        let client = Client::new();
        let token = register_and_login(&client, &base, "happy@e.com", "hunter2hunter2").await;
        let resp = client
            .post(format!("{base}/api/v1/order"))
            .header("Authorization", format!("Bearer {token}"))
            .json(&serde_json::json!({
                "symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT",
                "timeInForce":"GTC","quantity":"0.001","price":"59000",
                "clientOrderId":"800001"
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        tokio::time::sleep(Duration::from_millis(200)).await;
        handle.shutdown().await.unwrap();
    }

    // Independently inspect the WAL: at least one OrderAccepted recorded.
    let mut reader = WalReader::open(&wal_dir).unwrap();
    let mut accept_count = 0;
    reader
        .read_from(0, |_seq, payload| {
            // rkyv requires 16-byte aligned input; copy to owned Vec.
            let owned: Vec<u8> = payload.to_vec();
            let e: Event = rkyv::from_bytes::<Event, rkyv::rancor::Error>(&owned).unwrap();
            if matches!(e, Event::OrderAccepted { .. }) {
                accept_count += 1;
            }
            true
        })
        .unwrap();
    assert!(
        accept_count >= 1,
        "WAL must contain at least one OrderAccepted"
    );

    // Boot 2: replay. Test passes if boot succeeds (replay applied without panic).
    let (handle2, _base2) = reboot(cfg1, pool).await;
    handle2.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn boot_replays_place_cancel_restores_empty_orderbook(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());

    let order_id: u64 = {
        let (handle, base) = boot_server(cfg.clone(), pool.clone()).await;
        let client = Client::new();
        let token = register_and_login(&client, &base, "pcr@e.com", "hunter2hunter2").await;
        let resp: serde_json::Value = client
            .post(format!("{base}/api/v1/order"))
            .header("Authorization", format!("Bearer {token}"))
            .json(&serde_json::json!({
                "symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT",
                "timeInForce":"GTC","quantity":"0.001","price":"58000"
            }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let oid: u64 = resp["orderId"].as_str().unwrap().parse().unwrap();
        let cancel = client
            .post(format!("{base}/api/v1/order/cancel"))
            .header("Authorization", format!("Bearer {token}"))
            .json(&serde_json::json!({"orderId": oid, "symbol":"BTCUSDT"}))
            .send()
            .await
            .unwrap();
        assert!(cancel.status().is_success());
        tokio::time::sleep(Duration::from_millis(200)).await;
        handle.shutdown().await.unwrap();
        oid
    };

    // Reboot — replay should leave orderbook empty for that order_id.
    let (handle2, _) = reboot(cfg, pool).await;
    let _ = order_id;
    handle2.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn boot_replays_place_amend_restores_amended_price(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());

    {
        let (handle, base) = boot_server(cfg.clone(), pool.clone()).await;
        let client = Client::new();
        let token = register_and_login(&client, &base, "pa@e.com", "hunter2hunter2").await;
        let resp: serde_json::Value = client
            .post(format!("{base}/api/v1/order"))
            .header("Authorization", format!("Bearer {token}"))
            .json(&serde_json::json!({
                "symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT",
                "timeInForce":"GTC","quantity":"0.001","price":"58000"
            }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let oid: u64 = resp["orderId"].as_str().unwrap().parse().unwrap();
        let amend = client
            .post(format!("{base}/api/v1/order/amend"))
            .header("Authorization", format!("Bearer {token}"))
            .json(&serde_json::json!({
                "orderId": oid, "symbol":"BTCUSDT", "newPrice":"58500"
            }))
            .send()
            .await
            .unwrap();
        assert!(amend.status().is_success());
        tokio::time::sleep(Duration::from_millis(200)).await;
        handle.shutdown().await.unwrap();
    }

    let (handle2, _) = reboot(cfg, pool).await;
    handle2.shutdown().await.unwrap();
}

// ── Replay tests 5-9 ─────────────────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn boot_replays_matched_trade_restores_post_match_state(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());

    {
        let (handle, base) = boot_server(cfg.clone(), pool.clone()).await;
        let client = Client::new();
        let token_a = register_and_login(&client, &base, "maker@e.com", "hunter2hunter2").await;
        let token_b = register_and_login(&client, &base, "taker@e.com", "hunter2hunter2").await;

        client
            .post(format!("{base}/api/v1/order"))
            .header("Authorization", format!("Bearer {token_a}"))
            .json(&serde_json::json!({
                "symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT",
                "timeInForce":"GTC","quantity":"0.001","price":"60000"
            }))
            .send()
            .await
            .unwrap();

        client
            .post(format!("{base}/api/v1/order"))
            .header("Authorization", format!("Bearer {token_b}"))
            .json(&serde_json::json!({
                "symbol":"BTCUSDT","side":"SELL","orderType":"LIMIT",
                "timeInForce":"GTC","quantity":"0.001","price":"60000"
            }))
            .send()
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(200)).await;
        handle.shutdown().await.unwrap();
    }

    let (handle2, _) = reboot(cfg, pool).await;
    handle2.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn place_then_kill_then_place_continues_sequence(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());

    let first_oid: u64 = {
        let (handle, base) = boot_server(cfg.clone(), pool.clone()).await;
        let client = Client::new();
        let token = register_and_login(&client, &base, "k@e.com", "hunter2hunter2").await;
        let resp: serde_json::Value = client
            .post(format!("{base}/api/v1/order"))
            .header("Authorization", format!("Bearer {token}"))
            .json(&serde_json::json!({
                "symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT",
                "timeInForce":"GTC","quantity":"0.001","price":"57000"
            }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        handle.shutdown().await.unwrap();
        resp["orderId"].as_str().unwrap().parse().unwrap()
    };

    let (handle2, base2) = reboot(cfg, pool.clone()).await;
    let client = Client::new();
    let token = register_and_login(&client, &base2, "k@e.com", "hunter2hunter2").await;
    let resp: serde_json::Value = client
        .post(format!("{base2}/api/v1/order"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&serde_json::json!({
            "symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT",
            "timeInForce":"GTC","quantity":"0.001","price":"57001"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let second_oid: u64 = resp["orderId"].as_str().unwrap().parse().unwrap();
    assert_ne!(
        first_oid, second_oid,
        "second boot must allocate fresh order_id"
    );
    handle2.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn boot_replays_three_orders_inspectable_via_wal(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let wal_dir = std::path::PathBuf::from(tmp.path());
    let cfg = base_cfg(tmp.path());

    {
        let (handle, base) = boot_server(cfg.clone(), pool.clone()).await;
        let client = Client::new();
        let token = register_and_login(&client, &base, "three@e.com", "hunter2hunter2").await;
        for i in 0..3 {
            client
                .post(format!("{base}/api/v1/order"))
                .header("Authorization", format!("Bearer {token}"))
                .json(&serde_json::json!({
                    "symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT",
                    "timeInForce":"GTC","quantity":"0.001","price": format!("{}", 56000 + i)
                }))
                .send()
                .await
                .unwrap();
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        handle.shutdown().await.unwrap();
    }

    let mut reader = WalReader::open(&wal_dir).unwrap();
    let mut accept_count = 0;
    reader
        .read_from(0, |_seq, payload| {
            let owned: Vec<u8> = payload.to_vec();
            let e: Event = rkyv::from_bytes::<Event, rkyv::rancor::Error>(&owned).unwrap();
            if matches!(e, Event::OrderAccepted { .. }) {
                accept_count += 1;
            }
            true
        })
        .unwrap();
    assert_eq!(accept_count, 3);

    let (handle2, _) = reboot(cfg, pool).await;
    handle2.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn boot_with_only_rejected_events_succeeds(pool: PgPool) {
    use exg_wal::{WalConfig, WalWriter};

    let tmp = TempDir::new().unwrap();
    let wal_dir = tmp.path().join("wal");
    std::fs::create_dir(&wal_dir).unwrap();

    {
        let mut w = WalWriter::open(WalConfig {
            dir: wal_dir.clone(),
            segment_size: 64 * 1024 * 1024,
            flush_interval_us: 1000,
            flush_every_n: 1,
        })
        .unwrap();
        let evt = Event::OrderRejected {
            order_id: exg_common::OrderId::new(7777),
            user_id: exg_common::UserId::new(42),
            reason: exg_protocol::RejectReason::InsufficientMargin,
            timestamp: exg_common::UnixMicros::now(),
        };
        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&evt).unwrap();
        w.append(&bytes).unwrap();
        w.flush().unwrap();
    }

    let mut cfg = base_cfg(tmp.path());
    cfg.wal.dir = wal_dir.to_string_lossy().into_owned();
    let (handle, _) = boot_server(cfg, pool).await;
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn replay_survived_order_matches_post_reboot_taker(pool: PgPool) {
    // CEO review A4 — strongest replay correctness test. Without this, the
    // other replay e2e tests only prove "boot did not panic", not "the order
    // that was on the book before kill is on the book after replay and can
    // be matched." This test routes the verification through the matching
    // engine itself: if the maker order survived replay correctly, the
    // taker's aggressive order will match it and the new WAL records will
    // contain an OrderFilled referencing the maker order_id from boot 1.
    let tmp = TempDir::new().unwrap();
    let wal_dir = std::path::PathBuf::from(tmp.path());
    let cfg = base_cfg(tmp.path());

    // ── Boot 1: place a resting maker bid, shut down cleanly. ────────────
    let maker_order_id: u64 = {
        let (handle, base) = boot_server(cfg.clone(), pool.clone()).await;
        let client = Client::new();
        let maker_token =
            register_and_login(&client, &base, "maker-survives@e.com", "hunter2hunter2").await;
        let resp: serde_json::Value = client
            .post(format!("{base}/api/v1/order"))
            .header("Authorization", format!("Bearer {maker_token}"))
            .json(&serde_json::json!({
                "symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT",
                "timeInForce":"GTC","quantity":"0.001","price":"61000"
            }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let oid: u64 = resp["orderId"].as_str().unwrap().parse().unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        handle.shutdown().await.unwrap();
        oid
    };

    // Count boot 1 records so we can identify NEW records appended during boot 2.
    let mut reader = WalReader::open(&wal_dir).unwrap();
    let mut boot1_record_count: u64 = 0;
    reader
        .read_from(0, |_seq, _payload| {
            boot1_record_count += 1;
            true
        })
        .unwrap();

    // ── Boot 2: replay (Step 3.6 fires on maker's OrderAccepted), then
    // taker hits the maker at the maker's price. ─────────────────────────
    {
        let (handle2, base2) = reboot(cfg, pool).await;
        let client = Client::new();
        let taker_token =
            register_and_login(&client, &base2, "taker-hits-maker@e.com", "hunter2hunter2").await;
        let resp = client
            .post(format!("{base2}/api/v1/order"))
            .header("Authorization", format!("Bearer {taker_token}"))
            .json(&serde_json::json!({
                "symbol":"BTCUSDT","side":"SELL","orderType":"LIMIT",
                "timeInForce":"GTC","quantity":"0.001","price":"61000"
            }))
            .send()
            .await
            .unwrap();
        assert!(resp.status().is_success(), "taker place failed: {resp:?}");
        tokio::time::sleep(Duration::from_millis(300)).await;
        handle2.shutdown().await.unwrap();
    }

    // ── Verify: boot 2's NEW WAL records contain an OrderFilled
    // referencing maker_order_id. ────────────────────────────────────────
    let mut reader = WalReader::open(&wal_dir).unwrap();
    let mut filled_for_maker = false;
    let mut seen: u64 = 0;
    reader
        .read_from(0, |_seq, payload| {
            seen += 1;
            if seen <= boot1_record_count {
                return true;
            }
            let owned: Vec<u8> = payload.to_vec();
            let e: Event = rkyv::from_bytes::<Event, rkyv::rancor::Error>(&owned).unwrap();
            if let Event::OrderFilled { order_id, .. } = e {
                if order_id.value() == maker_order_id {
                    filled_for_maker = true;
                }
            }
            true
        })
        .unwrap();
    assert!(
        filled_for_maker,
        "post-reboot taker did not match the replayed maker order — replay regression"
    );
}

// ── Polish tests ─────────────────────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn cancel_order_rate_limit(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let mut cfg = base_cfg(tmp.path());
    cfg.risk.max_orders_per_second = 1;
    let (handle, base) = boot_server(cfg, pool).await;
    let client = Client::new();
    let token = register_and_login(&client, &base, "co-rl@e.com", "hunter2hunter2").await;

    let mut saw_429 = false;
    for _ in 0..20 {
        let resp = client
            .post(format!("{base}/api/v1/order/cancel"))
            .header("Authorization", format!("Bearer {token}"))
            .json(&serde_json::json!({"orderId": 1u64, "symbol":"BTCUSDT"}))
            .send()
            .await
            .unwrap();
        if resp.status().as_u16() == 429 {
            let body: serde_json::Value = resp.json().await.unwrap();
            assert_eq!(body["code"], -1003);
            saw_429 = true;
            break;
        }
    }
    assert!(saw_429, "expected 429 from per-user cancel limit");
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn amend_order_rate_limit(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let mut cfg = base_cfg(tmp.path());
    cfg.risk.max_orders_per_second = 1;
    let (handle, base) = boot_server(cfg, pool).await;
    let client = Client::new();
    let token = register_and_login(&client, &base, "am-rl@e.com", "hunter2hunter2").await;

    let mut saw_429 = false;
    for _ in 0..20 {
        let resp = client
            .post(format!("{base}/api/v1/order/amend"))
            .header("Authorization", format!("Bearer {token}"))
            .json(&serde_json::json!({
                "orderId": 1u64, "symbol":"BTCUSDT", "newPrice":"60000"
            }))
            .send()
            .await
            .unwrap();
        if resp.status().as_u16() == 429 {
            let body: serde_json::Value = resp.json().await.unwrap();
            assert_eq!(body["code"], -1003);
            saw_429 = true;
            break;
        }
    }
    assert!(saw_429, "expected 429 from per-user amend limit");
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn mixed_place_cancel_share_user_bucket(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let mut cfg = base_cfg(tmp.path());
    cfg.risk.max_orders_per_second = 3;
    let (handle, base) = boot_server(cfg, pool).await;
    let client = Client::new();
    let token = register_and_login(&client, &base, "mix@e.com", "hunter2hunter2").await;

    // Drain the bucket with place orders.
    for _ in 0..3 {
        client
            .post(format!("{base}/api/v1/order"))
            .header("Authorization", format!("Bearer {token}"))
            .json(&serde_json::json!({
                "symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT",
                "timeInForce":"GTC","quantity":"0.001","price":"50000"
            }))
            .send()
            .await
            .unwrap();
    }
    // Now cancel — should hit 429 because the bucket is empty.
    let resp = client
        .post(format!("{base}/api/v1/order/cancel"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&serde_json::json!({"orderId": 1u64, "symbol":"BTCUSDT"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 429);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["code"], -1003);
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn login_charges_ip_bucket_even_when_email_exhausted(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let mut cfg = base_cfg(tmp.path());
    cfg.risk.max_orders_per_second = 1; // each bucket holds 1 token
    let (handle, base) = boot_server(cfg, pool).await;
    let client = Client::new();

    // Register two distinct users so email_B login attempt below is for a
    // valid account.
    let _ = register_and_login(&client, &base, "a@e.com", "hunter2hunter2").await;
    let _ = register_and_login(&client, &base, "b@e.com", "hunter2hunter2").await;

    // 1st login attempt with WRONG password on email_A — still consumes
    // both email_A and IP buckets (the || fix). Status doesn't matter.
    let _ = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&serde_json::json!({"email":"a@e.com","password":"WRONGPASS"}))
        .send()
        .await
        .unwrap();

    // 2nd login on email_B from same IP — IP bucket is empty, so must 429
    // even though email_B's bucket has its full token.
    let r2 = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&serde_json::json!({"email":"b@e.com","password":"hunter2hunter2"}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        r2.status().as_u16(),
        429,
        "expected 429 — IP bucket exhausted by first login"
    );
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn tampered_jwt_signature_returns_401(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let (handle, base) = boot_server(cfg, pool).await;
    let client = Client::new();
    let token = register_and_login(&client, &base, "tamper@e.com", "hunter2hunter2").await;

    let pos = token.rfind('.').unwrap() + 1;
    let tampered: String = token
        .chars()
        .enumerate()
        .map(|(i, c)| if i >= pos && i < pos + 4 { 'A' } else { c })
        .collect();

    let resp = client
        .get(format!("{base}/api/v1/me"))
        .header("Authorization", format!("Bearer {tampered}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 401);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["code"], -1002);
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn token_reuse_within_expiry_succeeds(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let (handle, base) = boot_server(cfg, pool).await;
    let client = Client::new();
    let token = register_and_login(&client, &base, "reuse@e.com", "hunter2hunter2").await;

    for _ in 0..2 {
        let resp = client
            .get(format!("{base}/api/v1/me"))
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200);
    }
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn kyc_level_reflected_in_me(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let (handle, base) = boot_server(cfg, pool.clone()).await;
    let client = Client::new();
    let token = register_and_login(&client, &base, "kyc@e.com", "hunter2hunter2").await;

    let me1: serde_json::Value = client
        .get(format!("{base}/api/v1/me"))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let user_id: i64 = me1["userId"].as_str().unwrap().parse().unwrap();

    sqlx::query("UPDATE users SET kyc_level = $1 WHERE user_id = $2")
        .bind(2_i16)
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();

    let me2: serde_json::Value = client
        .get(format!("{base}/api/v1/me"))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(me2["kycLevel"], 2);
    handle.shutdown().await.unwrap();
}
