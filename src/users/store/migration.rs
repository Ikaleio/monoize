use super::*;

impl UserStore {
    pub(super) async fn migrate_transform_rule_ids(&self) -> Result<(), String> {
        const TRANSFORM_MIGRATION_MARKER: &str = "migration.api_key_transform_rule_ids.v2";
        const OBSOLETE_TRANSFORM_MIGRATION_MARKER: &str = "migration.api_key_transform_rule_ids.v1";
        const TRANSFORM_MIGRATION_CHUNK_SIZE: usize = 300;
        let marker = self
            .db
            .read()
            .query_one(self.db.stmt(
                "SELECT value FROM system_settings WHERE key = $1",
                vec![TRANSFORM_MIGRATION_MARKER.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;
        if marker
            .as_ref()
            .and_then(|row| row.try_get::<String>("", "value").ok())
            .as_deref()
            == Some("complete")
        {
            return Ok(());
        }
        let mut cursor: Option<String> = None;
        loop {
            let tx = self.db.begin_write().await.map_err(|e| e.to_string())?;
            let rows = match cursor.as_deref() {
                Some(cursor) => {
                    tx.query_all(self.db.stmt(
                        &format!(
                            "SELECT id, transforms FROM api_keys
                             WHERE id > $1 ORDER BY id LIMIT {TRANSFORM_MIGRATION_CHUNK_SIZE}"
                        ),
                        vec![cursor.into()],
                    ))
                    .await
                }
                None => {
                    tx.query_all(self.db.stmt(
                        &format!(
                            "SELECT id, transforms FROM api_keys
                             ORDER BY id LIMIT {TRANSFORM_MIGRATION_CHUNK_SIZE}"
                        ),
                        vec![],
                    ))
                    .await
                }
            }
            .map_err(|e| e.to_string())?;
            if rows.is_empty() {
                tx.commit().await.map_err(|e| e.to_string())?;
                break;
            }

            let mut updates = Vec::new();
            for row in rows {
                let id: String = row.try_get("", "id").map_err(|e| e.to_string())?;
                cursor = Some(id.clone());
                let raw: String = row
                    .try_get("", "transforms")
                    .unwrap_or_else(|_| "[]".to_string());
                let Ok(mut transforms) = serde_json::from_str::<Vec<TransformRuleConfig>>(&raw)
                else {
                    tracing::warn!(api_key_id = %id, "skip invalid api key transforms during transform id migration");
                    continue;
                };
                if !canonicalize_transform_rules(&mut transforms) {
                    continue;
                }
                let encoded = serde_json::to_string(&transforms).map_err(|e| e.to_string())?;
                updates.push((id, encoded));
            }
            self.write_transform_migration_updates(&tx, &updates)
                .await?;
            tx.commit().await.map_err(|e| e.to_string())?;
            for (id, _) in updates {
                self.api_key_cache.invalidate_by_key_id(&id);
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

impl UserStore {
    async fn write_transform_migration_updates(
        &self,
        tx: &DatabaseTransaction,
        updates: &[(String, String)],
    ) -> Result<(), String> {
        if !updates.is_empty() {
            let mut values: Vec<SeaValue> = Vec::with_capacity(updates.len() * 3);
            let mut cases = Vec::with_capacity(updates.len());
            let mut ids = Vec::with_capacity(updates.len());
            for (id, transforms) in updates {
                let id_index = values.len() + 1;
                values.push(id.clone().into());
                let transforms_index = values.len() + 1;
                values.push(transforms.clone().into());
                cases.push(format!("WHEN ${id_index} THEN ${transforms_index}"));
            }
            for (id, _) in updates {
                let id_index = values.len() + 1;
                values.push(id.clone().into());
                ids.push(format!("${id_index}"));
            }
            tx.execute(self.db.stmt(
                &format!(
                    "UPDATE api_keys SET transforms = CASE id {} ELSE transforms END
                         WHERE id IN ({})",
                    cases.join(" "),
                    ids.join(", ")
                ),
                values,
            ))
            .await
            .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}
