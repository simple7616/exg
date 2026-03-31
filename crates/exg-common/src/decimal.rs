use std::cmp::Ordering;
use std::fmt;
use std::iter::Sum;
use std::ops::{Add, Div, Mul, Neg, Sub};
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// 18 decimal fractional digits.
const SCALE: i128 = 1_000_000_000_000_000_000; // 10^18
const SCALE_DIGITS: u32 = 18;

/// Fixed-point decimal with 18 fractional digits, backed by `i128`.
///
/// Range: roughly ±1.7 × 10^20 (the integer part can be up to ~170 quintillion).
/// This covers any realistic price, quantity, or balance in a perpetual futures exchange.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct Decimal128 {
    /// Raw value = real_value × 10^18
    raw: i128,
}

impl Decimal128 {
    pub const SCALE: i128 = SCALE;
    pub const ZERO: Self = Self { raw: 0 };
    pub const ONE: Self = Self { raw: SCALE };
    pub const MAX: Self = Self { raw: i128::MAX };
    pub const MIN: Self = Self { raw: i128::MIN };

    /// Create from the raw internal representation.
    #[inline]
    pub const fn from_raw(raw: i128) -> Self {
        Self { raw }
    }

    /// Access the raw i128 value.
    #[inline]
    pub const fn raw(self) -> i128 {
        self.raw
    }

    #[inline]
    pub const fn is_zero(&self) -> bool {
        self.raw == 0
    }

    #[inline]
    pub const fn is_positive(&self) -> bool {
        self.raw > 0
    }

    #[inline]
    pub const fn is_negative(&self) -> bool {
        self.raw < 0
    }

    #[inline]
    pub const fn abs(&self) -> Self {
        // i128::MIN.abs() would overflow; mirror Rust's behaviour (panic in debug).
        Self {
            raw: self.raw.abs(),
        }
    }

    // ── Checked arithmetic ──────────────────────────────────────────────

    #[inline]
    pub const fn checked_add(self, rhs: Self) -> Option<Self> {
        match self.raw.checked_add(rhs.raw) {
            Some(v) => Some(Self { raw: v }),
            None => None,
        }
    }

    #[inline]
    pub const fn checked_sub(self, rhs: Self) -> Option<Self> {
        match self.raw.checked_sub(rhs.raw) {
            Some(v) => Some(Self { raw: v }),
            None => None,
        }
    }

    /// Checked multiplication.
    ///
    /// Strategy: try the simple `(a * b) / SCALE` first. If the intermediate
    /// multiplication overflows i128, fall back to a wide-multiply via
    /// splitting into high/low 64-bit halves.
    #[inline]
    pub const fn checked_mul(self, rhs: Self) -> Option<Self> {
        // Fast path — fits in i128.
        if let Some(prod) = self.raw.checked_mul(rhs.raw) {
            return Some(Self { raw: prod / SCALE });
        }
        // Slow path — wide multiply then divide.
        wide_mul_div(self.raw, rhs.raw, SCALE)
    }

    /// Checked division. Returns `None` on division by zero or overflow.
    #[inline]
    pub const fn checked_div(self, rhs: Self) -> Option<Self> {
        if rhs.raw == 0 {
            return None;
        }
        // (a * SCALE) / b — try fast path first.
        if let Some(scaled) = self.raw.checked_mul(SCALE) {
            return Some(Self {
                raw: scaled / rhs.raw,
            });
        }
        // Slow path.
        wide_mul_div(self.raw, SCALE, rhs.raw)
    }

    // ── Saturating arithmetic ───────────────────────────────────────────

    #[inline]
    pub const fn saturating_add(self, rhs: Self) -> Self {
        Self {
            raw: self.raw.saturating_add(rhs.raw),
        }
    }

    #[inline]
    pub const fn saturating_sub(self, rhs: Self) -> Self {
        Self {
            raw: self.raw.saturating_sub(rhs.raw),
        }
    }

    // ── Min / Max ───────────────────────────────────────────────────────

    #[inline]
    pub const fn min(self, other: Self) -> Self {
        if self.raw <= other.raw {
            self
        } else {
            other
        }
    }

    #[inline]
    pub const fn max(self, other: Self) -> Self {
        if self.raw >= other.raw {
            self
        } else {
            other
        }
    }
}

// ── Wide multiply-then-divide helper ────────────────────────────────────

