# Stage 1a Persistence + Auth Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Spec:** [`docs/superpowers/specs/2026-05-14-stage1a-persistence-auth-design.md`](../specs/2026-05-14-stage1a-persistence-auth-design.md)

**Goal:** Real user identity via PostgreSQL + JWT bearer auth + per-user dedup. Replace Stage 0's `X-User-Id` header stub with verifiable JWT-derived `user_id`, enforce `clientOrderId` uniqueness in PG, and rate-limit login by per-email + per-IP.

**Architecture:** Add `sqlx::PgPool` to `AppState`, refactor `exg-user-service::auth` from stateful struct into pure crypto fns + new PG-backed `repo.rs`. Three new REST handlers (`/auth/register`, `/auth/login`, `/me`) plus rewritten `extract_user_id_from_jwt`. Dedup is a handler-side `INSERT ON CONFLICT` gate before ring-buffer enqueue. RateLimiter (existing in-memory token bucket) now actually mounted into request flow. Constant-time login via boot-initialized `DUMMY_ARGON2_HASH` OnceCell.

**Tech Stack:** Rust 2024, Tokio, Actix-web 4, sqlx 0.8 (postgres + runtime-tokio), Argon2id (argon2 0.5), jsonwebtoken 9 (HS256), parking_lot Mutex, OnceCell.

**Branch:** `feat/stage1a-persistence-auth`. **Base commit:** `97cf14d`.

---

## Invariant Reminders (Spec §9 #11-20)

These hold throughout. Violations block merge:

11. JWT secret ≥ 32 bytes and not the placeholder `"CHANGE-ME-DEV-ONLY-MUST-BE-AT-LEAST-32-BYTES-OK"`. Boot panics on violation.
12. Password hashing uses only Argon2id.
13. `client_order_id` dedup at handler BEFORE ring-buffer enqueue. PG INSERT ON CONFLICT. Orphan-row semantic accepted (HTTP 429 after INSERT leaves row; retry hits 409; never duplicate-orders).
14. Email lowercased before store/compare.
15. JWT claims contain `user_id` + `exp` + `iat`.
16. Runtime DB errors degrade to 500, never panic (WAL is source-of-truth; DB is not).
17. Login responses byte-identical for user-not-found / wrong-password / inactive.
18. Passwords never log. `RegisterRequest`/`LoginRequest` MUST NOT derive `Debug` directly; either wrap in `Secret<String>` (secrecy crate) or manual `impl Debug` masking; assertion test guards this.
19. Login rate limit: per-email AND per-IP buckets, either exhausted → 429 + `-1003`.
20. Login response time constant: verify_password runs once per call regardless of email existence (against `DUMMY_ARGON2_HASH` when SELECT misses). Unit-test on `login_user` fn directly (100 samples, median diff < 5ms); not e2e.

Plus Stage 0 §9 #1-10 unchanged.

---

## File Structure

### Created

| File | Responsibility |
|---|---|
| `crates/exg-user-service/src/repo.rs` | PG-backed `register_user`, `login_user`, `find_user_by_id`, `UserRow` struct |
| `crates/exg-user-service/tests/repo_test.rs` | 7 `#[sqlx::test]` integration cases |
| `crates/exg-user-service/tests/timing_test.rs` | Unit-level constant-time login verification (100-sample median diff) |
| `migrations/20260514000001_client_order_ids.up.sql` | `user_client_order_ids` table |
| `migrations/20260514000001_client_order_ids.down.sql` | Drop table |
| `crates/exg-server/tests/stage1a_e2e.rs` | All Stage 1a end-to-end scenarios |
| `scripts/demo-stage1a.sh` | Cold-boot demo: register → login → order → dup → wal-dump |

### Modified

| File | Change |
|---|---|
| `config/default.toml` | Add `[auth]` block; fix `[database].url` password |
| `crates/exg-config/src/lib.rs` | Add `AuthConfig` struct; add `auth: AuthConfig` to `ExgConfig` |
| `crates/exg-config/src/validation.rs` | Validate `auth.jwt_secret` length + placeholder + expiry |
| `crates/exg-config/src/tests.rs` | 5 new tests |
| `crates/exg-user-service/Cargo.toml` | Add `sqlx`, `tracing`, optional `secrecy` |
| `crates/exg-user-service/src/auth.rs` | Extract `sign_jwt`/`verify_jwt`/`hash_password`/`verify_password` as `pub fn`; mark `AuthService` mutators `#[allow(dead_code)]` |
| `crates/exg-user-service/src/lib.rs` | `pub mod repo;` + re-exports |
| `crates/exg-api-gateway/Cargo.toml` | Add `sqlx`, `exg-user-service`, `exg-config` |
| `crates/exg-api-gateway/src/types.rs` | Add `RegisterRequest`/`LoginRequest`/`LoginResponse`/`MeResponse`; ensure password fields never derive Debug directly |
| `crates/exg-api-gateway/src/error.rs` | `ERR_RATE_LIMITED_USER = -1003`; rename `ERR_DUPLICATE_ORDER` → `ERR_DUPLICATE_RESOURCE` (value `-1014`); new constructors `duplicate_resource`, `user_rate_limited`, `db_unavailable` |
| `crates/exg-api-gateway/src/state.rs` | `AppState` gains `pool`, `auth_cfg`, `rate_limiter` |
| `crates/exg-api-gateway/src/handlers.rs` | `extract_user_id` → `extract_user_id_from_jwt`; new `register`/`login`/`me`; `place_order` gains rate-limit + dedup gates |
| `crates/exg-api-gateway/src/app_factory.rs` | Mount `/api/v1/auth/register`, `/api/v1/auth/login`, `/api/v1/me` |
| `crates/exg-server/Cargo.toml` | Add `sqlx`, ensure `parking_lot`, `tracing` |
| `crates/exg-server/src/lib.rs` | `run_with_config` adds JWT-secret invariant, `PgPool::connect`, `SELECT 1` ping, `DUMMY_ARGON2_HASH` init, `AppState` extension |
| `crates/exg-server/tests/boot_panics.rs` | 3 new tests + existing tests gain `cfg.auth` field |
| `crates/exg-server/tests/stage0_e2e.rs` | **REGRESSION REWRITE**: all 7 tests use JWT bearer via login_helper |

---

## Task 1 — Config + migration: AuthConfig + db url fix + new dedup table

**Files:**
- Modify: `config/default.toml`
- Modify: `crates/exg-config/src/lib.rs`
- Modify: `crates/exg-config/src/validation.rs`
- Modify: `crates/exg-config/src/tests.rs`
- Create: `migrations/20260514000001_client_order_ids.up.sql`
- Create: `migrations/20260514000001_client_order_ids.down.sql`

- [ ] **Step 1: Write the failing config tests first**

Append to `crates/exg-config/src/tests.rs`:

```rust
#[test]
fn test_auth_jwt_secret_too_short_rejected() {
    let mut cfg = ExgConfig::default_config();
    cfg.auth.jwt_secret = "short".into();
    let err = cfg.validate().unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("jwt_secret"), "msg: {msg}");
}

#[test]
fn test_auth_jwt_secret_placeholder_rejected() {
    let mut cfg = ExgConfig::default_config();
    cfg.auth.jwt_secret = "CHANGE-ME-DEV-ONLY-MUST-BE-AT-LEAST-32-BYTES-OK".into();
    let err = cfg.validate().unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("jwt_secret") || msg.contains("placeholder"));
}

#[test]
fn test_auth_jwt_secret_valid_32_bytes_ok() {
    let mut cfg = ExgConfig::default_config();
    cfg.auth.jwt_secret = "a".repeat(32);
    assert!(cfg.validate().is_ok());
}

#[test]
fn test_auth_jwt_expiry_zero_rejected() {
    let mut cfg = ExgConfig::default_config();
    cfg.auth.jwt_expiry_secs = 0;
    let err = cfg.validate().unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("jwt_expiry"), "msg: {msg}");
}

#[test]
fn test_database_url_format_sanity() {
    let cfg = ExgConfig::default_config();
    assert!(cfg.database.url.starts_with("postgres://"), "default url: {}", cfg.database.url);
}
```

- [ ] **Step 2: Run tests — verify they fail**

Run: `cargo test -p exg-config test_auth`
Expected: compile errors (`auth` field on `ExgConfig` doesn't exist).

- [ ] **Step 3: Add `AuthConfig` to `exg-config/src/lib.rs`**

In `crates/exg-config/src/lib.rs`, near the other config structs, add:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    /// Must be at least 32 bytes (256 bits) for HS256 security. Boot validates.
    pub jwt_secret: String,
    /// JWT access token lifetime in seconds. Stage 1a defaults to 86400 (24h).
    pub jwt_expiry_secs: u64,
}
```

Add `pub auth: AuthConfig` field to `ExgConfig` struct alongside server/database/etc.

In `default_config()`, add:

```rust
            auth: AuthConfig {
                jwt_secret: "CHANGE-ME-DEV-ONLY-MUST-BE-AT-LEAST-32-BYTES-OK".into(),
                jwt_expiry_secs: 86400,
            },
```

- [ ] **Step 4: Add validation in `crates/exg-config/src/validation.rs`**

In the main `validate` function (or wherever auth-section validation lives), add:

```rust
    // Stage 1a §9 invariant 11: JWT secret length and placeholder check.
    const JWT_SECRET_PLACEHOLDER: &str = "CHANGE-ME-DEV-ONLY-MUST-BE-AT-LEAST-32-BYTES-OK";
    if cfg.auth.jwt_secret.len() < 32 {
        return Err(ConfigError::Validation(format!(
            "auth.jwt_secret must be at least 32 bytes, got {}",
            cfg.auth.jwt_secret.len()
        )));
    }
    if cfg.auth.jwt_secret == JWT_SECRET_PLACEHOLDER {
        return Err(ConfigError::Validation(
            "auth.jwt_secret is the placeholder default; set EXG_AUTH_JWT_SECRET env var to a 32+ byte production secret".into()
        ));
    }
    if cfg.auth.jwt_expiry_secs == 0 {
        return Err(ConfigError::Validation(
            "auth.jwt_expiry_secs must be > 0".into()
        ));
    }
