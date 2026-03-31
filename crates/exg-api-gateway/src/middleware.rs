use exg_common::UnixMicros;
use rustc_hash::FxHashMap;

use crate::error::ApiError;

// ── Token bucket rate limiter ────────────────────────────────────────────

pub struct TokenBucket {
    pub tokens: f64,
    pub last_refill: UnixMicros,
}

/// Rate limiter using token bucket algorithm.
pub struct RateLimiter {
    buckets: FxHashMap<String, TokenBucket>,
    max_tokens: u32,
    refill_rate: f64, // tokens per second
}

impl RateLimiter {
    pub fn new(max_tokens: u32, refill_rate: f64) -> Self {
        Self {
            buckets: FxHashMap::default(),
            max_tokens,
            refill_rate,
        }
    }

    /// Refill tokens for the given key based on elapsed time, then return
    /// whether at least one token is available (without consuming).
    pub fn check(&mut self, key: &str, now: UnixMicros) -> bool {
        let max = self.max_tokens as f64;
        let rate = self.refill_rate;

        let bucket = self
            .buckets
            .entry(key.to_owned())
            .or_insert_with(|| TokenBucket {
                tokens: max,
                last_refill: now,
            });

        refill(bucket, now, max, rate);
        bucket.tokens >= 1.0
    }

    /// Refill, check, and consume one token. Returns `true` if the request
    /// is allowed (token was consumed), `false` if rate-limited.
    pub fn consume(&mut self, key: &str, now: UnixMicros) -> bool {
        let max = self.max_tokens as f64;
        let rate = self.refill_rate;

        let bucket = self
            .buckets
            .entry(key.to_owned())
            .or_insert_with(|| TokenBucket {
                tokens: max,
                last_refill: now,
            });

        refill(bucket, now, max, rate);

        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

fn refill(bucket: &mut TokenBucket, now: UnixMicros, max: f64, rate: f64) {
    let elapsed_us = now.as_micros().saturating_sub(bucket.last_refill.as_micros());
    if elapsed_us > 0 {
        let elapsed_secs = elapsed_us as f64 / 1_000_000.0;
        bucket.tokens = (bucket.tokens + elapsed_secs * rate).min(max);
        bucket.last_refill = now;
    }
}

// ── Timestamp validation ─────────────────────────────────────────────────

/// Validate that the request timestamp is within an acceptable window of the
/// server timestamp. Both values are in milliseconds.
pub fn validate_timestamp(request_ts_ms: u64, server_ts_ms: u64, window_ms: u64) -> bool {
    request_ts_ms.abs_diff(server_ts_ms) <= window_ms
}

// ── API key auth header parsing ──────────────────────────────────────────

/// Parsed API key authentication headers.
pub struct ApiKeyAuth {
    pub key_id: String,
    pub signature: String,
    pub timestamp: u64,
}

impl ApiKeyAuth {
    /// Parse API key, signature, and timestamp from request header values.
    pub fn from_headers(
        api_key: Option<&str>,
        signature: Option<&str>,
        timestamp: Option<&str>,
    ) -> Result<Self, ApiError> {
        let key_id = api_key
            .filter(|k| !k.is_empty())
            .ok_or_else(|| ApiError::unauthorized("Missing API key"))?
            .to_owned();

        let signature = signature
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ApiError::unauthorized("Missing signature"))?
            .to_owned();

        let timestamp = timestamp
            .ok_or_else(|| ApiError::unauthorized("Missing timestamp"))?
            .parse::<u64>()
            .map_err(|_| ApiError::bad_request("Invalid timestamp format"))?;

        Ok(Self {
            key_id,
            signature,
            timestamp,
        })
    }
}
