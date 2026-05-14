# Stage 1a: Persistence + Auth — Design

**Date**: 2026-05-14
**Status**: Approved (pending final user spec review before CEO/Eng review)
**Predecessor**: [`2026-05-14-stage0-runnable-skeleton-design.md`](./2026-05-14-stage0-runnable-skeleton-design.md)

## 1. Background

Stage 0 shipped (merge commit `4eecbe7`): HTTP → Ring Buffer → Matching → WAL command hot path with no DB, no auth, no WebSocket. Authentication is a stub (`X-User-Id` header trust). Persistence is `WalWriter` only; matching state is in-memory and lost on restart.

Stage 1 in the original 8-stage decomposition covers persistence + auth + Redis rate-limit + `client_order_id` dedup + WAL replay on restart. That bundle is too large for one PR; this spec covers only **Stage 1a**, which deliberately defers WAL replay + boot-time host-invariant removal to **Stage 1b** (separate spec/PR). Stage 1a is the minimum that lets a real user register, log in, and place an order with the engine seeing a real `UserId` instead of a forged header.

Stage 0 spec §11 forward-pointers explicitly carved out:

- `X-User-Id` header → JWT middleware (interface to handlers unchanged) — **lands in Stage 1a**
- `client_order_id` deduplication via per-user index — **lands in Stage 1a**
- In-memory matching state → persisted snapshots + WAL replay on restart — **Stage 1b**
- Boot-time host-binding assert removal — **Stage 1b** (paired with replay)
- Static `mark_price` from config → oracle feed — Stage 2

## 2. Goal

A user can:

1. `POST /api/v1/auth/register` with `email` + `password` → PostgreSQL `users` row created, returns `userId`
2. `POST /api/v1/auth/login` with `email` + `password` → JWT bearer token (HS256, 24h expiry)
3. `GET /api/v1/me` with `Authorization: Bearer <jwt>` → returns own `userId`, `email`, `kycLevel`
4. `POST /api/v1/order` with `Authorization: Bearer <jwt>` → engine receives real `UserId` from JWT claims (replaces Stage 0's `X-User-Id` header trust)
5. Two `POST /api/v1/order` calls with the same `(user_id, clientOrderId)` → second returns HTTP 409, only one event in WAL

PostgreSQL backs `users` table (already migrated in Stage 0's `migrations/`) and a new `user_client_order_ids` dedup table. JWT secret loaded from config with boot-time validation.

## 3. Non-Goals (Deferred)

| Item | Stage |
|---|---|
| WAL replay on restart + `MatchingEngine::restore_from_snapshot` boot wiring | **1b** |
| Boot-time host-binding assert removal | **1b** (paired with replay) |
| Refresh token / token rotation | 2+ |
| Logout / session blacklist | 2+ (needs Redis) |
| 2FA endpoints (lib already implemented in `auth.rs`) | 2+ |
| API Key authentication (table already migrated, lib exists) | 2+ |
| Sub-account routing | 2+ |
| Real mark-price feed / oracle | 2 |
| WebSocket / downstream service tasks | 2 |
| Redis-backed rate limiter / dedup TTL cleanup / `login_history` writes | 7 |
| Wallet / deposit / withdrawal | 3+ |

## 4. Architecture

### 4.1 Threading Model — unchanged from Stage 0

Same: Tokio multi-thread runtime + dedicated matching OS thread + same-thread batched WAL fsync. Stage 1a adds an `sqlx::PgPool` accessed by Actix worker threads only (the matching thread does not touch PG — DB writes are limited to the HTTP request path).

### 4.2 Startup Sequence (additions vs Stage 0 §4.5)

```
Stage 0 step 1   tracing init
Stage 0 step 2   ExgConfig::load + validate     ← new auth invariants added in validate
NEW   step 2.5   assert auth.jwt_secret.len() >= 32   (per §9 #11)
NEW   step 2.6   assert auth.jwt_secret != "CHANGE-ME-DEV-ONLY"
Stage 0 step 3   host loopback invariant         ← unchanged in 1a (removed in 1b)
Stage 0 step 3a  symbols.len() == 1
Stage 0 step 3b  WAL dir empty
NEW   step 3.5   let pool = PgPool::connect(cfg.database.url).await?
NEW   step 3.6   sqlx::query("SELECT 1").execute(&pool).await?
NEW   step 3.7   DUMMY_ARGON2_HASH.set(hash_password("__dummy_constant_for_timing_equalization__")?)
                 — OnceCell init for constant-time login (per §4.3.2 + §9 #20)
Stage 0 step 4   WAL open
Stage 0 step 5   RingBuffer + Producer/Consumer (Box::leak)
Stage 0 step 6   SymbolConfig conversion
Stage 0 step 7   MatchingEngine::new + set_mark_price
Stage 0 step 8   SnowflakeGen
Stage 0 step 9   shutdown_flag
Stage 0 step 10  Prometheus exporter
Stage 0 step 11  spawn matching OS thread
NEW   step 11.5  build AppState { producer, snowflake, cfg, pool, auth_cfg, rate_limiter }
Stage 0 step 12  Actix HttpServer
Stage 0 step 13-15  bind / ctrl_c / 5-step shutdown
```

Stage 1a does **not** auto-migrate the database. Operators run `scripts/migrate.sh up` (already exists from Stage 0 baseline). `run_with_config` performs only a `SELECT 1` ping; a stale schema is the operator's responsibility. Auto-migrate would couple boot to schema state in a way that breaks rolling deploys and is hard to reason about; sticking with explicit `migrate.sh` keeps the schema-change discipline visible.

**Deployment ordering**: `migrate.sh up` MUST run BEFORE deploying the new binary. The Stage 1a binary expects the `user_client_order_ids` table to exist (and the `users` table from Stage 0); if the binary boots against a stale schema, `SELECT 1` ping succeeds but the first `INSERT INTO user_client_order_ids` returns a `relation does not exist` error, mapped to `ApiError::db_unavailable` → 500. Operators verify schema readiness via `scripts/migrate.sh status` showing the latest migration as applied.

### 4.3 Auth Module Shape

The existing `crates/exg-user-service/src/auth.rs` (615 LOC) implements an `AuthService` struct with **in-memory** `HashMap<UserId, User>` plus JWT signing/verification and Argon2id password handling. Stage 1a refactors it:

- **Pure crypto functions** (no DB, no state):
  - `pub fn sign_jwt(secret: &[u8], claims: &JwtClaims) -> Result<String, AuthError>`
  - `pub fn verify_jwt(secret: &[u8], token: &str) -> Result<JwtClaims, AuthError>`
  - `pub fn hash_password(plain: &str) -> Result<String, AuthError>`
  - `pub fn verify_password(plain: &str, hash: &str) -> Result<bool, AuthError>`
- **PG-backed lib** (new `crates/exg-user-service/src/repo.rs`):
  - `pub async fn register_user(&PgPool, &SnowflakeGen, email, password) -> Result<UserId, AuthError>`
  - `pub async fn login_user(&PgPool, &AuthConfig, email, password) -> Result<LoginResponse, AuthError>`
  - `pub async fn find_user_by_id(&PgPool, UserId) -> Result<Option<UserRow>, AuthError>`
- **Deprecated in Stage 1a** (mark `#[allow(dead_code)]`, keep for later stages): in-memory `register`/`login` on `AuthService` struct; 2FA / API key / sub-account / login_history mutators. These are not removed because Stage 2+ will revive their PG-backed equivalents.

### 4.3.1 Login Rate Limit (CEO review finding 3.2 fix)

Before `login_user` runs, the handler consults `state.rate_limiter`:
- key1 = `format!("login:email:{}", email_normalized)`
- key2 = `format!("login:ip:{}", req.peer_addr().map(|a| a.ip().to_string()).unwrap_or_else(|| "unknown".into()))` (proxy-aware extraction is Stage 7 work; Stage 1a accepts that requests behind a reverse proxy share the "unknown" bucket)
- If either bucket is exhausted → 429 + `-1003` `"login rate limit"`.

Both buckets use `cfg.risk.max_orders_per_second` as the rate (Stage 7 may split with a dedicated `cfg.risk.max_login_per_second`).

### 4.3.2 Constant-Time Login (CEO review finding 3.3 fix)

Email enumeration via response-time difference (SELECT-miss ~1ms vs SELECT-hit + Argon2-verify ~50ms) is blocked:

```rust
// In login_user:
let user_opt = sqlx::query_as!(...).fetch_optional(&pool).await?;
let (stored_hash, user_id, is_active) = match user_opt {
    Some(row) => (row.password_hash, row.user_id, row.is_active),
    None => (
        // Constant dummy hash precomputed at boot (OnceCell), so verify_password
        // runs against it and takes the same ~50ms even when email doesn't exist.
        DUMMY_ARGON2_HASH.get().expect("dummy hash inited at boot").clone(),
        UserId::new(0),  // sentinel; will fail the next branch anyway
        false,
    ),
};
let pw_ok = verify_password(&password, &stored_hash)?;
if user_opt.is_none() || !pw_ok || !is_active {
    return Err(AuthError::InvalidCredentials);
}
```

The constant dummy hash is hashed once at boot from a fixed input (e.g. `hash_password("__dummy_constant_for_timing_equalization__")`) and stored in a `OnceCell<String>`. This ensures `verify_password` runs exactly once per login regardless of which branch we're in.

### 4.4 AppState Shape Evolution

```
Stage 0:   AppState { producer, snowflake, cfg }
Stage 1a:  AppState { producer, snowflake, cfg, pool, auth_cfg, rate_limiter }
```

`pool: sqlx::PgPool` (cheap to clone — internally `Arc`). `auth_cfg: Arc<AuthConfig>` lifted from `cfg` for handler ergonomics. `rate_limiter: Arc<Mutex<RateLimiter>>` from the existing `exg-api-gateway::middleware::RateLimiter`, mounted into actual request flow (Stage 0 had the lib but never wired it).

### 4.5 Auth Middleware Pattern

Stage 0's `extract_user_id(&req) -> Result<UserId, ApiError>` reads `X-User-Id` header. Stage 1a renames and reworks:

```rust
fn extract_user_id_from_jwt(
    req: &HttpRequest,
    secret: &[u8],
) -> Result<UserId, ApiError> {
    let h = req.headers().get("Authorization")
        .ok_or_else(|| ApiError::unauthorized("missing Authorization header"))?;
    let s = h.to_str().map_err(|_| ApiError::unauthorized("..."))?;
    let token = s.strip_prefix("Bearer ")
        .ok_or_else(|| ApiError::unauthorized("Authorization must be Bearer"))?;
    let claims = exg_user_service::verify_jwt(secret, token)
        .map_err(|_| ApiError::unauthorized("invalid or expired token"))?;
    Ok(UserId::new(claims.user_id))
}
```

The handler signature does not change (`place_order`, `cancel_order`, `amend_order` still call the helper; only the helper's body changes). This is intentional — Stage 0 §11 forward-pointer guaranteed "interface to handlers unchanged".

### 4.6 Dedup Architecture

Insert-then-check at the HTTP boundary before the ring-buffer push:

```rust
// In place_order handler, after extract_user_id_from_jwt, before enqueue:
if let Some(coid_str) = body.client_order_id.as_deref() {
    let coid: u64 = coid_str.parse()
        .map_err(|_| ApiError::bad_request("clientOrderId must be u64"))?;
    let inserted = sqlx::query(
        "INSERT INTO user_client_order_ids (user_id, client_order_id, created_at)
         VALUES ($1, $2, $3) ON CONFLICT DO NOTHING"
    )
    .bind(user_id.value() as i64)
    .bind(coid as i64)
    .bind(now_micros() as i64)
    .execute(&state.pool).await
    .map_err(ApiError::db_unavailable)?;
    if inserted.rows_affected() == 0 {
        return Err(ApiError::duplicate_order(
            "duplicate clientOrderId for this user"
        ));
    }
}
```

`cancel_order` and `amend_order` do NOT touch this table. They reference an existing `order_id` whose ownership the matching engine validates (Stage 0 IDOR invariant, `engine.rs:442`).

The dedup table grows monotonically in Stage 1a. Stage 7 adds a cleanup job (delete rows older than 30 days). This is acceptable because each row is 24 bytes and even a million orders/day produces ~9GB/year — well within reasonable maintenance window.

## 5. Component Changes

### 5.1 No-Touch Crates

`exg-common`, `exg-protocol`, `exg-ringbuffer`, `exg-wal`, `exg-risk-engine`, `exg-matching-engine`, `exg-wal-dump`, `exg-clearing`, `exg-market-data`, `exg-order-service`, `exg-wallet-service`, `exg-admin-service` — zero changes.

### 5.2 Modified

| File | Change |
|---|---|
| `Cargo.toml` (workspace) | No new workspace dep additions (`sqlx`, `tracing`, `jsonwebtoken`, `argon2`, `tokio` already declared) |
| `config/default.toml` | (a) Add `[auth]` section with `jwt_secret = "CHANGE-ME-DEV-ONLY-MUST-BE-AT-LEAST-32-BYTES-OK"` (placeholder forcing operator to override via env) + `jwt_expiry_secs = 86400`. (b) Fix `[database].url` from `postgres://exg:exg@...` to `postgres://exg:exg_dev_password@localhost:5432/exg` to match `docker-compose.yml` + `scripts/migrate.sh`. |
| `crates/exg-config/src/lib.rs` | Add `pub struct AuthConfig { pub jwt_secret: String, pub jwt_expiry_secs: u64 }`. Add `pub auth: AuthConfig` to `ExgConfig`. Update `default_config()`. |
| `crates/exg-config/src/validation.rs` | New checks: `auth.jwt_secret.len() >= 32`; `auth.jwt_secret != "CHANGE-ME-DEV-ONLY-MUST-BE-AT-LEAST-32-BYTES-OK"` (placeholder rejection); `auth.jwt_expiry_secs > 0`. |
| `crates/exg-config/src/tests.rs` | 5 new tests: jwt_secret length too short / equals placeholder / valid 32+ bytes / jwt_expiry_secs=0 rejected / database.url format sanity. |
| `crates/exg-user-service/Cargo.toml` | Add `sqlx = { workspace = true, features = ["runtime-tokio", "postgres", "chrono"] }` and `tracing`. `jsonwebtoken`, `argon2`, `rand`, `uuid`, `chrono` are already declared (confirmed pre-existing). `secrecy = "0.10"` added if `Secret<String>` path chosen for password redaction (per invariant #18); otherwise manual `impl Debug` suffices and no new dep needed. |
| `crates/exg-user-service/src/auth.rs` | Extract `sign_jwt` / `verify_jwt` / `hash_password` / `verify_password` as top-level `pub fn`. Mark `AuthService` mutating methods (`register`, `login`, `enable_2fa`, etc.) with `#[allow(dead_code)]` and a `// Stage 1a: replaced by repo::*` doc comment. Do NOT delete — Stage 2+ revives them. |
| `crates/exg-user-service/src/lib.rs` | Add `pub mod repo;` + re-export `sign_jwt`, `verify_jwt`, `hash_password`, `verify_password`, `JwtClaims`, `LoginResponse`, `AuthError`, `repo::*`. |
| `crates/exg-api-gateway/Cargo.toml` | Add `sqlx` (workspace) and `exg-user-service` (workspace). |
| `crates/exg-api-gateway/src/state.rs` | `AppState` gains `pub pool: sqlx::PgPool` + `pub auth_cfg: Arc<exg_config::AuthConfig>` + `pub rate_limiter: Arc<parking_lot::Mutex<crate::middleware::RateLimiter>>`. `parking_lot` is already a workspace dep (used by Stage 0 `Mutex<Producer>`). The existing `RateLimiter::consume(key: &str, now)` is keyed by arbitrary string — login uses `login:email:<x>` / `login:ip:<x>` keys, order handlers use `user:<id>` keys. Algorithm is the existing token bucket (one bucket per key); refill rate from `cfg.risk.max_orders_per_second`. |
| `crates/exg-api-gateway/src/types.rs` | Add `RegisterRequest`, `LoginRequest`, `LoginResponse`, `MeResponse`. All with `#[serde(rename_all = "camelCase")]`. |
| `crates/exg-api-gateway/src/error.rs` | Add `pub const ERR_RATE_LIMITED_USER: i32 = -1003`; `pub const ERR_DUPLICATE_RESOURCE: i32 = -1014`. New constructors `duplicate_resource(msg)`, `user_rate_limited()`, `db_unavailable(sqlx::Error)`. `status_code` map: `-1014 → 409 CONFLICT`, `-1003 → 429 TOO_MANY_REQUESTS`. **Single -1014 constant reused for both "duplicate email" (register) and "duplicate clientOrderId" (place_order) 409 responses** — msg differs (`"email already registered"` vs `"duplicate clientOrderId"`). This is a deliberate semantic stretch acceptable for Stage 1a (both are 409-conflict); Stage 7 may split into distinct codes if client UX requires it. |
| `crates/exg-api-gateway/src/handlers.rs` | Add `register`, `login`, `me` handlers. Rename `extract_user_id` → `extract_user_id_from_jwt` and rewrite body per §4.5. Add rate-limit gate + dedup gate in `place_order` per §4.6. |
| `crates/exg-api-gateway/src/app_factory.rs` | Mount `/api/v1/auth/register`, `/api/v1/auth/login`, `/api/v1/me`. |
| `crates/exg-server/Cargo.toml` | Add `sqlx` with features `runtime-tokio,postgres,macros`. |
| `crates/exg-server/src/lib.rs` | `run_with_config` adds steps 2.5, 2.6, 3.5, 3.6, 11.5 per §4.2. Pass `pool` + `auth_cfg` into `AppState`. |
| `crates/exg-server/tests/boot_panics.rs` | Add 3 tests: `boot_panics_on_short_jwt_secret`, `boot_panics_on_default_jwt_secret`, `boot_panics_on_db_unreachable`. Existing 4 tests gain `cfg.auth.jwt_secret` field for compile compatibility. |

### 5.3 New

| File | Responsibility |
|---|---|
| `migrations/20260514000001_client_order_ids.up.sql` | `CREATE TABLE user_client_order_ids (user_id BIGINT NOT NULL, client_order_id BIGINT NOT NULL, created_at BIGINT NOT NULL, PRIMARY KEY (user_id, client_order_id)); CREATE INDEX idx_user_client_order_ids_created_at ON user_client_order_ids (created_at);` |
| `migrations/20260514000001_client_order_ids.down.sql` | `DROP TABLE user_client_order_ids;` |
| `crates/exg-user-service/src/repo.rs` | PG-backed `register_user`, `login_user`, `find_user_by_id`. All `async`. Use `sqlx::query` (string form), not `query!` macro — keeps build-time PG dependency out of CI until Stage 7 prepare. |
| `crates/exg-user-service/tests/repo_test.rs` | `#[sqlx::test(migrations = "../../migrations")]`. **Path is resolved by the macro at compile time relative to `CARGO_MANIFEST_DIR` (the crate root, `crates/exg-user-service/`), NOT relative to runtime CWD.** From the crate root, `../../migrations` correctly resolves to the workspace-root `migrations/` dir. Verified against sqlx 0.8 macro semantics. If the path mis-resolves in CI, the fallback is `env!("CARGO_MANIFEST_DIR")` join, but Stage 1a sticks with the relative form. 7 cases per §5.2. |
| `crates/exg-server/tests/stage1a_e2e.rs` | 10 e2e cases per §5.3. Uses `sqlx::migrate!("../../migrations").run(&pool)` at boot. |
| `scripts/demo-stage1a.sh` | Cold-boot demo: docker-compose up postgres → migrate reset → cargo run server → curl register/login/order/dup/no-token → wal-dump → cleanup. |

## 6. Data Flow Contract (additions to Stage 0 §6)

### 6.1 New endpoints' semantics

- `POST /api/v1/auth/register` HTTP 201 = row inserted into `users`. HTTP 409 = email already existed (idempotent on identical retry — but second-attempt password may differ; we still reject).
- `POST /api/v1/auth/login` HTTP 200 = credentials verified, JWT issued. The same `"invalid credentials"` response covers "user not found", "wrong password", and "user inactive" to prevent enumeration.
- `GET /api/v1/me` HTTP 200 = token valid; returns row by `claims.user_id`. HTTP 401 covers all token failure modes.
- `POST /api/v1/order` HTTP 200 = (a) JWT verified, (b) per-user rate limit not exceeded, (c) clientOrderId not previously used by this user, (d) command enqueued. HTTP 401/429/409/503 cover the four failure stages respectively.

### 6.2 Response shapes (camelCase JSON, ID-as-string per Binance)

```jsonc
// register success (201)
{ "userId": "8123456789", "email": "alice@example.com", "status": "REGISTERED" }
// login success (200)
{ "accessToken": "<jwt>", "expiresIn": 86400, "userId": "8123456789" }
// me (200)
{ "userId": "8123456789", "email": "alice@example.com", "kycLevel": 0 }
```

## 7. Error Handling (additions to Stage 0 §7)

### 7.1 HTTP → ApiError map (new rows)

| Trigger | HTTP | code | msg |
|---|---|---|---|
| Missing/malformed `Authorization` header | 401 | -1002 | `missing or invalid Authorization header` |
| JWT signature / exp / claims invalid | 401 | -1002 | `invalid or expired token` |
| Email already registered | 409 | -1014 | `email already registered` |
| Password length out of [8, 128] | 400 | -1100 | `password: length must be 8-128` |
| Email length out of [1, 254] | 400 | -1100 | `email: length must be 1-254` |
| Login: any auth failure (user-not-found / wrong-password / inactive) | 401 | -1002 | `invalid credentials` |
| Per-user rate limit exceeded | 429 | -1003 | `rate limit exceeded for user` |
| Duplicate `(user_id, clientOrderId)` | 409 | -1014 | `duplicate clientOrderId` |
| `sqlx` connection / query error at runtime | 500 | -1000 | `database unavailable` |

### 7.2 Panic conditions (additions to Stage 0 §7.2)

Allowed (boot-time fail-loud):
- `cfg.validate()` now also rejects bad JWT secret / expiry — panic on fail.
- `PgPool::connect` returns Err → panic.
- `SELECT 1` ping fails → panic.

NOT allowed (runtime DB errors do not panic):
- Per-request `sqlx` errors → `ApiError::db_unavailable` + `tracing::error!`, response 500. Unlike WAL (Stage 0 §7.2), DB is not the source of truth — failure mode is degraded service, not silent data loss.

### 7.3 Logging discipline

- Passwords never appear in log lines. `RegisterRequest` and `LoginRequest` derive only what's needed; if anyone adds `#[derive(Debug)]`, the `password` field must be marked `#[serde(skip)]` for `Debug` or wrapped in a `Secret<String>` type.
- JWT tokens never appear in log lines (logs receive `claims.user_id`, not the token string).
- `email` may appear in logs at info level (it is the user-visible identifier).

## 8. Test Plan

### 8.1 Unit tests

| Location | Cases (count) |
|---|---|
| `exg-config/src/tests.rs` | jwt_secret length (3), expiry positive (1), db_url format (1) — **5** |
| `exg-user-service` inline `#[cfg(test)] mod tests` | sign+verify roundtrip / expired token rejected / wrong-secret rejected / hash prefix is argon2id / verify against own hash true / verify against random false — **6** |

### 8.2 Integration tests (sqlx::test isolated DB)

`crates/exg-user-service/tests/repo_test.rs` — **7** cases per §5.2.

### 8.3 e2e

`crates/exg-server/tests/stage1a_e2e.rs` — **10** cases per §5.3. Uses `sqlx::migrate!("../../migrations").run(&pool)` inside each test fixture for schema setup. Requires `DATABASE_URL` env var pointing at a running PG (CI uses docker-compose service).

### 8.4 Boot panic suite

`crates/exg-server/tests/boot_panics.rs` — existing 4 + **3 new** = **7**.

### 8.5 Demo script

`scripts/demo-stage1a.sh` cold-boot — runs end-to-end, asserts WAL ends up with NewOrder + (Canceled if amend) events, dedup table has exactly the unique clientOrderIds used.

### 8.6 Acceptance Checklist

Stage 1a complete iff all pass:

- [ ] `cargo check --workspace` 0 warnings
- [ ] `cargo clippy --workspace -- -D warnings` 0 warnings
- [ ] `cargo fmt --check` clean
- [ ] `docker-compose up -d postgres` + `scripts/migrate.sh up` runs cleanly
- [ ] `cargo test --workspace` all green (Stage 0 baseline 395 + Stage 1a ~28 ≈ 423+)
- [ ] `cargo test -p exg-server --test stage1a_e2e` — 10/10 pass
- [ ] `cargo test -p exg-server --test boot_panics` — 7/7 pass
- [ ] `cargo test -p exg-user-service` — 6 inline + 7 repo = 13/13 pass
- [ ] `scripts/demo-stage1a.sh` from cold — full flow runs, WAL contains successful order events, dedup table reflects insertions
- [ ] Negative curl: no token → -1002; expired token → -1002; duplicate clientOrderId → 409 + -1014; wrong password login indistinguishable from unknown email login (byte-for-byte response)
- [ ] `EXG_AUTH_JWT_SECRET=short cargo run -p exg-server` panics at startup with a message mentioning `jwt_secret`
- [ ] No `f64` introduced on the request path (`grep -r "f64" crates/exg-{user-service,api-gateway,server}/src/` produces zero hits in handler/conversion code)

## 9. Invariants (additions to Stage 0 §9)

Stage 0 §9 #1-10 unchanged. Stage 1a adds:

11. **JWT secret must be ≥ 32 bytes and not the placeholder value `"CHANGE-ME-DEV-ONLY-MUST-BE-AT-LEAST-32-BYTES-OK"`**. Boot panics on violation. Stage 7 connects this to KMS and rejects any plaintext config secret.
12. **Password hashing uses only Argon2id**. The hash format `$argon2id$v=19$m=...` is asserted by tests. Bcrypt / SHA-x / MD5 forms are rejected by `verify_password`.
13. **`client_order_id` dedup happens in the handler BEFORE ring-buffer enqueue**, persisted in `user_client_order_ids` table with `PRIMARY KEY (user_id, client_order_id)`. The matching engine is NOT modified to add internal dedup; ring-buffer events are dedup-free (Stage 1b WAL replay will replay all enqueued events without dedup ambiguity). **Orphan-row semantic**: dedup INSERT is NOT atomic with ring-buffer push. If INSERT succeeds and the subsequent `producer.try_push` fails (e.g. ring buffer full → 429), the row remains in `user_client_order_ids`. A client retry with the same `clientOrderId` will be rejected by dedup → 409. This is acceptable double-error semantic: the client never gets a duplicate order created. The HTTP-200-as-enqueued contract from Stage 0 §6.1 is preserved (HTTP 429 means NOT enqueued); the additional implied contract here is "any clientOrderId we INSERT, we never accept the same clientOrderId again, even if the corresponding push failed."
14. **Email is normalized to lowercase before store/compare**. Case-sensitive email registration would let "Alice@x" and "alice@x" register as distinct users and is a known security antipattern.
15. **JWT claims must include `user_id` (u64) + `exp` (Unix seconds) + `iat` (Unix seconds)**. `verify_jwt` rejects tokens missing any field or with `exp <= now`.
16. **DB error never panics at request time**. The matching thread / WAL path is unchanged — only HTTP handlers degrade to 500. DB is NOT the source of truth; WAL is.
17. **Login responses do not distinguish user-not-found from wrong-password**. The two paths return byte-identical responses (`HTTP 401`, code `-1002`, msg `"invalid credentials"`).
18. **Passwords never log**. No log line at any level may include the plaintext password. Mechanism (not just prose): `RegisterRequest` and `LoginRequest` MUST NOT `#[derive(Debug)]` directly. Either wrap the `password` field in `secrecy::Secret<String>` (preferred — opt-in display via `expose_secret()`) OR manually `impl Debug` that prints `password: "***"`. A unit test asserts `format!("{req:?}")` does not contain the literal password value. Code review must flag any new derive macro on these structs.

19. **Login endpoint rate-limit keys**: per-email (normalized lowercase) AND per-IP, both checked via the in-memory `RateLimiter`. Either bucket exhausted → 429 + `-1003`. This guards against single-email brute force and same-IP scan-multiple-emails. Per-user-id rate limit (on authenticated endpoints) is separate and continues to use `user_id` as key.

20. **Login response time must be constant regardless of email existence**. `login_user` always invokes `verify_password` exactly once — when the email is not found, it runs against a precomputed constant dummy hash (one-time computed at boot, stored in `OnceCell<String>`). Verified by integration test: median response time for "unknown email" and "known email + wrong password" must be within ±5ms.

## 10. Open Questions (resolved during plan stage)

1. Whether to use `sqlx::query!` macros (requires offline cache or live PG at build) vs `sqlx::query` strings. Spec defaults to strings; plan stage can revisit if devex becomes painful.
2. Exact placement of the `auth_cfg` field on `AppState` vs nested inside `cfg` — implementation choice; spec just requires it accessible from handlers without locking.
3. Whether `LoginRequest` accepts `email` only or also `username`. Spec is email-only; username support is Stage 2+ if at all.

## 11. Forward Pointers (Stage 1b+)

Stage 1b will:

- Implement WAL replay on boot: read all events from WAL after loading the latest snapshot (`MatchingEngine::restore_from_snapshot` already exists at `engine.rs:937`) and replay any newer events to reconstruct in-memory state.
- Periodically save snapshots from the matching thread.
- Remove the boot-time host-binding assert (safe once `Authorization` header is the only path to identify users; the assert was a defense against `X-User-Id` forgery).
- Add `client_order_id` dedup table loading at boot if Stage 1b decides to surface dedup state to matching engine internals.

Stage 2 will:
- Refresh token + logout blacklist (likely Redis-backed).
- 2FA endpoints (lib already exists in `auth.rs`).
- API key authentication (table already migrated).
- Downstream service tokio tasks consuming WAL.

Stage 7 will:
- Replace in-memory `RateLimiter` with Redis backend.
- KMS-managed JWT secret (rejecting plaintext config).
- Cleanup job for `user_client_order_ids` rows older than 30 days.
