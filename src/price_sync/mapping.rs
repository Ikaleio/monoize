use super::*;

/// MP-Y4..MP-Y9: models.dev grouped-variant mapping.
pub fn map_models_dev(root: &Value) -> Result<Vec<SyncCandidate>, String> {
    let grouped = group_models_dev_variants(root)?;
    let mut candidates = Vec::new();
    for (model_id, variants) in &grouped {
        // MP-Y7: models without any strictly positive input cost are skipped.
        if !has_positive_input_variant(variants) {
            continue;
        }
        let winner = pick_best_variant(model_id, variants);
        let mut providers_map = serde_json::Map::new();
        for variant in variants {
            providers_map.insert(variant.provider_id.clone(), variant.raw.clone());
        }
        let billing_expr = models_dev_billing_expr(model_id, winner)?;
        candidates.push(SyncCandidate {
            model_id: model_id.clone(),
            billing_mode: if billing_expr.is_some() {
                "tiered_expr".to_string()
            } else {
                "per_token".to_string()
            },
            input_usd_per_1m: winner.cost_input.clone(),
            output_usd_per_1m: winner.cost_output.clone(),
            cache_read_usd_per_1m: winner.cost_cache_read.clone(),
            cache_write_usd_per_1m: winner.cost_cache_write.clone(),
            reasoning_usd_per_1m: winner.cost_reasoning.clone(),
            per_request_usd: None,
            billing_expr,
            raw_json: json!({ "providers": Value::Object(providers_map) }),
        });
    }
    Ok(candidates)
}

fn put_tier_price(
    target: &mut serde_json::Map<String, Value>,
    target_field: &str,
    raw: Option<&Value>,
    model_id: &str,
) -> Result<(), String> {
    let Some(raw) = raw else {
        return Ok(());
    };
    let value = usd_cost_string(raw).ok_or_else(|| {
        format!("parse_failed: models.dev model {model_id} has invalid tier price {target_field}")
    })?;
    target.insert(target_field.to_string(), Value::String(value));
    Ok(())
}

const MODELS_DEV_TIER_FIELDS: &[(&str, &str)] = &[
    ("input", "input_usd_per_1m"),
    ("output", "output_usd_per_1m"),
    ("cache_read", "cache_read_usd_per_1m"),
    ("cache_write", "cache_write_usd_per_1m"),
    ("reasoning", "reasoning_usd_per_1m"),
];

fn cost_field_map(cost: &serde_json::Map<String, Value>) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for (source, target) in MODELS_DEV_TIER_FIELDS {
        if let Some(value) = cost.get(*source).and_then(usd_cost_string) {
            map.insert(target.to_string(), value);
        }
    }
    map
}

fn winner_base_prices(
    winner: &crate::model_registry_store::SyncProviderVariant,
) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for (field, value) in [
        ("input_usd_per_1m", winner.cost_input.as_ref()),
        ("output_usd_per_1m", winner.cost_output.as_ref()),
        ("cache_read_usd_per_1m", winner.cost_cache_read.as_ref()),
        ("cache_write_usd_per_1m", winner.cost_cache_write.as_ref()),
        ("reasoning_usd_per_1m", winner.cost_reasoning.as_ref()),
    ] {
        if let Some(value) = value {
            map.insert(field.to_string(), value.clone());
        }
    }
    map
}

fn unbounded_tier_from_prices(prices: &BTreeMap<String, String>) -> Option<Value> {
    if !prices.contains_key("input_usd_per_1m") {
        return None;
    }
    let mut tier = serde_json::Map::new();
    for (field, value) in prices {
        tier.insert(field.clone(), Value::String(value.clone()));
    }
    Some(Value::Object(tier))
}

