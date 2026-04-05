use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, QueryResult, Value};
use serde::{Deserialize, Serialize};

use crate::db::DbPool;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderType {
    Responses,
    ChatCompletion,
    Messages,
    Gemini,
    OpenaiImage,
    Group,
}

impl ProviderType {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "responses" => Some(Self::Responses),
            "chat_completion" => Some(Self::ChatCompletion),
            "messages" => Some(Self::Messages),
            "gemini" => Some(Self::Gemini),
            "openai_image" => Some(Self::OpenaiImage),
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
            Self::OpenaiImage => "openai_image",
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
    #[allow(clippy::should_implement_trait)]
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
    db: DbPool,
}

impl ProviderStore {
    pub async fn new(db: DbPool) -> Result<Self, String> {
        Ok(Self { db })
    }

    pub async fn list_providers(&self) -> Result<Vec<Provider>, String> {
        let rows = self
            .db
            .read()
            .query_all(self.db.stmt(
                r#"SELECT id, name, provider_type, base_url, auth_type, auth_value, auth_header_name,
                          auth_query_name, capabilities_json, strategy_json, enabled, priority,
                          weight, tag, groups_json, balance, created_at, updated_at
                   FROM providers ORDER BY priority DESC, created_at ASC"#,
                vec![],
            ))
            .await
            .map_err(|e| e.to_string())?;

        let mut providers = Vec::new();
        for row in &rows {
            let provider = self.row_to_provider(row)?;
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
        let row = self
            .db
            .read()
            .query_one(self.db.stmt(
                r#"SELECT id, name, provider_type, base_url, auth_type, auth_value, auth_header_name,
                          auth_query_name, capabilities_json, strategy_json, enabled, priority,
                          weight, tag, groups_json, balance, created_at, updated_at
                   FROM providers WHERE id = $1"#,
                vec![id.into()],
            ))
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
        } else if input.base_url.is_some() || input.auth.is_some() {
            return Err("group providers must not have base_url or auth".to_string());
        }

        let capabilities_json: Option<String> = None;

        let strategy_json = input
            .strategy
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| e.to_string())?;

        let groups_json = serde_json::to_string(&input.groups).map_err(|e| e.to_string())?;

        let auth_type = input.auth.as_ref().map(|a| a.auth_type.as_str());
        let auth_value = input.auth.as_ref().map(|a| a.value.as_str());
        let auth_header_name = input.auth.as_ref().and_then(|a| a.header_name.as_deref());
        let auth_query_name = input.auth.as_ref().and_then(|a| a.query_name.as_deref());

