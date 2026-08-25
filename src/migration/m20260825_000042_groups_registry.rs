use std::collections::{BTreeMap, BTreeSet};

use chrono::Utc;
use sea_orm::{
    ConnectionTrait, DatabaseTransaction, DbBackend, Statement, TransactionTrait, Value,
};
use sea_orm_migration::prelude::*;
use uuid::Uuid;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        if !matches!(backend, DbBackend::Sqlite | DbBackend::Postgres) {
            return Ok(());
        }
        let tx = manager.get_connection().begin().await?;
        if let Err(error) = migrate_up(&tx, backend).await {
            let _ = tx.rollback().await;
            return Err(error);
        }
        tx.commit().await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        if !matches!(backend, DbBackend::Sqlite | DbBackend::Postgres) {
            return Ok(());
        }
        let tx = manager.get_connection().begin().await?;
        if let Err(error) = migrate_down(&tx, backend).await {
            let _ = tx.rollback().await;
            return Err(error);
        }
        tx.commit().await
    }
}

async fn migrate_up(tx: &DatabaseTransaction, backend: DbBackend) -> Result<(), DbErr> {
    let now = Utc::now().to_rfc3339();

    for sql in [
        "CREATE TABLE monoize_groups (\
         id TEXT NOT NULL PRIMARY KEY, \
         name TEXT NOT NULL, \
         description TEXT NOT NULL DEFAULT '', \
         is_default INTEGER NOT NULL DEFAULT 0, \
         user_selectable INTEGER NOT NULL DEFAULT 0, \
         sort_order INTEGER NOT NULL DEFAULT 0, \
         created_at TEXT NOT NULL, \
         updated_at TEXT NOT NULL)",
        "CREATE UNIQUE INDEX uq_monoize_groups_name_lower ON monoize_groups (lower(name))",
    ] {
        tx.execute(Statement::from_string(backend, sql.to_string()))
            .await?;
    }

    let default_group_id = Uuid::new_v4().to_string();
    execute_bound(
        tx,
        backend,
        "INSERT INTO monoize_groups (id, name, description, is_default, user_selectable, sort_order, created_at, updated_at) \
         VALUES (?, 'default', '', 1, 1, 0, ?, ?)",
        vec![
            default_group_id.clone().into(),
            now.clone().into(),
            now.clone().into(),
        ],
    )
    .await?;

    // GM-3/GM-4: one registry row per distinct canonical legacy label, sort_order by
    // ascending label order (BTreeSet iteration order).
    let mut labels: BTreeSet<String> = BTreeSet::new();
    for (table, column) in [
        ("monoize_providers", "groups"),
        ("users", "allowed_groups"),
        ("api_keys", "allowed_groups"),
        ("billing_plans", "allowed_groups"),
    ] {
        let rows = tx
            .query_all(Statement::from_string(
                backend,
                format!("SELECT {column} AS labels FROM {table}"),
            ))
            .await?;
        for row in rows {
            let raw: Option<String> = row.try_get("", "labels")?;
            labels.extend(decode_legacy_labels(raw.as_deref()));
        }
    }
    labels.remove("default");

    let mut label_ids: BTreeMap<String, String> = BTreeMap::new();
    label_ids.insert("default".to_string(), default_group_id.clone());
    for (index, label) in labels.iter().enumerate() {
        let id = Uuid::new_v4().to_string();
        let sort_order: i32 = (index + 1).try_into().unwrap_or(i32::MAX);
        execute_bound(
            tx,
            backend,
            "INSERT INTO monoize_groups (id, name, description, is_default, user_selectable, sort_order, created_at, updated_at) \
             VALUES (?, ?, '', 0, 0, ?, ?, ?)",
            vec![
                id.clone().into(),
                label.clone().into(),
                sort_order.into(),
                now.clone().into(),
                now.clone().into(),
            ],
        )
        .await?;
        label_ids.insert(label.clone(), id);
    }

    // GM-5: users get a single group id (alphabetically first legacy label, default otherwise).
    add_column(tx, backend, "users", "group_id", "TEXT NOT NULL DEFAULT ''").await?;
    let rows = tx
        .query_all(Statement::from_string(
            backend,
            "SELECT id, allowed_groups FROM users".to_string(),
        ))
        .await?;
    for row in rows {
        let id: String = row.try_get("", "id")?;
        let raw: Option<String> = row.try_get("", "allowed_groups")?;
        let row_labels = decode_legacy_labels(raw.as_deref());
        let group_id = row_labels
            .iter()
            .next()
            .and_then(|label| label_ids.get(label))
            .cloned()
            .unwrap_or_else(|| default_group_id.clone());
        execute_bound(
            tx,
            backend,
            "UPDATE users SET group_id = ? WHERE id = ?",
            vec![group_id.into(), id.into()],
        )
        .await?;
    }
    drop_column(tx, backend, "users", "allowed_groups").await?;

    // GM-6: non-empty legacy key groups become an explicit ordered selection.
    add_column(
        tx,
        backend,
        "api_keys",
        "use_user_group",
        "INTEGER NOT NULL DEFAULT 1",
    )
    .await?;
    add_column(
        tx,
        backend,
        "api_keys",
        "group_ids",
        "TEXT NOT NULL DEFAULT '[]'",
    )
    .await?;
    let rows = tx
        .query_all(Statement::from_string(
            backend,
            "SELECT id, allowed_groups FROM api_keys".to_string(),
        ))
        .await?;
    for row in rows {
        let id: String = row.try_get("", "id")?;
        let raw: Option<String> = row.try_get("", "allowed_groups")?;
        let row_labels = decode_legacy_labels(raw.as_deref());
        if row_labels.is_empty() {
            continue;
        }
        let ids: Vec<String> = row_labels
            .iter()
            .filter_map(|label| label_ids.get(label).cloned())
            .collect();
        execute_bound(
            tx,
            backend,
            "UPDATE api_keys SET use_user_group = 0, group_ids = ? WHERE id = ?",
            vec![encode_ids(&ids)?.into(), id.into()],
        )
        .await?;
    }
    drop_column(tx, backend, "api_keys", "allowed_groups").await?;
    drop_column(tx, backend, "api_keys", "token_group").await?;

    // GM-7: legacy "public" providers (empty groups) are bound to the default group so
    // every provider row ends with a non-empty group set (GR-I2).
    add_column(
        tx,
        backend,
        "monoize_providers",
        "group_ids",
        "TEXT NOT NULL DEFAULT '[]'",
    )
    .await?;
    let rows = tx
        .query_all(Statement::from_string(
            backend,
            "SELECT id, groups FROM monoize_providers".to_string(),
        ))
        .await?;
    for row in rows {
        let id: String = row.try_get("", "id")?;
        let raw: Option<String> = row.try_get("", "groups")?;
        let row_labels = decode_legacy_labels(raw.as_deref());
        let ids: Vec<String> = if row_labels.is_empty() {
            vec![default_group_id.clone()]
        } else {
            row_labels
                .iter()
                .filter_map(|label| label_ids.get(label).cloned())
                .collect()
        };
        execute_bound(
            tx,
            backend,
            "UPDATE monoize_providers SET group_ids = ? WHERE id = ?",
            vec![encode_ids(&ids)?.into(), id.into()],
        )
        .await?;
    }
    drop_column(tx, backend, "monoize_providers", "groups").await?;

    // GM-8: plan ceilings keep multi-group semantics; empty stays unrestricted.
    add_column(
        tx,
        backend,
        "billing_plans",
        "group_ids",
        "TEXT NOT NULL DEFAULT '[]'",
    )
    .await?;
    let rows = tx
        .query_all(Statement::from_string(
            backend,
            "SELECT id, allowed_groups FROM billing_plans".to_string(),
        ))
        .await?;
    for row in rows {
        let id: String = row.try_get("", "id")?;
        let raw: Option<String> = row.try_get("", "allowed_groups")?;
        let row_labels = decode_legacy_labels(raw.as_deref());
        if row_labels.is_empty() {
            continue;
        }
        let ids: Vec<String> = row_labels
            .iter()
            .filter_map(|label| label_ids.get(label).cloned())
            .collect();
        execute_bound(
            tx,
            backend,
            "UPDATE billing_plans SET group_ids = ? WHERE id = ?",
            vec![encode_ids(&ids)?.into(), id.into()],
        )
        .await?;
    }
    drop_column(tx, backend, "billing_plans", "allowed_groups").await?;

    Ok(())
}

