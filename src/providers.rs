use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Pool, Row, Sqlite};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderType {
    Responses,
    ChatCompletion,
    Messages,
    Gemini,
    Grok,
    Group,
}

impl ProviderType {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "responses" => Some(Self::Responses),
            "chat_completion" => Some(Self::ChatCompletion),
            "messages" => Some(Self::Messages),
            "gemini" => Some(Self::Gemini),
            "grok" => Some(Self::Grok),
            "group" => Some(Self::Group),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Responses => "responses",
            Self::ChatCompletion => "chat_completion",
            Self::Messages => "messages",
            Self::Gemini => "gemini",
            Self::Grok => "grok",
            Self::Group => "group",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthType {
    Bearer,
    Header,
    Query,
}

impl AuthType {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "bearer" => Some(Self::Bearer),
            "header" => Some(Self::Header),
            "query" => Some(Self::Query),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Bearer => "bearer",
            Self::Header => "header",
            Self::Query => "query",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderAuth {
    #[serde(rename = "type")]
    pub auth_type: AuthType,
    #[serde(skip_serializing)]
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub header_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GroupStrategyType {
    #[default]
    WeightedRoundRobin,
    Failover,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupStrategy {
    #[serde(rename = "type", default)]
    pub strategy_type: GroupStrategyType,
    #[serde(default = "default_max_attempts")]
    pub max_attempts: usize,
    #[serde(default = "default_backoff_ms")]
    pub backoff_ms: Vec<u64>,
    #[serde(default = "default_retry_on")]
    pub retry_on: Vec<String>,
    #[serde(default = "default_non_retry_codes")]
    pub non_retry_codes: Vec<String>,
    #[serde(default = "default_fallback_on")]
    pub fallback_on: Vec<String>,
}

impl Default for GroupStrategy {
    fn default() -> Self {
        Self {
            strategy_type: GroupStrategyType::WeightedRoundRobin,
            max_attempts: default_max_attempts(),
            backoff_ms: default_backoff_ms(),
            retry_on: default_retry_on(),
            non_retry_codes: default_non_retry_codes(),
            fallback_on: default_fallback_on(),
        }
    }
}

fn default_max_attempts() -> usize {
    2
}

fn default_backoff_ms() -> Vec<u64> {
    vec![200, 500]
}

fn default_retry_on() -> Vec<String> {
    vec![
        "network".to_string(),
        "http_5xx".to_string(),
        "http_429".to_string(),
    ]
}

fn default_non_retry_codes() -> Vec<String> {
    vec![
        "insufficient_quota".to_string(),
        "invalid_api_key".to_string(),
        "invalid_request_error".to_string(),
        "model_not_found".to_string(),
        "permission_denied".to_string(),
    ]
}

fn default_fallback_on() -> Vec<String> {
    vec![
        "retry_exhausted".to_string(),
        "non_retryable".to_string(),
        "network".to_string(),
        "http_5xx".to_string(),
        "http_429".to_string(),
    ]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelMapping {
    pub id: String,
    pub provider_id: String,
    pub logical_model: String,
    pub upstream_model: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupMember {
    pub id: String,
    pub group_provider_id: String,
    pub member_provider_id: String,
    pub weight: u32,
    pub priority: i32,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provider {
    pub id: String,
    pub name: String,
    pub provider_type: ProviderType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth: Option<ProviderAuth>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strategy: Option<GroupStrategy>,
    pub enabled: bool,
    pub priority: i32,
    pub weight: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub balance: Option<f64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub model_map: Vec<ModelMapping>,
    #[serde(default)]
    pub members: Vec<GroupMember>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateProviderInput {
    pub id: Option<String>,
    pub name: String,
    pub provider_type: ProviderType,
    pub base_url: Option<String>,
    pub auth: Option<CreateProviderAuthInput>,
    pub strategy: Option<GroupStrategy>,
    #[serde(default)]
    pub model_map: Vec<CreateModelMappingInput>,
    #[serde(default)]
    pub members: Vec<CreateGroupMemberInput>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub priority: i32,
    #[serde(default = "default_weight_i32")]
    pub weight: i32,
    pub tag: Option<String>,
    #[serde(default)]
    pub groups: Vec<String>,
}

fn default_enabled() -> bool {
    true
}

fn default_weight_i32() -> i32 {
    1
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateProviderAuthInput {
    #[serde(rename = "type")]
    pub auth_type: AuthType,
    pub value: String,
    pub header_name: Option<String>,
    pub query_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateModelMappingInput {
    pub logical_model: String,
    pub upstream_model: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateGroupMemberInput {
    pub provider_id: String,
    #[serde(default = "default_weight")]
    pub weight: u32,
}

fn default_weight() -> u32 {
    1
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateProviderInput {
    pub name: Option<String>,
    pub base_url: Option<String>,
    pub auth: Option<CreateProviderAuthInput>,
    pub strategy: Option<GroupStrategy>,
    pub model_map: Option<Vec<CreateModelMappingInput>>,
    pub members: Option<Vec<CreateGroupMemberInput>>,
    pub enabled: Option<bool>,
    pub priority: Option<i32>,
    pub weight: Option<i32>,
    pub tag: Option<String>,
    pub groups: Option<Vec<String>>,
}

#[derive(Clone)]
pub struct ProviderStore {
    pool: Pool<Sqlite>,
}

impl ProviderStore {
    pub async fn new(pool: Pool<Sqlite>) -> Result<Self, String> {
        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS providers (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL DEFAULT '',
                provider_type TEXT NOT NULL CHECK (provider_type IN ('responses', 'chat_completion', 'messages', 'gemini', 'grok', 'group')),
                base_url TEXT,
                auth_type TEXT CHECK (auth_type IN ('bearer', 'header', 'query') OR auth_type IS NULL),
                auth_value TEXT,
                auth_header_name TEXT,
                auth_query_name TEXT,
                capabilities_json TEXT,
                strategy_json TEXT,
                enabled INTEGER NOT NULL DEFAULT 1,
                priority INTEGER NOT NULL DEFAULT 0,
                weight INTEGER NOT NULL DEFAULT 1,
                tag TEXT,
                groups_json TEXT NOT NULL DEFAULT '[]',
                balance REAL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )"#,
        )
        .execute(&pool)
        .await
        .map_err(|e| e.to_string())?;

        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS model_mappings (
                id TEXT PRIMARY KEY,
                provider_id TEXT NOT NULL REFERENCES providers(id) ON DELETE CASCADE,
                logical_model TEXT NOT NULL,
                upstream_model TEXT NOT NULL,
                created_at TEXT NOT NULL,
                UNIQUE (provider_id, logical_model)
            )"#,
        )
        .execute(&pool)
        .await
        .map_err(|e| e.to_string())?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_model_mappings_logical ON model_mappings(logical_model)")
            .execute(&pool)
            .await
            .map_err(|e| e.to_string())?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_model_mappings_provider ON model_mappings(provider_id)",
        )
        .execute(&pool)
        .await
        .map_err(|e| e.to_string())?;

        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS group_members (
                id TEXT PRIMARY KEY,
                group_provider_id TEXT NOT NULL REFERENCES providers(id) ON DELETE CASCADE,
                member_provider_id TEXT NOT NULL REFERENCES providers(id) ON DELETE RESTRICT,
                weight INTEGER NOT NULL DEFAULT 1 CHECK (weight >= 1),
                priority INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL,
                UNIQUE (group_provider_id, member_provider_id)
            )"#,
        )
        .execute(&pool)
        .await
        .map_err(|e| e.to_string())?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_group_members_group ON group_members(group_provider_id)")
            .execute(&pool)
            .await
            .map_err(|e| e.to_string())?;

        // Migrate existing providers table to add new columns if they don't exist
        let has_name_column: bool = sqlx::query_scalar::<_, i32>(
            "SELECT COUNT(*) FROM pragma_table_info('providers') WHERE name = 'name'",
        )
        .fetch_one(&pool)
        .await
        .map(|c| c > 0)
        .unwrap_or(false);

        if !has_name_column {
            sqlx::query("ALTER TABLE providers ADD COLUMN name TEXT NOT NULL DEFAULT ''")
                .execute(&pool)
                .await
                .ok();
            sqlx::query("ALTER TABLE providers ADD COLUMN weight INTEGER NOT NULL DEFAULT 1")
                .execute(&pool)
                .await
                .ok();
            sqlx::query("ALTER TABLE providers ADD COLUMN tag TEXT")
                .execute(&pool)
                .await
                .ok();
            sqlx::query("ALTER TABLE providers ADD COLUMN groups_json TEXT NOT NULL DEFAULT '[]'")
                .execute(&pool)
                .await
                .ok();
            sqlx::query("ALTER TABLE providers ADD COLUMN balance REAL")
                .execute(&pool)
                .await
                .ok();
        }

        Ok(Self { pool })
    }

    pub async fn list_providers(&self) -> Result<Vec<Provider>, String> {
        let rows = sqlx::query(
            r#"SELECT id, name, provider_type, base_url, auth_type, auth_value, auth_header_name,
                      auth_query_name, capabilities_json, strategy_json, enabled, priority,
                      weight, tag, groups_json, balance, created_at, updated_at
               FROM providers ORDER BY priority DESC, created_at ASC"#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        let mut providers = Vec::new();
        for row in rows {
            let provider = self.row_to_provider(&row)?;
            providers.push(provider);
        }

        for provider in &mut providers {
            provider.model_map = self.list_model_mappings(&provider.id).await?;
            if provider.provider_type == ProviderType::Group {
                provider.members = self.list_group_members(&provider.id).await?;
            }
        }

        Ok(providers)
    }

    pub async fn get_provider(&self, id: &str) -> Result<Option<Provider>, String> {
        let row = sqlx::query(
            r#"SELECT id, name, provider_type, base_url, auth_type, auth_value, auth_header_name,
                      auth_query_name, capabilities_json, strategy_json, enabled, priority,
                      weight, tag, groups_json, balance, created_at, updated_at
               FROM providers WHERE id = ?"#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        if let Some(row) = row {
            let mut provider = self.row_to_provider(&row)?;
            provider.model_map = self.list_model_mappings(&provider.id).await?;
            if provider.provider_type == ProviderType::Group {
                provider.members = self.list_group_members(&provider.id).await?;
            }
            Ok(Some(provider))
        } else {
            Ok(None)
        }
    }

    pub async fn create_provider(&self, input: CreateProviderInput) -> Result<Provider, String> {
        let id = input.id.unwrap_or_else(|| {
            format!("prov_{}", uuid::Uuid::new_v4().to_string().replace("-", ""))
        });
        let now = Utc::now();

        if input.provider_type != ProviderType::Group {
            if input.base_url.is_none() {
                return Err("base_url is required for concrete providers".to_string());
            }
            if input.auth.is_none() {
                return Err("auth is required for concrete providers".to_string());
            }
        } else {
            if input.base_url.is_some() || input.auth.is_some() {
                return Err("group providers must not have base_url or auth".to_string());
            }
        }

        let capabilities_json: Option<String> = None;

        let strategy_json = input
            .strategy
            .as_ref()
            .map(|s| serde_json::to_string(s))
            .transpose()
            .map_err(|e| e.to_string())?;

        let groups_json = serde_json::to_string(&input.groups).map_err(|e| e.to_string())?;

        let auth_type = input.auth.as_ref().map(|a| a.auth_type.as_str());
        let auth_value = input.auth.as_ref().map(|a| a.value.as_str());
        let auth_header_name = input.auth.as_ref().and_then(|a| a.header_name.as_deref());
        let auth_query_name = input.auth.as_ref().and_then(|a| a.query_name.as_deref());

        sqlx::query(
            r#"INSERT INTO providers (id, name, provider_type, base_url, auth_type, auth_value,
                                      auth_header_name, auth_query_name, capabilities_json,
                                      strategy_json, enabled, priority, weight, tag, groups_json,
                                      created_at, updated_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(&id)
        .bind(&input.name)
        .bind(input.provider_type.as_str())
        .bind(&input.base_url)
        .bind(auth_type)
        .bind(auth_value)
        .bind(auth_header_name)
        .bind(auth_query_name)
        .bind(&capabilities_json)
        .bind(&strategy_json)
        .bind(input.enabled)
        .bind(input.priority)
        .bind(input.weight)
        .bind(&input.tag)
        .bind(&groups_json)
        .bind(now.to_rfc3339())
        .bind(now.to_rfc3339())
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        for mapping in &input.model_map {
            self.create_model_mapping(&id, mapping).await?;
        }

        for member in &input.members {
            self.create_group_member(&id, member).await?;
        }

        self.get_provider(&id)
            .await?
            .ok_or_else(|| "provider not found after creation".to_string())
    }

    pub async fn update_provider(
        &self,
        id: &str,
        input: UpdateProviderInput,
    ) -> Result<Provider, String> {
        let existing = self
            .get_provider(id)
            .await?
            .ok_or_else(|| "provider not found".to_string())?;
        let now = Utc::now();

        let mut updates = Vec::new();
        let mut bindings: Vec<String> = Vec::new();

        if let Some(name) = &input.name {
            updates.push("name = ?");
            bindings.push(name.clone());
        }

        if let Some(base_url) = &input.base_url {
            updates.push("base_url = ?");
            bindings.push(base_url.clone());
        }

        if let Some(auth) = &input.auth {
            updates.push("auth_type = ?");
            bindings.push(auth.auth_type.as_str().to_string());
            updates.push("auth_value = ?");
            bindings.push(auth.value.clone());
            updates.push("auth_header_name = ?");
            bindings.push(auth.header_name.clone().unwrap_or_default());
            updates.push("auth_query_name = ?");
            bindings.push(auth.query_name.clone().unwrap_or_default());
        }

        if let Some(strategy) = &input.strategy {
            updates.push("strategy_json = ?");
            bindings.push(serde_json::to_string(strategy).map_err(|e| e.to_string())?);
        }

        if let Some(enabled) = input.enabled {
            updates.push("enabled = ?");
            bindings.push(if enabled { "1" } else { "0" }.to_string());
        }

        if let Some(priority) = input.priority {
            updates.push("priority = ?");
            bindings.push(priority.to_string());
        }

        if let Some(weight) = input.weight {
            updates.push("weight = ?");
            bindings.push(weight.to_string());
        }

        if let Some(tag) = &input.tag {
            updates.push("tag = ?");
            bindings.push(tag.clone());
        }

        if let Some(groups) = &input.groups {
            updates.push("groups_json = ?");
            bindings.push(serde_json::to_string(groups).map_err(|e| e.to_string())?);
        }

        if !updates.is_empty() {
            updates.push("updated_at = ?");
            bindings.push(now.to_rfc3339());
            bindings.push(id.to_string());

            let query = format!("UPDATE providers SET {} WHERE id = ?", updates.join(", "));

            let mut q = sqlx::query(&query);
            for b in &bindings {
                q = q.bind(b);
            }

            q.execute(&self.pool).await.map_err(|e| e.to_string())?;
        }

        if let Some(model_map) = &input.model_map {
            sqlx::query("DELETE FROM model_mappings WHERE provider_id = ?")
                .bind(id)
                .execute(&self.pool)
                .await
                .map_err(|e| e.to_string())?;

            for mapping in model_map {
                self.create_model_mapping(id, mapping).await?;
            }
        }

        if let Some(members) = &input.members {
            if existing.provider_type == ProviderType::Group {
                sqlx::query("DELETE FROM group_members WHERE group_provider_id = ?")
                    .bind(id)
                    .execute(&self.pool)
                    .await
                    .map_err(|e| e.to_string())?;

                for member in members {
                    self.create_group_member(id, member).await?;
                }
            }
        }

        self.get_provider(id)
            .await?
            .ok_or_else(|| "provider not found after update".to_string())
    }

    pub async fn delete_provider(&self, id: &str) -> Result<(), String> {
        let member_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM group_members WHERE member_provider_id = ?")
                .bind(id)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| e.to_string())?;

        if member_count > 0 {
            return Err("provider_in_use: provider is a member of one or more groups".to_string());
        }

        sqlx::query("DELETE FROM providers WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;

        Ok(())
    }

    pub async fn list_model_mappings(
        &self,
        provider_id: &str,
    ) -> Result<Vec<ModelMapping>, String> {
        let rows = sqlx::query(
            "SELECT id, provider_id, logical_model, upstream_model, created_at FROM model_mappings WHERE provider_id = ?"
        )
        .bind(provider_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        let mut mappings = Vec::new();
        for row in rows {
            let created_at_str: String = row.try_get("created_at").map_err(|e| e.to_string())?;
            let created_at = DateTime::parse_from_rfc3339(&created_at_str)
                .map_err(|e| e.to_string())?
                .with_timezone(&Utc);

            mappings.push(ModelMapping {
                id: row.try_get("id").map_err(|e| e.to_string())?,
                provider_id: row.try_get("provider_id").map_err(|e| e.to_string())?,
                logical_model: row.try_get("logical_model").map_err(|e| e.to_string())?,
                upstream_model: row.try_get("upstream_model").map_err(|e| e.to_string())?,
                created_at,
            });
        }

        Ok(mappings)
    }

    pub async fn list_group_members(&self, group_id: &str) -> Result<Vec<GroupMember>, String> {
        let rows = sqlx::query(
            "SELECT id, group_provider_id, member_provider_id, weight, priority, created_at FROM group_members WHERE group_provider_id = ? ORDER BY priority DESC"
        )
        .bind(group_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        let mut members = Vec::new();
        for row in rows {
            let created_at_str: String = row.try_get("created_at").map_err(|e| e.to_string())?;
            let created_at = DateTime::parse_from_rfc3339(&created_at_str)
                .map_err(|e| e.to_string())?
                .with_timezone(&Utc);

            members.push(GroupMember {
                id: row.try_get("id").map_err(|e| e.to_string())?,
                group_provider_id: row
                    .try_get("group_provider_id")
                    .map_err(|e| e.to_string())?,
                member_provider_id: row
                    .try_get("member_provider_id")
                    .map_err(|e| e.to_string())?,
                weight: row.try_get::<i32, _>("weight").map_err(|e| e.to_string())? as u32,
                priority: row.try_get("priority").map_err(|e| e.to_string())?,
                created_at,
            });
        }

        Ok(members)
    }

    async fn create_model_mapping(
        &self,
        provider_id: &str,
        input: &CreateModelMappingInput,
    ) -> Result<(), String> {
        let id = format!("mm_{}", uuid::Uuid::new_v4().to_string().replace("-", ""));
        let now = Utc::now();

        sqlx::query(
            "INSERT INTO model_mappings (id, provider_id, logical_model, upstream_model, created_at) VALUES (?, ?, ?, ?, ?)"
        )
        .bind(&id)
        .bind(provider_id)
        .bind(&input.logical_model)
        .bind(&input.upstream_model)
        .bind(now.to_rfc3339())
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        Ok(())
    }

    async fn create_group_member(
        &self,
        group_id: &str,
        input: &CreateGroupMemberInput,
    ) -> Result<(), String> {
        let member = self.get_provider(&input.provider_id).await?;
        if member.is_none() {
            return Err(format!("member provider {} not found", input.provider_id));
        }
        let member = member.unwrap();
        if member.provider_type == ProviderType::Group {
            return Err("cannot add a group as a member of another group".to_string());
        }

        let id = format!("gm_{}", uuid::Uuid::new_v4().to_string().replace("-", ""));
        let now = Utc::now();

        sqlx::query(
            "INSERT INTO group_members (id, group_provider_id, member_provider_id, weight, priority, created_at) VALUES (?, ?, ?, ?, ?, ?)"
        )
        .bind(&id)
        .bind(group_id)
        .bind(&input.provider_id)
        .bind(input.weight as i32)
        .bind(0i32)
        .bind(now.to_rfc3339())
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        Ok(())
    }

    fn row_to_provider(&self, row: &sqlx::sqlite::SqliteRow) -> Result<Provider, String> {
        let provider_type_str: String = row.try_get("provider_type").map_err(|e| e.to_string())?;
        let provider_type = ProviderType::from_str(&provider_type_str)
            .ok_or_else(|| format!("invalid provider type: {}", provider_type_str))?;

        let auth_type_str: Option<String> = row.try_get("auth_type").map_err(|e| e.to_string())?;
        let auth_value: Option<String> = row.try_get("auth_value").map_err(|e| e.to_string())?;
        let auth_header_name: Option<String> =
            row.try_get("auth_header_name").map_err(|e| e.to_string())?;
        let auth_query_name: Option<String> =
            row.try_get("auth_query_name").map_err(|e| e.to_string())?;

        let auth = if let (Some(auth_type_str), Some(value)) = (auth_type_str, auth_value) {
            let auth_type = AuthType::from_str(&auth_type_str)
                .ok_or_else(|| format!("invalid auth type: {}", auth_type_str))?;
            Some(ProviderAuth {
                auth_type,
                value,
                header_name: auth_header_name.filter(|s| !s.is_empty()),
                query_name: auth_query_name.filter(|s| !s.is_empty()),
            })
        } else {
            None
        };

        let strategy_json: Option<String> =
            row.try_get("strategy_json").map_err(|e| e.to_string())?;
        let strategy: Option<GroupStrategy> = strategy_json
            .map(|s| serde_json::from_str(&s))
            .transpose()
            .map_err(|e| e.to_string())?;

        let created_at_str: String = row.try_get("created_at").map_err(|e| e.to_string())?;
        let created_at = DateTime::parse_from_rfc3339(&created_at_str)
            .map_err(|e| e.to_string())?
            .with_timezone(&Utc);

        let updated_at_str: String = row.try_get("updated_at").map_err(|e| e.to_string())?;
        let updated_at = DateTime::parse_from_rfc3339(&updated_at_str)
            .map_err(|e| e.to_string())?
            .with_timezone(&Utc);

        let name: String = row.try_get("name").unwrap_or_else(|_| String::new());
        let weight: i32 = row.try_get("weight").unwrap_or(1);
        let tag: Option<String> = row.try_get("tag").unwrap_or(None);
        let groups_json: String = row
            .try_get("groups_json")
            .unwrap_or_else(|_| "[]".to_string());
        let groups: Vec<String> = serde_json::from_str(&groups_json).unwrap_or_default();
        let balance: Option<f64> = row.try_get("balance").unwrap_or(None);

        Ok(Provider {
            id: row.try_get("id").map_err(|e| e.to_string())?,
            name,
            provider_type,
            base_url: row.try_get("base_url").map_err(|e| e.to_string())?,
            auth,
            strategy,
            enabled: row
                .try_get::<i32, _>("enabled")
                .map_err(|e| e.to_string())?
                == 1,
            priority: row.try_get("priority").map_err(|e| e.to_string())?,
            weight,
            tag,
            groups,
            balance,
            created_at,
            updated_at,
            model_map: Vec::new(),
            members: Vec::new(),
        })
    }

    pub fn get_auth_value(&self, provider: &Provider) -> Option<String> {
        provider.auth.as_ref().map(|a| a.value.clone())
    }

    /// Find all enabled providers that have a model mapping for the given logical model.
    pub async fn find_providers_for_model(
        &self,
        logical_model: &str,
    ) -> Result<Vec<Provider>, String> {
        let all_providers = self.list_providers().await?;
        let mut result = Vec::new();

        for provider in all_providers {
            if !provider.enabled {
                continue;
            }

            // For concrete providers, check if they have a model mapping for this model
            if provider.provider_type != ProviderType::Group {
                if provider
                    .model_map
                    .iter()
                    .any(|m| m.logical_model == logical_model)
                {
                    result.push(provider);
                }
            }
        }

        Ok(result)
    }

    /// Find all enabled group providers that contain members serving the given logical model.
    pub async fn find_groups_for_model(
        &self,
        logical_model: &str,
    ) -> Result<Vec<Provider>, String> {
        let all_providers = self.list_providers().await?;
        let mut result = Vec::new();

        // Build a set of provider IDs that serve this model
        let serving_providers: std::collections::HashSet<String> = all_providers
            .iter()
            .filter(|p| p.enabled && p.provider_type != ProviderType::Group)
            .filter(|p| p.model_map.iter().any(|m| m.logical_model == logical_model))
            .map(|p| p.id.clone())
            .collect();

        // Find groups that have at least one member serving this model
        for provider in all_providers {
            if !provider.enabled || provider.provider_type != ProviderType::Group {
                continue;
            }

            let has_serving_member = provider
                .members
                .iter()
                .any(|m| serving_providers.contains(&m.member_provider_id));

            if has_serving_member {
                result.push(provider);
            }
        }

        Ok(result)
    }
}
