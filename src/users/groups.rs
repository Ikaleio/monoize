use super::UserStore;
use super::store::parse_group_ids_json;
use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, QueryResult, TransactionTrait, Value as SeaValue};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// One `monoize_groups` registry row (`groups-registry.spec.md` §1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Group {
    pub id: String,
    pub name: String,
    pub description: String,
    pub is_default: bool,
    pub user_selectable: bool,
    pub sort_order: i32,
    pub billing_ratio: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CreateGroupInput {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub user_selectable: bool,
    #[serde(default)]
    pub sort_order: i32,
    #[serde(default)]
    pub billing_ratio: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct UpdateGroupInput {
    pub name: Option<String>,
    pub description: Option<String>,
    pub user_selectable: Option<bool>,
    pub sort_order: Option<i32>,
    pub billing_ratio: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReorderGroupsInput {
    pub group_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupStoreError {
    NotFound,
    NameExists,
    InvalidName,
    InvalidDescription,
    InvalidBillingRatio,
    InvalidReorder(String),
    CannotDeleteDefault,
    Storage(String),
}

const GROUP_COLUMNS: &str = "id, name, description, is_default, user_selectable, sort_order, \
     billing_ratio, created_at, updated_at";

fn storage(error: impl std::fmt::Display) -> GroupStoreError {
    GroupStoreError::Storage(error.to_string())
}

fn validate_name(raw: &str) -> Result<String, GroupStoreError> {
    let name = raw.trim().to_string();
    if name.is_empty() || name.chars().count() > 64 {
        return Err(GroupStoreError::InvalidName);
    }
    Ok(name)
}

fn validate_description(raw: &str) -> Result<String, GroupStoreError> {
    let description = raw.trim().to_string();
    if description.chars().count() > 256 {
        return Err(GroupStoreError::InvalidDescription);
    }
    Ok(description)
}

/// GR-D8: `billing_ratio` is a base-10 decimal string `>= 0` with at most 12
/// integer digits and at most 9 fractional digits, canonicalized without
/// exponent notation or trailing fractional zeroes. Never parsed through
/// binary floating point.
fn validate_billing_ratio(raw: &str) -> Result<String, GroupStoreError> {
    let raw = raw.trim();
    if raw.is_empty()
        || raw.starts_with('+')
        || raw.starts_with('-')
        || !raw
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'.')
        || raw.bytes().filter(|byte| *byte == b'.').count() > 1
    {
        return Err(GroupStoreError::InvalidBillingRatio);
    }
    let value = rust_decimal::Decimal::from_str_exact(raw)
        .map_err(|_| GroupStoreError::InvalidBillingRatio)?;
    if value.is_sign_negative() || value.scale() > 9 {
        return Err(GroupStoreError::InvalidBillingRatio);
    }
    let canonical = value.normalize();
    if canonical.trunc().to_string().trim_start_matches('-').len() > 12 {
        return Err(GroupStoreError::InvalidBillingRatio);
    }
    Ok(canonical.to_string())
}

fn parse_time(row: &QueryResult, column: &str) -> Result<DateTime<Utc>, GroupStoreError> {
    let raw: String = row.try_get("", column).map_err(storage)?;
    DateTime::parse_from_rfc3339(&raw)
        .map(|value| value.with_timezone(&Utc))
        .map_err(storage)
}

fn row_to_group(row: &QueryResult) -> Result<Group, GroupStoreError> {
    Ok(Group {
        id: row.try_get("", "id").map_err(storage)?,
        name: row.try_get("", "name").map_err(storage)?,
        description: row.try_get("", "description").map_err(storage)?,
        is_default: row.try_get::<i32>("", "is_default").map_err(storage)? != 0,
        user_selectable: row.try_get::<i32>("", "user_selectable").map_err(storage)? != 0,
        sort_order: row.try_get("", "sort_order").map_err(storage)?,
        billing_ratio: row.try_get("", "billing_ratio").map_err(storage)?,
        created_at: parse_time(row, "created_at")?,
        updated_at: parse_time(row, "updated_at")?,
    })
}

fn is_name_unique_violation(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    (lower.contains("unique") || lower.contains("duplicate"))
        && (lower.contains("name") || lower.contains("uq_monoize_groups_name_lower"))
}

/// GR-X7: upper bound on rows per bulk cascade UPDATE. At most 3 binds per
/// row keeps every chunk below the portable placeholder ceiling shared by
/// SQLite and PostgreSQL.
const GROUP_CASCADE_UPDATE_CHUNK_ROWS: usize = 250;

/// One bulk `UPDATE {table} SET group_ids = CASE id ... END WHERE id IN (...)`
/// statement for `(id, group_ids_json)` pairs. Each id placeholder is numbered
/// once and reused by both its CASE arm and the IN list.
fn group_ids_bulk_update(table: &str, entries: &[(String, String)]) -> (String, Vec<SeaValue>) {
    let mut values: Vec<SeaValue> = Vec::with_capacity(entries.len() * 2);
    let mut cases = Vec::with_capacity(entries.len());
    let mut ids = Vec::with_capacity(entries.len());
    for (id, group_ids_json) in entries {
        let id_index = values.len() + 1;
        values.push(id.clone().into());
        ids.push(format!("${id_index}"));
        let group_ids_index = values.len() + 1;
        values.push(group_ids_json.clone().into());
        cases.push(format!("WHEN ${id_index} THEN ${group_ids_index}"));
    }
    (
        format!(
            "UPDATE {table} SET group_ids = CASE id {} ELSE group_ids END WHERE id IN ({})",
            cases.join(" "),
            ids.join(", ")
        ),
        values,
    )
}

/// GR-X2 bulk rewrite for `api_keys`: `group_ids` and `use_user_group` are
/// rewritten together from `(id, group_ids_json, use_user_group)` triples.
fn api_key_group_cascade_bulk_update(
    entries: &[(String, String, i32)],
) -> (String, Vec<SeaValue>) {
    let mut values: Vec<SeaValue> = Vec::with_capacity(entries.len() * 3);
    let mut group_cases = Vec::with_capacity(entries.len());
    let mut flag_cases = Vec::with_capacity(entries.len());
    let mut ids = Vec::with_capacity(entries.len());
    for (id, group_ids_json, use_user_group) in entries {
        let id_index = values.len() + 1;
        values.push(id.clone().into());
        ids.push(format!("${id_index}"));
        let group_ids_index = values.len() + 1;
        values.push(group_ids_json.clone().into());
        group_cases.push(format!("WHEN ${id_index} THEN ${group_ids_index}"));
        let flag_index = values.len() + 1;
        values.push(SeaValue::Int(Some(*use_user_group)));
        flag_cases.push(format!("WHEN ${id_index} THEN ${flag_index}"));
    }
    (
        format!(
            "UPDATE api_keys SET group_ids = CASE id {} ELSE group_ids END, \
             use_user_group = CASE id {} ELSE use_user_group END WHERE id IN ({})",
            group_cases.join(" "),
            flag_cases.join(" "),
            ids.join(", ")
        ),
        values,
    )
}

impl UserStore {
    /// List every registry row in canonical order (GR-D5).
    pub async fn list_groups(&self) -> Result<Vec<Group>, String> {
        let rows = self
            .db
            .read()
            .query_all(self.db.stmt(
                &format!(
                    "SELECT {GROUP_COLUMNS} FROM monoize_groups \
                     ORDER BY sort_order ASC, created_at ASC, id ASC"
                ),
                vec![],
            ))
            .await
            .map_err(|e| e.to_string())?;
        rows.iter()
            .map(|row| row_to_group(row).map_err(|error| format!("{error:?}")))
            .collect()
    }

    pub async fn get_group_by_id(&self, id: &str) -> Result<Option<Group>, String> {
        let row = self
            .db
            .read()
            .query_one(self.db.stmt(
                &format!("SELECT {GROUP_COLUMNS} FROM monoize_groups WHERE id = $1"),
                vec![id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;
        match row {
            Some(row) => Ok(Some(row_to_group(&row).map_err(|e| format!("{e:?}"))?)),
            None => Ok(None),
        }
    }

    /// The id of the single `is_default = 1` row (GR-D2).
    pub async fn default_group_id(&self) -> Result<String, String> {
        let row = self
            .db
            .read()
            .query_one(self.db.stmt(
                "SELECT id FROM monoize_groups WHERE is_default = 1 LIMIT 1",
                vec![],
            ))
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "default group row missing (GR-D2 violated)".to_string())?;
        row.try_get("", "id").map_err(|e| e.to_string())
    }

    /// GR-C3: every element must reference an existing registry row.
    /// Returns the first unknown id, or `None` when all ids exist.
    pub async fn find_unknown_group_id(
        &self,
        group_ids: &[String],
    ) -> Result<Option<String>, String> {
        for id in group_ids {
            if self.get_group_by_id(id).await?.is_none() {
                return Ok(Some(id.clone()));
            }
        }
        Ok(None)
    }

    pub async fn create_group(&self, input: CreateGroupInput) -> Result<Group, GroupStoreError> {
        let name = validate_name(&input.name)?;
        let description = validate_description(&input.description)?;
        let billing_ratio = match input.billing_ratio.as_deref() {
            Some(raw) => validate_billing_ratio(raw)?,
            None => "1".to_string(),
        };
        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now();

        if self.group_name_exists(None, &name).await? {
            return Err(GroupStoreError::NameExists);
        }
        let result = self
            .db
            .write()
            .await
            .execute(self.db.stmt(
                "INSERT INTO monoize_groups (id, name, description, is_default, user_selectable, sort_order, billing_ratio, created_at, updated_at) \
                 VALUES ($1, $2, $3, 0, $4, $5, $6, $7, $7)",
                vec![
                    id.clone().into(),
                    name.clone().into(),
                    description.clone().into(),
                    SeaValue::Int(Some(if input.user_selectable { 1 } else { 0 })),
                    SeaValue::Int(Some(input.sort_order)),
                    billing_ratio.clone().into(),
                    now.to_rfc3339().into(),
                ],
            ))
            .await;
        if let Err(error) = result {
            let message = error.to_string();
            if is_name_unique_violation(&message) {
                return Err(GroupStoreError::NameExists);
            }
            return Err(GroupStoreError::Storage(message));
        }

        Ok(Group {
            id,
            name,
            description,
            is_default: false,
            user_selectable: input.user_selectable,
            sort_order: input.sort_order,
            billing_ratio,
            created_at: now,
            updated_at: now,
        })
    }

    pub async fn update_group(
        &self,
        id: &str,
        input: UpdateGroupInput,
    ) -> Result<Group, GroupStoreError> {
        let name = input.name.as_deref().map(validate_name).transpose()?;
        let description = input
            .description
            .as_deref()
            .map(validate_description)
            .transpose()?;
        let billing_ratio = input
            .billing_ratio
            .as_deref()
            .map(validate_billing_ratio)
            .transpose()?;

        let existing = self
            .get_group_by_id(id)
            .await
            .map_err(GroupStoreError::Storage)?
            .ok_or(GroupStoreError::NotFound)?;

        if let Some(name) = &name
            && self.group_name_exists(Some(id), name).await?
        {
            return Err(GroupStoreError::NameExists);
        }

        let mut set_clauses = Vec::new();
        let mut values: Vec<SeaValue> = Vec::new();
        let mut idx = 1usize;
        if let Some(name) = &name {
            set_clauses.push(format!("name = ${idx}"));
            values.push(name.clone().into());
            idx += 1;
        }
        if let Some(description) = &description {
            set_clauses.push(format!("description = ${idx}"));
            values.push(description.clone().into());
            idx += 1;
        }
        if let Some(user_selectable) = input.user_selectable {
            set_clauses.push(format!("user_selectable = ${idx}"));
            values.push(SeaValue::Int(Some(if user_selectable { 1 } else { 0 })));
            idx += 1;
        }
        if let Some(sort_order) = input.sort_order {
            set_clauses.push(format!("sort_order = ${idx}"));
            values.push(SeaValue::Int(Some(sort_order)));
            idx += 1;
        }
        if let Some(billing_ratio) = &billing_ratio {
            set_clauses.push(format!("billing_ratio = ${idx}"));
            values.push(billing_ratio.clone().into());
            idx += 1;
        }
        if set_clauses.is_empty() {
            return Ok(existing);
        }
        let now = Utc::now();
        set_clauses.push(format!("updated_at = ${idx}"));
        values.push(now.to_rfc3339().into());
        idx += 1;
        values.push(id.into());

        let result = self
            .db
            .write()
            .await
            .execute(self.db.stmt(
                &format!(
                    "UPDATE monoize_groups SET {} WHERE id = ${idx}",
                    set_clauses.join(", ")
                ),
                values,
            ))
            .await;
        if let Err(error) = result {
            let message = error.to_string();
            if is_name_unique_violation(&message) {
                return Err(GroupStoreError::NameExists);
            }
            return Err(GroupStoreError::Storage(message));
        }

        // GR-A6: cached authentication results are keyed to registry state.
        self.api_key_cache.invalidate_all();

        Ok(Group {
            id: existing.id,
            name: name.unwrap_or(existing.name),
            description: description.unwrap_or(existing.description),
            is_default: existing.is_default,
            user_selectable: input.user_selectable.unwrap_or(existing.user_selectable),
            sort_order: input.sort_order.unwrap_or(existing.sort_order),
            billing_ratio: billing_ratio.unwrap_or(existing.billing_ratio),
            created_at: existing.created_at,
            updated_at: now,
        })
    }

    pub async fn reorder_groups(&self, input: ReorderGroupsInput) -> Result<(), GroupStoreError> {
        if input.group_ids.len() > 199 {
            return Err(GroupStoreError::InvalidReorder(
                "group reorder accepts at most 199 ids".to_string(),
            ));
        }

        let mut unique_ids = HashSet::new();
        for id in &input.group_ids {
            if !unique_ids.insert(id.clone()) {
                return Err(GroupStoreError::InvalidReorder(
                    "group_ids contains duplicates".to_string(),
                ));
            }
        }

        let write = self.db.write().await;
        let tx = write.begin().await.map_err(storage)?;
        if self.db.is_postgres() {
            tx.execute_unprepared("LOCK TABLE monoize_groups IN SHARE ROW EXCLUSIVE MODE")
                .await
                .map_err(storage)?;
        }

        let rows = tx
            .query_all(
                self.db
                    .stmt("SELECT id FROM monoize_groups ORDER BY id", vec![]),
            )
            .await
            .map_err(storage)?;
        if rows.len() != input.group_ids.len() {
            return Err(GroupStoreError::InvalidReorder(
                "group_ids must contain all groups exactly once".to_string(),
            ));
        }
        let existing_ids: HashSet<String> = rows
            .into_iter()
            .map(|row| row.try_get("", "id").map_err(storage))
            .collect::<Result<_, _>>()?;
        if existing_ids != unique_ids {
            return Err(GroupStoreError::InvalidReorder(
                "group_ids must contain all groups exactly once".to_string(),
            ));
        }
        if input.group_ids.is_empty() {
            tx.commit().await.map_err(storage)?;
            return Ok(());
        }

        let mut values = Vec::with_capacity(input.group_ids.len() * 2 + 1);
        let mut cases = Vec::with_capacity(input.group_ids.len());
        for (sort_order, id) in input.group_ids.iter().enumerate() {
            let id_index = values.len() + 1;
            values.push(id.clone().into());
            let sort_order_index = values.len() + 1;
            values.push(SeaValue::Int(Some(sort_order as i32)));
            cases.push(format!("WHEN ${id_index} THEN ${sort_order_index}"));
        }
        let updated_at_index = values.len() + 1;
        values.push(Utc::now().to_rfc3339().into());
        tx.execute(self.db.stmt(
            &format!(
                "UPDATE monoize_groups \
                 SET sort_order = CASE id {} END, updated_at = ${updated_at_index}",
                cases.join(" ")
            ),
            values,
        ))
        .await
        .map_err(storage)?;
        tx.commit().await.map_err(storage)?;
        self.api_key_cache.invalidate_all();
        Ok(())
    }

    /// Delete a non-default group and apply the GR-X1..GR-X5 cascade in one
    /// transaction. The caller must bump the routing config revision after a
    /// successful delete (GR-X6).
    pub async fn delete_group(&self, id: &str) -> Result<(), GroupStoreError> {
        let target = self
            .get_group_by_id(id)
            .await
            .map_err(GroupStoreError::Storage)?
            .ok_or(GroupStoreError::NotFound)?;
        if target.is_default {
            return Err(GroupStoreError::CannotDeleteDefault);
        }
        let default_group_id = self
            .default_group_id()
            .await
            .map_err(GroupStoreError::Storage)?;

        let write = self.db.write().await;
        let tx = write.begin().await.map_err(storage)?;

        // GR-X1: members move to the default group.
        tx.execute(self.db.stmt(
            "UPDATE users SET group_id = $1 WHERE group_id = $2",
            vec![default_group_id.clone().into(), id.into()],
        ))
        .await
        .map_err(storage)?;

        // GR-X2: drop the id from key selections; empty selections fall back
        // to inheriting the owner's group. Affected rows are rewritten with
        // chunked bulk statements (GR-X7) instead of one UPDATE per row.
        let rows = tx
            .query_all(self.db.stmt(
                "SELECT id, use_user_group, group_ids FROM api_keys",
                vec![],
            ))
            .await
            .map_err(storage)?;
        let mut key_updates: Vec<(String, String, i32)> = Vec::new();
        for row in rows {
            let row_id: String = row.try_get("", "id").map_err(storage)?;
            let use_user_group: i32 = row.try_get("", "use_user_group").map_err(storage)?;
            let raw: Option<String> = row.try_get("", "group_ids").map_err(storage)?;
            let group_ids = parse_group_ids_json(raw.as_deref(), "api_keys.group_ids")
                .map_err(GroupStoreError::Storage)?;
            if !group_ids.iter().any(|gid| gid == id) {
                continue;
            }
            let remaining: Vec<String> = group_ids.into_iter().filter(|gid| gid != id).collect();
            let next_use_user_group = if remaining.is_empty() {
                1
            } else {
                use_user_group
            };
            key_updates.push((
                row_id,
                serde_json::to_string(&remaining).map_err(storage)?,
                next_use_user_group,
            ));
        }
        for chunk in key_updates.chunks(GROUP_CASCADE_UPDATE_CHUNK_ROWS) {
            let (sql, values) = api_key_group_cascade_bulk_update(chunk);
            tx.execute(self.db.stmt(&sql, values))
                .await
                .map_err(storage)?;
        }

        // GR-X3: providers keep a non-empty group set (GR-I2).
        let rows = tx
            .query_all(
                self.db
                    .stmt("SELECT id, group_ids FROM monoize_providers", vec![]),
            )
            .await
            .map_err(storage)?;
        let mut provider_updates: Vec<(String, String)> = Vec::new();
        for row in rows {
            let row_id: String = row.try_get("", "id").map_err(storage)?;
            let raw: Option<String> = row.try_get("", "group_ids").map_err(storage)?;
            let group_ids = parse_group_ids_json(raw.as_deref(), "monoize_providers.group_ids")
                .map_err(GroupStoreError::Storage)?;
            if !group_ids.iter().any(|gid| gid == id) {
                continue;
            }
            let mut remaining: Vec<String> =
                group_ids.into_iter().filter(|gid| gid != id).collect();
            if remaining.is_empty() {
                remaining.push(default_group_id.clone());
            }
            provider_updates.push((row_id, serde_json::to_string(&remaining).map_err(storage)?));
        }
        for chunk in provider_updates.chunks(GROUP_CASCADE_UPDATE_CHUNK_ROWS) {
            let (sql, values) = group_ids_bulk_update("monoize_providers", chunk);
            tx.execute(self.db.stmt(&sql, values))
                .await
                .map_err(storage)?;
        }

        // GR-X4: an emptied plan ceiling stays [] (unrestricted).
        let rows = tx
            .query_all(
                self.db
                    .stmt("SELECT id, group_ids FROM billing_plans", vec![]),
            )
            .await
            .map_err(storage)?;
        let mut plan_updates: Vec<(String, String)> = Vec::new();
        for row in rows {
            let row_id: String = row.try_get("", "id").map_err(storage)?;
            let raw: Option<String> = row.try_get("", "group_ids").map_err(storage)?;
            let group_ids = parse_group_ids_json(raw.as_deref(), "billing_plans.group_ids")
                .map_err(GroupStoreError::Storage)?;
            if !group_ids.iter().any(|gid| gid == id) {
                continue;
            }
            let remaining: Vec<String> = group_ids.into_iter().filter(|gid| gid != id).collect();
            plan_updates.push((row_id, serde_json::to_string(&remaining).map_err(storage)?));
        }
        for chunk in plan_updates.chunks(GROUP_CASCADE_UPDATE_CHUNK_ROWS) {
            let (sql, values) = group_ids_bulk_update("billing_plans", chunk);
            tx.execute(self.db.stmt(&sql, values))
                .await
                .map_err(storage)?;
        }

        let result = tx
            .execute(
                self.db
                    .stmt("DELETE FROM monoize_groups WHERE id = $1", vec![id.into()]),
            )
            .await
            .map_err(storage)?;
        if result.rows_affected() != 1 {
            return Err(GroupStoreError::NotFound);
        }

        tx.commit().await.map_err(storage)?;
        self.api_key_cache.invalidate_all();
        Ok(())
    }

    async fn group_name_exists(
        &self,
        exclude_id: Option<&str>,
        name: &str,
    ) -> Result<bool, GroupStoreError> {
        let (sql, values): (&str, Vec<SeaValue>) = match exclude_id {
            Some(id) => (
                "SELECT COUNT(*) AS cnt FROM monoize_groups WHERE lower(name) = lower($1) AND id != $2",
                vec![name.into(), id.into()],
            ),
            None => (
                "SELECT COUNT(*) AS cnt FROM monoize_groups WHERE lower(name) = lower($1)",
                vec![name.into()],
            ),
        };
        let row = self
            .db
            .read()
            .query_one(self.db.stmt(sql, values))
            .await
            .map_err(storage)?
            .ok_or_else(|| GroupStoreError::Storage("count query returned no row".to_string()))?;
        let count: i64 = row.try_get("", "cnt").map_err(storage)?;
        Ok(count > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbPool;
    use crate::migration::Migrator;
    use crate::users::{UserRole, UserStore};
    use sea_orm_migration::MigratorTrait;

    #[test]
    fn group_ids_bulk_update_reuses_id_placeholders() {
        let entries = vec![
            ("prov-1".to_string(), r#"["g2"]"#.to_string()),
            ("prov-2".to_string(), "[]".to_string()),
        ];
        let (sql, values) = group_ids_bulk_update("monoize_providers", &entries);
        assert_eq!(
            sql,
            "UPDATE monoize_providers SET group_ids = CASE id \
             WHEN $1 THEN $2 WHEN $3 THEN $4 ELSE group_ids END \
             WHERE id IN ($1, $3)"
        );
        assert_eq!(values.len(), 4);
    }

    #[test]
    fn api_key_group_cascade_bulk_update_covers_both_columns() {
        let entries = vec![
            ("key-1".to_string(), "[]".to_string(), 1),
            ("key-2".to_string(), r#"["g2"]"#.to_string(), 0),
        ];
        let (sql, values) = api_key_group_cascade_bulk_update(&entries);
        assert!(
            sql.contains("group_ids = CASE id WHEN $1 THEN $2 WHEN $4 THEN $5 ELSE group_ids END")
        );
        assert!(sql.contains(
            "use_user_group = CASE id WHEN $1 THEN $3 WHEN $4 THEN $6 ELSE use_user_group END"
        ));
        assert!(sql.ends_with("WHERE id IN ($1, $4)"));
        assert_eq!(values.len(), 6);
    }

    async fn exec(db: &DbPool, sql: &str) {
        db.write()
            .await
            .execute(db.stmt(sql, vec![]))
            .await
            .expect("statement executes");
    }

    async fn scalar(db: &DbPool, sql: &str) -> String {
        db.read()
            .query_one(db.stmt(sql, vec![]))
            .await
            .expect("query succeeds")
            .expect("row exists")
            .try_get("", "value")
            .expect("value decodes")
    }

    /// GR-X2/GR-X3/GR-X4 end state after the GR-X7 bulk rewrite: emptied key
    /// selections fall back to inheritance, providers rebind to the default
    /// group, plan ceilings keep their remaining ids.
    #[tokio::test]
    async fn delete_group_cascade_rewrites_keys_providers_and_plans() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("db connects");
        {
            let write = db.write().await;
            Migrator::up(&*write, None).await.expect("migrates");
        }
        let (log_tx, _) = tokio::sync::broadcast::channel(1);
        let store = UserStore::new(db.clone(), log_tx)
            .await
            .expect("store creates");

        let default_id = store.default_group_id().await.expect("default exists");
        let doomed = store
            .create_group(CreateGroupInput {
                name: "doomed".to_string(),
                description: String::new(),
                user_selectable: false,
                sort_order: 1,
            })
            .await
            .expect("doomed group creates");
        let kept = store
            .create_group(CreateGroupInput {
                name: "kept".to_string(),
                description: String::new(),
                user_selectable: false,
                sort_order: 2,
            })
            .await
            .expect("kept group creates");

        let user = store
            .create_user("cascade-user", "password123", UserRole::User, None)
            .await
            .expect("user creates");
        let (only_key, _) = store
            .create_api_key(&user.id, "only-doomed", None)
            .await
            .expect("only key creates");
        let (mixed_key, _) = store
            .create_api_key(&user.id, "doomed-and-kept", None)
            .await
            .expect("mixed key creates");
        exec(
            &db,
            &format!(
                r#"UPDATE api_keys SET use_user_group = 0, group_ids = '["{}"]' WHERE id = '{}'"#,
                doomed.id, only_key.id
            ),
        )
        .await;
        exec(
            &db,
            &format!(
                r#"UPDATE api_keys SET use_user_group = 0, group_ids = '["{}","{}"]' WHERE id = '{}'"#,
                doomed.id, kept.id, mixed_key.id
            ),
        )
        .await;

        exec(
            &db,
            &format!(
                r#"INSERT INTO monoize_providers (id, name, group_ids, created_at, updated_at) VALUES ('prov-cascade', 'cascade', '["{}"]', '2026-08-26T00:00:00Z', '2026-08-26T00:00:00Z')"#,
                doomed.id
            ),
        )
        .await;
        exec(
            &db,
            &format!(
                r#"INSERT INTO billing_plans (id, name, grant_amount_nano_usd, schedule, group_ids, created_at, updated_at) VALUES ('plan-cascade', 'cascade', '0', '0 0 * * *', '["{}","{}"]', '2026-08-26T00:00:00Z', '2026-08-26T00:00:00Z')"#,
                doomed.id, kept.id
            ),
        )
        .await;

        store.delete_group(&doomed.id).await.expect("delete runs");

        assert_eq!(
            scalar(
                &db,
                &format!(
                    "SELECT CAST(use_user_group AS TEXT) || '|' || group_ids AS value \
                     FROM api_keys WHERE id = '{}'",
                    only_key.id
                )
            )
            .await,
            "1|[]"
        );
        assert_eq!(
            scalar(
                &db,
                &format!(
                    "SELECT CAST(use_user_group AS TEXT) || '|' || group_ids AS value \
                     FROM api_keys WHERE id = '{}'",
                    mixed_key.id
                )
            )
            .await,
            format!("0|[\"{}\"]", kept.id)
        );
        assert_eq!(
            scalar(
                &db,
                "SELECT group_ids AS value FROM monoize_providers WHERE id = 'prov-cascade'"
            )
            .await,
            format!("[\"{default_id}\"]")
        );
        assert_eq!(
            scalar(
                &db,
                "SELECT group_ids AS value FROM billing_plans WHERE id = 'plan-cascade'"
            )
            .await,
            format!("[\"{}\"]", kept.id)
        );
        assert_eq!(
            scalar(
                &db,
                &format!(
                    "SELECT CAST(COUNT(*) AS TEXT) AS value FROM monoize_groups WHERE id = '{}'",
                    doomed.id
                )
            )
            .await,
            "0"
        );
    }
}
