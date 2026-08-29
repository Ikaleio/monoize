use std::collections::BTreeSet;

use sea_orm::{ConnectionTrait, DatabaseTransaction, DbBackend, Statement, TransactionTrait};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[derive(Clone, Copy)]
struct RequestLogColumn {
    name: &'static str,
    sqlite_definition: &'static str,
    postgres_definition: &'static str,
    required: bool,
}

const REQUEST_LOG_COLUMNS: [RequestLogColumn; 42] = [
    RequestLogColumn {
        name: "id",
        sqlite_definition: "TEXT NOT NULL PRIMARY KEY",
        postgres_definition: "TEXT",
        required: true,
    },
    RequestLogColumn {
        name: "request_id",
        sqlite_definition: "TEXT",
        postgres_definition: "TEXT",
        required: false,
    },
    RequestLogColumn {
        name: "user_id",
        sqlite_definition: "TEXT NOT NULL",
        postgres_definition: "TEXT",
        required: true,
    },
    RequestLogColumn {
        name: "api_key_id",
        sqlite_definition: "TEXT",
        postgres_definition: "TEXT",
        required: false,
    },
    RequestLogColumn {
        name: "model",
        sqlite_definition: "TEXT NOT NULL",
        postgres_definition: "TEXT",
        required: true,
    },
    RequestLogColumn {
        name: "provider_id",
        sqlite_definition: "TEXT",
        postgres_definition: "TEXT",
        required: false,
    },
    RequestLogColumn {
        name: "upstream_model",
        sqlite_definition: "TEXT",
        postgres_definition: "TEXT",
        required: false,
    },
    RequestLogColumn {
        name: "channel_id",
        sqlite_definition: "TEXT",
        postgres_definition: "TEXT",
        required: false,
    },
    RequestLogColumn {
        name: "is_stream",
        sqlite_definition: "INTEGER NOT NULL DEFAULT 0",
        postgres_definition: "INTEGER",
        required: true,
    },
    RequestLogColumn {
        name: "input_tokens",
        sqlite_definition: "INTEGER",
        postgres_definition: "BIGINT",
        required: false,
    },
    RequestLogColumn {
        name: "output_tokens",
        sqlite_definition: "INTEGER",
        postgres_definition: "BIGINT",
        required: false,
    },
    RequestLogColumn {
        name: "cache_read_tokens",
        sqlite_definition: "INTEGER",
        postgres_definition: "BIGINT",
        required: false,
    },
    RequestLogColumn {
        name: "cache_creation_tokens",
        sqlite_definition: "INTEGER",
        postgres_definition: "BIGINT",
        required: false,
    },
    RequestLogColumn {
        name: "tool_prompt_tokens",
        sqlite_definition: "INTEGER",
        postgres_definition: "BIGINT",
        required: false,
    },
    RequestLogColumn {
        name: "reasoning_tokens",
        sqlite_definition: "INTEGER",
        postgres_definition: "BIGINT",
        required: false,
    },
    RequestLogColumn {
        name: "accepted_prediction_tokens",
        sqlite_definition: "INTEGER",
        postgres_definition: "BIGINT",
        required: false,
    },
    RequestLogColumn {
        name: "rejected_prediction_tokens",
        sqlite_definition: "INTEGER",
        postgres_definition: "BIGINT",
        required: false,
    },
    RequestLogColumn {
        name: "provider_multiplier",
        sqlite_definition: "TEXT",
        postgres_definition: "TEXT",
        required: false,
    },
    RequestLogColumn {
        name: "charge_nano_usd",
        sqlite_definition: "TEXT",
        postgres_definition: "TEXT",
        required: false,
    },
    RequestLogColumn {
        name: "status",
        sqlite_definition: "TEXT NOT NULL",
        postgres_definition: "TEXT",
        required: true,
    },
    RequestLogColumn {
        name: "usage_breakdown_json",
        sqlite_definition: "TEXT",
        postgres_definition: "TEXT",
        required: false,
    },
    RequestLogColumn {
        name: "billing_breakdown_json",
        sqlite_definition: "TEXT",
        postgres_definition: "TEXT",
        required: false,
    },
    RequestLogColumn {
        name: "error_code",
        sqlite_definition: "TEXT",
        postgres_definition: "TEXT",
        required: false,
    },
    RequestLogColumn {
        name: "error_message",
        sqlite_definition: "TEXT",
        postgres_definition: "TEXT",
        required: false,
    },
    RequestLogColumn {
        name: "error_http_status",
        sqlite_definition: "INTEGER",
        postgres_definition: "BIGINT",
        required: false,
    },
    RequestLogColumn {
        name: "duration_ms",
        sqlite_definition: "INTEGER",
        postgres_definition: "BIGINT",
        required: false,
    },
    RequestLogColumn {
        name: "ttfb_ms",
        sqlite_definition: "INTEGER",
        postgres_definition: "BIGINT",
        required: false,
    },
    RequestLogColumn {
        name: "first_visible_output_ms",
        sqlite_definition: "INTEGER",
        postgres_definition: "BIGINT",
        required: false,
    },
    RequestLogColumn {
        name: "last_visible_output_ms",
        sqlite_definition: "INTEGER",
        postgres_definition: "BIGINT",
        required: false,
    },
    RequestLogColumn {
        name: "visible_generation_ms",
        sqlite_definition: "INTEGER",
        postgres_definition: "BIGINT",
        required: false,
    },
    RequestLogColumn {
        name: "visible_output_tokens",
        sqlite_definition: "INTEGER",
        postgres_definition: "BIGINT",
        required: false,
    },
    RequestLogColumn {
        name: "tps_mode",
        sqlite_definition: "TEXT",
        postgres_definition: "TEXT",
        required: false,
    },
    RequestLogColumn {
        name: "request_ip",
        sqlite_definition: "TEXT",
        postgres_definition: "TEXT",
        required: false,
    },
    RequestLogColumn {
        name: "reasoning_effort",
        sqlite_definition: "TEXT",
        postgres_definition: "TEXT",
        required: false,
    },
    RequestLogColumn {
        name: "tried_providers_json",
        sqlite_definition: "TEXT",
        postgres_definition: "TEXT",
        required: false,
    },
    RequestLogColumn {
        name: "request_kind",
        sqlite_definition: "TEXT",
        postgres_definition: "TEXT",
        required: false,
    },
    RequestLogColumn {
        name: "effective_provider_type",
        sqlite_definition: "TEXT",
        postgres_definition: "TEXT",
        required: false,
    },
    RequestLogColumn {
        name: "affinity_hit",
        sqlite_definition: "INTEGER",
        postgres_definition: "INTEGER",
        required: false,
    },
    RequestLogColumn {
        name: "affinity_key_hash",
        sqlite_definition: "TEXT",
        postgres_definition: "TEXT",
        required: false,
    },
    RequestLogColumn {
        name: "affinity_target",
        sqlite_definition: "TEXT",
        postgres_definition: "TEXT",
        required: false,
    },
    RequestLogColumn {
        name: "created_at",
        sqlite_definition: "TEXT NOT NULL",
        postgres_definition: "TEXT",
        required: true,
    },
    RequestLogColumn {
        name: "created_at_unix_ms",
        sqlite_definition: "INTEGER",
        postgres_definition: "BIGINT",
        required: false,
    },
];

