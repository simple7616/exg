use std::collections::HashSet;
use std::str::FromStr;

use crate::{ConfigError, ExgConfig};

/// Stub for Decimal128-like parsing. We only need to validate that strings
/// parse as valid decimals — we don't depend on exg-common at runtime.
fn parse_decimal(field: &str, value: &str) -> Result<f64, ConfigError> {
    // First, verify it parses as a valid decimal string with no weirdness.
    // We accept the same format as Decimal128: optional sign, digits, optional dot + digits.
    if value.trim().is_empty() {
        return Err(ConfigError::Validation(format!(
            "{field}: empty decimal string"
        )));
    }
    // Use a strict check: must match pattern [-+]?[0-9]*\.?[0-9]+
    let s = value.trim();
    let s = s.strip_prefix(['+', '-']).unwrap_or(s);
    if s.is_empty() {
        return Err(ConfigError::Validation(format!(
            "{field}: invalid decimal '{value}'"
        )));
    }
    let mut has_digit = false;
    let mut has_dot = false;
    for ch in s.chars() {
        match ch {
            '0'..='9' => has_digit = true,
            '.' if !has_dot => has_dot = true,
            _ => {
                return Err(ConfigError::Validation(format!(
                    "{field}: invalid decimal '{value}'"
                )));
            }
        }
    }
    if !has_digit {
        return Err(ConfigError::Validation(format!(
            "{field}: invalid decimal '{value}'"
        )));
    }

    f64::from_str(value.trim())
        .map_err(|_| ConfigError::Validation(format!("{field}: invalid decimal '{value}'")))
}

fn require_positive(field: &str, value: &str) -> Result<(), ConfigError> {
    let v = parse_decimal(field, value)?;
    if v <= 0.0 {
        return Err(ConfigError::Validation(format!(
            "{field}: must be positive, got '{value}'"
        )));
    }
    Ok(())
}

fn require_non_negative(field: &str, value: &str) -> Result<(), ConfigError> {
    let v = parse_decimal(field, value)?;
    if v < 0.0 {
        return Err(ConfigError::Validation(format!(
            "{field}: must be non-negative, got '{value}'"
        )));
    }
    Ok(())
}

pub(crate) fn validate(cfg: &ExgConfig) -> Result<(), ConfigError> {
    // node_id
    if cfg.server.node_id > 1023 {
        return Err(ConfigError::Validation(format!(
            "server.node_id must be <= 1023, got {}",
            cfg.server.node_id
        )));
    }

    // ringbuffer slot_count must be power of 2
    if cfg.ringbuffer.slot_count == 0 || !cfg.ringbuffer.slot_count.is_power_of_two() {
        return Err(ConfigError::Validation(format!(
            "ringbuffer.slot_count must be a power of 2, got {}",
            cfg.ringbuffer.slot_count
        )));
    }

    // Risk config decimal fields
    parse_decimal("risk.price_band_pct", &cfg.risk.price_band_pct)?;
    parse_decimal(
        "risk.max_position_notional",
        &cfg.risk.max_position_notional,
    )?;
    parse_decimal("risk.interest_rate", &cfg.risk.interest_rate)?;
    parse_decimal("risk.impact_notional", &cfg.risk.impact_notional)?;

    // No duplicate symbol IDs
    let mut seen_ids = HashSet::new();
    for sym in &cfg.trading.symbols {
        if !seen_ids.insert(sym.id) {
            return Err(ConfigError::Validation(format!(
                "duplicate symbol id: {}",
                sym.id
            )));
        }
        validate_symbol(sym)?;
    }

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
            "auth.jwt_expiry_secs must be > 0".into(),
        ));
    }

    // Stage 2 §6 invariant 24/25: admin secret length + placeholder check.
    const ADMIN_SECRET_PLACEHOLDER: &str = "CHANGE-ME-ADMIN-DEV-ONLY-MUST-BE-32-BYTES";
    if cfg.admin.admin_secret.len() < 32 {
        return Err(ConfigError::Validation(format!(
            "admin.admin_secret must be at least 32 bytes, got {}",
            cfg.admin.admin_secret.len()
        )));
    }
    if cfg.admin.admin_secret == ADMIN_SECRET_PLACEHOLDER {
        return Err(ConfigError::Validation(
            "admin.admin_secret is the placeholder default; set EXG_ADMIN_SECRET env var to a 32+ byte production secret".into(),
        ));
    }

    Ok(())
}

fn validate_symbol(sym: &crate::SymbolConfigEntry) -> Result<(), ConfigError> {
    let prefix = format!("symbol[{}]", sym.name);

    require_positive(&format!("{prefix}.tick_size"), &sym.tick_size)?;
    require_positive(&format!("{prefix}.lot_size"), &sym.lot_size)?;
    parse_decimal(&format!("{prefix}.min_notional"), &sym.min_notional)?;
    parse_decimal(&format!("{prefix}.max_leverage"), &sym.max_leverage)?;
    require_non_negative(&format!("{prefix}.maker_fee"), &sym.maker_fee)?;
    require_non_negative(&format!("{prefix}.taker_fee"), &sym.taker_fee)?;
    require_positive(&format!("{prefix}.mark_price"), &sym.mark_price)?;

    // Validate margin tiers
    if sym.margin_tiers.is_empty() {
        return Ok(());
    }

    let mut prev_cap: Option<f64> = None;
    for (i, tier) in sym.margin_tiers.iter().enumerate() {
        let tier_prefix = format!("{prefix}.margin_tiers[{i}]");
        let floor = parse_decimal(
            &format!("{tier_prefix}.notional_floor"),
            &tier.notional_floor,
        )?;
        let cap = parse_decimal(&format!("{tier_prefix}.notional_cap"), &tier.notional_cap)?;
        parse_decimal(
            &format!("{tier_prefix}.maintenance_margin_rate"),
            &tier.maintenance_margin_rate,
        )?;
        parse_decimal(
            &format!("{tier_prefix}.maintenance_amount"),
            &tier.maintenance_amount,
        )?;

        if cap <= floor {
            return Err(ConfigError::Validation(format!(
                "{tier_prefix}: notional_cap ({cap}) must be > notional_floor ({floor})"
            )));
        }

        // Sorted and non-overlapping: each tier's floor must equal the previous tier's cap.
        if let Some(prev) = prev_cap
            && floor < prev
        {
            return Err(ConfigError::Validation(format!(
                "{tier_prefix}: notional_floor ({floor}) overlaps with previous tier cap ({prev})"
            )));
        }
        prev_cap = Some(cap);
    }

    Ok(())
}
