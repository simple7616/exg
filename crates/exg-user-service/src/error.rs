#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("user not found")]
    UserNotFound,
    #[error("email already registered")]
    EmailAlreadyExists,
    #[error("invalid credentials")]
    InvalidCredentials,
    #[error("invalid token: {0}")]
    InvalidToken(String),
    #[error("token expired")]
    TokenExpired,
    #[error("2fa not enabled")]
    TwoFactorNotEnabled,
    #[error("invalid 2fa code")]
    Invalid2faCode,
    #[error("api key not found")]
    ApiKeyNotFound,
    #[error("api key revoked")]
    ApiKeyRevoked,
    #[error("invalid signature")]
    InvalidSignature,
    #[error("permission denied")]
    PermissionDenied,
    #[error("password too weak: {0}")]
    WeakPassword(String),
    #[error("internal error: {0}")]
    Internal(String),
    #[error("jwt error: {0}")]
    JwtError(String),
    #[error("hash error: {0}")]
    HashError(String),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("email already registered")]
    EmailExists,
    #[error("database error: {0}")]
    DbError(String),
}