fn expression_tiers_from_models_dev_cost(
    model_id: &str,
    cost: &serde_json::Map<String, Value>,
    base_prices: &BTreeMap<String, String>,
    path: &str,
) -> Result<Option<Vec<Value>>, String> {
    let source_tiers = match cost.get("tiers") {
        None => return Ok(None),
        Some(Value::Array(tiers)) if tiers.is_empty() => return Ok(None),
        Some(Value::Array(tiers)) => tiers,
        Some(_) => {
            return Err(format!(
                "parse_failed: models.dev model {model_id} {path} is not an array"
            ));
        }
    };
    if source_tiers.len() > 7 {
        return Err(format!(
            "parse_failed: models.dev model {model_id} {path} has more than 7 cost tiers"
        ));
    }
    if !base_prices.contains_key("input_usd_per_1m") {
        return Err(format!(
            "parse_failed: models.dev model {model_id} {path} is missing a base input price"
        ));
    }

    let mut parsed = Vec::with_capacity(source_tiers.len());
    for (index, source_tier) in source_tiers.iter().enumerate() {
        let tier = source_tier.as_object().ok_or_else(|| {
            format!("parse_failed: models.dev model {model_id} {path}[{index}] is not an object")
        })?;
        let selector = tier.get("tier").and_then(Value::as_object).ok_or_else(|| {
            format!(
                "parse_failed: models.dev model {model_id} {path}[{index}].tier is not an object"
            )
        })?;
        if selector.get("type").and_then(Value::as_str) != Some("context") {
            return Err(format!(
                "parse_failed: models.dev model {model_id} {path}[{index}] is not a context tier"
            ));
        }
        let size = selector
            .get("size")
            .and_then(Value::as_u64)
            .filter(|size| *size >= 1)
            .ok_or_else(|| {
                format!(
                    "parse_failed: models.dev model {model_id} {path}[{index}].tier.size is not an integer >= 1"
                )
            })?;
        let mut mapped = serde_json::Map::new();
        for (source, target) in MODELS_DEV_TIER_FIELDS {
            put_tier_price(&mut mapped, target, tier.get(*source), model_id)?;
        }
        if !mapped.contains_key("input_usd_per_1m") {
            return Err(format!(
                "parse_failed: models.dev model {model_id} {path}[{index}] has no input price"
            ));
        }
        parsed.push((size, mapped));
    }
    parsed.sort_by_key(|(size, _)| *size);
    if parsed.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return Err(format!(
            "parse_failed: models.dev model {model_id} {path} has duplicate cost tier sizes"
        ));
    }

    let mut base = serde_json::Map::new();
    base.insert(
        "when_input_tokens_lte".to_string(),
        Value::from(parsed[0].0),
    );
    for (field, value) in base_prices {
        base.insert(field.clone(), Value::String(value.clone()));
    }

    let mut tiers = vec![Value::Object(base)];
    for index in 0..parsed.len() {
        let mut tier = parsed[index].1.clone();
        if let Some((next_size, _)) = parsed.get(index + 1) {
            tier.insert("when_input_tokens_lte".to_string(), Value::from(*next_size));
        }
        tiers.push(Value::Object(tier));
    }
    Ok(Some(tiers))
}

fn strictly_positive_decimal(raw: &str) -> Option<Decimal> {
    Decimal::from_str_exact(raw)
        .ok()
        .filter(|value| value.is_sign_positive() && !value.is_zero())
}

fn scale_price_field(
    standard_tier_value: &str,
    standard_base: &str,
    mode_base: &str,
) -> Option<String> {
    let standard_tier = strictly_positive_decimal(standard_tier_value)?;
    let standard_base = strictly_positive_decimal(standard_base)?;
    let mode_base = strictly_positive_decimal(mode_base)?;
    let scaled = standard_tier
        .checked_mul(mode_base)
        .and_then(|value| value.checked_div(standard_base))?;
    Some(trunc9(scaled))
}

