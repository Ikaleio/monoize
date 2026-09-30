use sea_orm::{ConnectionTrait, DbBackend, Statement, TransactionTrait};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const READ_PAGE_ROWS: usize = 5_000;
const UPDATE_BATCH_ROWS: usize = 500;

async fn execute<C: ConnectionTrait>(
    connection: &C,
    backend: DbBackend,
    sql: &str,
) -> Result<(), DbErr> {
    connection
        .execute(Statement::from_string(backend, sql.to_string()))
        .await?;
    Ok(())
}

// Identifiers and timestamps come from the table itself; doubling quotes keeps
// the inlined literals exact on both backends.
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

async fn write_cumulatives<C: ConnectionTrait>(
    connection: &C,
    backend: DbBackend,
    rows: &[(String, i128)],
) -> Result<(), DbErr> {
    for batch in rows.chunks(UPDATE_BATCH_ROWS) {
        let mut cases = String::new();
        let mut ids = String::new();
        for (index, (id, cumulative)) in batch.iter().enumerate() {
            let id = quote(id);
            cases.push_str(&format!(" WHEN {id} THEN '{cumulative}'"));
            if index > 0 {
                ids.push_str(", ");
            }
            ids.push_str(&id);
        }
        execute(
            connection,
            backend,
            &format!(
                "UPDATE billing_plan_usage SET cumulative_nano_usd = CASE id{cases} END WHERE id IN ({ids})"
            ),
        )
        .await?;
    }
    Ok(())
}

// BP-M4: prefix sums per subscription in (created_at, id) order, using the
// database's own ordering so the backfill agrees with the runtime lookups.
async fn backfill<C: ConnectionTrait>(connection: &C, backend: DbBackend) -> Result<(), DbErr> {
    let mut cursor: Option<(String, String, String)> = None;
    let mut subscription_id = String::new();
    let mut running: i128 = 0;
    loop {
        let filter = cursor.as_ref().map_or(String::new(), |(sub, created, id)| {
            format!(
                "WHERE (subscription_id, created_at, id) > ({}, {}, {}) ",
                quote(sub),
                quote(created),
                quote(id)
            )
        });
        let rows = connection
            .query_all(Statement::from_string(
                backend,
                format!(
                    "SELECT id, subscription_id, amount_nano_usd, created_at FROM billing_plan_usage {filter}ORDER BY subscription_id ASC, created_at ASC, id ASC LIMIT {READ_PAGE_ROWS}"
                ),
            ))
            .await?;
        if rows.is_empty() {
            return Ok(());
        }
        let mut updates = Vec::with_capacity(rows.len());
        for row in &rows {
            let id = row.try_get::<String>("", "id")?;
            let row_subscription = row.try_get::<String>("", "subscription_id")?;
            let raw_amount = row.try_get::<String>("", "amount_nano_usd")?;
            let amount = raw_amount
                .trim()
                .parse::<i128>()
                .ok()
                .filter(|amount| *amount > 0)
                .ok_or_else(|| {
                    DbErr::Custom(format!(
                        "billing_plan_usage {id} has invalid amount_nano_usd {raw_amount:?}"
                    ))
                })?;
            if row_subscription != subscription_id {
                subscription_id = row_subscription;
                running = 0;
            }
            running = running.checked_add(amount).ok_or_else(|| {
                DbErr::Custom(format!(
                    "billing_plan_usage cumulative overflow for subscription {subscription_id}"
                ))
            })?;
            updates.push((id, running));
        }
        write_cumulatives(connection, backend, &updates).await?;
        let last = rows.last().expect("page is non-empty");
        cursor = Some((
            last.try_get::<String>("", "subscription_id")?,
            last.try_get::<String>("", "created_at")?,
            last.try_get::<String>("", "id")?,
        ));
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        if !matches!(backend, DbBackend::Sqlite | DbBackend::Postgres) {
            return Ok(());
        }
        let tx = manager.get_connection().begin().await?;
        execute(
            &tx,
            backend,
            "ALTER TABLE billing_plan_usage ADD COLUMN cumulative_nano_usd TEXT NOT NULL DEFAULT '0'",
        )
        .await?;
        execute(
            &tx,
            backend,
            "CREATE INDEX idx_billing_plan_usage_subscription_order ON billing_plan_usage (subscription_id, created_at, id)",
        )
        .await?;
        execute(
            &tx,
            backend,
            "DROP INDEX IF EXISTS idx_billing_plan_usage_subscription_time",
        )
        .await?;
        backfill(&tx, backend).await?;
        if backend == DbBackend::Postgres {
            execute(
                &tx,
                backend,
                "ALTER TABLE billing_plan_usage ALTER COLUMN cumulative_nano_usd DROP DEFAULT",
            )
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        if !matches!(backend, DbBackend::Sqlite | DbBackend::Postgres) {
            return Ok(());
        }
        let tx = manager.get_connection().begin().await?;
        for sql in [
            "CREATE INDEX idx_billing_plan_usage_subscription_time ON billing_plan_usage (subscription_id, created_at)",
            "DROP INDEX IF EXISTS idx_billing_plan_usage_subscription_order",
            "ALTER TABLE billing_plan_usage DROP COLUMN cumulative_nano_usd",
        ] {
            execute(&tx, backend, sql).await?;
        }
        tx.commit().await?;
        Ok(())
    }
}
