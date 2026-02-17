# Model Registry Storage Specification

## Overview

This subsystem provides persistent database storage for model registry records, enabling runtime management of model definitions through a dashboard API.

## Data Model

### ModelRegistryRecord

A model registry record contains the following fields:

| Field | Type | Nullable | Description |
|-------|------|----------|-------------|
| id | TEXT | NO | Primary key, auto-generated UUID with prefix `model_` |
| logical_model | TEXT | NO | The model name exposed to clients (e.g., "gpt-4o") |
| provider_id | TEXT | NO | Reference to the provider that serves this model |
| upstream_model | TEXT | NO | The actual model name sent to the upstream provider |
| capabilities_json | TEXT | NO | JSON serialization of ModelCapabilities |
| enabled | INTEGER | NO | 1 if model is active, 0 otherwise (default: 1) |
| priority | INTEGER | NO | Higher priority models are preferred (default: 0) |
| created_at | TEXT | NO | RFC3339 timestamp of creation |
| updated_at | TEXT | NO | RFC3339 timestamp of last update |

Constraints:
- UNIQUE (logical_model, provider_id): A model can only be registered once per provider

### ModelCapabilities

Stored as JSON within `capabilities_json`:

```json
{
  "max_context_tokens": 128000,
  "max_output_tokens": 16384,
  "supports_streaming": true,
  "supports_tools": true,
  "supports_parallel_tool_calls": true,
  "supports_structured_output": true,
  "supports_reasoning_controls": {
    "supported": false,
    "mode": "none",
    "effort_levels": [],
    "max_reasoning_tokens": null
  },
  "supports_image_input": {
    "supported": true,
    "max_images": 10
  },
  "supports_file_input": {
    "supported": false,
    "max_files": null
  },
  "supports_image_output": {
    "supported": false
  },
  "tokenizer": "cl100k_base"
}
```

## Database Schema

```sql
CREATE TABLE IF NOT EXISTS model_registry_records (
    id TEXT PRIMARY KEY,
    logical_model TEXT NOT NULL,
    provider_id TEXT NOT NULL,
    upstream_model TEXT NOT NULL,
    capabilities_json TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    priority INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE (logical_model, provider_id)
);

CREATE INDEX IF NOT EXISTS idx_model_registry_logical ON model_registry_records(logical_model);
CREATE INDEX IF NOT EXISTS idx_model_registry_provider ON model_registry_records(provider_id);
CREATE INDEX IF NOT EXISTS idx_model_registry_enabled ON model_registry_records(enabled);
```

## Store Interface

### ModelRegistryStore

```rust
pub struct ModelRegistryStore {
    pool: Pool<Sqlite>,
}

impl ModelRegistryStore {
    pub async fn new(pool: Pool<Sqlite>) -> Result<Self, String>;
    pub async fn list_models(&self) -> Result<Vec<DbModelRecord>, String>;
    pub async fn get_model(&self, id: &str) -> Result<Option<DbModelRecord>, String>;
    pub async fn get_model_by_logical_and_provider(&self, logical_model: &str, provider_id: &str) -> Result<Option<DbModelRecord>, String>;
    pub async fn create_model(&self, input: CreateModelInput) -> Result<DbModelRecord, String>;
    pub async fn update_model(&self, id: &str, input: UpdateModelInput) -> Result<DbModelRecord, String>;
    pub async fn delete_model(&self, id: &str) -> Result<(), String>;
    pub async fn find_by_logical_model(&self, logical_model: &str) -> Result<Vec<DbModelRecord>, String>;
}
```

### CreateModelInput

```rust
pub struct CreateModelInput {
    pub id: Option<String>,
    pub logical_model: String,
    pub provider_id: String,
    pub upstream_model: String,
    pub capabilities: ModelCapabilities,
    pub enabled: Option<bool>,      // defaults to true
    pub priority: Option<i32>,      // defaults to 0
}
```

### UpdateModelInput

```rust
pub struct UpdateModelInput {
    pub logical_model: Option<String>,
    pub provider_id: Option<String>,
    pub upstream_model: Option<String>,
    pub capabilities: Option<ModelCapabilities>,
    pub enabled: Option<bool>,
    pub priority: Option<i32>,
}
```

## Integration with ModelRegistry

The `ModelRegistry` struct maintains an in-memory cache of model records. It is initialized from enabled records in `model_registry_records` only.

### Refresh Behavior

When `ModelRegistry` is refreshed after startup or model mutations:

1. Load all enabled records from `model_registry_records`.
2. Replace the in-memory cache with exactly the loaded set.

## Dashboard API Endpoints

All endpoints require admin authentication.

### GET /api/dashboard/models

List all model registry records.

**Response:** `200 OK`
```json
[
  {
    "id": "model_abc123",
    "logical_model": "gpt-4o",
    "provider_id": "openai",
    "upstream_model": "gpt-4o-2024-08-06",
    "capabilities": { ... },
    "enabled": true,
    "priority": 0,
    "created_at": "2025-01-01T00:00:00Z",
    "updated_at": "2025-01-01T00:00:00Z"
  }
]
```

### POST /api/dashboard/models

Create a new model registry record.

**Request:**
```json
{
  "logical_model": "gpt-4o",
  "provider_id": "openai",
  "upstream_model": "gpt-4o-2024-08-06",
  "capabilities": {
    "max_context_tokens": 128000,
    "max_output_tokens": 16384,
    "supports_streaming": true,
    "supports_tools": true,
    "supports_parallel_tool_calls": true,
    "supports_structured_output": true,
    "supports_reasoning_controls": {
      "supported": false,
      "mode": "none",
      "effort_levels": [],
      "max_reasoning_tokens": null
    },
    "supports_image_input": { "supported": true, "max_images": 10 },
    "supports_file_input": { "supported": false, "max_files": null },
    "supports_image_output": { "supported": false },
    "tokenizer": "cl100k_base"
  },
  "enabled": true,
  "priority": 0
}
```

**Response:** `201 Created`

**Errors:**
- `409 Conflict`: Model with same logical_model + provider_id already exists

### GET /api/dashboard/models/{model_id}

Get a specific model by ID.

**Response:** `200 OK`

**Errors:**
- `404 Not Found`: Model does not exist

### PUT /api/dashboard/models/{model_id}

Update an existing model. All fields are optional; only provided fields are updated.

**Request:**
```json
{
  "upstream_model": "gpt-4o-2024-11-20",
  "capabilities": { ... },
  "enabled": false
}
```

**Response:** `200 OK`

**Errors:**
- `404 Not Found`: Model does not exist
- `409 Conflict`: Update would create duplicate logical_model + provider_id

### DELETE /api/dashboard/models/{model_id}

Delete a model registry record.

**Response:** `200 OK`
```json
{ "success": true }
```

**Errors:**
- `404 Not Found`: Model does not exist

## Side Effects

After any mutating operation (create, update, delete), the `ModelRegistry` in-memory cache should be refreshed to reflect the changes. This ensures that subsequent API requests use the updated model definitions.

## Invariants

1. The combination of (logical_model, provider_id) must be unique across all records.
2. All timestamps are stored in RFC3339 format.
3. capabilities_json must be valid JSON parseable into ModelCapabilities.
4. Active in-memory model registry content is exactly the set of enabled rows in `model_registry_records`.
5. Disabled models (enabled=0) are excluded from the active model registry but remain in the database.
