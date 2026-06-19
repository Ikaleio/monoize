use sea_orm::{ConnectionTrait, DbBackend, Statement};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        let backend = manager.get_database_backend();

        create_channel_models_table(conn, backend).await?;
        add_channel_columns(conn, backend).await?;
        add_request_log_columns(conn, backend).await?;

        if column_exists(conn, backend, "monoize_providers", "provider_type").await? {
            conn.execute(Statement::from_string(
                backend,
                match backend {
                    DbBackend::Sqlite => {
                        "UPDATE monoize_channels
                         SET provider_type = COALESCE(
                           (SELECT provider_type FROM monoize_providers WHERE monoize_providers.id = monoize_channels.provider_id),
                           provider_type,
                           'chat_completion'
                         )"
                    }
                    DbBackend::Postgres => {
                        "UPDATE monoize_channels AS c
                         SET provider_type = COALESCE(p.provider_type, c.provider_type, 'chat_completion')
                         FROM monoize_providers AS p
                         WHERE p.id = c.provider_id"
                    }
                    _ => "",
                }
                .to_string(),
            ))
            .await?;

            drop_column_if_exists(conn, backend, "monoize_providers", "provider_type").await?;
        }

        populate_channel_models(conn, backend).await?;

        for table in ["group_members", "model_mappings", "providers"] {
            conn.execute(Statement::from_string(
                backend,
                format!("DROP TABLE IF EXISTS {table}"),
            ))
            .await?;
        }

        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Ok(())
    }
}

async fn create_channel_models_table(
    conn: &SchemaManagerConnection<'_>,
    backend: DbBackend,
) -> Result<(), DbErr> {
    let sql = match backend {
        DbBackend::Sqlite => {
            r#"CREATE TABLE IF NOT EXISTS monoize_channel_models (
                id TEXT PRIMARY KEY NOT NULL,
                channel_id TEXT NOT NULL,
                model_name TEXT NOT NULL,
                created_at TEXT NOT NULL,
                FOREIGN KEY(channel_id) REFERENCES monoize_channels(id) ON DELETE CASCADE
            )"#
        }
        DbBackend::Postgres => {
            r#"CREATE TABLE IF NOT EXISTS monoize_channel_models (
                id TEXT PRIMARY KEY NOT NULL,
                channel_id TEXT NOT NULL REFERENCES monoize_channels(id) ON DELETE CASCADE,
                model_name TEXT NOT NULL,
                created_at TEXT NOT NULL
            )"#
        }
        _ => return Ok(()),
    };
    conn.execute(Statement::from_string(backend, sql.to_string()))
        .await?;
    conn.execute(Statement::from_string(
        backend,
        "CREATE INDEX IF NOT EXISTS idx_mcm_channel_id ON monoize_channel_models(channel_id)"
            .to_string(),
    ))
    .await?;
    conn.execute(Statement::from_string(
        backend,
        "CREATE UNIQUE INDEX IF NOT EXISTS uq_mcm_channel_id_model_name ON monoize_channel_models(channel_id, model_name)"
            .to_string(),
    ))
    .await?;
    Ok(())
}

async fn add_channel_columns(
    conn: &SchemaManagerConnection<'_>,
    backend: DbBackend,
) -> Result<(), DbErr> {
    add_column_if_missing(
        conn,
        backend,
        "monoize_channels",
        "provider_type",
        "TEXT NOT NULL DEFAULT 'chat_completion'",
    )
    .await?;
    add_column_if_missing(
        conn,
        backend,
        "monoize_channels",
        "active_probe_enabled_override",
        "INTEGER",
    )
    .await?;
    add_column_if_missing(
        conn,
        backend,
        "monoize_channels",
        "active_probe_interval_seconds_override",
        "INTEGER",
    )
    .await?;
    add_column_if_missing(
        conn,
        backend,
        "monoize_channels",
        "active_probe_success_threshold_override",
        "INTEGER",
    )
    .await?;
    add_column_if_missing(
        conn,
        backend,
        "monoize_channels",
        "active_probe_model_override",
        "TEXT",
    )
    .await?;
    Ok(())
}

