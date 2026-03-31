use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

// ── Newtype IDs ────────────────────────────────────────────────��────────

macro_rules! define_id {
    ($(#[$meta:meta])* $name:ident, $inner:ty) => {
        $(#[$meta])*
        #[derive(
            Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default,
            Serialize, Deserialize,
            rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
        )]
        #[serde(transparent)]
        #[rkyv(derive(Debug))]
        pub struct $name(pub $inner);

        impl $name {
            #[inline]
            pub const fn new(v: $inner) -> Self {
                Self(v)
            }

            #[inline]
            pub const fn value(self) -> $inner {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl From<$inner> for $name {
            #[inline]
            fn from(v: $inner) -> Self {
                Self(v)
            }
        }
    };
}

define_id!(
    /// Unique order identifier (Snowflake-generated).
    OrderId, u64
);
define_id!(
    /// Unique trade identifier (Snowflake-generated).
    TradeId, u64
);
define_id!(
    /// User identifier.
    UserId, u64
);
define_id!(
    /// Account identifier (supports sub-accounts).
    AccountId, u64
);
define_id!(
    /// Symbol (trading pair) identifier. u16 is sufficient for < 65K pairs.
    SymbolId, u16
);

// ── UnixMicros ────────���─────────────────────────────────────────────────

/// Microsecond-precision UTC timestamp.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default,
    Serialize, Deserialize,
    rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
#[serde(transparent)]
#[rkyv(derive(Debug))]
pub struct UnixMicros(pub u64);

impl UnixMicros {
    #[inline]
    pub fn now() -> Self {
        let d = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before UNIX epoch");
        Self(d.as_micros() as u64)
    }

    #[inline]
    pub const fn from_micros(us: u64) -> Self {
        Self(us)
    }

    #[inline]
    pub const fn as_micros(self) -> u64 {
        self.0
    }

    /// Convert to milliseconds (truncating).
    #[inline]
    pub const fn as_millis(self) -> u64 {
        self.0 / 1_000
    }

    /// Convert to seconds (truncating).
    #[inline]
    pub const fn as_secs(self) -> u64 {
        self.0 / 1_000_000
    }
}

impl fmt::Display for UnixMicros {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ── Snowflake ID Generator ──────────────────────────────────────────────

/// Twitter-style Snowflake ID generator.
///
/// Layout (64 bits):
/// ```text
/// [1 bit unused] [41 bits timestamp_ms] [10 bits node_id] [12 bits sequence]
/// ```
///
/// - Timestamp: milliseconds since custom epoch (2024-01-01 00:00:00 UTC).
/// - Node ID: 0..1023, identifies the generating process/machine.
/// - Sequence: 0..4095 per millisecond per node.
///
/// Monotonically increasing within a single node.
/// Uses a single packed atomic (timestamp_ms << 12 | sequence) for correctness.
pub struct SnowflakeGen {
    node_id: u64,
    /// Packed state: upper 52 bits = timestamp_ms, lower 12 bits = sequence.
    state: AtomicU64,
}

/// Custom epoch: 2024-01-01T00:00:00Z in UNIX milliseconds.
const SNOWFLAKE_EPOCH_MS: u64 = 1_704_067_200_000;

const NODE_BITS: u64 = 10;
const SEQ_BITS: u64 = 12;
const MAX_NODE: u64 = (1 << NODE_BITS) - 1;
const MAX_SEQ: u64 = (1 << SEQ_BITS) - 1;

impl SnowflakeGen {
    /// Create a new generator for the given `node_id` (0..1023).
    ///
    /// # Panics
    ///
    /// Panics if `node_id > 1023`.
    pub fn new(node_id: u16) -> Self {
        let node_id = node_id as u64;
        assert!(node_id <= MAX_NODE, "node_id must be <= {MAX_NODE}");
        Self {
            node_id,
            state: AtomicU64::new(0),
        }
    }

    /// Generate the next unique ID.
    ///
    /// If the sequence overflows within the same millisecond, this function
    /// spin-waits until the next millisecond.
    pub fn next_id(&self) -> u64 {
        loop {
            let now_ms = current_ms();
            let old_state = self.state.load(Ordering::Acquire);
            let old_ms = old_state >> SEQ_BITS;
            let old_seq = old_state & MAX_SEQ;

            let (new_ms, new_seq) = if now_ms > old_ms {
                // New millisecond — reset sequence.
                (now_ms, 0u64)
            } else {
                // Same (or older — clock skew) millisecond.
                let next_seq = old_seq + 1;
                if next_seq > MAX_SEQ {
                    // Sequence exhausted — spin until next ms.
                    std::hint::spin_loop();
                    continue;
                }
                (old_ms, next_seq)
            };

            let new_state = (new_ms << SEQ_BITS) | new_seq;
            if self
                .state
                .compare_exchange(old_state, new_state, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                let ts = new_ms - SNOWFLAKE_EPOCH_MS;
                return (ts << (NODE_BITS + SEQ_BITS)) | (self.node_id << SEQ_BITS) | new_seq;
            }
            // CAS failed — retry.
        }
    }
}

#[inline]
fn current_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_millis() as u64
}

// ══════════════════════════════════════════════════════════════════════════
// Tests
// ══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn test_id_newtype_display() {
        assert_eq!(OrderId::new(42).to_string(), "42");
        assert_eq!(UserId::new(100).to_string(), "100");
        assert_eq!(SymbolId::new(1).to_string(), "1");
    }

    #[test]
    fn test_id_from_inner() {
        let oid: OrderId = 123u64.into();
        assert_eq!(oid.value(), 123);
    }

    #[test]
    fn test_id_ordering() {
        assert!(OrderId::new(1) < OrderId::new(2));
        assert_eq!(TradeId::new(5), TradeId::new(5));
    }

    #[test]
    fn test_id_serde_roundtrip() {
        let id = OrderId::new(999);
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "999");
        let parsed: OrderId = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, id);
    }

    #[test]
    fn test_unix_micros_now() {
        let t = UnixMicros::now();
        assert!(t.as_micros() > 0);
        assert!(t.as_secs() > 1_700_000_000); // post-2023
    }

    #[test]
    fn test_unix_micros_conversions() {
        let t = UnixMicros::from_micros(1_500_000);
        assert_eq!(t.as_millis(), 1_500);
        assert_eq!(t.as_secs(), 1);
    }

    #[test]
    fn test_snowflake_uniqueness() {
        let sf = SnowflakeGen::new(1);
        let mut ids = HashSet::new();
        for _ in 0..10_000 {
            let id = sf.next_id();
            assert!(ids.insert(id), "duplicate snowflake ID: {id}");
        }
    }

    #[test]
    fn test_snowflake_monotonic() {
        let sf = SnowflakeGen::new(0);
        let mut prev = 0u64;
        for _ in 0..1_000 {
            let id = sf.next_id();
            assert!(id > prev, "non-monotonic: {id} <= {prev}");
            prev = id;
        }
    }

    #[test]
    #[should_panic]
    fn test_snowflake_invalid_node() {
        SnowflakeGen::new(1024);
    }

    #[test]
    fn test_snowflake_node_id_encoded() {
        let sf = SnowflakeGen::new(42);
        let id = sf.next_id();
        let node = (id >> SEQ_BITS) & MAX_NODE;
        assert_eq!(node, 42);
    }
}