/// Compute `(a * b) / d` without intermediate overflow, using 256-bit
/// arithmetic emulated with two u128 halves.
///
/// Returns `None` if the final result does not fit in i128.
const fn wide_mul_div(a: i128, b: i128, d: i128) -> Option<Decimal128> {
    // Work with absolute values and track sign.
    let neg = (a < 0) ^ (b < 0) ^ (d < 0);
    let a_abs = a.unsigned_abs();
    let b_abs = b.unsigned_abs();
    let d_abs = d.unsigned_abs();

    // 256-bit product via four 64-bit parts.
    let (prod_hi, prod_lo) = mul_u128(a_abs, b_abs);

    // 256-bit / 128-bit division.
    let quot = div_u256_by_u128(prod_hi, prod_lo, d_abs);

    // quot must fit in i128.
    let raw = if neg {
        // Negative: quot up to 2^127 is representable (as i128::MIN).
        if quot > (i128::MAX as u128) + 1 {
            return None;
        }
        if quot == (i128::MAX as u128) + 1 {
            i128::MIN
        } else {
            -(quot as i128)
        }
    } else {
        if quot > i128::MAX as u128 {
            return None;
        }
        quot as i128
    };
    Some(Decimal128 { raw })
}

/// Full 128×128 → 256-bit unsigned multiply. Returns (hi, lo).
const fn mul_u128(a: u128, b: u128) -> (u128, u128) {
    let a_lo = a & 0xFFFF_FFFF_FFFF_FFFF;
    let a_hi = a >> 64;
    let b_lo = b & 0xFFFF_FFFF_FFFF_FFFF;
    let b_hi = b >> 64;

    let ll = a_lo * b_lo;
    let lh = a_lo * b_hi;
    let hl = a_hi * b_lo;
    let hh = a_hi * b_hi;

    // mid = lh + hl, but may overflow u128 — track carry.
    let mid_sum = lh.wrapping_add(hl);
    let mid_carry: u128 = if mid_sum < lh { 1 } else { 0 };

    let lo = ll.wrapping_add(mid_sum << 64);
    let carry_from_lo: u128 = if lo < ll { 1 } else { 0 };

    let hi = hh
        .wrapping_add(mid_sum >> 64)
        .wrapping_add(mid_carry << 64)
        .wrapping_add(carry_from_lo);
    (hi, lo)
}

/// 256-bit / 128-bit unsigned division → quotient as u128.
///
/// Uses binary long division (shift-and-subtract).
/// If the true quotient exceeds u128, returns `u128::MAX`.
const fn div_u256_by_u128(hi: u128, lo: u128, d: u128) -> u128 {
    if d == 0 {
        panic!("division by zero in div_u256_by_u128");
    }
    if hi == 0 {
        return lo / d;
    }

    // If hi >= d the quotient won't fit in u128.
    if hi >= d {
        return u128::MAX;
    }

    // Use binary long division (shift-and-subtract).
    // We know the quotient fits in u128 since hi < d.
    //
    // The 256-bit dividend is (hi, lo). We find q such that:
    //   (hi, lo) = q * d + r, where 0 <= r < d.
    //
    // Process one bit at a time from the most significant bit of the quotient.
    // Since hi < d, we know the quotient is at most 128 bits.
    // We iterate over bits 127..=0 of the quotient.
    let mut rem: u128 = hi;
    let mut quot: u128 = 0;

    // Binary long division: shift bits of `lo` into remainder one at a time.
    let mut i: u32 = 128;
    while i > 0 {
        i -= 1;

        // Shift remainder left by 1 and bring in the next bit from lo.
        let bit = (lo >> i) & 1;

        // rem = rem * 2 + bit, but we need to handle potential overflow of rem*2.
        // Since rem < d <= u128::MAX, rem*2 could overflow u128.
        let overflow = rem >= (1u128 << 127); // will rem*2 overflow?
        rem = rem.wrapping_shl(1) | bit;

        // If there was overflow in the shift, or rem >= d, subtract d.
        if overflow || rem >= d {
            rem = rem.wrapping_sub(d);
            quot |= 1u128 << i;
        }
    }

    quot
}

// ── Ord / PartialOrd ────────────────────────────────────────────────────

impl PartialOrd for Decimal128 {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Decimal128 {
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        self.raw.cmp(&other.raw)
    }
}