fn scale_expression_tiers(
    standard_tiers: &[Value],
    standard_base: &BTreeMap<String, String>,
    mode_base: &BTreeMap<String, String>,
    model_id: &str,
    mode: &str,
) -> Result<Vec<Value>, String> {
    let standard_input = standard_base.get("input_usd_per_1m").ok_or_else(|| {
        format!(
            "parse_failed: models.dev model {model_id} cannot scale {mode} without a standard input price"
        )
    })?;
    let mode_input = mode_base.get("input_usd_per_1m").ok_or_else(|| {
        format!("parse_failed: models.dev model {model_id} mode {mode} has no input price")
    })?;
    if strictly_positive_decimal(standard_input).is_none()
        || strictly_positive_decimal(mode_input).is_none()
    {
        return Err(format!(
            "parse_failed: models.dev model {model_id} mode {mode} input price cannot scale"
        ));
    }
    let mut scaled = Vec::with_capacity(standard_tiers.len());
    for (index, tier) in standard_tiers.iter().enumerate() {
        let tier = tier.as_object().ok_or_else(|| {
            format!(
                "parse_failed: models.dev model {model_id} standard tier {index} is not an object"
            )
        })?;
        let mut mapped = serde_json::Map::new();
        if let Some(bound) = tier.get("when_input_tokens_lte") {
            mapped.insert("when_input_tokens_lte".to_string(), bound.clone());
        }
        for (_, field) in MODELS_DEV_TIER_FIELDS {
            let Some(Value::String(standard_value)) = tier.get(*field) else {
                continue;
            };
            let Some(standard_base_value) = standard_base.get(*field) else {
                continue;
            };
            let Some(mode_base_value) = mode_base.get(*field) else {
                continue;
            };
            let Some(value) =
                scale_price_field(standard_value, standard_base_value, mode_base_value)
            else {
                continue;
            };
            mapped.insert((*field).to_string(), Value::String(value));
        }
        if !mapped.contains_key("input_usd_per_1m") {
            return Err(format!(
                "parse_failed: models.dev model {model_id} mode {mode} tier {index} has no scaled input price"
            ));
        }
        scaled.push(Value::Object(mapped));
    }
    Ok(scaled)
}

fn canonicalize_service_mode_name(name: &str) -> Option<String> {
    let key = name.trim().to_ascii_lowercase();
    if key == "priority" {
        return Some("fast".to_string());
    }
    is_service_tier_key(&key).then_some(key)
}

