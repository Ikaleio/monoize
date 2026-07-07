use sea_orm::{ConnectionTrait, Statement};
use sea_orm_migration::prelude::*;
use serde_json::Value;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        let backend = manager.get_database_backend();
        let rows = conn
            .query_all(Statement::from_string(
                backend,
                "SELECT value FROM system_settings WHERE key = 'pricing_profile_model_patterns'"
                    .to_string(),
            ))
            .await?;
        let Some(row) = rows.first() else {
            return Ok(());
        };
        let raw: String = row.try_get("", "value")?;
        let Ok(mut patterns) = serde_json::from_str::<Vec<Value>>(&raw) else {
            return Ok(());
        };

        insert_pattern_if_missing(&mut patterns, "text-embedding-*", "openai", "gpt-*");
        insert_pattern_if_missing(&mut patterns, "gemini-*", "google", "grok-*");
        let next = serde_json::to_string(&patterns)
            .map_err(|e| DbErr::Custom(format!("serialize pricing patterns: {e}")))?;
        let escaped = next.replace('\'', "''");
        conn.execute(Statement::from_string(
            backend,
            format!(
                "UPDATE system_settings SET value = '{escaped}' WHERE key = 'pricing_profile_model_patterns'"
            ),
        ))
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        let backend = manager.get_database_backend();
        let rows = conn
            .query_all(Statement::from_string(
                backend,
                "SELECT value FROM system_settings WHERE key = 'pricing_profile_model_patterns'"
                    .to_string(),
            ))
            .await?;
        let Some(row) = rows.first() else {
            return Ok(());
        };
        let raw: String = row.try_get("", "value")?;
        let Ok(mut patterns) = serde_json::from_str::<Vec<Value>>(&raw) else {
            return Ok(());
        };
        patterns.retain(|entry| {
            !matches!(
                entry.get("pattern").and_then(Value::as_str),
                Some("text-embedding-*") | Some("gemini-*")
            )
        });
        let next = serde_json::to_string(&patterns)
            .map_err(|e| DbErr::Custom(format!("serialize pricing patterns: {e}")))?;
        let escaped = next.replace('\'', "''");
        conn.execute(Statement::from_string(
            backend,
            format!(
                "UPDATE system_settings SET value = '{escaped}' WHERE key = 'pricing_profile_model_patterns'"
            ),
        ))
        .await?;
        Ok(())
    }
}

fn insert_pattern_if_missing(
    patterns: &mut Vec<Value>,
    pattern: &str,
    pricing_profile: &str,
    before_pattern: &str,
) {
    if patterns
        .iter()
        .any(|entry| entry.get("pattern").and_then(Value::as_str) == Some(pattern))
    {
        return;
    }
    let entry = serde_json::json!({
        "pattern": pattern,
        "pricing_profile": pricing_profile
    });
    let idx = patterns
        .iter()
        .position(|entry| entry.get("pattern").and_then(Value::as_str) == Some(before_pattern))
        .unwrap_or(patterns.len());
    patterns.insert(idx, entry);
}
