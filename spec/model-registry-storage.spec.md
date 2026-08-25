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
    db: DbPool,
}

impl ModelRegistryStore {
    pub async fn new(db: DbPool) -> Result<Self, String>;
    pub async fn list_models(&self) -> Result<Vec<DbModelRecord>, String>;
    pub async fn get_model(&self, id: &str) -> Result<Option<DbModelRecord>, String>;
    pub async fn get_model_by_logical_and_provider(&self, logical_model: &str, provider_id: &str) -> Result<Option<DbModelRecord>, String>;
    pub async fn create_model(&self, input: CreateModelInput) -> Result<DbModelRecord, String>;
    pub async fn update_model(&self, id: &str, input: UpdateModelInput) -> Result<DbModelRecord, String>;
    pub async fn delete_model(&self, id: &str) -> Result<(), String>;
    pub async fn find_by_logical_model(&self, logical_model: &str) -> Result<Vec<DbModelRecord>, String>;
}
```

Connection routing constraints:

1. Read-only methods (`list_models`, `list_enabled_models`, `get_model`, `get_model_by_logical_and_provider`, `find_by_logical_model`, `list_model_metadata`, `list_priced_model_ids`, `get_model_metadata`, `get_model_pricing`) MUST use `DbPool::read()`.
2. Mutating methods (`create_model`, `update_model`, `delete_model`, `upsert_model_metadata`, `delete_model_metadata`, `sync_from_models_dev`) MUST use `DbPool::write()` or `DbPool::begin_write()`.
2a. `batch_delete_model_metadata` MUST use one write transaction for metadata rows and generated billing-rate mirror rows.
3. Schema creation and migration in `new(...)` MUST execute on the write connection.
4. A partial model or model-metadata update MUST preserve fields omitted from that update at the database write boundary. Concurrent partial updates to distinct fields MUST NOT restore omitted fields from a stale pre-update snapshot on SQLite or PostgreSQL.
5. `sync_from_models_dev` MUST read the set of protected manual model ids once per transaction. It MUST NOT issue a metadata lookup per synchronized model.
6. `sync_from_models_dev` MUST write model metadata and generated billing-rate rows with set-based statements split into fixed-size chunks. The number of database round trips MAY grow by chunk count but MUST NOT grow by one round trip per model or per rate row.
7. Every dynamic statement MUST remain below the portable SQLite bound-parameter limit. A PostgreSQL deployment MUST use the same chunking semantics.
8. Model and model-metadata row decoding MUST propagate every database type error. Nullable fields MAY decode to null only when the stored value is SQL null; a type mismatch MUST NOT be replaced with null or a default source value.

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

## Mutation read behavior

After a create, update, or delete mutation, the dashboard handler MUST NOT load all enabled model records. The mutation response MAY read only the affected record.

## Dashboard API Endpoints

All endpoints require admin authentication.

### POST /api/dashboard/model-metadata/batch-delete

Delete multiple model metadata rows and their generated billing-rate mirrors.

**Request:**
```json
{ "model_ids": ["gpt-4o", "claude-sonnet-4-20250514"] }
```

**Response:**
```json
{ "success": true, "deleted": 2, "not_found": [] }
```

The mutation MUST be atomic with respect to database errors. Missing IDs are reported in
`not_found` and do not roll back rows that existed.

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

## Runtime reads

The database is the source of truth for model registry reads. The runtime MUST NOT maintain a cross-request full-table `ModelRegistry` mirror. A mutation response MAY perform a point read for the affected row; it MUST NOT refresh or rebuild all enabled model records.

## Invariants

1. The combination of (logical_model, provider_id) must be unique across all records.
2. All timestamps are stored in RFC3339 format.
3. capabilities_json must be valid JSON parseable into ModelCapabilities.
4. Disabled models (`enabled = 0`) remain in the database and are excluded by methods whose contract is to list enabled models.
5. A completed mutation is visible to every later database read on the same process.