// ── Operator traits ─────────────────────────────────────────────────────

impl Add for Decimal128 {
    type Output = Self;

    #[inline]
    fn add(self, rhs: Self) -> Self {
        // In debug mode this panics on overflow (via i128::add).
        Self {
            raw: self.raw + rhs.raw,
        }
    }
}

impl Sub for Decimal128 {
    type Output = Self;

    #[inline]
    fn sub(self, rhs: Self) -> Self {
        Self {
            raw: self.raw - rhs.raw,
        }
    }
}

impl Mul for Decimal128 {
    type Output = Self;

    #[inline]
    fn mul(self, rhs: Self) -> Self {
        self.checked_mul(rhs).expect("Decimal128 multiplication overflow")
    }
}

impl Div for Decimal128 {
    type Output = Self;

    #[inline]
    fn div(self, rhs: Self) -> Self {
        self.checked_div(rhs).expect("Decimal128 division by zero or overflow")
    }
}

impl Neg for Decimal128 {
    type Output = Self;

    #[inline]
    fn neg(self) -> Self {
        Self { raw: -self.raw }
    }
}

// ── Sum ─────────────────────────────────────────────────────────────────

impl Sum for Decimal128 {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Self::ZERO, |acc, x| acc + x)
    }
}

impl<'a> Sum<&'a Decimal128> for Decimal128 {
    fn sum<I: Iterator<Item = &'a Self>>(iter: I) -> Self {
        iter.fold(Self::ZERO, |acc, x| acc + *x)
    }
}

// ── Display ─────────────────────────────────────────────────────────────

impl fmt::Display for Decimal128 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.raw == 0 {
            return f.write_str("0");
        }

        let negative = self.raw < 0;
        let abs = self.raw.unsigned_abs();

        let integer_part = abs / SCALE as u128;
        let frac_part = abs % SCALE as u128;

        if negative {
            f.write_str("-")?;
        }
        write!(f, "{integer_part}")?;

        if frac_part != 0 {
            // Format fractional part with leading zeros, then trim trailing zeros.
            let frac_str = format!("{:018}", frac_part);
            let trimmed = frac_str.trim_end_matches('0');
            write!(f, ".{trimmed}")?;
        }
        Ok(())
    }
}

// ── FromStr ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseDecimalError(String);

impl fmt::Display for ParseDecimalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid decimal: {}", self.0)
    }
}

impl std::error::Error for ParseDecimalError {}

impl FromStr for Decimal128 {
    type Err = ParseDecimalError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if s.is_empty() {
            return Err(ParseDecimalError("empty string".into()));
        }

        let (negative, s) = if let Some(rest) = s.strip_prefix('-') {
            if rest.starts_with('+') || rest.starts_with('-') {
                return Err(ParseDecimalError("invalid sign".into()));
            }
            (true, rest)
        } else if let Some(rest) = s.strip_prefix('+') {
            if rest.starts_with('+') || rest.starts_with('-') {
                return Err(ParseDecimalError("invalid sign".into()));
            }
            (false, rest)
        } else {
            (false, s)
        };

        if s.is_empty() {
            return Err(ParseDecimalError("no digits".into()));
        }

        let (int_str, frac_str) = match s.find('.') {
            Some(pos) => (&s[..pos], &s[pos + 1..]),
            None => (s, ""),
        };

        // Parse integer part.
        let int_val: i128 = if int_str.is_empty() {
            0
        } else {
            int_str
                .parse::<i128>()
                .map_err(|e| ParseDecimalError(e.to_string()))?
        };

        // Parse fractional part — pad or truncate to 18 digits.
        let frac_val: i128 = if frac_str.is_empty() {
            0
        } else {
            if frac_str.len() > SCALE_DIGITS as usize {
                // Truncate to 18 digits.
                let truncated = &frac_str[..SCALE_DIGITS as usize];
                truncated
                    .parse::<i128>()
                    .map_err(|e| ParseDecimalError(e.to_string()))?
            } else {
                // Right-pad with zeros.
                let mut padded = String::with_capacity(SCALE_DIGITS as usize);
                padded.push_str(frac_str);
                for _ in 0..(SCALE_DIGITS as usize - frac_str.len()) {
                    padded.push('0');
                }
                padded
                    .parse::<i128>()
                    .map_err(|e| ParseDecimalError(e.to_string()))?
            }
        };

