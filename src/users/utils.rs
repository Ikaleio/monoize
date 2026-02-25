pub fn parse_nano_usd(value: &str) -> Result<i128, String> {
    value
        .trim()
        .parse::<i128>()
        .map_err(|_| "invalid_nano_usd".to_string())
}

pub fn parse_usd_to_nano(value: &str) -> Result<i128, String> {
    let s = value.trim();
    if s.is_empty() {
        return Err("invalid_usd".to_string());
    }
    let (negative, rest) = if let Some(rem) = s.strip_prefix('-') {
        (true, rem)
    } else if let Some(rem) = s.strip_prefix('+') {
        (false, rem)
    } else {
        (false, s)
    };

    let (whole_raw, frac_raw) = match rest.split_once('.') {
        Some((w, f)) => (w, f),
        None => (rest, ""),
    };
    if whole_raw.is_empty() && frac_raw.is_empty() {
        return Err("invalid_usd".to_string());
    }
    if !whole_raw.chars().all(|c| c.is_ascii_digit()) {
        return Err("invalid_usd".to_string());
    }
    if !frac_raw.chars().all(|c| c.is_ascii_digit()) {
        return Err("invalid_usd".to_string());
    }

    let whole = if whole_raw.is_empty() {
        0i128
    } else {
        whole_raw
            .parse::<i128>()
            .map_err(|_| "invalid_usd".to_string())?
    };
    let mut frac = frac_raw.to_string();
    if frac.len() > 9 {
        frac.truncate(9);
    }
    while frac.len() < 9 {
        frac.push('0');
    }
    let frac_value = if frac.is_empty() {
        0i128
    } else {
        frac.parse::<i128>()
            .map_err(|_| "invalid_usd".to_string())?
    };

    let base = whole
        .checked_mul(1_000_000_000)
        .and_then(|v| v.checked_add(frac_value))
        .ok_or_else(|| "usd_overflow".to_string())?;
    if negative {
        base.checked_neg().ok_or_else(|| "usd_overflow".to_string())
    } else {
        Ok(base)
    }
}

pub fn format_nano_to_usd(nano: i128) -> String {
    let negative = nano < 0;
    let abs = nano.abs();
    let whole = abs / 1_000_000_000;
    let frac = abs % 1_000_000_000;
    if frac == 0 {
        return if negative {
            format!("-{whole}")
        } else {
            whole.to_string()
        };
    }
    let mut frac_str = format!("{frac:09}");
    while frac_str.ends_with('0') {
        frac_str.pop();
    }
    if negative {
        format!("-{whole}.{frac_str}")
    } else {
        format!("{whole}.{frac_str}")
    }
}
