use actix_web::HttpResponse;
use serde::Serialize;

// ── Standard error codes (Binance-compatible) ────────────────────────────

pub const ERR_UNKNOWN: i32 = -1000;
pub const ERR_UNAUTHORIZED: i32 = -1002;
pub const ERR_TOO_MANY_REQUESTS: i32 = -1015;
pub const ERR_INVALID_PARAMETER: i32 = -1100;
pub const ERR_ORDER_NOT_FOUND: i32 = -2013;
pub const ERR_INSUFFICIENT_BALANCE: i32 = -2010;

// ── API error response ──────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct ApiError {
    pub code: i32,
    pub msg: String,
}

impl ApiError {
    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self {
            code: ERR_INVALID_PARAMETER,
            msg: msg.into(),
        }
    }

    pub fn unauthorized(msg: impl Into<String>) -> Self {
        Self {
            code: ERR_UNAUTHORIZED,
            msg: msg.into(),
        }
    }

    pub fn not_found(msg: impl Into<String>) -> Self {
        Self {
            code: ERR_ORDER_NOT_FOUND,
            msg: msg.into(),
        }
    }

    pub fn rate_limited() -> Self {
        Self {
            code: ERR_TOO_MANY_REQUESTS,
            msg: "Too many requests".to_owned(),
        }
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        Self {
            code: ERR_UNKNOWN,
            msg: msg.into(),
        }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code, self.msg)
    }
}

impl std::error::Error for ApiError {}

impl actix_web::ResponseError for ApiError {
    fn status_code(&self) -> actix_web::http::StatusCode {
        match self.code {
            ERR_UNAUTHORIZED => actix_web::http::StatusCode::UNAUTHORIZED,
            ERR_TOO_MANY_REQUESTS => actix_web::http::StatusCode::TOO_MANY_REQUESTS,
            ERR_ORDER_NOT_FOUND => actix_web::http::StatusCode::NOT_FOUND,
            _ => actix_web::http::StatusCode::BAD_REQUEST,
        }
    }

    fn error_response(&self) -> HttpResponse {
        HttpResponse::build(self.status_code()).json(self)
    }
}