        let raw = int_val
            .checked_mul(SCALE)
            .and_then(|v| v.checked_add(frac_val))
            .ok_or_else(|| ParseDecimalError("value out of range".into()))?;

        Ok(Self {
            raw: if negative { -raw } else { raw },
        })
    }
}

// ── From<i64> ───────────────────────────────────────────────────────────

impl From<i64> for Decimal128 {
    #[inline]
    fn from(v: i64) -> Self {
        Self {
            raw: (v as i128) * SCALE,
        }
    }
}

// ── Serde (serialize as decimal string) ─────────────────────────────────

impl Serialize for Decimal128 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Decimal128 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::from_str(&s).map_err(serde::de::Error::custom)
    }
}

/// Convert from f64. WARNING: Only for testing — f64 cannot represent all Decimal128 values.
#[cfg(test)]
impl From<f64> for Decimal128 {
    fn from(v: f64) -> Self {
        // Multiply by SCALE and round to nearest integer
        let raw = (v * SCALE as f64).round() as i128;
        Self { raw }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    fn dec(s: &str) -> Decimal128 {
        s.parse().unwrap()
    }

    // ── Constants ────────────────────────────────────────────────────

    #[test]
    fn test_constants() {
        assert_eq!(Decimal128::ZERO.raw(), 0);
        assert_eq!(Decimal128::ONE.raw(), SCALE);
        assert_eq!(Decimal128::MAX.raw(), i128::MAX);
        assert_eq!(Decimal128::MIN.raw(), i128::MIN);
    }

    #[test]
    fn test_default_is_zero() {
        assert_eq!(Decimal128::default(), Decimal128::ZERO);
    }

    // ── Predicates ──────────────────────────────────────────────────

    #[test]
    fn test_is_zero() {
        assert!(Decimal128::ZERO.is_zero());
        assert!(!Decimal128::ONE.is_zero());
    }

    #[test]
    fn test_sign_predicates() {
        assert!(dec("5").is_positive());
        assert!(!dec("5").is_negative());
        assert!(dec("-3").is_negative());
        assert!(!dec("-3").is_positive());
        assert!(!Decimal128::ZERO.is_positive());
        assert!(!Decimal128::ZERO.is_negative());
    }

    #[test]
    fn test_abs() {
        assert_eq!(dec("-7.5").abs(), dec("7.5"));
        assert_eq!(dec("7.5").abs(), dec("7.5"));
        assert_eq!(Decimal128::ZERO.abs(), Decimal128::ZERO);
    }

    // ── From<i64> ───────────────────────────────────────────────────

    #[test]
    fn test_from_i64() {
        assert_eq!(Decimal128::from(0i64), Decimal128::ZERO);
        assert_eq!(Decimal128::from(1i64), Decimal128::ONE);
        assert_eq!(Decimal128::from(-42i64), dec("-42"));
    }

    // ── Ordering ────────────────────────────────────────────────────

    #[test]
    fn test_ordering() {
        assert!(dec("1") < dec("2"));
        assert!(dec("-1") < dec("0"));
        assert!(dec("0.999999999999999999") < Decimal128::ONE);
        assert_eq!(dec("100"), dec("100"));
    }

    // ── Addition / Subtraction ──────────────────────────────────────

    #[test]
    fn test_add() {
        assert_eq!(dec("1") + dec("2"), dec("3"));
        assert_eq!(dec("-1") + dec("1"), Decimal128::ZERO);
        assert_eq!(dec("0.1") + dec("0.2"), dec("0.3"));
    }

    #[test]
    fn test_sub() {
        assert_eq!(dec("5") - dec("3"), dec("2"));
        assert_eq!(dec("0") - dec("1"), dec("-1"));
        assert_eq!(dec("0.3") - dec("0.1"), dec("0.2"));
    }

    #[test]
    fn test_precision_exact() {
        // The classic floating-point trap — must be exact here.
        let a = dec("0.1");
        let b = dec("0.2");
        let c = dec("0.3");
        assert_eq!(a + b, c);
    }

    #[test]
    fn test_checked_add_overflow() {
        assert!(Decimal128::MAX.checked_add(Decimal128::ONE).is_none());
    }

    #[test]
    fn test_checked_sub_overflow() {
        assert!(Decimal128::MIN.checked_sub(Decimal128::ONE).is_none());
    }

    #[test]
    fn test_saturating_add() {
        assert_eq!(
            Decimal128::MAX.saturating_add(Decimal128::ONE),
            Decimal128::MAX
        );
    }

    #[test]
    fn test_saturating_sub() {
        assert_eq!(
            Decimal128::MIN.saturating_sub(Decimal128::ONE),
            Decimal128::MIN
        );
    }

    // ── Multiplication ──────────────────────────────────────────────

    #[test]
    fn test_mul_basic() {
        assert_eq!(dec("2") * dec("3"), dec("6"));
        assert_eq!(dec("0.5") * dec("4"), dec("2"));
        assert_eq!(dec("-3") * dec("7"), dec("-21"));
        assert_eq!(dec("-2") * dec("-5"), dec("10"));
    }

    #[test]
    fn test_mul_fractions() {
        assert_eq!(dec("0.1") * dec("0.1"), dec("0.01"));
        assert_eq!(dec("1.5") * dec("1.5"), dec("2.25"));
    }

    #[test]
    fn test_mul_by_zero() {
        assert_eq!(dec("12345.6789") * Decimal128::ZERO, Decimal128::ZERO);
    }

    #[test]
    fn test_mul_by_one() {
        let v = dec("99999.123456789");
        assert_eq!(v * Decimal128::ONE, v);
    }

    #[test]
    fn test_checked_mul_large_values() {
        // 10^9 * 10^9 = 10^18. Raw values: (10^27) * (10^27) / 10^18 = 10^36 — fits in i128.
        let a = dec("1000000000"); // 10^9
        let b = dec("1000000000"); // 10^9
        let result = a.checked_mul(b);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), dec("1000000000000000000")); // 10^18
    }

    #[test]
    fn test_checked_mul_overflow_returns_none() {
        // Two values whose product exceeds i128 range even after scaling back.
        let huge = Decimal128::from_raw(i128::MAX);
        assert!(huge.checked_mul(Decimal128::from(2)).is_none());
    }

    // ── Division ────────────────────────────────────────────────────

    #[test]
    fn test_div_basic() {
        assert_eq!(dec("6") / dec("3"), dec("2"));
        assert_eq!(dec("10") / dec("4"), dec("2.5"));
        assert_eq!(dec("-10") / dec("2"), dec("-5"));
        assert_eq!(dec("10") / dec("-2"), dec("-5"));
    }

    #[test]
    fn test_div_fractions() {
        assert_eq!(dec("1") / dec("3") * dec("3"), dec("0.999999999999999999"));
    }

    #[test]
    fn test_checked_div_by_zero() {
        assert!(dec("1").checked_div(Decimal128::ZERO).is_none());
    }

    #[test]
    #[should_panic]
    fn test_div_by_zero_panics() {
        let _ = dec("1") / Decimal128::ZERO;
    }

    #[test]
    fn test_div_one() {
        let v = dec("123.456");
        assert_eq!(v / Decimal128::ONE, v);
    }

    // ── Mul then div roundtrip ──────────────────────────────────────

    #[test]
    fn test_mul_div_roundtrip() {
        // Use values small enough that both mul and div stay on the fast path.
        let a = dec("123.456");
        let b = dec("7.89");
        let result = (a * b) / b;
        // Allow 1 ULP difference.
        let diff = (result - a).abs();
        assert!(
            diff.raw() <= 1,
            "roundtrip error too large: diff = {}",
            diff
        );
    }

    // ── Negation ────────────────────────────────────────────────────

    #[test]
    fn test_neg() {
        assert_eq!(-dec("5"), dec("-5"));
        assert_eq!(-dec("-3.14"), dec("3.14"));
        assert_eq!(-Decimal128::ZERO, Decimal128::ZERO);
    }

    // ── Min / Max ───────────────────────────────────────────────────

    #[test]
    fn test_min_max() {
        let a = dec("3");
        let b = dec("7");
        assert_eq!(a.min(b), a);
        assert_eq!(a.max(b), b);
    }

    // ── Sum ─────────────────────────────────────────────────────────

    #[test]
    fn test_sum() {
        let values = vec![dec("1"), dec("2"), dec("3")];
        let total: Decimal128 = values.into_iter().sum();
        assert_eq!(total, dec("6"));
    }

    #[test]
    fn test_sum_refs() {
        let values = vec![dec("10"), dec("20")];
        let total: Decimal128 = values.iter().sum();
        assert_eq!(total, dec("30"));
    }

    // ── Display ─────────────────────────────────────────────────────

    #[test]
    fn test_display_integer() {
        assert_eq!(dec("42").to_string(), "42");
        assert_eq!(dec("-100").to_string(), "-100");
    }

    #[test]
    fn test_display_fractional() {
        assert_eq!(dec("1.5").to_string(), "1.5");
        assert_eq!(dec("0.001").to_string(), "0.001");
        assert_eq!(dec("-0.1").to_string(), "-0.1");
    }

    #[test]
    fn test_display_zero() {
        assert_eq!(Decimal128::ZERO.to_string(), "0");
    }

    #[test]
    fn test_display_trailing_zeros_trimmed() {
        assert_eq!(dec("1.100").to_string(), "1.1");
        assert_eq!(dec("3.000000000000000001").to_string(), "3.000000000000000001");
    }

    // ── FromStr ─────────────────────────────────────────────────────

    #[test]
    fn test_fromstr_integer() {
        assert_eq!(dec("0"), Decimal128::ZERO);
        assert_eq!(dec("1"), Decimal128::ONE);
        assert_eq!(dec("-42"), Decimal128::from(-42i64));
    }

    #[test]
    fn test_fromstr_decimal() {
        assert_eq!(dec("1.5").raw(), 1_500_000_000_000_000_000);
        assert_eq!(dec("0.000000000000000001").raw(), 1);
    }

    #[test]
    fn test_fromstr_leading_dot() {
        assert_eq!(dec(".5"), dec("0.5"));
    }

    #[test]
    fn test_fromstr_invalid() {
        assert!("".parse::<Decimal128>().is_err());
        assert!("abc".parse::<Decimal128>().is_err());
        assert!("1.2.3".parse::<Decimal128>().is_err());
        assert!("--1".parse::<Decimal128>().is_err());
    }

    // ── Serde ───────────────────────────────────────────────────────

    #[test]
    fn test_serde_roundtrip() {
        let v = dec("123.456");
        let json = serde_json::to_string(&v).unwrap();
        assert_eq!(json, "\"123.456\"");
        let parsed: Decimal128 = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, v);
    }

    #[test]
    fn test_serde_negative() {
        let v = dec("-0.001");
        let json = serde_json::to_string(&v).unwrap();
        let parsed: Decimal128 = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, v);
    }

    #[test]
    fn test_serde_zero() {
        let v = Decimal128::ZERO;
        let json = serde_json::to_string(&v).unwrap();
        assert_eq!(json, "\"0\"");
        let parsed: Decimal128 = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, v);
    }

    // ── Wide multiply (large value edge cases) ──────────────────────

    #[test]
    fn test_wide_mul_moderate() {
        // 100_000 * 100_000 = 10^10 — raw values fit via wide path.
        let a = dec("100000");
        let b = dec("100000");
        let result = a.checked_mul(b);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), dec("10000000000"));
    }

    #[test]
    fn test_checked_div_large() {
        // 10^9 / 10^-18 = 10^27 — but raw = 10^27 * 10^18 = 10^45, overflows i128.
        let a = Decimal128::from(1_000_000_000i64);
        let b = dec("0.000000000000000001"); // 1 ULP
        let result = a.checked_div(b);
        assert!(result.is_none());
    }

    #[test]
    fn test_checked_div_large_ok() {
        // 1000 / 0.001 = 1_000_000 — fits fine.
        let a = dec("1000");
        let b = dec("0.001");
        let result = a.checked_div(b);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), dec("1000000"));
    }

    // ── From<f64> ───────────────────────────────────────────────────

    #[test]
    fn test_from_f64_basic() {
        let v = Decimal128::from(1.5f64);
        assert_eq!(v, dec("1.5"));
    }

    #[test]
    fn test_from_f64_zero() {
        let v = Decimal128::from(0.0f64);
        assert_eq!(v, Decimal128::ZERO);
    }

    #[test]
    fn test_from_f64_negative() {
        let v = Decimal128::from(-42.25f64);
        assert_eq!(v, dec("-42.25"));
    }

    #[test]
    fn test_from_f64_integer() {
        let v = Decimal128::from(100.0f64);
        assert_eq!(v, dec("100"));
    }
}