async fn migrate_down(tx: &DatabaseTransaction, backend: DbBackend) -> Result<(), DbErr> {
    let rows = tx
        .query_all(Statement::from_string(
            backend,
            "SELECT id, name, is_default FROM monoize_groups".to_string(),
        ))
        .await?;
    let mut group_names: BTreeMap<String, String> = BTreeMap::new();
    let mut default_group_id = String::new();
    for row in rows {
        let id: String = row.try_get("", "id")?;
        let name: String = row.try_get("", "name")?;
        let is_default: i32 = row.try_get("", "is_default")?;
        if is_default != 0 {
            default_group_id = id.clone();
        }
        group_names.insert(id, name.trim().to_lowercase());
    }

    add_column(
        tx,
        backend,
        "users",
        "allowed_groups",
        "TEXT NOT NULL DEFAULT '[]'",
    )
    .await?;
    let rows = tx
        .query_all(Statement::from_string(
            backend,
            "SELECT id, group_id FROM users".to_string(),
        ))
        .await?;
    for row in rows {
        let id: String = row.try_get("", "id")?;
        let group_id: String = row.try_get("", "group_id")?;
        let labels: Vec<String> = group_names.get(&group_id).cloned().into_iter().collect();
        execute_bound(
            tx,
            backend,
            "UPDATE users SET allowed_groups = ? WHERE id = ?",
            vec![encode_ids(&labels)?.into(), id.into()],
        )
        .await?;
    }
    drop_column(tx, backend, "users", "group_id").await?;

    add_column(
        tx,
        backend,
        "api_keys",
        "allowed_groups",
        "TEXT NOT NULL DEFAULT '[]'",
    )
    .await?;
    add_column(
        tx,
        backend,
        "api_keys",
        "token_group",
        "TEXT NOT NULL DEFAULT 'default'",
    )
    .await?;
    let rows = tx
        .query_all(Statement::from_string(
            backend,
            "SELECT id, use_user_group, group_ids FROM api_keys".to_string(),
        ))
        .await?;
    for row in rows {
        let id: String = row.try_get("", "id")?;
        let use_user_group: i32 = row.try_get("", "use_user_group")?;
        if use_user_group != 0 {
            continue;
        }
        let raw: Option<String> = row.try_get("", "group_ids")?;
        let labels = decode_id_labels(raw.as_deref(), &group_names);
        if labels.is_empty() {
            continue;
        }
        execute_bound(
            tx,
            backend,
            "UPDATE api_keys SET allowed_groups = ? WHERE id = ?",
            vec![encode_ids(&labels)?.into(), id.into()],
        )
        .await?;
    }
    drop_column(tx, backend, "api_keys", "use_user_group").await?;
    drop_column(tx, backend, "api_keys", "group_ids").await?;

    add_column(
        tx,
        backend,
        "monoize_providers",
        "groups",
        "TEXT NOT NULL DEFAULT '[]'",
    )
    .await?;
    let rows = tx
        .query_all(Statement::from_string(
            backend,
            "SELECT id, group_ids FROM monoize_providers".to_string(),
        ))
        .await?;
    for row in rows {
        let id: String = row.try_get("", "id")?;
        let raw: Option<String> = row.try_get("", "group_ids")?;
        let stored: Vec<String> =
            serde_json::from_str(raw.as_deref().unwrap_or("[]")).unwrap_or_default();
        // A provider whose only group is the default group was a legacy "public" provider.
        if stored == [default_group_id.clone()] {
            continue;
        }
        let labels = decode_id_labels(raw.as_deref(), &group_names);
        if labels.is_empty() {
            continue;
        }
        execute_bound(
            tx,
            backend,
            "UPDATE monoize_providers SET groups = ? WHERE id = ?",
            vec![encode_ids(&labels)?.into(), id.into()],
        )
        .await?;
    }
    drop_column(tx, backend, "monoize_providers", "group_ids").await?;

    add_column(
        tx,
        backend,
        "billing_plans",
        "allowed_groups",
        "TEXT NOT NULL DEFAULT '[]'",
    )
    .await?;
    let rows = tx
        .query_all(Statement::from_string(
            backend,
            "SELECT id, group_ids FROM billing_plans".to_string(),
        ))
        .await?;
    for row in rows {
        let id: String = row.try_get("", "id")?;
        let raw: Option<String> = row.try_get("", "group_ids")?;
        let labels = decode_id_labels(raw.as_deref(), &group_names);
        if labels.is_empty() {
            continue;
        }
        execute_bound(
            tx,
            backend,
            "UPDATE billing_plans SET allowed_groups = ? WHERE id = ?",
            vec![encode_ids(&labels)?.into(), id.into()],
        )
        .await?;
    }
    drop_column(tx, backend, "billing_plans", "group_ids").await?;

    tx.execute(Statement::from_string(
        backend,
        "DROP TABLE monoize_groups".to_string(),
    ))
    .await?;

    Ok(())
}

