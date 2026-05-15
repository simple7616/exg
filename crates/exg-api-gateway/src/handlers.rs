use actix_web::{HttpRequest, HttpResponse, web};
use exg_common::{OrderId, SymbolId, UnixMicros, UserId};
use exg_protocol::Command;
use tracing::{info, warn};

use crate::conversion::{to_amend_order_command, to_cancel_order_command, to_new_order_command};
use crate::error::ApiError;
use crate::state::AppState;
use crate::types::{
    AckResponse, AmendOrderRequest, CancelOrderRequest, HealthResponse, PlaceOrderRequest,
    PlaceOrderResponse,
};

/// Extract and verify the `Authorization: Bearer <jwt>` header, returning the
/// authenticated `UserId` on success.
fn extract_user_id_from_jwt(req: &HttpRequest, jwt_secret: &[u8]) -> Result<UserId, ApiError> {
    let h = req
        .headers()
        .get("Authorization")
        .ok_or_else(|| ApiError::unauthorized("missing Authorization header"))?;
    let s = h
        .to_str()
        .map_err(|_| ApiError::unauthorized("Authorization not valid ASCII"))?;
    let token = s
        .strip_prefix("Bearer ")
        .ok_or_else(|| ApiError::unauthorized("Authorization must be 'Bearer <jwt>'"))?;
    let claims = exg_user_service::verify_jwt(jwt_secret, token)
        .map_err(|_| ApiError::unauthorized("invalid or expired token"))?;
    Ok(UserId::new(claims.user_id))
}

fn now() -> UnixMicros {
    let micros = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0);
    UnixMicros::from_micros(micros)
}

fn lookup_symbol_id(cfg: &exg_config::ExgConfig, name: &str) -> Result<SymbolId, ApiError> {
    cfg.trading
        .symbols
        .iter()
        .find(|s| s.name == name)
        .map(|s| SymbolId::new(s.id))
        .ok_or_else(|| ApiError::bad_request(format!("unknown symbol: {name}")))
}

fn enqueue(state: &AppState, cmd: &Command) -> Result<(), ApiError> {
    let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(cmd)
        .map_err(|e| ApiError::internal(format!("rkyv encode: {e}")))?;
    let producer = state.producer.lock();
    producer.try_push(&bytes).map_err(|e| {
        use exg_ringbuffer::RingBufferError;
        match e {
            RingBufferError::WouldBlock => ApiError::rate_limited(),
            RingBufferError::MessageTooLarge { .. } => {
                ApiError::bad_request("command too large for ring slot")
            }
            other => ApiError::internal(format!("ring buffer push: {other}")),
        }
    })?;
    Ok(())
}

fn map_auth_error(e: exg_user_service::AuthError) -> ApiError {
    use exg_user_service::AuthError;
    match e {
        AuthError::InvalidInput(msg) => ApiError::bad_request(msg),
        AuthError::EmailExists | AuthError::EmailAlreadyExists => {
            ApiError::duplicate_resource("email already registered")
        }
        AuthError::InvalidCredentials => ApiError::unauthorized("invalid credentials"),
        AuthError::DbError(_) => ApiError::internal("database unavailable"),
        AuthError::JwtError(_) | AuthError::InvalidToken(_) | AuthError::TokenExpired => {
            ApiError::unauthorized("invalid token")
        }
        AuthError::HashError(msg) => ApiError::internal(msg),
        other => ApiError::internal(format!("auth error: {other}")),
    }
}

pub async fn health() -> HttpResponse {
    HttpResponse::Ok().json(HealthResponse { status: "ok" })
}

pub async fn place_order(
    state: web::Data<AppState>,
    req: HttpRequest,
    body: web::Json<PlaceOrderRequest>,
) -> Result<HttpResponse, ApiError> {
    let user_id = extract_user_id_from_jwt(&req, state.auth_cfg.jwt_secret.as_bytes())?;

    // Per-user rate limit gate (in-memory, no PG dependency).
    {
        let now_ts = UnixMicros::now();
        let key = format!("user:{}", user_id.value());
        let mut limiter = state.rate_limiter.lock();
        if !limiter.consume(&key, now_ts) {
            return Err(ApiError::user_rate_limited("rate limit exceeded for user"));
        }
    }

    // Per-clientOrderId dedup gate (handler-side INSERT ON CONFLICT).
    if let Some(coid_str) = body.client_order_id.as_deref() {
        let coid: u64 = coid_str
            .parse()
            .map_err(|_| ApiError::bad_request("clientOrderId must be numeric"))?;
        // PG BIGINT is signed i64. Reject values that would wrap into the
        // negative half of i64 — without this guard, two distinct u64 inputs
        // would collide on the unique constraint and silently defeat dedup.
        if coid > i64::MAX as u64 {
            return Err(ApiError::bad_request(
                "clientOrderId must fit in signed 64-bit (max 9223372036854775807)",
            ));
        }
        let now_micros = UnixMicros::now().as_micros() as i64;
        let inserted = sqlx::query(
            "INSERT INTO user_client_order_ids (user_id, client_order_id, created_at)
             VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
        )
        .bind(user_id.value() as i64)
        .bind(coid as i64)
        .bind(now_micros)
        .execute(&state.pool)
        .await
        .map_err(ApiError::db_unavailable)?;
        if inserted.rows_affected() == 0 {
            return Err(ApiError::duplicate_resource("duplicate clientOrderId"));
        }
    }

    let symbol = lookup_symbol_id(&state.cfg, &body.symbol)?;
    let order_id = OrderId::new(state.snowflake.next_id());
    let ts = now();
    info!(
        target: "handler",
        path = "/order",
        user_id = user_id.value(),
        order_id = order_id.value(),
        symbol_id = symbol.value(),
        "place_order in"
    );

    let cmd = to_new_order_command(&body, user_id, symbol, order_id, ts).inspect_err(|e| {
        warn!(target: "conversion", reason = %e.msg, "to_new_order_command failed");
    })?;
    enqueue(&state, &cmd)?;

    let resp = PlaceOrderResponse {
        order_id: order_id.value().to_string(),
        client_order_id: body.client_order_id.clone(),
        status: "ACCEPTED",
    };
    info!(
        target: "handler",
        path = "/order",
        status = 200,
        order_id = order_id.value(),
        "place_order out"
    );
    Ok(HttpResponse::Ok().json(resp))
}

