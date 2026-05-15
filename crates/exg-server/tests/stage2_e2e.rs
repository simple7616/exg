//! Stage 2 e2e: admin mark-price inject + funding tick + auth + port
//! isolation + replay. #[sqlx::test] per-test DB isolation.

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
    cfg.server.admin_port = 0; // OS-assigned for tests
    cfg.auth.jwt_secret = "stage2-test-secret-padding-32-bytes-ok".into();
    cfg.admin.admin_secret = "stage2-admin-secret-padding-32-bytesok".into();
    cfg
}

async fn boot(cfg: ExgConfig, pool: PgPool) -> (exg_server::ServerHandle, String, String) {
    let handle = exg_server::run_with_config_with_pool(cfg, Some(pool))
        .await
        .expect("server boot");
    let base = format!("http://127.0.0.1:{}", handle.bound_port);
    let admin = format!("http://127.0.0.1:{}", handle.admin_bound_port);
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
            return (handle, base, admin);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("server not ready");
}

const ADMIN_SECRET: &str = "stage2-admin-secret-padding-32-bytesok";

async fn register_and_login(client: &Client, base: &str, email: &str) -> String {
    let _ = client
        .post(format!("{base}/api/v1/auth/register"))
        .json(&serde_json::json!({"email": email, "password": "hunter2hunter2"}))
        .send()
        .await
        .unwrap();
    let resp: serde_json::Value = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&serde_json::json!({"email": email, "password": "hunter2hunter2"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    resp["accessToken"].as_str().unwrap().to_string()
}

#[sqlx::test(migrations = "../../migrations")]
async fn admin_mark_price_inject_triggers_stop_order(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let wal_dir = std::path::PathBuf::from(tmp.path());
    let cfg = base_cfg(tmp.path());
    let (handle, base, admin) = boot(cfg, pool).await;
    let client = Client::new();
    let token = register_and_login(&client, &base, "s2stop@e.com").await;

    // Place a STOP_MARKET sell triggered when mark <= 59000.
    client
        .post(format!("{base}/api/v1/order"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&serde_json::json!({
            "symbol":"BTCUSDT","side":"SELL","orderType":"STOP_MARKET",
            "timeInForce":"GTC","quantity":"0.001","stopPrice":"59000"
        }))
        .send()
        .await
        .unwrap();
    // Also rest a buy limit so the triggered market sell has a counterparty.
    let token2 = register_and_login(&client, &base, "s2buy@e.com").await;
    client
        .post(format!("{base}/api/v1/order"))
        .header("Authorization", format!("Bearer {token2}"))
        .json(&serde_json::json!({
            "symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT",
            "timeInForce":"GTC","quantity":"0.001","price":"59000"
        }))
        .send()
        .await
        .unwrap();

    // Admin injects a mark price that crosses the stop.
    let resp = client
        .post(format!("{admin}/api/v1/admin/mark-price"))
        .header("X-Admin-Secret", ADMIN_SECRET)
        .json(&serde_json::json!({"markPrice":"58000","indexPrice":"58000"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);

    tokio::time::sleep(Duration::from_millis(300)).await;
    handle.shutdown().await.unwrap();

    // WAL must contain a MarkPriceUpdate and an OrderFilled (triggered stop).
    let mut reader = WalReader::open(&wal_dir).unwrap();
    let (mut saw_mark, mut saw_fill) = (false, false);
    reader
        .read_from(0, |_seq, payload| {
            let owned: Vec<u8> = payload.to_vec();
            let e: Event = rkyv::from_bytes::<Event, rkyv::rancor::Error>(&owned).unwrap();
            match e {
                Event::MarkPriceUpdate { .. } => saw_mark = true,
                Event::OrderFilled { .. } => saw_fill = true,
                _ => {}
            }
            true
        })
        .unwrap();
    assert!(saw_mark, "WAL must contain MarkPriceUpdate");
    assert!(saw_fill, "stop order must have triggered an OrderFilled");
}

#[sqlx::test(migrations = "../../migrations")]
async fn admin_funding_tick_emits_funding_rate(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let wal_dir = std::path::PathBuf::from(tmp.path());
    let cfg = base_cfg(tmp.path());
    let (handle, _base, admin) = boot(cfg, pool).await;
    let client = Client::new();

    client
        .post(format!("{admin}/api/v1/admin/mark-price"))
        .header("X-Admin-Secret", ADMIN_SECRET)
        .json(&serde_json::json!({"markPrice":"60600","indexPrice":"60000"}))
        .send()
        .await
        .unwrap();
    let resp = client
        .post(format!("{admin}/api/v1/admin/funding-tick"))
        .header("X-Admin-Secret", ADMIN_SECRET)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);

    tokio::time::sleep(Duration::from_millis(300)).await;
    handle.shutdown().await.unwrap();

    let mut reader = WalReader::open(&wal_dir).unwrap();
    let mut rate = None;
    reader
        .read_from(0, |_seq, payload| {
            let owned: Vec<u8> = payload.to_vec();
            let e: Event = rkyv::from_bytes::<Event, rkyv::rancor::Error>(&owned).unwrap();
            if let Event::FundingRateUpdate { funding_rate, .. } = e {
                rate = Some(funding_rate);
            }
            true
        })
        .unwrap();
    // premium = (60600-60000)/60000 = 0.01 → clamp(0.01+0.0001, ±0.0075) = 0.0075
    assert_eq!(rate.unwrap(), "0.0075".parse().unwrap());
}

#[sqlx::test(migrations = "../../migrations")]
async fn admin_endpoint_missing_secret_returns_401(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let (handle, _base, admin) = boot(cfg, pool).await;
    let client = Client::new();
    let resp = client
        .post(format!("{admin}/api/v1/admin/funding-tick"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 401);
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn admin_endpoint_wrong_secret_returns_401(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let (handle, _base, admin) = boot(cfg, pool).await;
    let client = Client::new();
    let resp = client
        .post(format!("{admin}/api/v1/admin/funding-tick"))
        .header("X-Admin-Secret", "wrong-secret-wrong-secret-wrong!")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 401);
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn admin_endpoint_correct_secret_returns_200(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let (handle, _base, admin) = boot(cfg, pool).await;
    let client = Client::new();
    let resp = client
        .post(format!("{admin}/api/v1/admin/funding-tick"))
        .header("X-Admin-Secret", ADMIN_SECRET)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn admin_route_not_on_main_port(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let (handle, base, _admin) = boot(cfg, pool).await;
    let client = Client::new();
    let resp = client
        .post(format!("{base}/api/v1/admin/funding-tick"))
        .header("X-Admin-Secret", ADMIN_SECRET)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        404,
        "admin route must NOT exist on 8080"
    );
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn user_route_not_on_admin_port(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let (handle, _base, admin) = boot(cfg, pool).await;
    let client = Client::new();
    let resp = client
        .get(format!("{admin}/api/v1/health"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        404,
        "user route must NOT exist on admin port"
    );
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn admin_mark_price_bad_decimal_returns_400(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let (handle, _base, admin) = boot(cfg, pool).await;
    let client = Client::new();
    let resp = client
        .post(format!("{admin}/api/v1/admin/mark-price"))
        .header("X-Admin-Secret", ADMIN_SECRET)
        .json(&serde_json::json!({"markPrice":"not-a-number","indexPrice":"60000"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 400);
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn admin_mark_price_zero_index_returns_400(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let (handle, _base, admin) = boot(cfg, pool).await;
    let client = Client::new();
    let resp = client
        .post(format!("{admin}/api/v1/admin/mark-price"))
        .header("X-Admin-Secret", ADMIN_SECRET)
        .json(&serde_json::json!({"markPrice":"60000","indexPrice":"0"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 400);
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn replay_mark_price_trigger_survives_reboot(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());

    // Boot 1: rest a buy limit + a stop-sell, admin inject mark crossing
    // the stop → triggers a fill. Kill.
    {
        let (handle, base, admin) = boot(cfg.clone(), pool.clone()).await;
        let client = Client::new();
        let tb = register_and_login(&client, &base, "rb-buy@e.com").await;
        client
            .post(format!("{base}/api/v1/order"))
            .header("Authorization", format!("Bearer {tb}"))
            .json(&serde_json::json!({
                "symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT",
                "timeInForce":"GTC","quantity":"0.001","price":"59000"
            }))
            .send()
            .await
            .unwrap();
        let ts = register_and_login(&client, &base, "rb-stop@e.com").await;
        client
            .post(format!("{base}/api/v1/order"))
            .header("Authorization", format!("Bearer {ts}"))
            .json(&serde_json::json!({
                "symbol":"BTCUSDT","side":"SELL","orderType":"STOP_MARKET",
                "timeInForce":"GTC","quantity":"0.001","stopPrice":"59000"
            }))
            .send()
            .await
            .unwrap();
        client
            .post(format!("{admin}/api/v1/admin/mark-price"))
            .header("X-Admin-Secret", ADMIN_SECRET)
            .json(&serde_json::json!({"markPrice":"58000","indexPrice":"58000"}))
            .send()
            .await
            .unwrap();
        client
            .post(format!("{admin}/api/v1/admin/funding-tick"))
            .header("X-Admin-Secret", ADMIN_SECRET)
            .send()
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        handle.shutdown().await.unwrap();
    }

    // CEO review C10: observable assertion, NOT the weak "boot 2 didn't
    // panic" proxy (Stage 1b A4 rejected that). Count boot-1 WAL records,
    // then after reboot (NO new injects) assert: (a) the triggered stop's
    // OrderFilled appears within the boot-1 record range, and (b) NO new
    // OrderFilled for that order_id was appended during boot-2 — proving
    // replay applied the historical fill but did NOT re-trigger the stop.
    let wal_dir = std::path::PathBuf::from(
        // base_cfg sets cfg.wal.dir = tmp.path(); reuse it.
        cfg.wal.dir.clone(),
    );
    let mut reader = WalReader::open(&wal_dir).unwrap();
    let mut boot1_records: u64 = 0;
    let mut filled_order_ids_boot1: Vec<u64> = Vec::new();
    reader
        .read_from(0, |_seq, payload| {
            boot1_records += 1;
            let owned: Vec<u8> = payload.to_vec();
            let e: Event = rkyv::from_bytes::<Event, rkyv::rancor::Error>(&owned).unwrap();
            if let Event::OrderFilled { order_id, .. } = e {
                filled_order_ids_boot1.push(order_id.value());
            }
            true
        })
        .unwrap();
    assert!(
        !filled_order_ids_boot1.is_empty(),
        "boot 1 must have recorded at least one OrderFilled (triggered stop)"
    );

    // Boot 2: replay applies MarkPriceUpdate (passive) + the historical
    // OrderFilled (its own WAL event) + FundingRateUpdate. Health green.
    let (handle2, base2, _admin2) = boot(cfg, pool).await;
    let client = Client::new();
    let resp = client
        .get(format!("{base2}/api/v1/health"))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    handle2.shutdown().await.unwrap();

    // Re-scan: assert NO new OrderFilled was appended during boot-2 (replay
    // is passive — the MarkPriceUpdate must not re-trigger the stop and
    // produce a duplicate fill).
    let mut reader2 = WalReader::open(&wal_dir).unwrap();
    let mut total_records: u64 = 0;
    let mut filled_after_boot1 = 0u64;
    reader2
        .read_from(0, |_seq, payload| {
            total_records += 1;
            if total_records > boot1_records {
                let owned: Vec<u8> = payload.to_vec();
                let e: Event = rkyv::from_bytes::<Event, rkyv::rancor::Error>(&owned).unwrap();
                if matches!(e, Event::OrderFilled { .. }) {
                    filled_after_boot1 += 1;
                }
            }
            true
        })
        .unwrap();
    assert_eq!(
        filled_after_boot1, 0,
        "replay must NOT re-trigger the stop — no new OrderFilled after boot 1 \
         (would be a double-fill silent corruption)"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn admin_mark_price_negative_or_zero_mark_returns_400(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let (handle, _base, admin) = boot(cfg, pool).await;
    let client = Client::new();
    for bad in ["0", "-1"] {
        let resp = client
            .post(format!("{admin}/api/v1/admin/mark-price"))
            .header("X-Admin-Secret", ADMIN_SECRET)
            .json(&serde_json::json!({"markPrice": bad, "indexPrice": "60000"}))
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status().as_u16(),
            400,
            "markPrice {bad} must be rejected (mass stop-trigger guard)"
        );
    }
    handle.shutdown().await.unwrap();
}
