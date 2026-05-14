use actix_web::{HttpRequest, HttpResponse, web};
use exg_common::{OrderId, SymbolId, UnixMicros, UserId};
use exg_protocol::Command;
use tracing::{info, warn};

use crate::conversion::{
    to_amend_order_command, to_cancel_order_command, to_new_order_command,
};
use crate::error::ApiError;
use crate::state::AppState;
use crate::types::{
    AckResponse, AmendOrderRequest, CancelOrderRequest, HealthResponse, PlaceOrderRequest,
    PlaceOrderResponse,
};

/// Extract `X-User-Id` numeric header → `UserId`.
fn extract_user_id(req: &HttpRequest) -> Result<UserId, ApiError> {
    let h = req
        .headers()
        .get("X-User-Id")
        .ok_or_else(|| ApiError::unauthorized("missing X-User-Id header"))?;
    let s = h
        .to_str()
        .map_err(|_| ApiError::unauthorized("X-User-Id is not valid ASCII"))?;
    let n: u64 = s
        .parse()
        .map_err(|_| ApiError::unauthorized("X-User-Id is not numeric"))?;
    Ok(UserId::new(n))
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

pub async fn health() -> HttpResponse {
    HttpResponse::Ok().json(HealthResponse { status: "ok" })
}

pub async fn place_order(
    state: web::Data<AppState>,
    req: HttpRequest,
    body: web::Json<PlaceOrderRequest>,
) -> Result<HttpResponse, ApiError> {
    let user_id = extract_user_id(&req)?;
    // Validate symbol exists; to_new_order_command uses SymbolId(0) as placeholder
    // (real resolution wired in Stage 2), but we still reject unknown symbols at the
    // gateway boundary.
    let _symbol = lookup_symbol_id(&state.cfg, &body.symbol)?;
    let order_id = OrderId::new(state.snowflake.next_id());
    let ts = now();
    info!(
        target: "handler",
        path = "/order",
        user_id = user_id.value(),
        order_id = order_id.value(),
        "place_order in"
    );

    let cmd = to_new_order_command(&body, user_id, order_id, ts).inspect_err(|e| {
        warn!(target: "conversion", reason = %e.msg, "to_new_order_command failed");
    })?;
    enqueue(&state, &cmd)?;

    let resp = PlaceOrderResponse {
        order_id: order_id.value().to_string(),
        // PlaceOrderResponse.client_order_id is Option<u64>; PlaceOrderRequest.client_order_id
        // is Option<String>. The conversion fn already parsed it into u64 inside the Command;
        // return None here (client echoing deferred to Stage 1 read-path).
        client_order_id: None,
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
    let user_id = extract_user_id(&req)?;
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
    let user_id = extract_user_id(&req)?;
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use actix_web::{http::StatusCode, test};
    use exg_common::SnowflakeGen;
    use exg_config::ExgConfig;
    use exg_ringbuffer::RingBuffer;
    use parking_lot::Mutex;

    use super::*;
    use crate::app_factory::build_app;
    use crate::error::ERR_UNAUTHORIZED;

    fn test_state() -> AppState {
        // Leak the RingBuffer so it lives for the duration of the test binary.
        // `split()` returns raw-pointer handles; the mmap backing them must
        // outlive both Producer and Consumer.  In production, `main()` owns the
        // RingBuffer for the process lifetime.  In tests, Box::leak is the
        // simplest safe substitute.
        let rb = Box::leak(Box::new(RingBuffer::new(16, 4096).unwrap()));
        let (producer, _consumer) = rb.split();
        AppState {
            producer: Arc::new(Mutex::new(producer)),
            snowflake: Arc::new(SnowflakeGen::new(1)),
            cfg: Arc::new(ExgConfig::default_config()),
        }
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
        let body = r#"{"symbol":"BTCUSDT","side":"BUY","order_type":"LIMIT","time_in_force":"GTC","quantity":"0.001","price":"60000"}"#;
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
    async fn place_order_non_numeric_header_returns_401() {
        let app = test::init_service(build_app(test_state())).await;
        let body = r#"{"symbol":"BTCUSDT","side":"BUY","order_type":"LIMIT","time_in_force":"GTC","quantity":"0.001","price":"60000"}"#;
        let req = test::TestRequest::post()
            .uri("/api/v1/order")
            .insert_header(("X-User-Id", "abc"))
            .insert_header(("Content-Type", "application/json"))
            .set_payload(body)
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[actix_web::test]
    async fn place_order_malformed_json_returns_400() {
        let app = test::init_service(build_app(test_state())).await;
        let req = test::TestRequest::post()
            .uri("/api/v1/order")
            .insert_header(("X-User-Id", "42"))
            .insert_header(("Content-Type", "application/json"))
            .set_payload("not json")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert!(resp.status().is_client_error(), "got status {}", resp.status());
    }

    #[actix_web::test]
    async fn place_order_happy_returns_200_with_order_id() {
        let app = test::init_service(build_app(test_state())).await;
        let body = r#"{"symbol":"BTCUSDT","side":"BUY","order_type":"LIMIT","time_in_force":"GTC","quantity":"0.001","price":"60000"}"#;
        let req = test::TestRequest::post()
            .uri("/api/v1/order")
            .insert_header(("X-User-Id", "42"))
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