pub async fn cancel_order(
    state: web::Data<AppState>,
    req: HttpRequest,
    body: web::Json<CancelOrderRequest>,
) -> Result<HttpResponse, ApiError> {
    let user_id = extract_user_id_from_jwt(&req, state.auth_cfg.jwt_secret.as_bytes())?;
    let symbol = lookup_symbol_id(&state.cfg, &body.symbol)?;
    let ts = now();
    info!(
        target: "handler",
        path = "/order/cancel",
        user_id = user_id.value(),
        order_id = body.order_id,
        "cancel_order in"
    );

    let cmd = to_cancel_order_command(&body, user_id, symbol, ts)?;
    enqueue(&state, &cmd)?;

    let resp = AckResponse {
        order_id: body.order_id.to_string(),
        status: "ACCEPTED",
    };
    info!(target: "handler", path = "/order/cancel", status = 200, "cancel_order out");
    Ok(HttpResponse::Ok().json(resp))
}

pub async fn amend_order(
    state: web::Data<AppState>,
    req: HttpRequest,
    body: web::Json<AmendOrderRequest>,
) -> Result<HttpResponse, ApiError> {
    let user_id = extract_user_id_from_jwt(&req, state.auth_cfg.jwt_secret.as_bytes())?;
    let symbol = lookup_symbol_id(&state.cfg, &body.symbol)?;
    let ts = now();
    info!(
        target: "handler",
        path = "/order/amend",
        user_id = user_id.value(),
        order_id = body.order_id,
        "amend_order in"
    );

    let cmd = to_amend_order_command(&body, user_id, symbol, ts).inspect_err(|e| {
        warn!(target: "conversion", reason = %e.msg, "to_amend_order_command failed");
    })?;
    enqueue(&state, &cmd)?;

    let resp = AckResponse {
        order_id: body.order_id.to_string(),
        status: "ACCEPTED",
    };
    info!(target: "handler", path = "/order/amend", status = 200, "amend_order out");
    Ok(HttpResponse::Ok().json(resp))
}

pub async fn register(
    state: web::Data<AppState>,
    body: web::Json<crate::types::RegisterRequest>,
) -> Result<HttpResponse, ApiError> {
    let user_id =
        exg_user_service::register_user(&state.pool, &state.snowflake, &body.email, &body.password)
            .await
            .map_err(map_auth_error)?;
    let resp = crate::types::RegisterResponseBody {
        user_id: user_id.value().to_string(),
        email: body.email.to_lowercase(),
        status: "REGISTERED",
    };
    Ok(HttpResponse::Created().json(resp))
}

pub async fn login(
    state: web::Data<AppState>,
    req: HttpRequest,
    body: web::Json<crate::types::LoginRequest>,
) -> Result<HttpResponse, ApiError> {
    let now_ts = UnixMicros::now();
    let email_key = format!("login:email:{}", body.email.to_lowercase());
    let ip_key = format!(
        "login:ip:{}",
        req.peer_addr()
            .map(|a| a.ip().to_string())
            .unwrap_or_else(|| "unknown".into())
    );
    {
        let mut limiter = state.rate_limiter.lock();
        if !limiter.consume(&email_key, now_ts) || !limiter.consume(&ip_key, now_ts) {
            return Err(ApiError::user_rate_limited("login rate limit exceeded"));
        }
    }
    let resp_inner =
        exg_user_service::login_user(&state.pool, &state.auth_cfg, &body.email, &body.password)
            .await
            .map_err(map_auth_error)?;
    let resp = crate::types::LoginResponseBody {
        access_token: resp_inner.access_token,
        expires_in: resp_inner.expires_in,
        user_id: resp_inner.user_id.to_string(),
    };
    Ok(HttpResponse::Ok().json(resp))
}