const LEGACY_TOKEN_COLUMNS: [(&str, &str); 3] = [
    ("input_tokens", "prompt_tokens"),
    ("output_tokens", "completion_tokens"),
    ("cache_read_tokens", "cached_tokens"),
];

const REQUEST_LOG_INDEX_SQL: [&str; 4] = [
    "CREATE INDEX idx_request_logs_user_created_at ON request_logs (user_id, created_at_unix_ms DESC)",
    "CREATE INDEX idx_request_logs_created_at ON request_logs (created_at_unix_ms DESC)",
    "CREATE INDEX idx_request_logs_model ON request_logs (model)",
    "CREATE INDEX idx_request_logs_legacy_created_at ON request_logs (created_at) WHERE created_at_unix_ms IS NULL",
];

const POSTGRES_DROP_USER_CONSTRAINTS: [&str; 2] = [
    "ALTER TABLE request_logs DROP CONSTRAINT IF EXISTS request_logs_user_id_fkey",
    "ALTER TABLE request_logs DROP CONSTRAINT IF EXISTS fk_request_logs_user_id",
];

const POSTGRES_ORDINARY_INDEX_DROP_QUERY: &str = r#"
SELECT format('DROP INDEX IF EXISTS %I.%I', ns.nspname, index_class.relname) AS drop_sql
FROM pg_class AS table_class
JOIN pg_namespace AS ns ON ns.oid = table_class.relnamespace
JOIN pg_index AS index_meta ON index_meta.indrelid = table_class.oid
JOIN pg_class AS index_class ON index_class.oid = index_meta.indexrelid
LEFT JOIN pg_constraint AS constraint_meta ON constraint_meta.conindid = index_class.oid
WHERE ns.nspname = current_schema()
  AND table_class.relname = 'request_logs'
  AND table_class.relkind IN ('r', 'p')
  AND constraint_meta.oid IS NULL