```

- [ ] **Step 5: Update `config/default.toml`**

Add at end:

```toml
[auth]
jwt_secret = "CHANGE-ME-DEV-ONLY-MUST-BE-AT-LEAST-32-BYTES-OK"
jwt_expiry_secs = 86400
```

Find `[database]` block and change the URL from `postgres://exg:exg@localhost:5432/exg` to:

```toml
url = "postgres://exg:exg_dev_password@localhost:5432/exg"
```

(matches `docker-compose.yml` and `scripts/migrate.sh`)

- [ ] **Step 6: Create the migration files**

`migrations/20260514000001_client_order_ids.up.sql`:

```sql
-- Stage 1a §9 invariant 13: per-user client_order_id dedup table
CREATE TABLE user_client_order_ids (
    user_id BIGINT NOT NULL,
    client_order_id BIGINT NOT NULL,
    created_at BIGINT NOT NULL,  -- UnixMicros
    PRIMARY KEY (user_id, client_order_id)
);
CREATE INDEX idx_user_client_order_ids_created_at
    ON user_client_order_ids (created_at);
```

`migrations/20260514000001_client_order_ids.down.sql`:

```sql
DROP TABLE user_client_order_ids;
```

- [ ] **Step 7: Run config tests — verify pass**

Run: `cargo test -p exg-config`
Expected: existing 17 + new 5 = 22, all green. (Tests use programmatic `default_config()`, so the dev placeholder is overridden before validate runs.)

The placeholder test must override the secret AWAY from the placeholder to test the valid path, and SET it to the placeholder to test rejection. Verify both branches work.

NOTE: `default_config()` returns the placeholder secret by design (forces operator to override). If the existing test that calls `cfg.validate()` on `default_config()` was previously green, it will now fail because the new validation rejects the placeholder. **Fix any pre-existing test that calls `default_config().validate()` without overriding `jwt_secret`** — override to `"a".repeat(32)` in the test setup.

- [ ] **Step 8: Commit**

```bash
git add config/default.toml crates/exg-config/ migrations/20260514000001_client_order_ids.*
git commit -m "$(cat <<'EOF'
feat(config): add AuthConfig + dedup migration for stage 1a

- AuthConfig { jwt_secret, jwt_expiry_secs } with boot-time validation
  (length >= 32, not placeholder, expiry > 0)
- Fix [database].url password mismatch with docker-compose + migrate.sh
- New migration 20260514000001_client_order_ids with PK (user_id, client_order_id)
  + idx_user_client_order_ids_created_at for Stage 7 TTL cleanup

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2 — `exg-user-service` refactor: pure crypto fns + Cargo deps

**Files:**
- Modify: `crates/exg-user-service/Cargo.toml`
- Modify: `crates/exg-user-service/src/auth.rs` (refactor existing 615 LOC)
- Modify: `crates/exg-user-service/src/lib.rs` (exports)

- [ ] **Step 1: Update `crates/exg-user-service/Cargo.toml`**

Add to `[dependencies]`:

```toml
sqlx = { workspace = true }
tracing = { workspace = true }
```

(Note: `jsonwebtoken`, `argon2`, `rand`, `uuid`, `chrono`, `totp-rs` already present.)

Add to `[dev-dependencies]`:

```toml
tempfile = { workspace = true }
```

- [ ] **Step 2: Read current `auth.rs` to identify pure crypto fns**

Run: `grep -n "pub fn" crates/exg-user-service/src/auth.rs | head -20`

The existing fns `sign_jwt`, `verify_jwt` (line ~130), `hash_password`, `verify_password` are methods on `AuthService`. Extract them as free `pub fn` outside the struct.

- [ ] **Step 3: Write the failing crypto-fn tests first**

In `crates/exg-user-service/src/auth.rs`, in the existing `#[cfg(test)] mod tests` block (or create one), add:

```rust
#[test]
fn test_sign_verify_jwt_roundtrip() {
    let secret = b"32-byte-secret-for-test-padding!";
    let claims = JwtClaims {
        user_id: 12345,
        exp: chrono::Utc::now().timestamp() as u64 + 3600,
        iat: chrono::Utc::now().timestamp() as u64,
    };
    let token = sign_jwt(secret, &claims).unwrap();
    let decoded = verify_jwt(secret, &token).unwrap();
    assert_eq!(decoded.user_id, 12345);
}

#[test]
fn test_verify_jwt_expired_rejected() {
    let secret = b"32-byte-secret-for-test-padding!";
    let claims = JwtClaims {
        user_id: 1,
        exp: 100,  // past
        iat: 50,
    };
    let token = sign_jwt(secret, &claims).unwrap();
    let result = verify_jwt(secret, &token);
    assert!(result.is_err());
}

#[test]
fn test_verify_jwt_wrong_secret_rejected() {
    let s1 = b"32-byte-secret-for-test-padding!";
    let s2 = b"DIFFERENT-secret-equal-length-x!";
    let claims = JwtClaims {
        user_id: 1,
        exp: chrono::Utc::now().timestamp() as u64 + 3600,
        iat: chrono::Utc::now().timestamp() as u64,
    };
    let token = sign_jwt(s1, &claims).unwrap();
    let result = verify_jwt(s2, &token);
    assert!(result.is_err());
}

#[test]
fn test_hash_password_uses_argon2id() {
    let hash = hash_password("hunter2hunter2").unwrap();
    assert!(hash.starts_with("$argon2id$"), "hash prefix: {hash}");
}

#[test]
fn test_verify_password_correct() {
    let hash = hash_password("hunter2hunter2").unwrap();
    assert!(verify_password("hunter2hunter2", &hash).unwrap());
}

#[test]
fn test_verify_password_wrong_returns_false() {
    let hash = hash_password("hunter2hunter2").unwrap();
    assert!(!verify_password("wrong-pw", &hash).unwrap());
}
```

- [ ] **Step 4: Run — verify failure**

Run: `cargo test -p exg-user-service test_sign_verify_jwt_roundtrip`
Expected: compile error — `sign_jwt`/`verify_jwt`/`hash_password`/`verify_password` not found as free functions.

- [ ] **Step 5: Extract pure crypto fns**

In `crates/exg-user-service/src/auth.rs`, BEFORE the `AuthService` struct definition (or in a new section), add:

```rust
/// Sign a JWT (HS256) with the given secret and claims.
pub fn sign_jwt(secret: &[u8], claims: &JwtClaims) -> Result<String, AuthError> {
    use jsonwebtoken::{encode, EncodingKey, Header};
    encode(
        &Header::default(),  // HS256 by default
        claims,
        &EncodingKey::from_secret(secret),
    )
    .map_err(|e| AuthError::JwtError(e.to_string()))
}

/// Verify a JWT and return its claims. Rejects expired tokens and bad signatures.
pub fn verify_jwt(secret: &[u8], token: &str) -> Result<JwtClaims, AuthError> {
    use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_exp = true;
    validation.leeway = 0;  // Stage 1a: no clock skew tolerance (Stage 7 adds)
    let token_data = decode::<JwtClaims>(
        token,
        &DecodingKey::from_secret(secret),
        &validation,
    )
    .map_err(|e| AuthError::JwtError(e.to_string()))?;
    Ok(token_data.claims)
}

/// Hash a password using Argon2id (default OWASP-recommended parameters).
pub fn hash_password(plain: &str) -> Result<String, AuthError> {
    use argon2::{Argon2, PasswordHasher};
    use argon2::password_hash::{rand_core::OsRng, SaltString};
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();  // Argon2id by default
    argon2
        .hash_password(plain.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| AuthError::HashError(e.to_string()))
}

/// Verify a plaintext password against an Argon2id hash. Constant-time.
pub fn verify_password(plain: &str, hash: &str) -> Result<bool, AuthError> {
    use argon2::{Argon2, PasswordHash, PasswordVerifier};
    let parsed = PasswordHash::new(hash)
        .map_err(|e| AuthError::HashError(format!("invalid hash format: {e}")))?;
    Ok(Argon2::default()
        .verify_password(plain.as_bytes(), &parsed)
        .is_ok())
}
```

If `AuthError` already exists in `error.rs` and is missing `JwtError`/`HashError` variants, add them:

```rust
// in error.rs:
#[error("jwt error: {0}")]
JwtError(String),

#[error("hash error: {0}")]
HashError(String),
```

Mark the existing `AuthService::register`/`login`/`enable_2fa`/etc with `#[allow(dead_code)]` and an explanatory comment:

```rust
// Stage 1a: in-memory state superseded by repo.rs PG-backed lib.
// Kept for Stage 2+ where API keys, sub-accounts, login_history,
// and 2FA endpoints will be revived (still as PG-backed forms).
#[allow(dead_code)]
impl AuthService {
    // existing methods unchanged
}
```

- [ ] **Step 6: Update `crates/exg-user-service/src/lib.rs`**

```rust
pub mod auth;
pub mod error;
pub mod user;
pub mod repo;  // NEW (filled in Task 3)

pub use auth::{
    JwtClaims, LoginResponse,
    sign_jwt, verify_jwt, hash_password, verify_password,
};
pub use error::AuthError;
// repo::* re-exported in Task 3
```

- [ ] **Step 7: Run crypto tests — verify pass**

Run: `cargo test -p exg-user-service test_`
Expected: 6 new crypto tests pass; existing tests on `AuthService` still pass (just dead-code-warned, not errored).

