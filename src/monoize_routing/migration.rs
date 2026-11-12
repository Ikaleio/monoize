use super::store::MonoizeRoutingStore;
use crate::transforms::{TransformRuleConfig, canonicalize_transform_rules};
use chrono::Utc;
use sea_orm::ConnectionTrait;

const TRANSFORM_MIGRATION_BATCH_SIZE: usize = 199;
const TRANSFORM_MIGRATION_MARKER: &str = "migration.provider_transform_rule_ids.v2";
const OBSOLETE_TRANSFORM_MIGRATION_MARKER: &str = "migration.provider_transform_rule_ids.v1";

impl MonoizeRoutingStore {
    pub(super) async fn migrate_transform_rule_ids(&self) -> Result<(), String> {
        let marker = self
            .db
            .read()
            .query_one(self.db.stmt(
                "SELECT value FROM system_settings WHERE key = $1",
                vec![TRANSFORM_MIGRATION_MARKER.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;
        let marker_value = marker
            .map(|row| {
                row.try_get::<String>("", "value")
                    .map_err(|e| e.to_string())
            })
            .transpose()?;
        if marker_value.as_deref() == Some("complete") {
            return Ok(());
        }

        let mut last_id: Option<String> = None;
        loop {
            let tx = self.db.begin_write().await.map_err(|e| e.to_string())?;
            let (sql, values) = match last_id.as_deref() {
                Some(last_id) => (
                    format!(
                        "SELECT id, transforms FROM monoize_providers
                         WHERE id > $1 ORDER BY id ASC LIMIT {TRANSFORM_MIGRATION_BATCH_SIZE}"
                    ),
                    vec![last_id.into()],
                ),
                None => (
                    format!(
                        "SELECT id, transforms FROM monoize_providers
                         ORDER BY id ASC LIMIT {TRANSFORM_MIGRATION_BATCH_SIZE}"
                    ),
                    vec![],
                ),
            };
            let rows = tx
                .query_all(self.db.stmt(&sql, values))
                .await
                .map_err(|e| e.to_string())?;
            if rows.is_empty() {
                tx.commit().await.map_err(|e| e.to_string())?;
                break;
            }
            let batch_len = rows.len();
            let next_last_id: String = rows
                .last()
                .expect("non-empty transform migration batch")
                .try_get("", "id")
                .map_err(|e| e.to_string())?;
            let mut updates = Vec::with_capacity(batch_len);
            for row in rows {
                let id: String = row.try_get("", "id").map_err(|e| e.to_string())?;
                let raw: String = row.try_get("", "transforms").map_err(|e| e.to_string())?;
                let Ok(mut transforms) = serde_json::from_str::<Vec<TransformRuleConfig>>(&raw)
                else {
                    tracing::warn!(provider_id = %id, "skip invalid provider transforms during transform id migration");
                    continue;
                };
                if !canonicalize_transform_rules(&mut transforms) {
                    continue;
                }
                let encoded = serde_json::to_string(&transforms).map_err(|e| e.to_string())?;
                updates.push((id, encoded));
            }

            if !updates.is_empty() {
                let mut values: Vec<sea_orm::Value> = Vec::with_capacity(updates.len() * 2);
                let mut cases = Vec::with_capacity(updates.len());
                let mut ids = Vec::with_capacity(updates.len());
                for (id, transforms) in &updates {
                    let id_index = values.len() + 1;
                    values.push(id.clone().into());
                    ids.push(format!("${id_index}"));
                    let transforms_index = values.len() + 1;
                    values.push(transforms.clone().into());
                    cases.push(format!("WHEN ${id_index} THEN ${transforms_index}"));
                }
                tx.execute(self.db.stmt(
                    &format!(
                        "UPDATE monoize_providers
                         SET transforms = CASE id {} ELSE transforms END
                         WHERE id IN ({})",
                        cases.join(" "),
                        ids.join(", ")
                    ),
                    values,
                ))
                .await
                .map_err(|e| e.to_string())?;
            }

            tx.commit().await.map_err(|e| e.to_string())?;
            last_id = Some(next_last_id);
            if batch_len < TRANSFORM_MIGRATION_BATCH_SIZE {
                break;
            }
        }

        let tx = self.db.begin_write().await.map_err(|e| e.to_string())?;
        tx.execute(self.db.stmt(
            "INSERT INTO system_settings (key, value, updated_at) VALUES ($1, $2, $3)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            vec![
                TRANSFORM_MIGRATION_MARKER.into(),
                "complete".into(),
                Utc::now().to_rfc3339().into(),
            ],
        ))
        .await
        .map_err(|e| e.to_string())?;
        tx.execute(self.db.stmt(
            "DELETE FROM system_settings WHERE key = $1",
            vec![OBSOLETE_TRANSFORM_MIGRATION_MARKER.into()],
        ))
        .await
        .map_err(|e| e.to_string())?;
        tx.commit().await.map_err(|e| e.to_string())
    }
}
