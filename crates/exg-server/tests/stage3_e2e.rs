//! Stage 3 e2e: admin credit → open positions → funding tick moves
//! funds → reboot survives. #[sqlx::test] per-test DB isolation.

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
    cfg.server.admin_port = 0;
    cfg.auth.jwt_secret = "stage3-test-secret-padding-32-bytes-ok".into();
    cfg.admin.admin_secret = "stage3-admin-secret-padding-32-bytesok".into();
    cfg
}

const ADMIN_SECRET: &str = "stage3-admin-secret-padding-32-bytesok";

async fn boot(cfg: ExgConfig, pool: PgPool) -> (exg_server::ServerHandle, String, String) {
    let handle = exg_server::run_with_config_with_pool(cfg, Some(pool))
        .await
        .expect("boot");
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

/// The order-placement handler resolves the acting user via the JWT
/// claims (`exg_user_service::verify_jwt(...).user_id`). Decode the same
/// JWT to learn the id we must admin-credit so the credited user == the
/// user that opens the position. `JWT_SECRET` MUST equal
/// `base_cfg().auth.jwt_secret`.
const JWT_SECRET: &str = "stage3-test-secret-padding-32-bytes-ok";

fn jwt_user_id(token: &str) -> u64 {
    let claims =
        exg_user_service::verify_jwt(JWT_SECRET.as_bytes(), token).expect("decode test JWT");
    claims.user_id
}

#[sqlx::test(migrations = "../../migrations")]
async fn admin_credit_then_funding_tick_moves_funds(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let wal_dir = std::path::PathBuf::from(tmp.path());
    let cfg = base_cfg(tmp.path());
    let (handle, base, admin) = boot(cfg, pool).await;
    let client = Client::new();

    // Register/login each user once; derive the real user id from the JWT
    // (the order handler resolves the actor the same way), admin-credit
    // that exact id, then place the order under the same token.
    let t1 = register_and_login(&client, &base, "s3a@e.com").await;
    let t2 = register_and_login(&client, &base, "s3b@e.com").await;
    let uid1 = jwt_user_id(&t1);
    let uid2 = jwt_user_id(&t2);
    for (uid, tag) in [(uid1, "s3a"), (uid2, "s3b")] {
        let r = client
            .post(format!("{admin}/api/v1/admin/credit"))
            .header("X-Admin-Secret", ADMIN_SECRET)
            .json(&serde_json::json!({"userId": uid, "amount": "100000"}))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status().as_u16(), 200, "admin credit {tag}");
    }
    // user1 buys, user2 sells @60000 — they cross and open opposing positions.
    client
        .post(format!("{base}/api/v1/order"))
        .header("Authorization", format!("Bearer {t1}"))
        .json(&serde_json::json!({"symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT","timeInForce":"GTC","quantity":"1","price":"60000"}))
        .send()
        .await
        .unwrap();
    client
        .post(format!("{base}/api/v1/order"))
        .header("Authorization", format!("Bearer {t2}"))
        .json(&serde_json::json!({"symbol":"BTCUSDT","side":"SELL","orderType":"LIMIT","timeInForce":"GTC","quantity":"1","price":"60000"}))
        .send()
        .await
        .unwrap();

    client
        .post(format!("{admin}/api/v1/admin/mark-price"))
        .header("X-Admin-Secret", ADMIN_SECRET)
        .json(&serde_json::json!({"markPrice":"60000","indexPrice":"60000"}))
        .send()
        .await
        .unwrap();
    let r = client
        .post(format!("{admin}/api/v1/admin/funding-tick"))
        .header("X-Admin-Secret", ADMIN_SECRET)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 200);

    tokio::time::sleep(Duration::from_millis(300)).await;
    handle.shutdown().await.unwrap();

    let mut reader = WalReader::open(&wal_dir).unwrap();
    let (mut saw_credit, mut saw_settled) = (false, false);
    reader
        .read_from(0, |_s, p| {
            let owned: Vec<u8> = p.to_vec();
            match rkyv::from_bytes::<Event, rkyv::rancor::Error>(&owned).unwrap() {
                Event::AdminCredited { .. } => saw_credit = true,
                Event::FundingSettled { .. } => saw_settled = true,
                _ => {}
            }
            true
        })
        .unwrap();
    assert!(saw_credit, "WAL has AdminCredited");
    assert!(saw_settled, "WAL has FundingSettled");
}