fn service_tier_tables(
    model_id: &str,
    winner: &crate::model_registry_store::SyncProviderVariant,
    standard_tiers: &[Value],
    standard_base: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, Vec<Value>>, String> {
    let Some(modes) = winner
        .raw
        .get("experimental")
        .and_then(|value| value.get("modes"))
        .and_then(Value::as_object)
    else {
        return Ok(BTreeMap::new());
    };
    let mut tables = BTreeMap::new();
    for (name, mode) in modes {
        let Some(cost) = mode.get("cost").and_then(Value::as_object) else {
            continue;
        };
        let Some(key) = canonicalize_service_mode_name(name) else {
            continue;
        };
        if key == "fast" && name != "fast" && tables.contains_key("fast") {
            continue;
        }
        let mode_base = cost_field_map(cost);
        if !mode_base.contains_key("input_usd_per_1m") {
            return Err(format!(
                "parse_failed: models.dev model {model_id} mode {name} has no input price"
            ));
        }
        let path = format!("experimental.modes.{name}.cost.tiers");
        let tiers = if let Some(tiers) =
            expression_tiers_from_models_dev_cost(model_id, cost, &mode_base, &path)?
        {
            tiers
        } else if standard_tiers.len() >= 2 {
            scale_expression_tiers(standard_tiers, standard_base, &mode_base, model_id, &key)?
        } else {
            vec![unbounded_tier_from_prices(&mode_base).ok_or_else(|| {
                format!("parse_failed: models.dev model {model_id} mode {name} has no input price")
            })?]
        };
        tables.insert(key, tiers);
    }
    Ok(tables)
}

fn models_dev_billing_expr(
    model_id: &str,
    winner: &crate::model_registry_store::SyncProviderVariant,
) -> Result<Option<Value>, String> {
    let cost = winner.raw.get("cost").and_then(Value::as_object);
    let standard_base = winner_base_prices(winner);
    let mut standard_tiers = match cost {
        Some(cost) => {
            expression_tiers_from_models_dev_cost(model_id, cost, &standard_base, "cost.tiers")?
        }
        None => None,
    };
    let has_service_modes = winner
        .raw
        .get("experimental")
        .and_then(|value| value.get("modes"))
        .and_then(Value::as_object)
        .is_some_and(|modes| modes.values().any(|mode| mode.get("cost").is_some()));
    if standard_tiers.is_none() && !has_service_modes {
        return Ok(None);
    }
    if standard_tiers.is_none() {
        standard_tiers = Some(vec![unbounded_tier_from_prices(&standard_base).ok_or_else(
            || {
                format!(
                    "parse_failed: models.dev model {model_id} needs a base input price for service tiers"
                )
            },
        )?]);
    }
    let standard_tiers = standard_tiers.ok_or_else(|| {
        format!("parse_failed: models.dev model {model_id} missing standard price tiers")
    })?;
    let service_tables = service_tier_tables(model_id, winner, &standard_tiers, &standard_base)?;
    let mut expr_object = serde_json::Map::new();
    expr_object.insert("tiers".to_string(), Value::Array(standard_tiers));
    if !service_tables.is_empty() {
        let mut object = serde_json::Map::new();
        for (name, tiers) in service_tables {
            object.insert(name, json!({ "tiers": tiers }));
        }
        expr_object.insert("service_tiers".to_string(), Value::Object(object));
    }
    let expr = Value::Object(expr_object);
    validate_billing_expr(&expr).map_err(|error| {
        format!("parse_failed: models.dev model {model_id} produced invalid billing_expr: {error}")
    })?;
    Ok(Some(expr))
}

fn per_token_to_per_1m(value: &Value) -> Option<String> {
    let raw = match value {
        Value::Number(number) => number.to_string(),
        Value::String(value) => value.clone(),
        _ => return None,
    };
    if raw.contains(['e', 'E']) || raw.starts_with('+') {
        return None;
    }
    let per_token = Decimal::from_str_exact(&raw).ok()?;
    if per_token.is_sign_negative() {
        return None;
    }
    let per_1m = per_token.checked_mul(Decimal::from(1_000_000u32))?;
    Some(
        per_1m
            .round_dp_with_strategy(9, rust_decimal::RoundingStrategy::ToZero)
            .normalize()
            .to_string(),
    )
}

fn decimal_is_positive(raw: Option<&String>) -> bool {
    raw.and_then(|value| Decimal::from_str(value).ok())
        .is_some_and(|value| value.is_sign_positive() && !value.is_zero())
}

/// MP-Y11: OpenRouter per-token USD strings scale to USD per 1M exactly.
/// A model with non-positive prompt and completion prices is skipped. When
/// two entries normalize to the same canonical id, the higher prompt price
/// wins (same resale-loss rationale as MP-Y7).
pub fn map_openrouter(root: &Value) -> Result<Vec<SyncCandidate>, String> {
    let data = root
        .get("data")
        .and_then(|data| data.as_array())
        .ok_or_else(|| "parse_failed: data must be an array".to_string())?;
    let mut by_model: BTreeMap<String, SyncCandidate> = BTreeMap::new();
    for entry in data {
        let Some(id) = entry.get("id").and_then(|id| id.as_str()) else {
            continue;
        };
        let vendor = id.split('/').next().filter(|vendor| *vendor != id);
        let model_id = normalize_model_id(id, vendor);
        let pricing = entry.get("pricing").and_then(|pricing| pricing.as_object());
        let input = pricing
            .and_then(|pricing| pricing.get("prompt"))
            .and_then(per_token_to_per_1m);
        let output = pricing
            .and_then(|pricing| pricing.get("completion"))
            .and_then(per_token_to_per_1m);
        if !decimal_is_positive(input.as_ref()) && !decimal_is_positive(output.as_ref()) {
            continue;
        }
        let candidate = SyncCandidate {
            model_id: model_id.clone(),
            billing_mode: "per_token".to_string(),
            input_usd_per_1m: input,
            output_usd_per_1m: output,
            cache_read_usd_per_1m: pricing
                .and_then(|pricing| pricing.get("input_cache_read"))
                .and_then(per_token_to_per_1m),
            cache_write_usd_per_1m: pricing
                .and_then(|pricing| pricing.get("input_cache_write"))
                .and_then(per_token_to_per_1m),
            reasoning_usd_per_1m: None,
            per_request_usd: None,
            billing_expr: None,
            raw_json: entry.clone(),
        };
        match by_model.entry(model_id) {
            std::collections::btree_map::Entry::Vacant(slot) => {
                slot.insert(candidate);
            }
            std::collections::btree_map::Entry::Occupied(mut slot) => {
                let existing = decimal_value(slot.get().input_usd_per_1m.as_ref());
                let incoming = decimal_value(candidate.input_usd_per_1m.as_ref());
                if incoming > existing {
                    slot.insert(candidate);
                }
            }
        }
    }
    Ok(by_model.into_values().collect())
}

fn decimal_value(raw: Option<&String>) -> Decimal {
    raw.and_then(|value| Decimal::from_str(value).ok())
        .unwrap_or(Decimal::ZERO)
}

fn json_decimal(value: Option<&Value>) -> Option<Decimal> {
    let raw = match value? {
        Value::Number(number) => number.to_string(),
        Value::String(value) => value.clone(),
        _ => return None,
    };
    if raw.contains(['e', 'E']) || raw.starts_with('+') {
        return None;
    }
    Decimal::from_str_exact(&raw).ok()
}

fn trunc9(value: Decimal) -> String {
    value
        .round_dp_with_strategy(9, rust_decimal::RoundingStrategy::ToZero)
        .normalize()
        .to_string()
}

/// MP-Y12/MP-Y12a: new-api `/api/pricing` mapping with untrusted-placeholder
/// skip. Ratio `1` equals USD 2 per 1M tokens; all arithmetic is exact
/// decimal truncated to 9 fractional digits.
pub fn map_new_api(root: &Value) -> Result<Vec<SyncCandidate>, String> {
    let data = root
        .get("data")
        .and_then(|data| data.as_array())
        .or_else(|| root.as_array())
        .ok_or_else(|| "parse_failed: data must be an array".to_string())?;
    let mut by_model: BTreeMap<String, SyncCandidate> = BTreeMap::new();
    for entry in data {
        let Some(model_id) = entry
            .get("model_name")
            .and_then(|name| name.as_str())
            .map(str::trim)
            .filter(|name| !name.is_empty())
        else {
            continue;
        };
        let quota_type = entry.get("quota_type").and_then(|value| value.as_i64());
        let candidate = match quota_type {
            Some(0) => {
                let Some(model_ratio) = json_decimal(entry.get("model_ratio")) else {
                    continue;
                };
                let completion_ratio =
                    json_decimal(entry.get("completion_ratio")).unwrap_or(Decimal::ONE);
                if model_ratio.is_sign_negative() || completion_ratio.is_sign_negative() {
                    continue;
                }
                // MP-Y12a: new-api reports unknown models with the default
                // placeholder ratio 37.5 (75 USD/1M) and completion ratio 1;
                // these prices are untrusted and skipped.
                if model_ratio == Decimal::new(375, 1) && completion_ratio == Decimal::ONE {
                    continue;
                }
                let input = model_ratio * Decimal::TWO;
                let output = input * completion_ratio;
                SyncCandidate {
                    model_id: model_id.to_string(),
                    billing_mode: "per_token".to_string(),
                    input_usd_per_1m: Some(trunc9(input)),
                    output_usd_per_1m: Some(trunc9(output)),
                    cache_read_usd_per_1m: None,
                    cache_write_usd_per_1m: None,
                    reasoning_usd_per_1m: None,
                    per_request_usd: None,
                    billing_expr: None,
                    raw_json: entry.clone(),
                }
            }
            Some(1) => {
                let Some(price) = json_decimal(entry.get("model_price")) else {
                    continue;
                };
                if price.is_sign_negative() {
                    continue;
                }
                SyncCandidate {
                    model_id: model_id.to_string(),
                    billing_mode: "per_request".to_string(),
                    input_usd_per_1m: None,
                    output_usd_per_1m: None,
                    cache_read_usd_per_1m: None,
                    cache_write_usd_per_1m: None,
                    reasoning_usd_per_1m: None,
                    per_request_usd: Some(trunc9(price)),
                    billing_expr: None,
                    raw_json: entry.clone(),
                }
            }
            _ => continue,
        };
        by_model.insert(candidate.model_id.clone(), candidate);
    }
    Ok(by_model.into_values().collect())
}
