use super::*;

impl SyncPlan {
    fn insert_candidate(
        &mut self,
        source: SyncSource,
        candidate: SyncCandidate,
        now: chrono::DateTime<Utc>,
    ) {
        let after = row_price_values(
            &candidate.billing_mode,
            [
                ("input_usd_per_1m", &candidate.input_usd_per_1m),
                ("output_usd_per_1m", &candidate.output_usd_per_1m),
                ("cache_read_usd_per_1m", &candidate.cache_read_usd_per_1m),
                ("cache_write_usd_per_1m", &candidate.cache_write_usd_per_1m),
                ("reasoning_usd_per_1m", &candidate.reasoning_usd_per_1m),
                ("per_request_usd", &candidate.per_request_usd),
            ],
            &candidate.billing_expr,
        );
        self.inserted += 1;
        self.changes.push(SyncChange {
            model_id: candidate.model_id.clone(),
            kind: "insert",
            fields: after.keys().cloned().collect(),
            before: None,
            after: Some(after),
        });
        self.writes.push(ModelPriceRecord {
            model_id: candidate.model_id,
            billing_mode: candidate.billing_mode,
            input_usd_per_1m: candidate.input_usd_per_1m,
            output_usd_per_1m: candidate.output_usd_per_1m,
            cache_read_usd_per_1m: candidate.cache_read_usd_per_1m,
            cache_write_usd_per_1m: candidate.cache_write_usd_per_1m,
            cache_write_1h_usd_per_1m: None,
            reasoning_usd_per_1m: candidate.reasoning_usd_per_1m,
            per_request_usd: candidate.per_request_usd,
            billing_expr: candidate.billing_expr,
            source: source.as_str().to_string(),
            locked_fields: Vec::new(),
            raw_json: candidate.raw_json,
            enabled: true,
            updated_at: now,
        });
    }

    fn update_candidate(
        &mut self,
        source: SyncSource,
        stored: &ModelPriceRecord,
        candidate: SyncCandidate,
        now: chrono::DateTime<Utc>,
    ) {
        let locked = &stored.locked_fields;
        let mut changed = FieldDiff::default();
        let billing_mode = if locked.iter().any(|lock| lock == "billing_mode") {
            stored.billing_mode.clone()
        } else {
            if stored.billing_mode != candidate.billing_mode {
                changed.push(
                    "billing_mode",
                    &stored.billing_mode,
                    &candidate.billing_mode,
                );
            }
            candidate.billing_mode.clone()
        };
        let merged = ModelPriceRecord {
            model_id: candidate.model_id.clone(),
            billing_mode,
            input_usd_per_1m: merge_field(
                "input_usd_per_1m",
                locked,
                &stored.input_usd_per_1m,
                &candidate.input_usd_per_1m,
                &mut changed,
            ),
            output_usd_per_1m: merge_field(
                "output_usd_per_1m",
                locked,
                &stored.output_usd_per_1m,
                &candidate.output_usd_per_1m,
                &mut changed,
            ),
            cache_read_usd_per_1m: merge_field(
                "cache_read_usd_per_1m",
                locked,
                &stored.cache_read_usd_per_1m,
                &candidate.cache_read_usd_per_1m,
                &mut changed,
            ),
            cache_write_usd_per_1m: merge_field(
                "cache_write_usd_per_1m",
                locked,
                &stored.cache_write_usd_per_1m,
                &candidate.cache_write_usd_per_1m,
                &mut changed,
            ),
            // No sync source publishes a 1h cache-write price; the
            // stored value is always kept.
            cache_write_1h_usd_per_1m: stored.cache_write_1h_usd_per_1m.clone(),
            reasoning_usd_per_1m: merge_field(
                "reasoning_usd_per_1m",
                locked,
                &stored.reasoning_usd_per_1m,
                &candidate.reasoning_usd_per_1m,
                &mut changed,
            ),
            per_request_usd: merge_field(
                "per_request_usd",
                locked,
                &stored.per_request_usd,
                &candidate.per_request_usd,
                &mut changed,
            ),
            billing_expr: merge_field(
                "billing_expr",
                locked,
                &stored.billing_expr,
                &candidate.billing_expr,
                &mut changed,
            ),
            source: source.as_str().to_string(),
            locked_fields: stored.locked_fields.clone(),
            raw_json: candidate.raw_json,
            enabled: stored.enabled,
            updated_at: now,
        };
        if changed.fields.is_empty() {
            // MP-Y14: raw_json and updated_at still refresh, but an
            // unchanged row is not counted as an update.
            self.skipped += 1;
        } else {
            self.updated += 1;
            self.changes.push(SyncChange {
                model_id: candidate.model_id.clone(),
                kind: "update",
                fields: changed.fields,
                before: Some(changed.before),
                after: Some(changed.after),
            });
        }
        self.writes.push(merged);
    }
}