async fn add_request_log_columns(
    conn: &SchemaManagerConnection<'_>,
    backend: DbBackend,
) -> Result<(), DbErr> {
    for (column, definition) in [
        ("effective_provider_type", "TEXT"),
        ("affinity_hit", "INTEGER"),
        ("affinity_key_hash", "TEXT"),
        ("affinity_target", "TEXT"),
    ] {
        add_column_if_missing(conn, backend, "request_logs", column, definition).await?;
    }
    Ok(())
}

async fn populate_channel_models(
    conn: &SchemaManagerConnection<'_>,
    backend: DbBackend,
) -> Result<(), DbErr> {
    let sql = match backend {
        DbBackend::Sqlite => {
            r#"INSERT OR IGNORE INTO monoize_channel_models (id, channel_id, model_name, created_at)
               SELECT 'mono_ch_model_' || lower(hex(randomblob(16))), c.id, m.model_name,
                      strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
               FROM monoize_channels c
               JOIN monoize_provider_models m ON m.provider_id = c.provider_id"#
        }
        DbBackend::Postgres => {
            r#"INSERT INTO monoize_channel_models (id, channel_id, model_name, created_at)
               SELECT 'mono_ch_model_' || md5(c.id || ':' || m.model_name), c.id, m.model_name,
                      to_char((now() AT TIME ZONE 'UTC'), 'YYYY-MM-DD"T"HH24:MI:SS"Z"')
               FROM monoize_channels c
               JOIN monoize_provider_models m ON m.provider_id = c.provider_id
               ON CONFLICT (channel_id, model_name) DO NOTHING"#
        }
        _ => return Ok(()),
    };
    conn.execute(Statement::from_string(backend, sql.to_string()))
        .await?;
    Ok(())
}

async fn add_column_if_missing(
    conn: &SchemaManagerConnection<'_>,
    backend: DbBackend,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<(), DbErr> {
    if column_exists(conn, backend, table, column).await? {
        return Ok(());
    }
    let sql = match backend {
        DbBackend::Sqlite => format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
        DbBackend::Postgres => {
            format!("ALTER TABLE {table} ADD COLUMN IF NOT EXISTS {column} {definition}")
        }
        _ => return Ok(()),
    };
    conn.execute(Statement::from_string(backend, sql)).await?;
    Ok(())
}

async fn drop_column_if_exists(
    conn: &SchemaManagerConnection<'_>,
    backend: DbBackend,
    table: &str,
    column: &str,
) -> Result<(), DbErr> {
    if !column_exists(conn, backend, table, column).await? {
        return Ok(());
    }
    let sql = match backend {
        DbBackend::Sqlite => format!("ALTER TABLE {table} DROP COLUMN {column}"),
        DbBackend::Postgres => format!("ALTER TABLE {table} DROP COLUMN IF EXISTS {column}"),
        _ => return Ok(()),
    };
    conn.execute(Statement::from_string(backend, sql)).await?;
    Ok(())
}

async fn column_exists(
    conn: &SchemaManagerConnection<'_>,
    backend: DbBackend,
    table: &str,
    column: &str,
) -> Result<bool, DbErr> {
    let sql = match backend {
        DbBackend::Sqlite => format!("PRAGMA table_info({table})"),
        DbBackend::Postgres => format!(
            "SELECT column_name AS name FROM information_schema.columns WHERE table_name = '{table}'"
        ),
        _ => return Ok(false),
    };
    let rows = conn.query_all(Statement::from_string(backend, sql)).await?;
    Ok(rows
        .into_iter()
        .filter_map(|row| row.try_get::<String>("", "name").ok())
        .any(|name| name == column))
}
