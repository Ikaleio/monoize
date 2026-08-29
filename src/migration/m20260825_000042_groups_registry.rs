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