ORDER BY index_class.relname
"#;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        if !matches!(backend, DbBackend::Sqlite | DbBackend::Postgres) {
            return Ok(());
        }

        let tx = manager.get_connection().begin().await?;
        let result = match backend {
            DbBackend::Sqlite => migrate_sqlite(&tx).await,
            DbBackend::Postgres => migrate_postgres(&tx).await,
            _ => unreachable!(),
        };
        if let Err(error) = result {
            let _ = tx.rollback().await;
            return Err(error);
        }
        tx.commit().await
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Ok(())
    }
}

async fn migrate_sqlite(tx: &DatabaseTransaction) -> Result<(), DbErr> {
    let source_columns = request_log_columns(tx, DbBackend::Sqlite).await?;
    validate_required_columns(&source_columns)?;

    let definitions = REQUEST_LOG_COLUMNS
        .iter()
        .map(|column| {
            format!(
                "{} {}",
                quote_identifier(column.name),
                column.sqlite_definition
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    let target_columns = REQUEST_LOG_COLUMNS
        .iter()
        .map(|column| quote_identifier(column.name))
        .collect::<Vec<_>>()
        .join(", ");
    let projections = REQUEST_LOG_COLUMNS
        .iter()
        .map(|column| source_projection(column.name, &source_columns))
        .collect::<Vec<_>>()
        .join(", ");

    let mut statements = vec![
        "DROP TABLE IF EXISTS request_logs_without_user_fk".to_string(),
        format!("CREATE TABLE request_logs_without_user_fk ({definitions})"),
        format!(
            "INSERT INTO request_logs_without_user_fk ({target_columns}) SELECT {projections} FROM request_logs"
        ),
        "DROP TABLE request_logs".to_string(),
        "ALTER TABLE request_logs_without_user_fk RENAME TO request_logs".to_string(),
    ];
    statements.extend(REQUEST_LOG_INDEX_SQL.iter().map(|sql| (*sql).to_string()));
    execute_statements(tx, DbBackend::Sqlite, statements).await
}

async fn migrate_postgres(tx: &DatabaseTransaction) -> Result<(), DbErr> {
    let source_columns = request_log_columns(tx, DbBackend::Postgres).await?;
    let plan = postgres_plan(&source_columns)?;

    execute_statements(tx, DbBackend::Postgres, plan.prepare).await?;

    let index_drop_statements = tx
        .query_all(Statement::from_string(
            DbBackend::Postgres,
            POSTGRES_ORDINARY_INDEX_DROP_QUERY.to_string(),
        ))
        .await?
        .into_iter()
        .map(|row| row.try_get::<String>("", "drop_sql"))
        .collect::<Result<Vec<_>, _>>()?;
    execute_statements(tx, DbBackend::Postgres, index_drop_statements).await?;
    execute_statements(tx, DbBackend::Postgres, plan.drop_columns).await?;
    execute_statements(
        tx,
        DbBackend::Postgres,
        REQUEST_LOG_INDEX_SQL
            .iter()
            .map(|sql| (*sql).to_string())
            .collect(),
    )
    .await
}

struct PostgresPlan {
    prepare: Vec<String>,
    drop_columns: Vec<String>,
}

fn postgres_plan(source_columns: &BTreeSet<String>) -> Result<PostgresPlan, DbErr> {
    validate_required_columns(source_columns)?;
    let canonical_columns = canonical_column_names();
    let mut prepare = POSTGRES_DROP_USER_CONSTRAINTS
        .iter()
        .map(|sql| (*sql).to_string())
        .collect::<Vec<_>>();

    for column in REQUEST_LOG_COLUMNS {
        if !source_columns.contains(column.name) {
            prepare.push(format!(
                "ALTER TABLE request_logs ADD COLUMN {} {}",
                quote_identifier(column.name),
                column.postgres_definition
            ));
        }
    }

    for (canonical, legacy) in LEGACY_TOKEN_COLUMNS {
        if source_columns.contains(legacy) {
            prepare.push(format!(
                "UPDATE request_logs SET {canonical} = COALESCE({canonical}, {legacy}) WHERE {canonical} IS NULL AND {legacy} IS NOT NULL",
                canonical = quote_identifier(canonical),
                legacy = quote_identifier(legacy),
            ));
        }
    }

    let drop_columns = source_columns
        .difference(&canonical_columns)
        .map(|column| {
            format!(
                "ALTER TABLE request_logs DROP COLUMN {}",
                quote_identifier(column)
            )
        })
        .collect();

    Ok(PostgresPlan {
        prepare,
        drop_columns,
    })
}

fn validate_required_columns(source_columns: &BTreeSet<String>) -> Result<(), DbErr> {
    let missing = REQUEST_LOG_COLUMNS
        .iter()
        .filter(|column| column.required && !source_columns.contains(column.name))
        .map(|column| column.name)
        .collect::<Vec<_>>();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(DbErr::Custom(format!(
            "request_logs is missing required canonical columns: {}",
            missing.join(", ")
        )))
    }
}

fn source_projection(column: &str, source_columns: &BTreeSet<String>) -> String {
    let legacy = LEGACY_TOKEN_COLUMNS
        .iter()
        .find_map(|(canonical, legacy)| (*canonical == column).then_some(*legacy));
    match (source_columns.contains(column), legacy) {
        (true, Some(legacy)) if source_columns.contains(legacy) => format!(
            "COALESCE({}, {})",
            quote_identifier(column),
            quote_identifier(legacy)
        ),
        (true, _) => quote_identifier(column),
        (false, Some(legacy)) if source_columns.contains(legacy) => quote_identifier(legacy),
        (false, _) => "NULL".to_string(),
    }
}

fn canonical_column_names() -> BTreeSet<String> {
    REQUEST_LOG_COLUMNS
        .iter()
        .map(|column| column.name.to_string())
        .collect()
}

async fn request_log_columns(
    tx: &DatabaseTransaction,
    backend: DbBackend,
) -> Result<BTreeSet<String>, DbErr> {
    let sql = match backend {
        DbBackend::Sqlite => "PRAGMA table_info(request_logs)",
        DbBackend::Postgres => {
            "SELECT column_name AS name FROM information_schema.columns WHERE table_schema = current_schema() AND table_name = 'request_logs' ORDER BY ordinal_position"
        }
        _ => return Ok(BTreeSet::new()),
    };
    Ok(tx
        .query_all(Statement::from_string(backend, sql.to_string()))
        .await?
        .into_iter()
        .map(|row| row.try_get::<String>("", "name"))
        .collect::<Result<_, _>>()?)
}

async fn execute_statements(
    tx: &DatabaseTransaction,
    backend: DbBackend,
    statements: Vec<String>,
) -> Result<(), DbErr> {
    for sql in statements {
        tx.execute(Statement::from_string(backend, sql)).await?;
    }
    Ok(())
}

fn quote_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}