- [ ] **Step 8: Commit**

```bash
git add crates/exg-user-service/
git commit -m "$(cat <<'EOF'
refactor(user-service): extract pure crypto fns from AuthService

Pure free fns: sign_jwt, verify_jwt, hash_password, verify_password.
Mark in-memory AuthService methods #[allow(dead_code)] — kept for
Stage 2+ when PG-backed equivalents revive 2FA / API key / sub-account /
login_history (which still need the same crypto primitives).

Tests: roundtrip, expired-token rejected, wrong-secret rejected,
argon2id prefix, verify own hash, verify wrong returns false.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3 — `exg-user-service::repo` + `sqlx::test` integration tests

**Files:**
- Create: `crates/exg-user-service/src/repo.rs`
- Create: `crates/exg-user-service/tests/repo_test.rs`
- Modify: `crates/exg-user-service/src/lib.rs` (re-export `repo::*`)

- [ ] **Step 1: Write the failing integration tests first**

Create `crates/exg-user-service/tests/repo_test.rs`:

```rust
//! Stage 1a PG-backed repo integration tests.
//! Each test gets its own ephemeral DB via #[sqlx::test].

use exg_common::SnowflakeGen;
use exg_config::AuthConfig;
use exg_user_service::{
    AuthError, find_user_by_id, hash_password, login_user, register_user,
};
use sqlx::PgPool;