pub async fn me(state: web::Data<AppState>, req: HttpRequest) -> Result<HttpResponse, ApiError> {
    let user_id = extract_user_id_from_jwt(&req, state.auth_cfg.jwt_secret.as_bytes())?;
    let row = exg_user_service::find_user_by_id(&state.pool, user_id)
        .await
        .map_err(map_auth_error)?
        .ok_or_else(|| ApiError::unauthorized("user not found"))?;
    let resp = crate::types::MeResponse {
        user_id: row.user_id.value().to_string(),
        email: row.email,
        kyc_level: row.kyc_level,
    };
    Ok(HttpResponse::Ok().json(resp))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use actix_web::{http::StatusCode, test};
    use exg_common::SnowflakeGen;
    use exg_config::{AuthConfig, ExgConfig};
    use exg_ringbuffer::RingBuffer;
    use parking_lot::Mutex;
    use sqlx::PgPool;

    use super::*;
    use crate::app_factory::build_app;
    use crate::error::ERR_UNAUTHORIZED;
    use crate::middleware::RateLimiter;

    fn test_state() -> AppState {
        // Leak the RingBuffer so it lives for the duration of the test binary.
        // `split()` returns raw-pointer handles; the mmap backing them must
        // outlive both Producer and Consumer.  In production, `main()` owns the
        // RingBuffer for the process lifetime.  In tests, Box::leak is the
        // simplest safe substitute.
        let rb = Box::leak(Box::new(RingBuffer::new(16, 4096).unwrap()));
        let (producer, _consumer) = rb.split();
        let pool =
            PgPool::connect_lazy("postgres://exg:exg_dev_password@localhost:5433/exg").unwrap();
        AppState {
            producer: Arc::new(Mutex::new(producer)),
            snowflake: Arc::new(SnowflakeGen::new(1)),
            cfg: Arc::new(ExgConfig::default_config()),
            pool,
            auth_cfg: Arc::new(AuthConfig {
                jwt_secret: "a".repeat(32),
                jwt_expiry_secs: 3600,
            }),
            rate_limiter: Arc::new(Mutex::new(RateLimiter::new(100, 10.0))),
        }
    }

    /// Mint a valid JWT for the given user_id using the test state's auth_cfg.
    fn test_jwt_for_user(state: &AppState, user_id: u64) -> String {
        use exg_user_service::{JwtClaims, sign_jwt};
        let now = chrono::Utc::now().timestamp() as u64;
        let claims = JwtClaims {
            user_id,
            iat: now,
            exp: now + 3600,
        };
        sign_jwt(state.auth_cfg.jwt_secret.as_bytes(), &claims).unwrap()
    }

    #[actix_web::test]
    async fn health_returns_ok() {
        let app = test::init_service(build_app(test_state())).await;
        let req = test::TestRequest::get().uri("/api/v1/health").to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[actix_web::test]
    async fn place_order_missing_header_returns_401() {
        let app = test::init_service(build_app(test_state())).await;
        let body = r#"{"symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT","timeInForce":"GTC","quantity":"0.001","price":"60000"}"#;
        let req = test::TestRequest::post()
            .uri("/api/v1/order")
            .insert_header(("Content-Type", "application/json"))
            .set_payload(body)
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let body: serde_json::Value = test::read_body_json(resp).await;
        assert_eq!(body["code"], ERR_UNAUTHORIZED);
    }

    #[actix_web::test]
    async fn place_order_bad_authorization_returns_401() {
        let app = test::init_service(build_app(test_state())).await;
        let body = r#"{"symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT","timeInForce":"GTC","quantity":"0.001","price":"60000"}"#;
        // Send a malformed (non-Bearer) Authorization header.
        let req = test::TestRequest::post()
            .uri("/api/v1/order")
            .insert_header(("Authorization", "Basic abc"))
            .insert_header(("Content-Type", "application/json"))
            .set_payload(body)
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[actix_web::test]
    async fn place_order_malformed_json_returns_400() {
        let state = test_state();
        let token = test_jwt_for_user(&state, 42);
        let app = test::init_service(build_app(state)).await;
        let req = test::TestRequest::post()
            .uri("/api/v1/order")
            .insert_header(("Authorization", format!("Bearer {token}")))
            .insert_header(("Content-Type", "application/json"))
            .set_payload("not json")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert!(
            resp.status().is_client_error(),
            "got status {}",
            resp.status()
        );
    }

    #[actix_web::test]
    async fn place_order_happy_returns_200_with_order_id() {
        let state = test_state();
        let token = test_jwt_for_user(&state, 42);
        let app = test::init_service(build_app(state)).await;
        // No clientOrderId → dedup gate is skipped (no PG dependency).
        let body = r#"{"symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT","timeInForce":"GTC","quantity":"0.001","price":"60000"}"#;
        let req = test::TestRequest::post()
            .uri("/api/v1/order")
            .insert_header(("Authorization", format!("Bearer {token}")))
            .insert_header(("Content-Type", "application/json"))
            .set_payload(body)
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body: serde_json::Value = test::read_body_json(resp).await;
        assert_eq!(body["status"], "ACCEPTED");
        assert!(!body["orderId"].as_str().unwrap_or("").is_empty());
    }
}