        self.db
            .write().await
            .execute(self.db.stmt(
                r#"INSERT INTO providers (id, name, provider_type, base_url, auth_type, auth_value,
                                          auth_header_name, auth_query_name, capabilities_json,
                                          strategy_json, enabled, priority, weight, tag, groups_json,
                                          created_at, updated_at)
                   VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17)"#,
                vec![
                    id.clone().into(),
                    input.name.clone().into(),
                    input.provider_type.as_str().into(),
                    input.base_url.clone().into(),
                    auth_type.map(|s| s.to_string()).into(),
                    auth_value.map(|s| s.to_string()).into(),
                    auth_header_name.map(|s| s.to_string()).into(),
                    auth_query_name.map(|s| s.to_string()).into(),
                    capabilities_json.into(),
                    strategy_json.into(),
                    Value::Int(Some(if input.enabled { 1 } else { 0 })),
                    Value::Int(Some(input.priority)),
                    Value::Int(Some(input.weight)),
                    input.tag.clone().into(),
                    groups_json.into(),
                    now.to_rfc3339().into(),
                    now.to_rfc3339().into(),
                ],
            ))
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

        let mut set_clauses = Vec::new();
        let mut values: Vec<Value> = Vec::new();
        let mut idx = 1usize;

        if let Some(name) = &input.name {
            set_clauses.push(format!("name = ${idx}"));
            values.push(name.clone().into());
            idx += 1;
        }

        if let Some(base_url) = &input.base_url {
            set_clauses.push(format!("base_url = ${idx}"));
            values.push(base_url.clone().into());
            idx += 1;
        }

        if let Some(auth) = &input.auth {
            set_clauses.push(format!("auth_type = ${idx}"));
            values.push(auth.auth_type.as_str().into());
            idx += 1;
            set_clauses.push(format!("auth_value = ${idx}"));
            values.push(auth.value.clone().into());
            idx += 1;
            set_clauses.push(format!("auth_header_name = ${idx}"));
            values.push(auth.header_name.clone().unwrap_or_default().into());
            idx += 1;
            set_clauses.push(format!("auth_query_name = ${idx}"));
            values.push(auth.query_name.clone().unwrap_or_default().into());
            idx += 1;
        }

        if let Some(strategy) = &input.strategy {
            set_clauses.push(format!("strategy_json = ${idx}"));
            values.push(
                serde_json::to_string(strategy)
                    .map_err(|e| e.to_string())?
                    .into(),
            );
            idx += 1;
        }

        if let Some(enabled) = input.enabled {
            set_clauses.push(format!("enabled = ${idx}"));
            values.push(Value::Int(Some(if enabled { 1 } else { 0 })));
            idx += 1;
        }

        if let Some(priority) = input.priority {
            set_clauses.push(format!("priority = ${idx}"));
            values.push(Value::Int(Some(priority)));
            idx += 1;
        }

        if let Some(weight) = input.weight {
            set_clauses.push(format!("weight = ${idx}"));
            values.push(Value::Int(Some(weight)));
            idx += 1;
        }

        if let Some(tag) = &input.tag {
            set_clauses.push(format!("tag = ${idx}"));
            values.push(tag.clone().into());
            idx += 1;
        }

        if let Some(groups) = &input.groups {
            set_clauses.push(format!("groups_json = ${idx}"));
            values.push(
                serde_json::to_string(groups)
                    .map_err(|e| e.to_string())?
                    .into(),
            );
            idx += 1;
        }

        if !set_clauses.is_empty() {
            set_clauses.push(format!("updated_at = ${idx}"));
            values.push(now.to_rfc3339().into());
            idx += 1;

            let sql = format!(
                "UPDATE providers SET {} WHERE id = ${idx}",
                set_clauses.join(", ")
            );
            values.push(id.into());

            self.db
                .write()
                .await
                .execute(self.db.stmt(&sql, values))
                .await
                .map_err(|e| e.to_string())?;
        }

        if let Some(model_map) = &input.model_map {
            self.db
                .write()
                .await
                .execute(self.db.stmt(
                    "DELETE FROM model_mappings WHERE provider_id = $1",
                    vec![id.into()],
                ))
                .await
                .map_err(|e| e.to_string())?;

            for mapping in model_map {
                self.create_model_mapping(id, mapping).await?;
            }
        }

        if let Some(members) = &input.members {
            if existing.provider_type == ProviderType::Group {
                self.db
                    .write()
                    .await
                    .execute(self.db.stmt(
                        "DELETE FROM group_members WHERE group_provider_id = $1",
                        vec![id.into()],
                    ))
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
        let row = self
            .db
            .read()
            .query_one(self.db.stmt(
                "SELECT COUNT(*) as cnt FROM group_members WHERE member_provider_id = $1",
                vec![id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "count query returned no rows".to_string())?;

        let member_count: i64 = row.try_get("", "cnt").map_err(|e| e.to_string())?;

        if member_count > 0 {
            return Err("provider_in_use: provider is a member of one or more groups".to_string());
        }

        self.db
            .write()
            .await
            .execute(
                self.db
                    .stmt("DELETE FROM providers WHERE id = $1", vec![id.into()]),
            )
            .await
            .map_err(|e| e.to_string())?;

        Ok(())
    }

    pub async fn list_model_mappings(
        &self,
        provider_id: &str,
    ) -> Result<Vec<ModelMapping>, String> {
        let rows = self
            .db
            .read()
            .query_all(self.db.stmt(
                "SELECT id, provider_id, logical_model, upstream_model, created_at FROM model_mappings WHERE provider_id = $1",
                vec![provider_id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;

        let mut mappings = Vec::new();
        for row in &rows {
            let created_at_str: String =
                row.try_get("", "created_at").map_err(|e| e.to_string())?;
            let created_at = DateTime::parse_from_rfc3339(&created_at_str)
                .map_err(|e| e.to_string())?
                .with_timezone(&Utc);

            mappings.push(ModelMapping {
                id: row.try_get("", "id").map_err(|e| e.to_string())?,
                provider_id: row.try_get("", "provider_id").map_err(|e| e.to_string())?,
                logical_model: row
                    .try_get("", "logical_model")
                    .map_err(|e| e.to_string())?,
                upstream_model: row
                    .try_get("", "upstream_model")
                    .map_err(|e| e.to_string())?,
                created_at,
            });
        }

        Ok(mappings)
    }

    pub async fn list_group_members(&self, group_id: &str) -> Result<Vec<GroupMember>, String> {
        let rows = self
            .db
            .read()
            .query_all(self.db.stmt(
                "SELECT id, group_provider_id, member_provider_id, weight, priority, created_at FROM group_members WHERE group_provider_id = $1 ORDER BY priority DESC",
                vec![group_id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;

        let mut members = Vec::new();
        for row in &rows {
            let created_at_str: String =
                row.try_get("", "created_at").map_err(|e| e.to_string())?;
            let created_at = DateTime::parse_from_rfc3339(&created_at_str)
                .map_err(|e| e.to_string())?
                .with_timezone(&Utc);

            members.push(GroupMember {
                id: row.try_get("", "id").map_err(|e| e.to_string())?,
                group_provider_id: row
                    .try_get("", "group_provider_id")
                    .map_err(|e| e.to_string())?,
                member_provider_id: row
                    .try_get("", "member_provider_id")
                    .map_err(|e| e.to_string())?,
                weight: row
                    .try_get::<i32>("", "weight")
                    .map_err(|e| e.to_string())? as u32,
                priority: row.try_get("", "priority").map_err(|e| e.to_string())?,
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

        self.db
            .write().await
            .execute(self.db.stmt(
                "INSERT INTO model_mappings (id, provider_id, logical_model, upstream_model, created_at) VALUES ($1, $2, $3, $4, $5)",
                vec![
                    id.into(),
                    provider_id.into(),
                    input.logical_model.as_str().into(),
                    input.upstream_model.as_str().into(),
                    now.to_rfc3339().into(),
                ],
            ))
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

        self.db
            .write().await
            .execute(self.db.stmt(
                "INSERT INTO group_members (id, group_provider_id, member_provider_id, weight, priority, created_at) VALUES ($1, $2, $3, $4, $5, $6)",
                vec![
                    id.into(),
                    group_id.into(),
                    input.provider_id.as_str().into(),
                    Value::Int(Some(input.weight as i32)),
                    Value::Int(Some(0)),
                    now.to_rfc3339().into(),
                ],
            ))
            .await
            .map_err(|e| e.to_string())?;

        Ok(())
    }

    fn row_to_provider(&self, row: &QueryResult) -> Result<Provider, String> {
        let provider_type_str: String = row
            .try_get("", "provider_type")
            .map_err(|e| e.to_string())?;
        let provider_type = ProviderType::from_str(&provider_type_str)
            .ok_or_else(|| format!("invalid provider type: {provider_type_str}"))?;

        let auth_type_str: Option<String> =
            row.try_get("", "auth_type").map_err(|e| e.to_string())?;
        let auth_value: Option<String> =
            row.try_get("", "auth_value").map_err(|e| e.to_string())?;
        let auth_header_name: Option<String> = row
            .try_get("", "auth_header_name")
            .map_err(|e| e.to_string())?;
        let auth_query_name: Option<String> = row
            .try_get("", "auth_query_name")
            .map_err(|e| e.to_string())?;

        let auth = if let (Some(auth_type_str), Some(value)) = (auth_type_str, auth_value) {
            let auth_type = AuthType::from_str(&auth_type_str)
                .ok_or_else(|| format!("invalid auth type: {auth_type_str}"))?;
            Some(ProviderAuth {
                auth_type,
                value,
                header_name: auth_header_name.filter(|s| !s.is_empty()),
                query_name: auth_query_name.filter(|s| !s.is_empty()),
            })
        } else {
            None
        };

        let strategy_json: Option<String> = row
            .try_get("", "strategy_json")
            .map_err(|e| e.to_string())?;
        let strategy: Option<GroupStrategy> = strategy_json
            .map(|s| serde_json::from_str(&s))
            .transpose()
            .map_err(|e| e.to_string())?;

        let created_at_str: String = row.try_get("", "created_at").map_err(|e| e.to_string())?;
        let created_at = DateTime::parse_from_rfc3339(&created_at_str)
            .map_err(|e| e.to_string())?
            .with_timezone(&Utc);

        let updated_at_str: String = row.try_get("", "updated_at").map_err(|e| e.to_string())?;
        let updated_at = DateTime::parse_from_rfc3339(&updated_at_str)
            .map_err(|e| e.to_string())?
            .with_timezone(&Utc);

        let groups_json: String = row.try_get("", "groups_json").map_err(|e| e.to_string())?;
        let groups: Vec<String> = serde_json::from_str(&groups_json).map_err(|e| {
            format!(
                "invalid groups_json for provider {}: {e}",
                row.try_get::<String>("", "id").unwrap_or_default()
            )
        })?;

        Ok(Provider {
            id: row.try_get("", "id").map_err(|e| e.to_string())?,
            name: row.try_get("", "name").map_err(|e| e.to_string())?,
            provider_type,
            base_url: row.try_get("", "base_url").map_err(|e| e.to_string())?,
            auth,
            strategy,
            enabled: row
                .try_get::<i32>("", "enabled")
                .map_err(|e| e.to_string())?
                == 1,
            priority: row.try_get("", "priority").map_err(|e| e.to_string())?,
            weight: row.try_get("", "weight").map_err(|e| e.to_string())?,
            tag: row.try_get("", "tag").map_err(|e| e.to_string())?,
            groups,
            balance: row.try_get("", "balance").map_err(|e| e.to_string())?,
            created_at,
            updated_at,
            model_map: Vec::new(),
            members: Vec::new(),
        })
    }

    pub fn get_auth_value(&self, provider: &Provider) -> Option<String> {
        provider.auth.as_ref().map(|a| a.value.clone())
    }

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

            if provider.provider_type != ProviderType::Group
                && provider
                    .model_map
                    .iter()
                    .any(|m| m.logical_model == logical_model)
            {
                result.push(provider);
            }
        }

        Ok(result)
    }

    pub async fn find_groups_for_model(
        &self,
        logical_model: &str,
    ) -> Result<Vec<Provider>, String> {
        let all_providers = self.list_providers().await?;
        let mut result = Vec::new();

        let serving_providers: std::collections::HashSet<String> = all_providers
            .iter()
            .filter(|p| p.enabled && p.provider_type != ProviderType::Group)
            .filter(|p| p.model_map.iter().any(|m| m.logical_model == logical_model))
            .map(|p| p.id.clone())
            .collect();

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
