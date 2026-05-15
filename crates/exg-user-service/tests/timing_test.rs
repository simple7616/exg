//! Stage 1a §9 invariant #20: login_user response time must be constant
//! regardless of email existence. Compare median wall-time of 30 samples
//! for "known wrong pw" vs "unknown email"; difference must be < 5ms.
//!
//! Unit-level test (against login_user fn directly), NOT e2e — HTTP
//! overhead would mask the signal.

use exg_common::SnowflakeGen;
use exg_config::AuthConfig;
use exg_user_service::{init_dummy_argon2_hash_for_tests, login_user, register_user};
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
    init_dummy_argon2_hash_for_tests();
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

    eprintln!(
        "known median: {known_median}us, unknown median: {unknown_median}us, diff: {diff}us"
    );

    // Argon2id is ~50ms. Spec §9 #20 requires diff < 5ms (5000us).
    // In practice we expect <500us. 5ms is the documented spec bound.
    assert!(
        diff < 5_000,
        "login_user timing leak: diff {diff}us between known-wrong-pw and unknown-email"
    );
}
