//! PG-backed user operations. Pure async fns over `&PgPool`.

use exg_common::{SnowflakeGen, UnixMicros, UserId};
use exg_config::AuthConfig;
use once_cell::sync::OnceCell;
use sqlx::PgPool;
use tracing::warn;

use crate::{
    AuthError, JwtClaims, LoginResponse,
    hash_password, sign_jwt, verify_password,
};

/// Argon2id hash of a fixed constant, computed once at boot. Used by
/// `login_user` when the email lookup misses, so verify_password is always
/// invoked and the total wall-time is constant regardless of email existence.
/// See Stage 1a spec §9 invariant #20.
pub static DUMMY_ARGON2_HASH: OnceCell<String> = OnceCell::new();

const DUMMY_INPUT: &str = "__dummy_constant_for_timing_equalization__";

/// Initialize DUMMY_ARGON2_HASH idempotently. Called once from
/// `exg-server::run_with_config`; tests call `init_dummy_argon2_hash_for_tests`.
pub fn init_dummy_argon2_hash() -> Result<(), AuthError> {
    DUMMY_ARGON2_HASH
        .get_or_try_init(|| hash_password(DUMMY_INPUT))
        .map(|_| ())
}

/// Test helper — idempotent init wrapper. Tests should call this in any test
/// that exercises the user-not-found branch of `login_user`. Tests that always
/// register before login don't strictly need it (the hash is only consulted
/// when SELECT misses) but calling is harmless.
#[doc(hidden)]
pub fn init_dummy_argon2_hash_for_tests() {
    let _ = init_dummy_argon2_hash();
}

#[derive(Debug, Clone)]
pub struct UserRow {
    pub user_id: UserId,
    pub email: String,
    pub kyc_level: i16,
    pub is_active: bool,
}

/// Register a new user. Email is lowercased before insertion.
/// Returns AuthError::EmailExists on UNIQUE constraint hit.
pub async fn register_user(
    pool: &PgPool,
    id_gen: &SnowflakeGen,
    email: &str,
    password: &str,
) -> Result<UserId, AuthError> {
    if email.is_empty() || email.len() > 254 {
        return Err(AuthError::InvalidInput("email length must be 1-254".into()));
    }
    if password.len() < 8 || password.len() > 128 {
        return Err(AuthError::InvalidInput(
            "password length must be 8-128".into(),
        ));
    }
    let email_lc = email.to_lowercase();
    let pw_hash = hash_password(password)?;
    let user_id = UserId::new(id_gen.next_id());
    let now_micros = UnixMicros::now().as_micros() as i64;

    let result = sqlx::query(
        "INSERT INTO users (user_id, email, password_hash, kyc_level, is_active, created_at, updated_at)
         VALUES ($1, $2, $3, 0, true, $4, $4)
         ON CONFLICT (email) DO NOTHING
         RETURNING user_id",
    )
    .bind(user_id.value() as i64)
    .bind(&email_lc)
    .bind(&pw_hash)
    .bind(now_micros)
    .fetch_optional(pool)
    .await
    .map_err(|e| {
        warn!(target: "repo", err = %e, "register_user db error");
        AuthError::DbError(e.to_string())
    })?;

    match result {
        Some(_) => Ok(user_id),
        None => Err(AuthError::EmailExists),
    }
}

/// Log in by email + password. Returns AuthError::InvalidCredentials for any
/// failure mode (user-not-found / wrong-password / inactive); spec §9 #17.
/// Constant-time: verify_password runs once regardless of branch (against
/// DUMMY_ARGON2_HASH if SELECT misses); spec §9 #20.
pub async fn login_user(
    pool: &PgPool,
    auth_cfg: &AuthConfig,
    email: &str,
    password: &str,
) -> Result<LoginResponse, AuthError> {
    let email_lc = email.to_lowercase();

    let row: Option<(i64, String, bool)> = sqlx::query_as(
        "SELECT user_id, password_hash, is_active FROM users WHERE email = $1",
    )
    .bind(&email_lc)
    .fetch_optional(pool)
    .await
    .map_err(|e| {
        warn!(target: "repo", err = %e, "login_user db error");
        AuthError::DbError(e.to_string())
    })?;

    let dummy = DUMMY_ARGON2_HASH
        .get()
        .expect("DUMMY_ARGON2_HASH must be initialized before login_user is called");

    let (db_user_id, stored_hash, is_active) = match &row {
        Some((uid, hash, active)) => (*uid as u64, hash.clone(), *active),
        None => (0u64, dummy.clone(), false),
    };

    // Always run verify_password exactly once.
    let pw_ok = verify_password(password, &stored_hash)?;

    if row.is_none() || !pw_ok || !is_active {
        return Err(AuthError::InvalidCredentials);
    }

    let now = chrono::Utc::now().timestamp() as u64;
    let claims = JwtClaims {
        user_id: db_user_id,
        iat: now,
        exp: now + auth_cfg.jwt_expiry_secs,
    };
    let token = sign_jwt(auth_cfg.jwt_secret.as_bytes(), &claims)?;

    Ok(LoginResponse {
        access_token: token,
        expires_in: auth_cfg.jwt_expiry_secs,
        user_id: db_user_id,
    })
}

/// Find a user by ID. Returns None if not found.
pub async fn find_user_by_id(
    pool: &PgPool,
    user_id: UserId,
) -> Result<Option<UserRow>, AuthError> {
    let row: Option<(i64, String, i16, bool)> = sqlx::query_as(
        "SELECT user_id, email, kyc_level, is_active FROM users WHERE user_id = $1",
    )
    .bind(user_id.value() as i64)
    .fetch_optional(pool)
    .await
    .map_err(|e| AuthError::DbError(e.to_string()))?;
    Ok(row.map(|(uid, email, kyc, active)| UserRow {
        user_id: UserId::new(uid as u64),
        email,
        kyc_level: kyc,
        is_active: active,
    }))
}
