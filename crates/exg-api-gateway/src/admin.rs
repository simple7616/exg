//! Stage 2 — admin HTTP surface (separate port, X-Admin-Secret gated).
//!
//! Bound to `cfg.server.admin_port` by a second actix `HttpServer` (Task 7)
//! that shares `AppState` (same `Mutex<Producer>`) with the main server.

use actix_web::{App, HttpRequest, HttpResponse, web};
use exg_common::{Decimal128, SymbolId, UnixMicros, UserId};
use exg_protocol::Command;
use subtle::ConstantTimeEq;

use crate::error::ApiError;
use crate::state::AppState;
use crate::types::AdminMarkPriceRequest;

/// Constant-time compare of the `X-Admin-Secret` header against the
/// configured secret. Missing/mismatch → 401. Invariant 26: rejected
/// before any `Command` is produced.
fn check_admin_secret(req: &HttpRequest, expected: &str) -> Result<(), ApiError> {
    let provided = req
        .headers()
        .get("X-Admin-Secret")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| ApiError::unauthorized("missing X-Admin-Secret header"))?;
    let a = provided.as_bytes();
    let b = expected.as_bytes();
    // `ConstantTimeEq` needs equal-length slices; length mismatch is an
    // immediate (still constant-time-safe) reject.
    let ok = a.len() == b.len() && bool::from(a.ct_eq(b));
    if !ok {
        return Err(ApiError::unauthorized("invalid X-Admin-Secret"));
    }
    Ok(())
}

/// `POST /api/v1/admin/mark-price` — inject mark/index price.
pub async fn admin_mark_price(
    state: web::Data<AppState>,
    req: HttpRequest,
    body: web::Json<AdminMarkPriceRequest>,
) -> Result<HttpResponse, ApiError> {
    check_admin_secret(&req, &state.cfg.admin.admin_secret)?;

    let mark_price: Decimal128 = body
        .mark_price
        .parse()
        .map_err(|_| ApiError::bad_request("markPrice must be a decimal"))?;
    let index_price: Decimal128 = body
        .index_price
        .parse()
        .map_err(|_| ApiError::bad_request("indexPrice must be a decimal"))?;
    if index_price <= Decimal128::ZERO {
        return Err(ApiError::bad_request("indexPrice must be positive"));
    }
    // CEO review C5 / invariant 29: a non-positive mark price would make
    // `mark <= stop_price` true for every positive-stop sell → mass trigger.
    if mark_price <= Decimal128::ZERO {
        return Err(ApiError::bad_request("markPrice must be positive"));
    }

    let symbol = SymbolId::new(state.cfg.trading.symbols[0].id);
    // CEO review C6 / invariant 30: audit the high-privilege market-moving
    // action before enqueue.
    tracing::info!(
        target: "admin",
        mark_price = %mark_price,
        index_price = %index_price,
        "mark price injected"
    );
    let cmd = Command::UpdateMarkPrice {
        symbol,
        mark_price,
        index_price,
        timestamp: UnixMicros::now(),
    };
    enqueue_admin(&state, &cmd)?;
    Ok(HttpResponse::Ok().json(serde_json::json!({ "status": "ACCEPTED" })))
}

/// `POST /api/v1/admin/funding-tick` — trigger a funding computation.
pub async fn admin_funding_tick(
    state: web::Data<AppState>,
    req: HttpRequest,
) -> Result<HttpResponse, ApiError> {
    check_admin_secret(&req, &state.cfg.admin.admin_secret)?;
    let symbol = SymbolId::new(state.cfg.trading.symbols[0].id);
    tracing::info!(target: "admin", "funding tick");
    let cmd = Command::ComputeFunding {
        symbol,
        timestamp: UnixMicros::now(),
    };
    enqueue_admin(&state, &cmd)?;
    Ok(HttpResponse::Ok().json(serde_json::json!({ "status": "ACCEPTED" })))
}

/// `POST /api/v1/admin/credit` — credit a user's available balance.
pub async fn admin_credit(
    state: web::Data<AppState>,
    req: HttpRequest,
    body: web::Json<crate::types::AdminCreditRequest>,
) -> Result<HttpResponse, ApiError> {
    check_admin_secret(&req, &state.cfg.admin.admin_secret)?;
    let amount: Decimal128 = body
        .amount
        .parse()
        .map_err(|_| ApiError::bad_request("amount must be a decimal"))?;
    if !amount.is_positive() {
        return Err(ApiError::bad_request("amount must be positive"));
    }
    let user_id = UserId::new(body.user_id);
    let ts = UnixMicros::now();
    // CEO review C2: a ts-only key collides when two same-user credits
    // land in the same microsecond (e2e/demo issue rapid sequential
    // credits) → second deposit silently no-ops → silently lost funds.
    // Embed a process-unique monotonic counter so every accepted credit
    // gets a distinct key. The command carries it; replay re-applies the
    // recorded key (deterministic, still idempotent cross-machine).
    static ADMIN_CREDIT_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = ADMIN_CREDIT_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let idempotency_key = format!("admincredit_{}_{}_{}", body.user_id, ts.as_micros(), n);
    tracing::info!(target: "admin", user_id = body.user_id, amount = %amount, "admin credit");
    let cmd = Command::AdminCredit {
        user_id,
        amount,
        idempotency_key,
        timestamp: ts,
    };
    enqueue_admin(&state, &cmd)?;
    Ok(HttpResponse::Ok().json(serde_json::json!({ "status": "ACCEPTED" })))
}

/// Push an admin-produced command into the shared ring buffer. Mirrors
/// the order handlers' private `enqueue` (same `Mutex<Producer>`, same
/// 429 backpressure mapping); kept local because `handlers::enqueue` is
/// private to that module.
fn enqueue_admin(state: &AppState, cmd: &Command) -> Result<(), ApiError> {
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

/// Build the admin-only Actix `App` (bound to `admin_port` by exg-server
/// in Task 7). Return bound copied verbatim from `app_factory::build_app`.
pub fn build_admin_app(
    state: AppState,
) -> App<
    impl actix_web::dev::ServiceFactory<
        actix_web::dev::ServiceRequest,
        Config = (),
        Response = actix_web::dev::ServiceResponse<actix_web::body::BoxBody>,
        Error = actix_web::Error,
        InitError = (),
    >,
> {
    App::new()
        .app_data(web::Data::new(state))
        .route("/api/v1/admin/mark-price", web::post().to(admin_mark_price))
        .route(
            "/api/v1/admin/funding-tick",
            web::post().to(admin_funding_tick),
        )
        .route("/api/v1/admin/credit", web::post().to(admin_credit))
}
