use sea_orm::{ConnectionTrait, DbBackend, Statement};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        let backend = manager.get_database_backend();

        add_column_if_missing(conn, backend, "redirect", "TEXT").await?;
        add_column_if_missing(conn, backend, "multiplier", "REAL NOT NULL DEFAULT 1.0").await?;

        if manager.has_table("monoize_provider_models").await? {
            let sql = match backend {
                DbBackend::Sqlite => {
                    r#"UPDATE monoize_channel_models
                       SET redirect = (
                             SELECT pm.redirect
                             FROM monoize_channels c
                             JOIN monoize_provider_models pm ON pm.provider_id = c.provider_id
                             WHERE c.id = monoize_channel_models.channel_id
                               AND pm.model_name = monoize_channel_models.model_name
                           ),
                           multiplier = COALESCE((
                             SELECT pm.multiplier
                             FROM monoize_channels c
                             JOIN monoize_provider_models pm ON pm.provider_id = c.provider_id
                             WHERE c.id = monoize_channel_models.channel_id
                               AND pm.model_name = monoize_channel_models.model_name
                           ), 1.0)"#
                }
                DbBackend::Postgres => {
                    r#"UPDATE monoize_channel_models AS cm
                       SET redirect = pm.redirect, multiplier = pm.multiplier
                       FROM monoize_channels AS c
                       JOIN monoize_provider_models AS pm ON pm.provider_id = c.provider_id
                       WHERE c.id = cm.channel_id AND pm.model_name = cm.model_name"#
                }
                _ => "",
            };
            if !sql.is_empty() {
                conn.execute(Statement::from_string(backend, sql.to_string()))
                    .await?;
            }
            conn.execute(Statement::from_string(
                backend,
                "DROP TABLE monoize_provider_models".to_string(),
            ))
            .await?;
        }

        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Ok(())
    }
}

async fn add_column_if_missing(
    conn: &SchemaManagerConnection<'_>,
    backend: DbBackend,
    column: &str,
    definition: &str,
) -> Result<(), DbErr> {
    if column_exists(conn, backend, column).await? {
        return Ok(());
    }
    let sql = match backend {
        DbBackend::Sqlite => {
            format!("ALTER TABLE monoize_channel_models ADD COLUMN {column} {definition}")
        }
        DbBackend::Postgres => format!(
            "ALTER TABLE monoize_channel_models ADD COLUMN IF NOT EXISTS {column} {definition}"
        ),
        _ => return Ok(()),
    };
    conn.execute(Statement::from_string(backend, sql)).await?;
    Ok(())
}

async fn column_exists(
    conn: &SchemaManagerConnection<'_>,
    backend: DbBackend,
    column: &str,
) -> Result<bool, DbErr> {
    let (sql, values) = match backend {
        DbBackend::Sqlite => (
            "SELECT COUNT(*) AS n FROM pragma_table_info('monoize_channel_models') WHERE name = ?",
            vec![column.into()],
        ),
        DbBackend::Postgres => (
            "SELECT COUNT(*) AS n FROM information_schema.columns WHERE table_schema = current_schema() AND table_name = $1 AND column_name = $2",
            vec!["monoize_channel_models".into(), column.into()],
        ),
        _ => return Ok(false),
    };
    let row = conn
        .query_one(Statement::from_sql_and_values(backend, sql, values))
        .await?;
    let count: i64 = row
        .and_then(|value| value.try_get("", "n").ok())
        .unwrap_or(0);
    Ok(count > 0)
}