#[sqlx::test(migrations = "../../migrations")]
async fn funding_settlement_survives_reboot(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    // Boot 1: credit + open + funding tick, then shutdown.
    {
        let (handle, base, admin) = boot(cfg.clone(), pool.clone()).await;
        let client = Client::new();
        let t1 = register_and_login(&client, &base, "rb1@e.com").await;
        let t2 = register_and_login(&client, &base, "rb2@e.com").await;
        for uid in [jwt_user_id(&t1), jwt_user_id(&t2)] {
            client
                .post(format!("{admin}/api/v1/admin/credit"))
                .header("X-Admin-Secret", ADMIN_SECRET)
                .json(&serde_json::json!({"userId": uid, "amount":"100000"}))
                .send()
                .await
                .unwrap();
        }
        client
            .post(format!("{base}/api/v1/order"))
            .header("Authorization", format!("Bearer {t1}"))
            .json(&serde_json::json!({"symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT","timeInForce":"GTC","quantity":"1","price":"60000"}))
            .send()
            .await
            .unwrap();
        client
            .post(format!("{base}/api/v1/order"))
            .header("Authorization", format!("Bearer {t2}"))
            .json(&serde_json::json!({"symbol":"BTCUSDT","side":"SELL","orderType":"LIMIT","timeInForce":"GTC","quantity":"1","price":"60000"}))
            .send()
            .await
            .unwrap();
        client
            .post(format!("{admin}/api/v1/admin/mark-price"))
            .header("X-Admin-Secret", ADMIN_SECRET)
            .json(&serde_json::json!({"markPrice":"60000","indexPrice":"60000"}))
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
    // Boot 2: replay must succeed (post-replay verify_all_invariants gate)
    // and health green. (Observable: boot 2 does not panic AND health 200;
    // the verify_all_invariants gate in Task 7 Step 2 makes a divergent
    // replay fail the boot, so a green boot 2 is a real assertion that the
    // ledger reconstructed consistently — not the weak Stage-1b proxy.)
    let (handle2, base2, _a2) = boot(cfg, pool).await;
    let client = Client::new();
    let resp = client
        .get(format!("{base2}/api/v1/health"))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "boot 2 replay healthy");
    handle2.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn admin_credit_missing_secret_401(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let (handle, _b, admin) = boot(base_cfg(tmp.path()), pool).await;
    let r = Client::new()
        .post(format!("{admin}/api/v1/admin/credit"))
        .json(&serde_json::json!({"userId":1,"amount":"100"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 401);
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn admin_credit_negative_amount_400(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let (handle, _b, admin) = boot(base_cfg(tmp.path()), pool).await;
    let c = Client::new();
    for bad in ["0", "-1"] {
        let r = c
            .post(format!("{admin}/api/v1/admin/credit"))
            .header("X-Admin-Secret", ADMIN_SECRET)
            .json(&serde_json::json!({"userId":1,"amount":bad}))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status().as_u16(), 400, "amount {bad}");
    }
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn admin_credit_route_not_on_main_port(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let (handle, base, _a) = boot(base_cfg(tmp.path()), pool).await;
    let r = Client::new()
        .post(format!("{base}/api/v1/admin/credit"))
        .header("X-Admin-Secret", ADMIN_SECRET)
        .json(&serde_json::json!({"userId":1,"amount":"100"}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status().as_u16(),
        404,
        "admin route must not be on main port"
    );
    handle.shutdown().await.unwrap();
}