/// Changed field names plus their stored and incoming values (MP-A6).
#[derive(Default)]
struct FieldDiff {
    fields: Vec<String>,
    before: serde_json::Map<String, Value>,
    after: serde_json::Map<String, Value>,
}

impl FieldDiff {
    fn push<T: Serialize>(&mut self, field: &str, before: &T, after: &T) {
        self.fields.push(field.to_string());
        self.before.insert(field.to_string(), json!(before));
        self.after.insert(field.to_string(), json!(after));
    }
}

fn merge_field<T: Clone + PartialEq + Serialize>(
    field: &str,
    locked: &[String],
    stored: &Option<T>,
    incoming: &Option<T>,
    diff: &mut FieldDiff,
) -> Option<T> {
    if locked.iter().any(|lock| lock == field) {
        return stored.clone();
    }
    if stored != incoming {
        diff.push(field, stored, incoming);
    }
    incoming.clone()
}

/// Price columns shown for an inserted or deleted row in the MP-A6 preview.
fn row_price_values(
    billing_mode: &str,
    prices: [(&str, &Option<String>); 6],
    billing_expr: &Option<Value>,
) -> serde_json::Map<String, Value> {
    let mut values = serde_json::Map::new();
    values.insert("billing_mode".to_string(), json!(billing_mode));
    for (name, value) in prices {
        if let Some(value) = value {
            values.insert(name.to_string(), json!(value));
        }
    }
    if let Some(expr) = billing_expr {
        values.insert("billing_expr".to_string(), expr.clone());
    }
    values
}

/// MP-Y13..MP-Y15: computes the apply plan for one snapshot. `existing` is
/// the full stored row set; ownership, locks, and models_dev deletes are
/// resolved here so apply only needs set-based writes.
pub fn compute_sync_plan(
    source: SyncSource,
    existing: &[ModelPriceRecord],
    candidates: Vec<SyncCandidate>,
) -> SyncPlan {
    let existing_by_id: HashMap<&str, &ModelPriceRecord> = existing
        .iter()
        .map(|row| (row.model_id.as_str(), row))
        .collect();
    let now = Utc::now();
    let mut plan = SyncPlan {
        inserted: 0,
        updated: 0,
        skipped: 0,
        deleted: 0,
        changes: Vec::new(),
        writes: Vec::new(),
        delete_ids: Vec::new(),
    };

    let mut candidate_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    for candidate in candidates {
        candidate_ids.insert(candidate.model_id.clone());
        match existing_by_id.get(candidate.model_id.as_str()) {
            None => plan.insert_candidate(source, candidate, now),
            Some(stored) if stored.source != source.as_str() => {
                // MP-Y13: manual rows and rows owned by another source are
                // never modified by this run.
                plan.skipped += 1;
                plan.changes.push(SyncChange {
                    model_id: candidate.model_id.clone(),
                    kind: "skip",
                    fields: Vec::new(),
                    before: None,
                    after: None,
                });
            }
            Some(stored) => plan.update_candidate(source, stored, candidate, now),
        }
    }

    if source == SyncSource::ModelsDev {
        for row in existing {
            if row.source == "models_dev" && !candidate_ids.contains(&row.model_id) {
                plan.deleted += 1;
                plan.changes.push(SyncChange {
                    model_id: row.model_id.clone(),
                    kind: "delete",
                    fields: Vec::new(),
                    before: Some(row_price_values(
                        &row.billing_mode,
                        [
                            ("input_usd_per_1m", &row.input_usd_per_1m),
                            ("output_usd_per_1m", &row.output_usd_per_1m),
                            ("cache_read_usd_per_1m", &row.cache_read_usd_per_1m),
                            ("cache_write_usd_per_1m", &row.cache_write_usd_per_1m),
                            ("reasoning_usd_per_1m", &row.reasoning_usd_per_1m),
                            ("per_request_usd", &row.per_request_usd),
                        ],
                        &row.billing_expr,
                    )),
                    after: None,
                });
                plan.delete_ids.push(row.model_id.clone());
            }
        }
    }

    plan
}
