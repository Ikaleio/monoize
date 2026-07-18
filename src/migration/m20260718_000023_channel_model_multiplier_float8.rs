use sea_orm::{ConnectionTrait, DbBackend, Statement};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.get_database_backend() != DbBackend::Postgres {
            return Ok(());
        }

        manager
            .get_connection()
            .execute(Statement::from_string(
                DbBackend::Postgres,
                postgres_upgrade_sql().to_string(),
            ))
            .await?;
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Ok(())
    }
}

fn postgres_upgrade_sql() -> &'static str {
    "ALTER TABLE monoize_channel_models ALTER COLUMN multiplier TYPE DOUBLE PRECISION USING multiplier::DOUBLE PRECISION"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_converts_multiplier_to_float8() {
        assert!(postgres_upgrade_sql().contains("TYPE DOUBLE PRECISION"));
        assert!(postgres_upgrade_sql().contains("USING multiplier::DOUBLE PRECISION"));
    }
}