/// Decode one legacy label-array cell with the legacy runtime canonicalization:
/// trim, lowercase, drop empties, deduplicate. Malformed values contribute zero labels
/// (GM-3 requires the migration to survive corrupt legacy rows).
fn decode_legacy_labels(raw: Option<&str>) -> BTreeSet<String> {
    let Some(raw) = raw else {
        return BTreeSet::new();
    };
    let parsed: Vec<String> = serde_json::from_str(raw).unwrap_or_default();
    parsed
        .into_iter()
        .map(|label| label.trim().to_lowercase())
        .filter(|label| !label.is_empty())
        .collect()
}

fn decode_id_labels(raw: Option<&str>, group_names: &BTreeMap<String, String>) -> Vec<String> {
    let stored: Vec<String> = serde_json::from_str(raw.unwrap_or("[]")).unwrap_or_default();
    let mut labels: Vec<String> = Vec::new();
    for id in stored {
        if let Some(name) = group_names.get(&id)
            && !labels.contains(name)
        {
            labels.push(name.clone());
        }
    }
    labels
}

fn encode_ids(ids: &[String]) -> Result<String, DbErr> {
    serde_json::to_string(ids).map_err(|error| DbErr::Custom(error.to_string()))
}

async fn add_column(
    tx: &DatabaseTransaction,
    backend: DbBackend,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<(), DbErr> {
    let sql = match backend {
        DbBackend::Postgres => {
            format!("ALTER TABLE {table} ADD COLUMN IF NOT EXISTS {column} {definition}")
        }
        _ => format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
    };
    tx.execute(Statement::from_string(backend, sql)).await?;
    Ok(())
}

async fn drop_column(
    tx: &DatabaseTransaction,
    backend: DbBackend,
    table: &str,
    column: &str,
) -> Result<(), DbErr> {
    let sql = match backend {
        DbBackend::Postgres => format!("ALTER TABLE {table} DROP COLUMN IF EXISTS {column}"),
        _ => format!("ALTER TABLE {table} DROP COLUMN {column}"),
    };
    tx.execute(Statement::from_string(backend, sql)).await?;
    Ok(())
}

async fn execute_bound(
    tx: &DatabaseTransaction,
    backend: DbBackend,
    sql: &str,
    values: Vec<Value>,
) -> Result<(), DbErr> {
    tx.execute(Statement::from_sql_and_values(
        backend,
        numbered_placeholders(backend, sql),
        values,
    ))
    .await?;
    Ok(())
}

/// Rewrite `?` placeholders as `$1..$n` for PostgreSQL. No SQL text in this migration
/// contains a literal question mark, so a plain character scan is sufficient.
fn numbered_placeholders(backend: DbBackend, sql: &str) -> String {
    if backend != DbBackend::Postgres {
        return sql.to_string();
    }
    let mut out = String::with_capacity(sql.len() + 8);
    let mut n = 0usize;
    for ch in sql.chars() {
        if ch == '?' {
            n += 1;
            out.push('$');
            out.push_str(&n.to_string());
        } else {
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use crate::db::DbPool;
    use crate::migration::Migrator;
    use crate::users::UserRole;
    use sea_orm::ConnectionTrait;
    use sea_orm_migration::MigratorTrait;
    use serde_json::json;
    use std::collections::BTreeMap;

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

    /// GM-1..GM-8 on a populated legacy database: run the full stack up (empty
    /// path), revert through this migration to the legacy label schema, plant
    /// legacy label arrays, then re-run the migration and verify every mapping rule.
    #[tokio::test]
    async fn groups_registry_migration_maps_populated_legacy_labels() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("db connects");
        {
            let write = db.write().await;
            Migrator::up(&*write, None).await.expect("empty-db up");
        }

        // Seed rows through the current stores so every non-group column is valid.
        let (log_tx, _) = tokio::sync::broadcast::channel(1);
        let user_store = crate::users::UserStore::new(db.clone(), log_tx)
            .await
            .expect("user store creates");
        let user = user_store
            .create_user("legacy-user", "password123", UserRole::User, None)
            .await
            .expect("user creates");
        let (inherit_key, _) = user_store
            .create_api_key(&user.id, "inherit-key", None)
            .await
            .expect("inherit key creates");
        let (scoped_key, _) = user_store
            .create_api_key(&user.id, "scoped-key", None)
            .await
            .expect("scoped key creates");
        let plan = user_store
            .create_billing_plan(crate::users::BillingPlanInput {
                name: "legacy-plan".to_string(),
                grant_amount_nano_usd: Some("0".to_string()),
                grant_amount_usd: None,
                schedule: "0 0 * * *".to_string(),
                group_ids: None,
                enabled: Some(true),
            })
            .await
            .expect("plan storage ok")
            .expect("plan creates");
        drop(user_store);

        let routing_store = crate::monoize_routing::MonoizeRoutingStore::new(db.clone())
            .await
            .expect("routing store creates");
        let channel = json!({
            "name": "primary",
            "provider_type": "responses",
            "base_url": "https://example.com",
            "api_key": "secret",
            "models": { "gpt-5": { "redirect": null, "multiplier": "1" } }
        });
        let public_provider = routing_store
            .create_provider(
                serde_json::from_value(json!({ "name": "public", "channels": [channel.clone()] }))
                    .expect("payload deserializes"),
            )
            .await
            .expect("public provider creates");
        let scoped_provider = routing_store
            .create_provider(
                serde_json::from_value(json!({ "name": "scoped", "channels": [channel] }))
                    .expect("payload deserializes"),
            )
            .await
            .expect("scoped provider creates");
        drop(routing_store);

        {
            let write = db.write().await;
            let down_steps = Migrator::migrations()
                .into_iter()
                .rev()
                .position(|migration| migration.name() == "m20260825_000042_groups_registry")
                .map(|index| index + 1)
                .expect("groups registry migration is registered");
            Migrator::down(&*write, Some(down_steps as u32))
                .await
                .expect("down through groups registry restores legacy schema");
        }

        // Plant legacy labels exactly as the pre-registry system stored them.
        exec(
            &db,
            &format!(
                r#"UPDATE users SET allowed_groups = '[" Team-A ","beta","TEAM-A"]' WHERE id = '{}'"#,
                user.id
            ),
        )
        .await;
        exec(
            &db,
            &format!(
                r#"UPDATE api_keys SET allowed_groups = '["beta"]' WHERE id = '{}'"#,
                scoped_key.id
            ),
        )
        .await;
        exec(
            &db,
            &format!(
                r#"UPDATE monoize_providers SET groups = '["Team-A"]' WHERE id = '{}'"#,
                scoped_provider.id
            ),
        )
        .await;
        exec(
            &db,
            &format!(
                r#"UPDATE billing_plans SET allowed_groups = '["team-a","gamma"]' WHERE id = '{}'"#,
                plan.id
            ),
        )
        .await;

        {
            let write = db.write().await;
            Migrator::up(&*write, None).await.expect("populated-db up");
        }

        // GM-2: exactly one default row named "default".
        assert_eq!(
            scalar(
                &db,
                "SELECT CAST(COUNT(*) AS TEXT) AS value FROM monoize_groups WHERE is_default = 1"
            )
            .await,
            "1"
        );
        let rows = db
            .read()
            .query_all(db.stmt(
                "SELECT id, name, sort_order FROM monoize_groups ORDER BY sort_order",
                vec![],
            ))
            .await
            .expect("groups query succeeds");
        let mut ids: BTreeMap<String, String> = BTreeMap::new();
        let mut ordered_names: Vec<String> = Vec::new();
        for row in rows {
            let id: String = row.try_get("", "id").expect("id");
            let name: String = row.try_get("", "name").expect("name");
            ordered_names.push(name.clone());
            ids.insert(name, id);
        }
        // GM-3/GM-4: canonical labels, alphabetical sort order after default.
        assert_eq!(ordered_names, vec!["default", "beta", "gamma", "team-a"]);

        // GM-5: alphabetically first legacy label wins for the user.
        assert_eq!(
            scalar(
                &db,
                &format!(
                    "SELECT group_id AS value FROM users WHERE id = '{}'",
                    user.id
                )
            )
            .await,
            ids["beta"]
        );

        // GM-6: empty legacy key selection inherits; non-empty becomes explicit.
        assert_eq!(
            scalar(
                &db,
                &format!(
                    "SELECT CAST(use_user_group AS TEXT) || '|' || group_ids AS value \
                     FROM api_keys WHERE id = '{}'",
                    inherit_key.id
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
                    scoped_key.id
                )
            )
            .await,
            format!("0|[\"{}\"]", ids["beta"])
        );

        // GM-7: public providers bind to the default group; labeled ones map.
        assert_eq!(
            scalar(
                &db,
                &format!(
                    "SELECT group_ids AS value FROM monoize_providers WHERE id = '{}'",
                    public_provider.id
                )
            )
            .await,
            format!("[\"{}\"]", ids["default"])
        );
        assert_eq!(
            scalar(
                &db,
                &format!(
                    "SELECT group_ids AS value FROM monoize_providers WHERE id = '{}'",
                    scoped_provider.id
                )
            )
            .await,
            format!("[\"{}\"]", ids["team-a"])
        );

        // GM-8: plan ceilings map labels in canonical (alphabetical) order.
        assert_eq!(
            scalar(
                &db,
                &format!(
                    "SELECT group_ids AS value FROM billing_plans WHERE id = '{}'",
                    plan.id
                )
            )
            .await,
            format!("[\"{}\",\"{}\"]", ids["gamma"], ids["team-a"])
        );

        // Legacy columns are gone (no compatibility aliases survive).
        for (table, column) in [
            ("users", "allowed_groups"),
            ("api_keys", "allowed_groups"),
            ("api_keys", "token_group"),
            ("monoize_providers", "groups"),
            ("billing_plans", "allowed_groups"),
        ] {
            assert_eq!(
                scalar(
                    &db,
                    &format!(
                        "SELECT CAST(COUNT(*) AS TEXT) AS value \
                         FROM pragma_table_info('{table}') WHERE name = '{column}'"
                    )
                )
                .await,
                "0",
                "{table}.{column} must be dropped"
            );
        }
    }
}
