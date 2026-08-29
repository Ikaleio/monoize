//! Exact decimal arithmetic for the recharge system (`recharge-system.spec.md`
//! RC-U1..RC-U7). Every amount is carried as a checked `i128` mantissa plus a
//! decimal scale; no value passes through `f32`/`f64`.

/// A positive decimal parsed into `mantissa * 10^-scale` form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PositiveDecimal {
    pub mantissa: i128,
    pub scale: u32,
}

/// RC-U5: parse a `usd_rate`-format decimal string: positive, base-10, at most
/// 12 integer digits and at most 9 fractional digits, no exponent, no leading
/// `+`, no leading zeros other than a single `0` before the decimal point.
pub fn parse_positive_decimal(raw: &str) -> Result<PositiveDecimal, String> {
    let (int_part, frac_part) = match raw.split_once('.') {
        Some((int_part, frac_part)) => (int_part, frac_part),
        None => (raw, ""),
    };
    let canonical_int = !int_part.is_empty()
        && int_part.bytes().all(|byte| byte.is_ascii_digit())
        && (int_part == "0" || !int_part.starts_with('0'));
    let canonical_frac = if raw.contains('.') {
        !frac_part.is_empty() && frac_part.bytes().all(|byte| byte.is_ascii_digit())
    } else {
        true
    };
    if !canonical_int || !canonical_frac || int_part.len() > 12 || frac_part.len() > 9 {
        return Err("invalid decimal".to_string());
    }
    let scale = frac_part.len() as u32;
    let mantissa = format!("{int_part}{frac_part}")
        .parse::<i128>()
        .map_err(|_| "invalid decimal".to_string())?;
    if mantissa <= 0 {
        return Err("decimal must be positive".to_string());
    }
    Ok(PositiveDecimal { mantissa, scale })
}

/// Exact conversion of a positive RC-U5-format USD decimal into nano-USD.
pub fn parse_positive_usd_to_nano(raw: &str) -> Result<i128, String> {
    let decimal = parse_positive_decimal(raw)?;
    let factor = 10_i128
        .checked_pow(9 - decimal.scale)
        .ok_or_else(|| "usd scale overflow".to_string())?;
    decimal
        .mantissa
        .checked_mul(factor)
        .ok_or_else(|| "usd overflow".to_string())
}

/// RC-O2 amount input: a canonical positive `i128` nano-USD decimal string
/// (no sign prefix, no leading zeros).
pub fn parse_canonical_positive_nano(raw: &str) -> Result<i128, String> {
    if raw.is_empty()
        || !raw.bytes().all(|byte| byte.is_ascii_digit())
        || (raw.len() > 1 && raw.starts_with('0'))
    {
        return Err("invalid nano amount".to_string());
    }
    let value = raw
        .parse::<i128>()
        .map_err(|_| "invalid nano amount".to_string())?;
    if value <= 0 {
        return Err("nano amount must be positive".to_string());
    }
    Ok(value)
}

/// RC-U6: `pay_amount = ceil_to_scale(credit_usd * usd_rate, scale)` computed
/// exactly on integers. Returns the integer count of minor payment units
/// (`pay_amount * 10^scale`).
///
/// `credit_usd * usd_rate = credit_nano * rate_mantissa * 10^-(9 + rate_scale)`,
/// so the minor-unit count is the ceiling of
/// `credit_nano * rate_mantissa / 10^(9 + rate_scale - scale)`.
pub fn pay_minor_units(
    credit_nano_usd: i128,
    usd_rate: PositiveDecimal,
    scale: u32,
) -> Result<i128, String> {
    if credit_nano_usd <= 0 {
        return Err("credit must be positive".to_string());
    }
    // RC-P4 bounds scale to {0, 2} and RC-U5 bounds rate scale to <= 9, so the
    // exponent stays in [7, 18] and the divisor always fits an i128.
    let exponent = (9 + usd_rate.scale)
        .checked_sub(scale)
        .ok_or_else(|| "pay scale exceeds precision".to_string())?;
    let divisor = 10_i128
        .checked_pow(exponent)
        .ok_or_else(|| "rate scale overflow".to_string())?;
    let product = credit_nano_usd
        .checked_mul(usd_rate.mantissa)
        .ok_or_else(|| "pay amount overflow".to_string())?;
    // Ceiling division of positive integers; RC-U7: the result is >= 1.
    let quotient = product / divisor;
    let units = if product % divisor == 0 {
        quotient
    } else {
        quotient
            .checked_add(1)
            .ok_or_else(|| "pay amount overflow".to_string())?
    };
    Ok(units.max(1))
}

/// RC-U4: render minor units as a canonical decimal string with exactly
/// `scale` fractional digits.
pub fn format_minor_units(units: i128, scale: u32) -> String {
    if scale == 0 {
        return units.to_string();
    }
    let divisor = 10_i128.pow(scale);
    let whole = units / divisor;
    let fraction = units % divisor;
    format!("{whole}.{fraction:0width$}", width = scale as usize)
}

/// RC-N5 numeric equality: both operands parse as non-negative decimals and
/// compare equal after scale normalization.
pub fn decimals_equal(left: &str, right: &str) -> bool {
    fn parts(raw: &str) -> Option<(i128, u32)> {
        let trimmed = raw.trim();
        let (int_part, frac_part) = match trimmed.split_once('.') {
            Some((int_part, frac_part)) => (int_part, frac_part),
            None => (trimmed, ""),
        };
        if int_part.is_empty() && frac_part.is_empty() {
            return None;
        }
        if !int_part.bytes().all(|byte| byte.is_ascii_digit())
            || !frac_part.bytes().all(|byte| byte.is_ascii_digit())
        {
            return None;
        }
        let frac_trimmed = frac_part.trim_end_matches('0');
        let mantissa = format!(
            "{}{}",
            if int_part.is_empty() { "0" } else { int_part },
            frac_trimmed
        )
        .parse::<i128>()
        .ok()?;
        Some((mantissa, frac_trimmed.len() as u32))
    }
    match (parts(left), parts(right)) {
        (Some((lm, ls)), Some((rm, rs))) => lm == rm && ls == rs,
        _ => false,
    }
}
