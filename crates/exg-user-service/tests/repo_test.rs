//! Stage 1a PG-backed repo integration tests.
//! Each test gets its own ephemeral DB via #[sqlx::test].

use exg_common::SnowflakeGen;
use exg_config::AuthConfig;
use exg_user_service::{
    AuthError, find_user_by_id, init_dummy_argon2_hash_for_tests, login_user, register_user,
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
    let result = register_user(&pool, &id_gen, "bob@example.com", "different-password").await;
    assert!(matches!(result, Err(AuthError::EmailExists)));
}

#[sqlx::test(migrations = "../../migrations")]
async fn register_normalizes_email_to_lowercase(pool: PgPool) {
    let id_gen = SnowflakeGen::new(1);
    register_user(&pool, &id_gen, "Alice@Example.COM", "hunter2hunter2")
        .await
        .unwrap();
    let result = register_user(&pool, &id_gen, "alice@example.com", "other-password").await;
    assert!(matches!(result, Err(AuthError::EmailExists)));
}

#[sqlx::test(migrations = "../../migrations")]
async fn login_with_correct_password(pool: PgPool) {
    init_dummy_argon2_hash_for_tests();
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
    init_dummy_argon2_hash_for_tests();
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
    init_dummy_argon2_hash_for_tests();
    let cfg = test_auth_cfg();
    let result = login_user(&pool, &cfg, "ghost@example.com", "any-pw").await;
    assert!(matches!(result, Err(AuthError::InvalidCredentials)));
}

#[sqlx::test(migrations = "../../migrations")]
async fn login_inactive_user_rejected(pool: PgPool) {
    init_dummy_argon2_hash_for_tests();
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