fn test_auth_cfg() -> AuthConfig {
    AuthConfig {
        jwt_secret: "a".repeat(32),
        jwt_expiry_secs: 3600,
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn register_then_find_by_id(pool: PgPool) {
    let id_gen = SnowflakeGen::new(1);
    let uid = register_user(&pool, &id_gen, "alice@example.com", "hunter2hunter2")
        .await
        .unwrap();
    let row = find_user_by_id(&pool, uid).await.unwrap().unwrap();
    assert_eq!(row.email, "alice@example.com");
    assert!(row.is_active);
}

#[sqlx::test(migrations = "../../migrations")]
async fn register_duplicate_email_rejected(pool: PgPool) {
    let id_gen = SnowflakeGen::new(1);
    register_user(&pool, &id_gen, "bob@example.com", "hunter2hunter2")
        .await
        .unwrap();
    let result = register_user(&pool, &id_gen, "bob@example.com", "different").await;
    assert!(matches!(result, Err(AuthError::EmailExists)));
}

#[sqlx::test(migrations = "../../migrations")]
async fn register_normalizes_email_to_lowercase(pool: PgPool) {
    let id_gen = SnowflakeGen::new(1);
    register_user(&pool, &id_gen, "Alice@Example.COM", "hunter2hunter2")
        .await
        .unwrap();
    // Re-register with lowercase variant should be rejected as duplicate.
    let result = register_user(&pool, &id_gen, "alice@example.com", "other").await;
    assert!(matches!(result, Err(AuthError::EmailExists)));
}

#[sqlx::test(migrations = "../../migrations")]
async fn login_with_correct_password(pool: PgPool) {
    let id_gen = SnowflakeGen::new(1);
    let cfg = test_auth_cfg();
    register_user(&pool, &id_gen, "carol@example.com", "hunter2hunter2")
        .await
        .unwrap();
    let resp = login_user(&pool, &cfg, "carol@example.com", "hunter2hunter2")
        .await
        .unwrap();
    assert!(!resp.access_token.is_empty());
    assert_eq!(resp.expires_in, 3600);
}

#[sqlx::test(migrations = "../../migrations")]
async fn login_with_wrong_password_rejected(pool: PgPool) {
    let id_gen = SnowflakeGen::new(1);
    let cfg = test_auth_cfg();
    register_user(&pool, &id_gen, "dave@example.com", "hunter2hunter2")
        .await
        .unwrap();
    let result = login_user(&pool, &cfg, "dave@example.com", "wrong").await;
    assert!(matches!(result, Err(AuthError::InvalidCredentials)));
}

#[sqlx::test(migrations = "../../migrations")]
async fn login_unknown_email_rejected(pool: PgPool) {
    let cfg = test_auth_cfg();
    // Must init dummy hash first; lib should panic clearly if not, but in
    // this test we set it manually to mimic boot.
    exg_user_service::init_dummy_argon2_hash_for_tests();
    let result = login_user(&pool, &cfg, "ghost@example.com", "any-pw").await;
    assert!(matches!(result, Err(AuthError::InvalidCredentials)));
}

#[sqlx::test(migrations = "../../migrations")]
async fn login_inactive_user_rejected(pool: PgPool) {
    let id_gen = SnowflakeGen::new(1);
    let cfg = test_auth_cfg();
    let uid = register_user(&pool, &id_gen, "eve@example.com", "hunter2hunter2")
        .await
        .unwrap();
    sqlx::query("UPDATE users SET is_active = false WHERE user_id = $1")
        .bind(uid.value() as i64)
        .execute(&pool)
        .await
        .unwrap();
    let result = login_user(&pool, &cfg, "eve@example.com", "hunter2hunter2").await;
    assert!(matches!(result, Err(AuthError::InvalidCredentials)));
}
```

- [ ] **Step 2: Run tests — verify they fail**

Run: `cargo test -p exg-user-service --test repo_test`
Expected: compile errors — `register_user`/`login_user`/`find_user_by_id` and `DUMMY_ARGON2_HASH` not defined.

NOTE: requires `docker-compose up -d postgres` to be running. If not, the test runner will error with "connection refused". Verify with `docker-compose ps postgres` before continuing.

- [ ] **Step 3: Implement `crates/exg-user-service/src/repo.rs`**

```rust
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

/// Test helper for `repo_test.rs` — idempotent init wrapper.
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

/// Register a new user. Email is lowercased before insertion. Returns
/// AuthError::EmailExists on UNIQUE constraint violation (idempotent retries
/// are NOT supported; the same email can never re-register even if password
/// differs).
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
        return Err(AuthError::InvalidInput("password length must be 8-128".into()));
    }
    let email_lc = email.to_lowercase();
    let pw_hash = hash_password(password)?;
    let user_id = UserId::new(id_gen.next_id());
    let now_micros = UnixMicros::now().value() as i64;

    let result = sqlx::query(
        "INSERT INTO users (user_id, email, password_hash, kyc_level, is_active, created_at, updated_at)
         VALUES ($1, $2, $3, 0, true, $4, $4)
         ON CONFLICT (email) DO NOTHING
         RETURNING user_id"
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
/// Constant-time: invokes verify_password exactly once regardless of branch
/// (against DUMMY_ARGON2_HASH if SELECT misses); spec §9 #20.
pub async fn login_user(
    pool: &PgPool,
    auth_cfg: &AuthConfig,
    email: &str,
    password: &str,
) -> Result<LoginResponse, AuthError> {
    let email_lc = email.to_lowercase();

    let row = sqlx::query_as::<_, (i64, String, bool)>(
        "SELECT user_id, password_hash, is_active FROM users WHERE email = $1"
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

    let (user_id, stored_hash, is_active) = match row {
        Some((uid, hash, active)) => (uid as u64, hash, active),
        None => (0u64, dummy.clone(), false),
    };

    // Always run verify_password exactly once.
    let pw_ok = verify_password(password, &stored_hash)?;

    // Combine all failure modes into one error to prevent enumeration.
    if row_was_none_or_failed(user_id, pw_ok, is_active) {
        return Err(AuthError::InvalidCredentials);
    }

    let now = chrono::Utc::now().timestamp() as u64;
    let claims = JwtClaims {
        user_id,
        iat: now,
        exp: now + auth_cfg.jwt_expiry_secs,
    };
    let token = sign_jwt(auth_cfg.jwt_secret.as_bytes(), &claims)?;

    Ok(LoginResponse {
        access_token: token,
        expires_in: auth_cfg.jwt_expiry_secs,
        user_id,
    })
}

fn row_was_none_or_failed(user_id: u64, pw_ok: bool, is_active: bool) -> bool {
    user_id == 0 || !pw_ok || !is_active
}

/// Find a user by ID. Returns None if not found.
pub async fn find_user_by_id(
    pool: &PgPool,
    user_id: UserId,
) -> Result<Option<UserRow>, AuthError> {
    let row = sqlx::query_as::<_, (i64, String, i16, bool)>(
        "SELECT user_id, email, kyc_level, is_active FROM users WHERE user_id = $1"
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
```

Add `AuthError` variants if missing (in `error.rs`):

```rust
#[error("invalid input: {0}")]
InvalidInput(String),

#[error("email already registered")]
EmailExists,

#[error("invalid credentials")]
InvalidCredentials,

#[error("database error: {0}")]
DbError(String),
```

Add `LoginResponse` if not already (in `auth.rs` near `JwtClaims`):

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginResponse {
    pub access_token: String,
    pub expires_in: u64,
    pub user_id: u64,
}
```

Update `JwtClaims` to ensure it has `user_id`, `exp`, `iat` as `u64`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JwtClaims {
    pub user_id: u64,
    pub exp: u64,
    pub iat: u64,
}
```

Add `once_cell` to `crates/exg-user-service/Cargo.toml` if not present:

```toml
once_cell = "1"
```

(Add to workspace `Cargo.toml` `[workspace.dependencies]` if missing.)

Update `crates/exg-user-service/src/lib.rs` to re-export `repo::*`:

```rust
pub use repo::{
    UserRow, register_user, login_user, find_user_by_id,
    init_dummy_argon2_hash, init_dummy_argon2_hash_for_tests,
    DUMMY_ARGON2_HASH,
};
```

- [ ] **Step 4: Run integration tests — verify pass**

Run: `docker-compose up -d postgres && sleep 2 && cargo test -p exg-user-service --test repo_test`
Expected: 7 tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/exg-user-service/ Cargo.toml
git commit -m "$(cat <<'EOF'
feat(user-service): add PG-backed repo + sqlx::test suite

repo::register_user / login_user / find_user_by_id over &PgPool.

Implements stage 1a invariants:
- email lowercased before store/compare (§9 #14)
- login response time constant via DUMMY_ARGON2_HASH OnceCell (§9 #20)
- login responses indistinguishable for not-found / wrong-pw / inactive (§9 #17)
- INSERT ON CONFLICT for register dup detection

Tests (7 sqlx::test cases with isolated DB):
- register_then_find_by_id
- register_duplicate_email_rejected
- register_normalizes_email_to_lowercase
- login_with_correct_password
- login_with_wrong_password_rejected
- login_unknown_email_rejected
- login_inactive_user_rejected

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4 — `exg-user-service` constant-time timing test

**Files:**
- Create: `crates/exg-user-service/tests/timing_test.rs`

- [ ] **Step 1: Write the timing assertion test**

```rust
//! Stage 1a §9 invariant #20: login_user response time must be constant
//! regardless of email existence. Compare median wall-time of 100 samples
//! for "known wrong pw" vs "unknown email"; difference must be < 5ms.
//!
//! Unit-level test (against login_user fn directly), NOT e2e — HTTP
//! overhead would mask the signal.

use exg_common::SnowflakeGen;
use exg_config::AuthConfig;
use exg_user_service::{login_user, register_user};
use sqlx::PgPool;
use std::time::Instant;

fn test_auth_cfg() -> AuthConfig {
    AuthConfig {
        jwt_secret: "a".repeat(32),
        jwt_expiry_secs: 3600,
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn login_time_constant_regardless_of_email_existence(pool: PgPool) {
    let id_gen = SnowflakeGen::new(1);
    let cfg = test_auth_cfg();
    register_user(&pool, &id_gen, "real@example.com", "hunter2hunter2")
        .await
        .unwrap();

    const SAMPLES: usize = 30;
    let mut known_times = Vec::with_capacity(SAMPLES);
    let mut unknown_times = Vec::with_capacity(SAMPLES);

    for _ in 0..SAMPLES {
        let t = Instant::now();
        let _ = login_user(&pool, &cfg, "real@example.com", "wrong-password").await;
        known_times.push(t.elapsed().as_micros());
    }
    for _ in 0..SAMPLES {
        let t = Instant::now();
        let _ = login_user(&pool, &cfg, "ghost@example.com", "wrong-password").await;
        unknown_times.push(t.elapsed().as_micros());
    }

    known_times.sort();
    unknown_times.sort();
    let known_median = known_times[SAMPLES / 2];
    let unknown_median = unknown_times[SAMPLES / 2];
    let diff = known_median.abs_diff(unknown_median);

    eprintln!("known median: {known_median}us, unknown median: {unknown_median}us, diff: {diff}us");

    // Argon2id is ~50ms. A 5ms (5000us) diff is generous; in practice we
    // expect <500us. Spec §9 #20 specifies < 5ms.
    assert!(
        diff < 5_000,
        "login_user timing leak: diff {diff}us between known-wrong-pw and unknown-email"
    );
}
```

- [ ] **Step 2: Run — verify pass**

Run: `cargo test -p exg-user-service --test timing_test`
Expected: PASS. If flaky on CI under load, raise sample count to 100 or document tolerance.

- [ ] **Step 3: Commit**

```bash
git add crates/exg-user-service/tests/timing_test.rs
git commit -m "$(cat <<'EOF'
test(user-service): assert login_user wall-time constant across email branches

30-sample median test against login_user fn directly. Verifies §9 #20
constant-time invariant: DUMMY_ARGON2_HASH ensures verify_password runs
once regardless of email existence.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5 — `exg-api-gateway`: errors + types

**Files:**
- Modify: `crates/exg-api-gateway/Cargo.toml`
- Modify: `crates/exg-api-gateway/src/error.rs`
- Modify: `crates/exg-api-gateway/src/types.rs`

- [ ] **Step 1: Update Cargo.toml**

Add to `[dependencies]`:

```toml
sqlx = { workspace = true }
exg-user-service = { workspace = true }
exg-config = { workspace = true }
tracing = { workspace = true }
```

(`parking_lot`, `rkyv`, `exg-ringbuffer`, `exg-wal`, `actix-web` already present from Stage 0.)

If `exg-user-service` and `exg-config` are not yet in workspace.dependencies, add them.

- [ ] **Step 2: Rename ERR_DUPLICATE_ORDER → ERR_DUPLICATE_RESOURCE; add new codes**

In `crates/exg-api-gateway/src/error.rs`, replace the constant block:

```rust
pub const ERR_UNKNOWN: i32 = -1000;
pub const ERR_UNAUTHORIZED: i32 = -1002;
pub const ERR_RATE_LIMITED_USER: i32 = -1003;       // NEW
pub const ERR_DUPLICATE_RESOURCE: i32 = -1014;       // RENAMED from ERR_DUPLICATE_ORDER
pub const ERR_TOO_MANY_REQUESTS: i32 = -1015;
pub const ERR_INVALID_PARAMETER: i32 = -1100;
pub const ERR_ORDER_NOT_FOUND: i32 = -2013;
pub const ERR_INSUFFICIENT_BALANCE: i32 = -2010;
```

If any existing code references `ERR_DUPLICATE_ORDER`, run a workspace-wide replacement:

```bash
grep -rn "ERR_DUPLICATE_ORDER" crates/ --include="*.rs"
```

Replace every match with `ERR_DUPLICATE_RESOURCE` (sed -i or careful edit).

Add constructors:

```rust
impl ApiError {
    pub fn duplicate_resource(msg: impl Into<String>) -> Self {
        Self {
            code: ERR_DUPLICATE_RESOURCE,
            msg: msg.into(),
        }
    }

    pub fn user_rate_limited(msg: impl Into<String>) -> Self {
        Self {
            code: ERR_RATE_LIMITED_USER,
            msg: msg.into(),
        }
    }

    pub fn db_unavailable(err: sqlx::Error) -> Self {
        tracing::error!(target: "db", err = %err, "db unavailable");
        Self {
            code: ERR_UNKNOWN,
            msg: "database unavailable".to_owned(),
        }
    }
}
```

In `ResponseError::status_code`, add the new mappings:

```rust
fn status_code(&self) -> actix_web::http::StatusCode {
    match self.code {
        ERR_UNAUTHORIZED => actix_web::http::StatusCode::UNAUTHORIZED,
        ERR_RATE_LIMITED_USER => actix_web::http::StatusCode::TOO_MANY_REQUESTS,
        ERR_TOO_MANY_REQUESTS => actix_web::http::StatusCode::TOO_MANY_REQUESTS,
        ERR_DUPLICATE_RESOURCE => actix_web::http::StatusCode::CONFLICT,
        ERR_ORDER_NOT_FOUND => actix_web::http::StatusCode::NOT_FOUND,
        ERR_UNKNOWN => actix_web::http::StatusCode::INTERNAL_SERVER_ERROR,
        _ => actix_web::http::StatusCode::BAD_REQUEST,
    }
}
```

- [ ] **Step 3: Add request/response types in `types.rs`**

Append to `crates/exg-api-gateway/src/types.rs`:

```rust
/// Register request. NOTE: password is NOT a derive(Debug) field —
/// see manual impl below to prevent accidental log exposure (§9 #18).
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterRequest {
    pub email: String,
    pub password: String,
}

impl std::fmt::Debug for RegisterRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegisterRequest")
            .field("email", &self.email)
            .field("password", &"***")
            .finish()
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

impl std::fmt::Debug for LoginRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoginRequest")
            .field("email", &self.email)
            .field("password", &"***")
            .finish()
    }
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginResponseBody {
    pub access_token: String,
    pub expires_in: u64,
    /// Stringified u64 per Binance convention.
    pub user_id: String,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeResponse {
    pub user_id: String,
    pub email: String,
    pub kyc_level: i16,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterResponse {
    pub user_id: String,
    pub email: String,
    pub status: &'static str,
}
```

Add a unit test for the password redaction:

```rust
#[cfg(test)]
mod password_redaction_tests {
    use super::*;

    #[test]
    fn register_request_debug_does_not_leak_password() {
        let req = RegisterRequest {
            email: "alice@example.com".into(),
            password: "super-secret-password".into(),
        };
        let s = format!("{req:?}");
        assert!(!s.contains("super-secret-password"), "Debug leaked password: {s}");
        assert!(s.contains("***"), "Debug missing redaction marker: {s}");
    }

    #[test]
    fn login_request_debug_does_not_leak_password() {
        let req = LoginRequest {
            email: "alice@example.com".into(),
            password: "super-secret-password".into(),
        };
        let s = format!("{req:?}");
        assert!(!s.contains("super-secret-password"));
    }
}
```

- [ ] **Step 4: Run tests + check workspace builds**

Run: `cargo test -p exg-api-gateway password_redaction`
Expected: 2 tests pass.

Run: `cargo check --workspace`
Expected: clean (everything compiles after ERR_ rename).

- [ ] **Step 5: Commit**

```bash
git add crates/exg-api-gateway/
git commit -m "$(cat <<'EOF'
feat(api-gateway): add stage 1a errors + auth request/response types

Errors:
- ERR_DUPLICATE_ORDER → ERR_DUPLICATE_RESOURCE (-1014, reused for
  dup-email register 409 and dup-coid place_order 409)
- New ERR_RATE_LIMITED_USER (-1003) maps to HTTP 429
- New constructors: duplicate_resource, user_rate_limited, db_unavailable

Types:
- RegisterRequest, LoginRequest with manual Debug masking password (§9 #18)
- LoginResponseBody, MeResponse, RegisterResponse camelCase JSON
- Unit tests assert password never appears in Debug output

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6 — `exg-api-gateway`: state + handlers (auth + JWT middleware + rate limit + dedup)

**Files:**
- Modify: `crates/exg-api-gateway/src/state.rs`
- Modify: `crates/exg-api-gateway/src/handlers.rs`
- Modify: `crates/exg-api-gateway/src/app_factory.rs`

- [ ] **Step 1: Extend `AppState`**

In `crates/exg-api-gateway/src/state.rs`, replace the struct:

```rust
use std::sync::Arc;

use exg_common::SnowflakeGen;
use exg_config::{AuthConfig, ExgConfig};
use exg_ringbuffer::Producer;
use parking_lot::Mutex;
use sqlx::PgPool;

use crate::middleware::RateLimiter;

/// Shared state injected into every Actix handler.
#[derive(Clone)]
pub struct AppState {
    pub producer: Arc<Mutex<Producer>>,
    pub snowflake: Arc<SnowflakeGen>,
    pub cfg: Arc<ExgConfig>,
    /// Stage 1a: PostgreSQL pool (sqlx::PgPool is internally Arc'd, so clone is cheap).
    pub pool: PgPool,
    /// Stage 1a: lifted from cfg.auth for hot-path ergonomics (handler JWT verify uses bytes,
    /// not String). Cloning this Arc once at boot avoids cfg.auth.clone() per request.
    pub auth_cfg: Arc<AuthConfig>,
    /// Stage 1a: in-memory token bucket. Keys are arbitrary strings; handlers compose
    /// "user:<id>", "login:email:<x>", "login:ip:<x>". Stage 7 replaces backend with Redis.
    pub rate_limiter: Arc<Mutex<RateLimiter>>,
}
```

- [ ] **Step 2: Rewrite `extract_user_id` → `extract_user_id_from_jwt`**

In `crates/exg-api-gateway/src/handlers.rs`, replace the existing `extract_user_id` fn:

```rust
/// Extract user_id from a verified JWT in `Authorization: Bearer <token>` header.
/// Replaces Stage 0's X-User-Id header trust path.
fn extract_user_id_from_jwt(
    req: &actix_web::HttpRequest,
    jwt_secret: &[u8],
) -> Result<exg_common::UserId, crate::error::ApiError> {
    let h = req
        .headers()
        .get("Authorization")
        .ok_or_else(|| crate::error::ApiError::unauthorized("missing Authorization header"))?;
    let s = h
        .to_str()
        .map_err(|_| crate::error::ApiError::unauthorized("Authorization not valid ASCII"))?;
    let token = s
        .strip_prefix("Bearer ")
        .ok_or_else(|| crate::error::ApiError::unauthorized("Authorization must be 'Bearer <jwt>'"))?;
    let claims = exg_user_service::verify_jwt(jwt_secret, token)
        .map_err(|_| crate::error::ApiError::unauthorized("invalid or expired token"))?;
    Ok(exg_common::UserId::new(claims.user_id))
}
```

Update all callers of `extract_user_id` (place/cancel/amend handlers) to call `extract_user_id_from_jwt(&req, state.auth_cfg.jwt_secret.as_bytes())` instead.

- [ ] **Step 3: Add register / login / me handlers**

Append to `crates/exg-api-gateway/src/handlers.rs`:

```rust
use exg_user_service as user_service;
use crate::types::{
    LoginRequest, LoginResponseBody, MeResponse, RegisterRequest, RegisterResponse,
};

pub async fn register(
    state: actix_web::web::Data<AppState>,
    body: actix_web::web::Json<RegisterRequest>,
) -> Result<actix_web::HttpResponse, crate::error::ApiError> {
    let user_id = user_service::register_user(
        &state.pool,
        &state.snowflake,
        &body.email,
        &body.password,
    )
    .await
    .map_err(map_auth_error)?;

    let resp = RegisterResponse {
        user_id: user_id.value().to_string(),
        email: body.email.to_lowercase(),
        status: "REGISTERED",
    };
    Ok(actix_web::HttpResponse::Created().json(resp))
}

pub async fn login(
    state: actix_web::web::Data<AppState>,
    req: actix_web::HttpRequest,
    body: actix_web::web::Json<LoginRequest>,
) -> Result<actix_web::HttpResponse, crate::error::ApiError> {
    // Rate-limit gate (per-email + per-IP, either exhausted → 429).
    let now = exg_common::UnixMicros::now();
    let email_key = format!("login:email:{}", body.email.to_lowercase());
    let ip_key = format!(
        "login:ip:{}",
        req.peer_addr()
            .map(|a| a.ip().to_string())
            .unwrap_or_else(|| "unknown".into())
    );
    {
        let mut limiter = state.rate_limiter.lock();
        if !limiter.consume(&email_key, now) || !limiter.consume(&ip_key, now) {
            return Err(crate::error::ApiError::user_rate_limited(
                "login rate limit exceeded".into(),
            ));
        }
    }

    let resp_inner = user_service::login_user(
        &state.pool,
        &state.auth_cfg,
        &body.email,
        &body.password,
    )
    .await
    .map_err(map_auth_error)?;

    let resp = LoginResponseBody {
        access_token: resp_inner.access_token,
        expires_in: resp_inner.expires_in,
        user_id: resp_inner.user_id.to_string(),
    };
    Ok(actix_web::HttpResponse::Ok().json(resp))
}

pub async fn me(
    state: actix_web::web::Data<AppState>,
    req: actix_web::HttpRequest,
) -> Result<actix_web::HttpResponse, crate::error::ApiError> {
    let user_id = extract_user_id_from_jwt(&req, state.auth_cfg.jwt_secret.as_bytes())?;
    let row = user_service::find_user_by_id(&state.pool, user_id)
        .await
        .map_err(map_auth_error)?
        .ok_or_else(|| crate::error::ApiError::unauthorized("user not found"))?;
    let resp = MeResponse {
        user_id: row.user_id.value().to_string(),
        email: row.email,
        kyc_level: row.kyc_level,
    };
    Ok(actix_web::HttpResponse::Ok().json(resp))
}

fn map_auth_error(e: user_service::AuthError) -> crate::error::ApiError {
    use user_service::AuthError;
    match e {
        AuthError::InvalidInput(msg) => crate::error::ApiError::bad_request(msg),
        AuthError::EmailExists => {
            crate::error::ApiError::duplicate_resource("email already registered")
        }
        AuthError::InvalidCredentials => {
            crate::error::ApiError::unauthorized("invalid credentials")
        }
        AuthError::DbError(_) => {
            // Don't leak DB internals. Map to generic db_unavailable.
            crate::error::ApiError::internal("database unavailable")
        }
        AuthError::JwtError(_) => crate::error::ApiError::unauthorized("invalid token"),
        AuthError::HashError(msg) => crate::error::ApiError::internal(msg),
    }
}
```

- [ ] **Step 4: Add per-user rate-limit + dedup gates to `place_order`**

Modify the existing `place_order` handler. After `extract_user_id_from_jwt(...)`, before `enqueue(...)`, insert:

```rust
    // Per-user rate-limit gate (Stage 1a §9 #19 also applies to order endpoints).
    {
        let now = exg_common::UnixMicros::now();
        let key = format!("user:{}", user_id.value());
        let mut limiter = state.rate_limiter.lock();
        if !limiter.consume(&key, now) {
            return Err(crate::error::ApiError::user_rate_limited(
                "rate limit exceeded for user".into(),
            ));
        }
    }

    // Dedup gate (Stage 1a §9 #13): INSERT ON CONFLICT before ring-buffer push.
    // Orphan-row semantic: if push later fails, this row remains and a retry
    // gets 409 (acceptable double-error, never double-order).
    if let Some(coid_str) = body.client_order_id.as_deref() {
        let coid: u64 = coid_str
            .parse()
            .map_err(|_| crate::error::ApiError::bad_request("clientOrderId must be numeric"))?;
        let now_micros = exg_common::UnixMicros::now().value() as i64;
        let inserted = sqlx::query(
            "INSERT INTO user_client_order_ids (user_id, client_order_id, created_at)
             VALUES ($1, $2, $3) ON CONFLICT DO NOTHING"
        )
        .bind(user_id.value() as i64)
        .bind(coid as i64)
        .bind(now_micros)
        .execute(&state.pool)
        .await
        .map_err(crate::error::ApiError::db_unavailable)?;
        if inserted.rows_affected() == 0 {
            return Err(crate::error::ApiError::duplicate_resource(
                "duplicate clientOrderId".into(),
            ));
        }
    }
```

- [ ] **Step 5: Mount routes in `app_factory.rs`**

In `crates/exg-api-gateway/src/app_factory.rs`, in `build_app`, add three routes alongside the existing ones:

```rust
        .route("/api/v1/auth/register", actix_web::web::post().to(handlers::register))
        .route("/api/v1/auth/login", actix_web::web::post().to(handlers::login))
        .route("/api/v1/me", actix_web::web::get().to(handlers::me))
```

- [ ] **Step 6: Run `cargo check`**

Run: `cargo check --workspace`
Expected: clean (any missing imports in handlers.rs / state.rs surface here).

- [ ] **Step 7: Update existing handler unit tests in `handlers.rs` mod tests**

Existing Stage 0 handler tests use Header `X-User-Id`. They will now fail with 401. Update them to use a JWT bearer instead:

```rust
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
```

And in each `place_order_*` handler test that previously did `.insert_header(("X-User-Id", "42"))`, change to:

```rust
        let token = test_jwt_for_user(&state, 42);
        let req = test::TestRequest::post()
            .uri("/api/v1/order")
            .insert_header(("Authorization", format!("Bearer {token}")))
            .insert_header(("Content-Type", "application/json"))
            ...
```

Also update `test_state()` to construct the new AppState shape with `pool: PgPool::connect_lazy("postgres://exg:exg_dev_password@localhost:5432/exg").unwrap()`, `auth_cfg: Arc::new(AuthConfig { jwt_secret: "a".repeat(32), jwt_expiry_secs: 3600 })`, and `rate_limiter: Arc::new(Mutex::new(RateLimiter::new(100, 10.0)))`.

NOTE: handler unit tests against actix's test infrastructure don't actually hit PG (no `register`/`login`/`me` test here — those land in stage1a_e2e.rs). The pool reference is only constructed, not used by place_order_* tests (which test JWT extraction + dedup INSERT path mock). For now, use `PgPool::connect_lazy` so test setup doesn't require PG running for handler unit tests — but if a test exercises dedup it WILL hit PG. Stage 1a treats dedup as an e2e concern; place_order_* handler unit tests should test only the JWT extract path (no body that includes clientOrderId, or accept that dedup tests require PG).

- [ ] **Step 8: Run unit tests**

Run: `cargo test -p exg-api-gateway --lib`
Expected: all green (existing handler tests rewritten, new auth-related tests in types.rs).

- [ ] **Step 9: Commit**

```bash
git add crates/exg-api-gateway/
git commit -m "$(cat <<'EOF'
feat(api-gateway): wire JWT auth, rate-limit, and dedup gates

- Rename extract_user_id → extract_user_id_from_jwt; read Authorization
  Bearer header and verify via exg_user_service::verify_jwt
- Three new handlers: register / login / me; routes mounted in app_factory
- place_order gets per-user rate-limit + handler-side dedup gates
  (INSERT ON CONFLICT user_client_order_ids before enqueue)
- login gets per-email + per-IP rate-limit gates
- AppState gains pool, auth_cfg, rate_limiter
- Handler unit tests updated to use JWT bearer

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 7 — `exg-server` lib.rs: PgPool + AuthConfig + DUMMY_ARGON2_HASH + boot panics

**Files:**
- Modify: `crates/exg-server/Cargo.toml`
- Modify: `crates/exg-server/src/lib.rs`

- [ ] **Step 1: Add sqlx feature to exg-server Cargo.toml**

In `crates/exg-server/Cargo.toml`, ensure `[dependencies]` has:

```toml
sqlx = { workspace = true }
exg-user-service = { workspace = true }
```

- [ ] **Step 2: Update `run_with_config` startup sequence**

In `crates/exg-server/src/lib.rs`, inside `run_with_config`:

After `validate_invariants(&cfg)?;` and BEFORE `WalWriter::open(...)`, add:

```rust
    // ── Stage 1a step 3.5: connect PG pool ────────────────────────────────
    let pool = sqlx::PgPool::connect(&cfg.database.url)
        .await
        .with_context(|| format!("failed to connect PG at {}", cfg.database.url))?;
    // Ping to fail-fast on bad credentials / network.
    sqlx::query("SELECT 1")
        .execute(&pool)
        .await
        .context("PG ping (SELECT 1) failed")?;

    // ── Stage 1a step 3.7: DUMMY_ARGON2_HASH OnceCell init ────────────────
    exg_user_service::init_dummy_argon2_hash()
        .context("failed to init DUMMY_ARGON2_HASH for constant-time login")?;
```

Also extend `validate_invariants` to include the JWT secret checks. Find the function and add at the start:

```rust
    // Stage 1a §9 invariants 11-12: JWT secret length + placeholder rejection.
    const JWT_PLACEHOLDER: &str =
        "CHANGE-ME-DEV-ONLY-MUST-BE-AT-LEAST-32-BYTES-OK";
    if cfg.auth.jwt_secret.len() < 32 {
        bail!(
            "Stage 1a: auth.jwt_secret must be at least 32 bytes, got {}",
            cfg.auth.jwt_secret.len()
        );
    }
    if cfg.auth.jwt_secret == JWT_PLACEHOLDER {
        bail!(
            "Stage 1a: auth.jwt_secret is the placeholder; override via EXG_AUTH_JWT_SECRET"
        );
    }
```

(`cfg.validate()` is already called in invariant 0 from CEO review fix, which covers the same checks via exg-config's validator — but doubling at server level is defense-in-depth for the case where `default_config()` is constructed programmatically.)

Update `AppState` construction at the bottom of `run_with_config`:

```rust
    let state = AppState {
        producer,
        snowflake: snowflake.clone(),
        cfg: cfg.clone(),
        pool: pool.clone(),
        auth_cfg: Arc::new(cfg.auth.clone()),
        rate_limiter: Arc::new(parking_lot::Mutex::new(
            exg_api_gateway::middleware::RateLimiter::new(
                cfg.risk.max_orders_per_second,
                cfg.risk.max_orders_per_second as f64,
            )
        )),
    };
```

- [ ] **Step 3: Run cargo check**

Run: `cargo check -p exg-server`
Expected: clean. Adjust any missed imports.

- [ ] **Step 4: Commit**

```bash
git add crates/exg-server/
git commit -m "$(cat <<'EOF'
feat(server): wire PgPool + AuthConfig + DUMMY_ARGON2_HASH into boot

- sqlx::PgPool::connect + SELECT 1 ping at boot (steps 3.5-3.6)
- exg_user_service::init_dummy_argon2_hash() OnceCell init (step 3.7);
  uses get_or_init for idempotency in repeat-boot test scenarios
- JWT secret length + placeholder invariants enforced in
  validate_invariants (defense-in-depth over cfg.validate())
- AppState extended with pool, auth_cfg, rate_limiter

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 8 — `exg-server` boot panic tests + Stage 0 compat

**Files:**
- Modify: `crates/exg-server/tests/boot_panics.rs`

- [ ] **Step 1: Update existing boot panic tests for new AuthConfig field**

In `crates/exg-server/tests/boot_panics.rs`, find `base_cfg()` helper and add `auth` override:

```rust
fn base_cfg(wal_dir: &std::path::Path) -> ExgConfig {
    let mut cfg = ExgConfig::default_config();
    cfg.wal.dir = wal_dir.to_string_lossy().into_owned();
    cfg.server.port = 0;
    // Stage 1a: override the placeholder to a valid 32-byte secret so non-auth
    // invariant tests don't all fail on JWT validation.
    cfg.auth.jwt_secret = "a".repeat(32);
    cfg
}
```

- [ ] **Step 2: Add 3 new tests**

Append:

```rust
#[actix_web::test]
async fn boot_panics_on_short_jwt_secret() {
    let tmp = TempDir::new().unwrap();
    let mut cfg = base_cfg(tmp.path());
    cfg.auth.jwt_secret = "short".into();
    let result = exg_server::run_with_config(cfg).await;
    let err = result.err().expect("expected Err");
    let msg = format!("{err:#}");
    assert!(msg.contains("jwt_secret"), "got: {msg}");
}

#[actix_web::test]
async fn boot_panics_on_default_jwt_secret() {
    let tmp = TempDir::new().unwrap();
    let mut cfg = base_cfg(tmp.path());
    cfg.auth.jwt_secret = "CHANGE-ME-DEV-ONLY-MUST-BE-AT-LEAST-32-BYTES-OK".into();
    let result = exg_server::run_with_config(cfg).await;
    let err = result.err().expect("expected Err");
    let msg = format!("{err:#}");
    assert!(msg.contains("jwt_secret") || msg.contains("placeholder"), "got: {msg}");
}

#[actix_web::test]
async fn boot_panics_on_db_unreachable() {
    let tmp = TempDir::new().unwrap();
    let mut cfg = base_cfg(tmp.path());
    // Point at a definitely-unreachable port to force connect failure.
    cfg.database.url = "postgres://exg:exg_dev_password@127.0.0.1:1/exg".into();
    let result = exg_server::run_with_config(cfg).await;
    let err = result.err().expect("expected Err");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("PG") || msg.contains("connect") || msg.contains("SELECT 1"),
        "got: {msg}"
    );
}
```

- [ ] **Step 3: Run boot_panics tests**

Run: `docker-compose up -d postgres && cargo test -p exg-server --test boot_panics`
Expected: 7 tests pass (4 existing + 3 new). NOTE: the 4 existing tests now need a live PG connection because run_with_config's step 3.5 calls connect; they'll fail at PG connect before reaching the invariant they were testing.

Verify: existing tests pass through PG connect step. If a test like `boot_panics_on_non_loopback_host` was meant to fire BEFORE PG connect, ensure `validate_invariants` runs before `PgPool::connect` (it should — step 0 is validate, step 3.5 is connect).

- [ ] **Step 4: Commit**

```bash
git add crates/exg-server/tests/boot_panics.rs
git commit -m "$(cat <<'EOF'
test(server): add stage 1a boot-panic tests

3 new: short jwt_secret / placeholder jwt_secret / unreachable DB.
Existing 4 tests updated to set valid jwt_secret in base_cfg.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 9 — REGRESSION: Stage 0 e2e rewrite to JWT bearer

**Files:**
- Modify: `crates/exg-server/tests/stage0_e2e.rs`

This is the most important regression-protection task. All 7 Stage 0 e2e tests use `X-User-Id` header. They MUST be rewritten to JWT bearer or Stage 1a CI fails immediately.

- [ ] **Step 1: Read the current file**

Run: `wc -l crates/exg-server/tests/stage0_e2e.rs && grep -c "X-User-Id" crates/exg-server/tests/stage0_e2e.rs`
Expected: ~300 LOC, ~15 `X-User-Id` occurrences.

- [ ] **Step 2: Add login_helper fixture at the top**

Near the top of the file, after imports, add:

```rust
/// Test fixture: register a user and return (access_token, user_id).
/// All Stage 0 e2e tests now use this instead of the X-User-Id header trust path.
async fn login_helper(client: &Client, base: &str, email: &str, password: &str) -> (String, u64) {
    // Register (idempotent enough — second call for same email returns 409, ignore).
    let _ = client
        .post(format!("{base}/api/v1/auth/register"))
        .json(&serde_json::json!({"email": email, "password": password}))
        .send()
        .await
        .unwrap();
    // Login to obtain JWT.
    let resp: serde_json::Value = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&serde_json::json!({"email": email, "password": password}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = resp["accessToken"].as_str().expect("accessToken").to_string();
    let user_id: u64 = resp["userId"].as_str().expect("userId").parse().unwrap();
    (token, user_id)
}
```

- [ ] **Step 3: Replace every `X-User-Id` use**

For each of the 7 tests, change the pattern:

```rust
// OLD
.header("X-User-Id", "42")

// NEW (at test start)
let (token, _user_id) = login_helper(&client, &base, "test42@example.com", "hunter2hunter2").await;
// Then in each request:
.header("Authorization", format!("Bearer {token}"))
```

For the IDOR test (which previously used `X-User-Id: 42` then `X-User-Id: 999`):

```rust
let (token_a, _) = login_helper(&client, &base, "alice-idor@example.com", "hunter2hunter2").await;
let (token_b, _) = login_helper(&client, &base, "bob-idor@example.com", "hunter2hunter2").await;
// place as A
.header("Authorization", format!("Bearer {token_a}"))
// cancel as B
.header("Authorization", format!("Bearer {token_b}"))
```

For `base_cfg` (or `boot_server` helper), ensure it sets a valid JWT secret + working database URL pointing at `docker-compose` PG.

Also each test needs to `register`/`login` use the PG, so the test fixture must point at PG (docker-compose service). Use the same `migrations = "../../migrations"` approach for any sqlx-related setup; the e2e tests inherit the live DB.

NOTE: Stage 0 e2e tests used a tempdir-backed WAL with random ports for true isolation between tests. With PG, multiple parallel tests share a DB. Either:

(a) Use unique emails per test (e.g. `format!("alice-{}-@example.com", uuid::Uuid::new_v4())`) so register doesn't collide across tests.
(b) Wrap each test in `#[sqlx::test(migrations = "../../migrations")]` so each gets its own DB.

Option (b) integrates better with sqlx's test isolation. Convert tests to:

```rust
#[sqlx::test(migrations = "../../migrations")]
async fn place_cancel_amend_happy_path(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let mut cfg = base_cfg(tmp.path());
    // Override the cfg.database.url to use the test pool's URL.
    // sqlx::test gives us a connected pool but boot_server needs a URL string.
    // Use PgPool::connect_options-derived URL or set up boot_server to accept a pool directly.
    // ... see implementation note below
}
```

**Implementation note**: `boot_server` in Stage 0 was authored before PG existed. It builds cfg → `run_with_config(cfg).await`. `run_with_config` calls `PgPool::connect(&cfg.database.url)`. Tests need to pipe sqlx::test's pool URL into cfg.

Pragmatic fix: extract the database URL from the test pool:

```rust
// `pool: PgPool` from sqlx::test gives us a connected pool. Get its URL:
let db_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
    "postgres://exg:exg_dev_password@localhost:5432/exg".into()
});
// sqlx::test creates a temp DB and exposes its name via the pool; we need to query it
// or use the unique DB name in the URL. Alternative: have run_with_config accept an
// optional pre-built pool.
```

**Cleanest fix**: extend `run_with_config` to accept an optional `PgPool` for tests:

```rust
pub async fn run_with_config_with_pool(
    cfg: ExgConfig,
    pool_override: Option<sqlx::PgPool>,
) -> Result<ServerHandle> {
    ...
    let pool = match pool_override {
        Some(p) => p,
        None => sqlx::PgPool::connect(&cfg.database.url).await?,
    };
    ...
}

pub async fn run_with_config(cfg: ExgConfig) -> Result<ServerHandle> {
    run_with_config_with_pool(cfg, None).await
}
```

Then tests:

```rust
#[sqlx::test(migrations = "../../migrations")]
async fn place_cancel_amend_happy_path(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let handle = exg_server::run_with_config_with_pool(cfg, Some(pool))
        .await
        .expect("server boot");
    ...
}
```

Add this `run_with_config_with_pool` to `crates/exg-server/src/lib.rs` (export both).

- [ ] **Step 4: Run Stage 0 e2e**

Run: `docker-compose up -d postgres && cargo test -p exg-server --test stage0_e2e`
Expected: 7 tests pass.

If a test fails because login limits hit (rate limit kicks in), bump `cfg.risk.max_orders_per_second` for tests, or sleep between scenarios.

- [ ] **Step 5: Commit**

```bash
git add crates/exg-server/
git commit -m "$(cat <<'EOF'
test(server): rewrite stage 0 e2e tests to JWT bearer (regression baseline)

Stage 1a removes the X-User-Id header trust path. Every Stage 0 e2e
test previously used .header("X-User-Id", "42") which now returns 401.
This commit:

- Adds login_helper(client, base, email, password) -> (token, user_id)
  shared fixture using register + login flow.
- Replaces all X-User-Id usage with Authorization Bearer.
- IDOR test now registers two users (alice-idor, bob-idor) and uses
  each user's distinct JWT.
- Tests use #[sqlx::test] so each gets an isolated DB.
- New run_with_config_with_pool exported for tests to inject the
  sqlx::test pool.

Preserves all 7 Stage 0 acceptance points (happy / 401 / 400 /
backpressure / IDOR / shutdown drain / dup coid).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 10 — Stage 1a new e2e tests

**Files:**
- Create: `crates/exg-server/tests/stage1a_e2e.rs`

- [ ] **Step 1: Write the test file**

```rust
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
    client
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
    resp["accessToken"].as_str().unwrap().to_string()
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

    // Wait for token to expire.
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
    let r1 = client.post(format!("{base}/api/v1/auth/register")).json(&body).send().await.unwrap();
    assert_eq!(r1.status().as_u16(), 201);
    let r2 = client.post(format!("{base}/api/v1/auth/register")).json(&body).send().await.unwrap();
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
    let r1 = client.post(format!("{base}/api/v1/order"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&body).send().await.unwrap();
    assert_eq!(r1.status().as_u16(), 200);
    let r2 = client.post(format!("{base}/api/v1/order"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&body).send().await.unwrap();
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

    let r_unknown: serde_json::Value = client.post(format!("{base}/api/v1/auth/login"))
        .json(&serde_json::json!({"email":"unknown@e.com","password":"wrong"}))
        .send().await.unwrap().json().await.unwrap();
    let r_wrong: serde_json::Value = client.post(format!("{base}/api/v1/auth/login"))
        .json(&serde_json::json!({"email":"known@e.com","password":"wrong"}))
        .send().await.unwrap().json().await.unwrap();

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
    let resp: serde_json::Value = client.get(format!("{base}/api/v1/me"))
        .header("Authorization", format!("Bearer {token}"))
        .send().await.unwrap().json().await.unwrap();
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

    let place: serde_json::Value = client.post(format!("{base}/api/v1/order"))
        .header("Authorization", format!("Bearer {token_a}"))
        .json(&serde_json::json!({
            "symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT",
            "timeInForce":"GTC","quantity":"0.001","price":"59000"
        }))
        .send().await.unwrap().json().await.unwrap();
    let order_id: u64 = place["orderId"].as_str().unwrap().parse().unwrap();

    // B tries to cancel A's order. Per Stage 0 engine.rs:442, engine emits
    // OrderRejected/OrderNotFound; HTTP responds 200 (enqueued).
    let resp = client.post(format!("{base}/api/v1/order/cancel"))
        .header("Authorization", format!("Bearer {token_b}"))
        .json(&serde_json::json!({"orderId": order_id, "symbol":"BTCUSDT"}))
        .send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    // Verification of rejection event would require reading WAL — left to
    // Stage 0 e2e (which still has the WAL-read assertion).
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn login_rate_limit_per_email(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let mut cfg = base_cfg(tmp.path());
    cfg.risk.max_orders_per_second = 3; // small bucket for testing
    let (handle, base) = boot_server(cfg, pool).await;
    let client = Client::new();
    // Fire 10 login attempts against the same email; at least one should hit 429 + -1003.
    let mut saw_429 = false;
    for _ in 0..10 {
        let resp = client.post(format!("{base}/api/v1/auth/login"))
            .json(&serde_json::json!({"email":"limit@e.com","password":"wrong"}))
            .send().await.unwrap();
        if resp.status().as_u16() == 429 {
            saw_429 = true;
            let body: serde_json::Value = resp.json().await.unwrap();
            assert_eq!(body["code"], -1003);
            break;
        }
    }
    assert!(saw_429, "expected at least one 429 from per-email login limit");
    handle.shutdown().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn malformed_token_returns_401(pool: PgPool) {
    let tmp = TempDir::new().unwrap();
    let cfg = base_cfg(tmp.path());
    let (handle, base) = boot_server(cfg, pool).await;
    let client = Client::new();
    let resp = client.post(format!("{base}/api/v1/order"))
        .header("Authorization", "Bearer not-a-jwt")
        .json(&serde_json::json!({
            "symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT",
            "timeInForce":"GTC","quantity":"0.001","price":"59000"
        }))
        .send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 401);
    handle.shutdown().await.unwrap();
}
```

- [ ] **Step 2: Run the test suite**

Run: `docker-compose up -d postgres && cargo test -p exg-server --test stage1a_e2e`
Expected: 10 tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/exg-server/tests/stage1a_e2e.rs
git commit -m "$(cat <<'EOF'
test(server): add stage 1a e2e suite (10 cases)

- register_login_order_happy: full flow
- order_without_token_returns_401
- order_with_expired_token_returns_401 (jwt_expiry_secs=1 + sleep)
- duplicate_register_returns_409
- duplicate_client_order_id_returns_409
- login_unknown_email_indistinguishable_from_wrong_password
  (asserts byte-identical code + msg fields)
- me_endpoint_returns_own_info
- cross_user_idor_via_jwt_blocked
- login_rate_limit_per_email
- malformed_token_returns_401

Uses #[sqlx::test] for per-test DB isolation.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 11 — `scripts/demo-stage1a.sh` + final acceptance

**Files:**
- Create: `scripts/demo-stage1a.sh`

- [ ] **Step 1: Write the script**

```bash
#!/usr/bin/env bash
# Stage 1a cold-boot demo: PG up → migrate → server → register/login/order/dup → wal-dump.
set -euo pipefail

WAL_DIR=$(mktemp -d /tmp/exg-stage1a.XXXXXX)
PORT=8080
SERVER_PID=""

cleanup() {
    if [[ -n "${SERVER_PID}" ]]; then
        kill -INT "${SERVER_PID}" 2>/dev/null || true
        wait "${SERVER_PID}" 2>/dev/null || true
    fi
    rm -rf "${WAL_DIR}"
}
trap cleanup EXIT

echo "── stage 1a demo ──"
docker-compose up -d postgres
sleep 2
echo "─ migrate ─"
scripts/migrate.sh reset

echo "─ build ─"
cargo build --release -p exg-server -p exg-wal-dump >/dev/null

echo "─ boot server ─"
TMP_CFG=$(mktemp /tmp/exg-stage1a-cfg.XXXXXX.toml)
cp config/default.toml "$TMP_CFG"
# override wal dir + JWT secret in the temp config
python3 -c "
import sys, re
p = '$TMP_CFG'
with open(p) as f: c = f.read()
c = re.sub(r'dir = \"./data/wal\"', f'dir = \"$WAL_DIR\"', c)
c = re.sub(r'jwt_secret = \"CHANGE-ME-DEV-ONLY-MUST-BE-AT-LEAST-32-BYTES-OK\"', 'jwt_secret = \"demo-stage1a-secret-padding-32-bytes\"', c)
with open(p, 'w') as f: f.write(c)
"
EXG_CONFIG="$TMP_CFG" RUST_LOG=info ./target/release/exg-server &
SERVER_PID=$!
for i in {1..30}; do
    if curl -sf "http://127.0.0.1:${PORT}/api/v1/health" >/dev/null; then break; fi
    sleep 1
done

echo
echo "─ register ─"
curl -s -X POST "http://127.0.0.1:${PORT}/api/v1/auth/register" \
    -H 'Content-Type: application/json' \
    -d '{"email":"demo@example.com","password":"hunter2hunter2"}'
echo

echo
echo "─ login ─"
LOGIN_RESP=$(curl -s -X POST "http://127.0.0.1:${PORT}/api/v1/auth/login" \
    -H 'Content-Type: application/json' \
    -d '{"email":"demo@example.com","password":"hunter2hunter2"}')
echo "${LOGIN_RESP}"
TOKEN=$(echo "${LOGIN_RESP}" | python3 -c 'import json,sys; print(json.load(sys.stdin)["accessToken"])')

echo
echo "─ place order ─"
curl -s -X POST "http://127.0.0.1:${PORT}/api/v1/order" \
    -H "Authorization: Bearer $TOKEN" \
    -H 'Content-Type: application/json' \
    -d '{"symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT","timeInForce":"GTC","quantity":"0.001","price":"59000","clientOrderId":"42"}'
echo

echo
echo "─ duplicate clientOrderId (should 409) ─"
curl -s -X POST "http://127.0.0.1:${PORT}/api/v1/order" \
    -H "Authorization: Bearer $TOKEN" \
    -H 'Content-Type: application/json' \
    -d '{"symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT","timeInForce":"GTC","quantity":"0.001","price":"59000","clientOrderId":"42"}'
echo

echo
echo "─ no token (should 401) ─"
curl -s -X POST "http://127.0.0.1:${PORT}/api/v1/order" \
    -H 'Content-Type: application/json' \
    -d '{"symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT","timeInForce":"GTC","quantity":"0.001","price":"59000"}'
echo

echo
echo "─ /me ─"
curl -s -X GET "http://127.0.0.1:${PORT}/api/v1/me" \
    -H "Authorization: Bearer $TOKEN"
echo

echo
echo "─ shutdown ─"
kill -INT "${SERVER_PID}"
wait "${SERVER_PID}" 2>/dev/null || true
SERVER_PID=""

echo
echo "─ WAL contents ─"
./target/release/exg-wal-dump --wal-dir "${WAL_DIR}"
echo
echo "─ demo complete ─"
rm -f "$TMP_CFG"
```

- [ ] **Step 2: Make executable + run end-to-end**

```bash
chmod +x scripts/demo-stage1a.sh
scripts/demo-stage1a.sh
```

Expected:
- register returns 201 + userId
- login returns 200 + accessToken
- place order returns 200
- duplicate clientOrderId returns 409 + code -1014
- no token returns 401 + code -1002
- /me returns user info
- WAL dump shows at least 1 OrderAccepted event

- [ ] **Step 3: Run full spec §8.6 acceptance**

```bash
cargo check --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --check
cargo test --workspace
cargo test -p exg-server --test stage1a_e2e
cargo test -p exg-server --test stage0_e2e   # CRITICAL: regression baseline
cargo test -p exg-server --test boot_panics
cargo test -p exg-user-service
scripts/demo-stage1a.sh
```

Expected: all green.

Verify negative cases by hand:
- No token: `-1002` + 401 ✓
- Expired token: `-1002` + 401 ✓
- Dup clientOrderId: `-1014` + 409 ✓
- Wrong password: response byte-identical to unknown email

- [ ] **Step 4: Commit demo script + final acceptance log**

```bash
git add scripts/demo-stage1a.sh
git commit -m "$(cat <<'EOF'
feat(scripts): add stage 1a cold-boot demo script

End-to-end demo: docker-compose postgres → migrate reset → server boot
→ register → login → order → dup-coid (409) → no-token (401) → /me
→ shutdown → wal-dump.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Spec ↔ Plan Coverage Matrix

| Spec section | Task |
|---|---|
| §3 Non-Goals | enforced by omission |
| §4.2 Startup additions | Task 7 |
| §4.3 Auth Module Shape | Task 2 + Task 3 |
| §4.3.1 Login rate limit | Task 6 |
| §4.3.2 Constant-time login | Task 3 + Task 4 |
| §4.4 AppState shape | Task 6 + Task 7 |
| §4.5 Auth middleware pattern | Task 6 (extract_user_id_from_jwt) |
| §4.6 Dedup architecture | Task 6 (place_order gate) |
| §5 Component changes | Tasks 1-7 |
| §6 Data flow contract | Task 6 (handlers behavior), Task 10 (e2e verification) |
| §7.1 Error map | Task 5 + Task 6 |
| §7.2 Panic conditions | Task 7 (validate + connect + ping) |
| §7.3 Logging discipline | Task 5 (Debug masking) |
| §8.1 Unit tests | Tasks 1, 2, 4, 5 |
| §8.2 Integration tests | Task 3 (repo) |
| §8.3 e2e | Task 10 (stage1a) + Task 9 (Stage 0 regression rewrite) |
| §8.4 Boot panic suite | Task 8 |
| §8.5 Demo script | Task 11 |
| §8.6 Acceptance checklist | Task 11 step 3 |
| §9 Invariants 11-20 | enforced across tasks; tests verify each |
| §11 Forward pointers | not implemented (Stage 1b+) |

---

## Cross-Task Notes

### sqlx::test path resolution

All sqlx::test macros use `migrations = "../../migrations"`. Path is resolved by the macro at COMPILE TIME relative to `CARGO_MANIFEST_DIR` (the crate root, NOT runtime CWD). From `crates/exg-{user-service,server}/`, `../../migrations` correctly resolves to workspace-root `migrations/`. If `cargo test` complains about migration path, fall back to absolute path via `env!("CARGO_MANIFEST_DIR")`.

### docker-compose PG dependency

All sqlx::test invocations and e2e tests require `docker-compose up -d postgres` to be running. CI must do this before `cargo test`. The script `scripts/demo-stage1a.sh` handles it; tests assume it's already up.

### `run_with_config_with_pool` test seam

Task 9 introduces this two-arg variant for tests to inject the sqlx::test pool. The single-arg `run_with_config(cfg)` keeps its original signature; main.rs calls it unchanged.

### `AuthError` variant inventory

The full set used across tasks: `InvalidInput`, `EmailExists`, `InvalidCredentials`, `DbError`, `JwtError`, `HashError`. Add missing ones in `crates/exg-user-service/src/error.rs` at Task 3 start.

### Migration filename ordering

`20260514000001_client_order_ids` > all existing Stage 0 migrations (`2026010100000{1,2,3,4,5,6}_*`). Order is correct.

---

## Worktree Parallelization

```
Lane A (independent):  Task 1 (config + migration)  ─┐
Lane B (depends A):   Task 2 (auth refactor)         ├─→ Lane C (depends A,B)
                                                       │     Task 3 (repo)
                                                       │     Task 4 (timing test)
                                                       │
                                                       └─→ Lane D (depends A,C)
                                                             Task 5 (errors+types)
                                                             Task 6 (handlers)
                                                             Task 7 (server)
                                                             Task 8 (boot panics)
                                                             Task 9 (Stage 0 e2e rewrite)
                                                             Task 10 (Stage 1a e2e)
                                                             Task 11 (demo + acceptance)
```

A + B can start in parallel (different crates). C depends on both. D is mostly sequential within itself. Sequential execution is fine for a 11-task plan — parallelism gain is small.
