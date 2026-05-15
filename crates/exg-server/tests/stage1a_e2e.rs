//! Stage 1a end-to-end integration tests.
//! Covers register / login / me / JWT verify / dedup / rate limit / IDOR.

use exg_config::ExgConfig;
use reqwest::Client;
use sqlx::PgPool;
use std::time::Duration;
use tempfile::TempDir;

fn base_cfg(wal_dir: &std::path::Path) -> ExgConfig {
    let mut cfg = ExgConfig::default_config();
    cfg.wal.dir = wal_dir.to_string_lossy().into_owned();
    cfg.server.host = "127.0.0.1".into();
    cfg.server.port = 0;
    cfg.auth.jwt_secret = "stage1a-test-secret-padding-32-bytes-ok".into();
    cfg.admin.admin_secret = "stage1a-admin-secret-padding-32-bytes-ok".into();
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

#[sqlx::test(migrations = "../../migrations")]
async fn register_login_order_happy(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let (handle, base) = boot_server(cfg, pool).await;
    let client = Client::new();
    let token = register_and_login(&client, &base, "happy@e.com", "hunter2hunter2").await;

    let resp = client
        .post(format!("{base}/api/v1/order"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&serde_json::json!({
            "symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT",
            "timeInForce":"GTC","quantity":"0.001","price":"59000",
            "clientOrderId":"100001"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ACCEPTED");
    assert_eq!(
        body["clientOrderId"], "100001",
        "submitted clientOrderId must be echoed back: {body}"
    );

    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn order_without_token_returns_401(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let (handle, base) = boot_server(cfg, pool).await;
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
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["code"], -1002);
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn order_with_expired_token_returns_401(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let mut cfg = base_cfg(tmp.path());
    cfg.auth.jwt_expiry_secs = 1; // very short
    let (handle, base) = boot_server(cfg, pool).await;
    let client = Client::new();
    let token = register_and_login(&client, &base, "exp@e.com", "hunter2hunter2").await;

    // Wait for token to expire. leeway=0 in verify_jwt; 2s > 1s expiry.
    tokio::time::sleep(Duration::from_secs(2)).await;

    let resp = client
        .post(format!("{base}/api/v1/order"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&serde_json::json!({
            "symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT",
            "timeInForce":"GTC","quantity":"0.001","price":"59000"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 401);
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn duplicate_register_returns_409(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let (handle, base) = boot_server(cfg, pool).await;
    let client = Client::new();
    let body = serde_json::json!({"email": "dup@e.com", "password": "hunter2hunter2"});
    let r1 = client
        .post(format!("{base}/api/v1/auth/register"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(r1.status().as_u16(), 201);
    let r2 = client
        .post(format!("{base}/api/v1/auth/register"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(r2.status().as_u16(), 409);
    let body2: serde_json::Value = r2.json().await.unwrap();
    assert_eq!(body2["code"], -1014);
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn duplicate_client_order_id_returns_409(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let (handle, base) = boot_server(cfg, pool).await;
    let client = Client::new();
    let token = register_and_login(&client, &base, "coid@e.com", "hunter2hunter2").await;
    let body = serde_json::json!({
        "symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT",
        "timeInForce":"GTC","quantity":"0.001","price":"59000",
        "clientOrderId":"200001"
    });
    let r1 = client
        .post(format!("{base}/api/v1/order"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(r1.status().as_u16(), 200);
    let r2 = client
        .post(format!("{base}/api/v1/order"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(r2.status().as_u16(), 409);
    let body2: serde_json::Value = r2.json().await.unwrap();
    assert_eq!(body2["code"], -1014);
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn login_unknown_email_indistinguishable_from_wrong_password(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let (handle, base) = boot_server(cfg, pool).await;
    let client = Client::new();
    // Register a known user.
    let _ = register_and_login(&client, &base, "known@e.com", "hunter2hunter2").await;

    let r_unknown: serde_json::Value = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&serde_json::json!({"email":"unknown@e.com","password":"wrong"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let r_wrong: serde_json::Value = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&serde_json::json!({"email":"known@e.com","password":"wrong"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(r_unknown["code"], r_wrong["code"]);
    assert_eq!(r_unknown["msg"], r_wrong["msg"]);
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn me_endpoint_returns_own_info(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let (handle, base) = boot_server(cfg, pool).await;
    let client = Client::new();
    let token = register_and_login(&client, &base, "me@e.com", "hunter2hunter2").await;
    let resp: serde_json::Value = client
        .get(format!("{base}/api/v1/me"))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    // register_user lowercases emails; "me@e.com" is already lowercase.
    assert_eq!(resp["email"], "me@e.com");
    assert_eq!(resp["kycLevel"], 0);
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn cross_user_idor_via_jwt_blocked(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let (handle, base) = boot_server(cfg, pool).await;
    let client = Client::new();
    let token_a = register_and_login(&client, &base, "alice-idor@e.com", "hunter2hunter2").await;
    let token_b = register_and_login(&client, &base, "bob-idor@e.com", "hunter2hunter2").await;

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

    // B tries to cancel A's order. Engine emits OrderRejected/OrderNotFound;
    // HTTP responds 200 (command enqueued). No user data leaked at HTTP layer.
    let resp = client
        .post(format!("{base}/api/v1/order/cancel"))
        .header("Authorization", format!("Bearer {token_b}"))
        .json(&serde_json::json!({"orderId": order_id, "symbol":"BTCUSDT"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    // Verification of OrderRejected/OrderNotFound event is covered in stage0_e2e.
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn login_rate_limit_per_email(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let mut cfg = base_cfg(tmp.path());
    cfg.risk.max_orders_per_second = 1; // bucket of 1: second request guaranteed to hit limit
    let (handle, base) = boot_server(cfg, pool).await;
    let client = Client::new();
    // Fire 10 login attempts against the same email; at least one should hit 429 + -1003.
    let mut saw_429 = false;
    for _ in 0..10 {
        let resp = client
            .post(format!("{base}/api/v1/auth/login"))
            .json(&serde_json::json!({"email":"limit@e.com","password":"wrong"}))
            .send()
            .await
            .unwrap();
        if resp.status().as_u16() == 429 {
            saw_429 = true;
            let body: serde_json::Value = resp.json().await.unwrap();
            assert_eq!(body["code"], -1003);
            break;
        }
    }
    assert!(
        saw_429,
        "expected at least one 429 from per-email login limit"
    );
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn malformed_token_returns_401(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let (handle, base) = boot_server(cfg, pool).await;
    let client = Client::new();
    let resp = client
        .post(format!("{base}/api/v1/order"))
        .header("Authorization", "Bearer not-a-jwt")
        .json(&serde_json::json!({
            "symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT",
            "timeInForce":"GTC","quantity":"0.001","price":"59000"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 401);
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn register_rate_limit_per_ip(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let mut cfg = base_cfg(tmp.path());
    cfg.risk.max_orders_per_second = 1; // tiny bucket to deterministically trip
    let (handle, base) = boot_server(cfg, pool).await;
    let client = Client::new();
    // Fire 10 register attempts from the same loopback IP with distinct
    // emails (so we exercise the IP bucket, not the email-exists path).
    let mut saw_429 = false;
    for i in 0..10 {
        let resp = client
            .post(format!("{base}/api/v1/auth/register"))
            .json(&serde_json::json!({
                "email": format!("ratereg{i}@e.com"),
                "password": "hunter2hunter2"
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
    assert!(
        saw_429,
        "expected at least one 429 from per-IP register limit"
    );
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn client_order_id_exceeding_i64_max_returns_400(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let (handle, base) = boot_server(cfg, pool).await;
    let client = Client::new();
    let token = register_and_login(&client, &base, "coid-bound@e.com", "hunter2hunter2").await;

    // u64::MAX is 18446744073709551615 — wraps to -1 if cast to i64. Without
    // the boundary guard the row would silently insert with client_order_id=-1
    // and collide with future coid=0 submissions.
    let resp = client
        .post(format!("{base}/api/v1/order"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&serde_json::json!({
            "symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT",
            "timeInForce":"GTC","quantity":"0.001","price":"59000",
            "clientOrderId":"18446744073709551615"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["code"], -1100);
    handle.shutdown().await.unwrap();
}
