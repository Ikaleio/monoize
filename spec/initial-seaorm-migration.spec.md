# Initial SeaORM Migration Specification

## 0. Scope

ISM0.1. This specification defines a single initial SeaORM migration module under `src/migration/`.

ISM0.2. The initial migration MUST create exactly 16 tables and their required constraints/indexes.

ISM0.3. The migration MUST execute on SQLite and PostgreSQL without requiring database-specific SQL branches.

## 1. Migration module structure

ISM1.1. `src/migration/mod.rs` MUST define `Migrator` implementing `MigratorTrait`.

ISM1.2. `Migrator::migrations()` MUST return exactly one migration entry:

- `m20250101_000001_create_tables::Migration`

ISM1.3. `src/migration/mod.rs` MUST declare `mod m20250101_000001_create_tables;`.

## 2. Initial migration identity and ordering

ISM2.1. `src/migration/m20250101_000001_create_tables.rs` MUST define a migration type deriving `DeriveMigrationName`.

ISM2.2. `up()` MUST create tables in this dependency-safe order:

1. `users`
2. `sessions`
3. `api_keys`
4. `billing_ledger`
5. `request_logs`
6. `system_settings`
7. `model_registry_records`
8. `model_metadata_records`
9. `monoize_providers`
10. `monoize_provider_models`
11. `monoize_channels`
12. `providers`
13. `model_mappings`
14. `group_members`
15. `state_records`
16. `file_bytes`

ISM2.3. `down()` MUST drop the same 16 tables in reverse dependency order.

## 3. Type mapping and key rules

ISM3.1. The migration MUST use `Table::create()` for every table.

ISM3.2. Column type rules MUST be:

- logical TEXT → `.text()`
- logical INTEGER → `.integer()`
- logical REAL → `.double()`
- logical BLOB → `.binary()`

ISM3.3. Single-column primary keys MUST be declared inline on the column with `.primary_key()`.

ISM3.4. Composite primary keys MUST be declared at table level via `.primary_key(Index::create()...)`.

ISM3.5. The migration MUST NOT use auto-increment primary keys.

ISM3.6. The migration MUST NOT define CHECK constraints.

ISM3.7. Boolean-like fields MUST be represented as INTEGER columns with `0/1` defaults when required.

## 4. Table schema requirements

ISM4.1. `users` columns:

- `id` TEXT PK
- `username` TEXT NOT NULL UNIQUE
- `password_hash` TEXT NOT NULL
- `role` TEXT NOT NULL
- `created_at` TEXT NOT NULL
- `updated_at` TEXT NOT NULL
- `last_login_at` TEXT NULL
- `enabled` INTEGER NOT NULL DEFAULT 1
- `balance_nano_usd` TEXT NOT NULL DEFAULT '0'
- `balance_unlimited` INTEGER NOT NULL DEFAULT 0
- `email` TEXT NULL

ISM4.2. `sessions` columns:

- `id` TEXT PK
- `user_id` TEXT NOT NULL
- `token` TEXT NOT NULL UNIQUE
- `created_at` TEXT NOT NULL
- `expires_at` TEXT NOT NULL

ISM4.3. `api_keys` columns:

- `id` TEXT PK
- `user_id` TEXT NOT NULL
- `name` TEXT NOT NULL
- `key_prefix` TEXT NOT NULL
- `key` TEXT NOT NULL
- `key_hash` TEXT NOT NULL
- `created_at` TEXT NOT NULL
- `expires_at` TEXT NULL
- `last_used_at` TEXT NULL
- `enabled` INTEGER NOT NULL DEFAULT 1
- `quota_remaining` INTEGER NULL
- `quota_unlimited` INTEGER NOT NULL DEFAULT 0
- `model_limits_enabled` INTEGER NOT NULL DEFAULT 0
- `model_limits` TEXT NOT NULL DEFAULT '{}'
- `ip_whitelist` TEXT NOT NULL DEFAULT '[]'
- `token_group` TEXT NOT NULL DEFAULT 'default'
- `max_multiplier` REAL NULL
- `transforms` TEXT NOT NULL DEFAULT '[]'
- `reasoning_envelope_enabled` INTEGER NOT NULL DEFAULT 1, added by migration `m20260404_000014_api_key_reasoning_envelope_switch`

ISM4.4. `billing_ledger` columns:

- `id` TEXT PK
- `user_id` TEXT NOT NULL
- `kind` TEXT NOT NULL
- `delta_nano_usd` TEXT NOT NULL
- `balance_after_nano_usd` TEXT NULL
- `meta_json` TEXT NOT NULL
- `created_at` TEXT NOT NULL

ISM4.5. `request_logs` columns:

- `id` TEXT PK
- `request_id` TEXT NULL
- `user_id` TEXT NOT NULL
- `api_key_id` TEXT NULL
- `model` TEXT NOT NULL
- `provider_id` TEXT NULL
- `upstream_model` TEXT NULL
- `channel_id` TEXT NULL
- `is_stream` INTEGER NOT NULL DEFAULT 0
- `input_tokens` INTEGER NULL
- `output_tokens` INTEGER NULL
- `cache_read_tokens` INTEGER NULL
- `cache_creation_tokens` INTEGER NULL
- `tool_prompt_tokens` INTEGER NULL
- `reasoning_tokens` INTEGER NULL
- `accepted_prediction_tokens` INTEGER NULL
- `rejected_prediction_tokens` INTEGER NULL
- `provider_multiplier` REAL NULL
- `charge_nano_usd` TEXT NULL
- `status` TEXT NOT NULL
- `usage_breakdown_json` TEXT NULL
- `billing_breakdown_json` TEXT NULL
- `error_code` TEXT NULL
- `error_message` TEXT NULL
- `error_http_status` INTEGER NULL
- `duration_ms` INTEGER NULL
- `ttfb_ms` INTEGER NULL
- `request_ip` TEXT NULL
- `reasoning_effort` TEXT NULL
- `tried_providers_json` TEXT NULL
- `request_kind` TEXT NULL
- `created_at` TEXT NOT NULL

ISM4.6. `system_settings` columns:

- `key` TEXT PK
- `value` TEXT NOT NULL
- `updated_at` TEXT NOT NULL

ISM4.7. `model_registry_records` columns:

- `id` TEXT PK
- `logical_model` TEXT NOT NULL
- `provider_id` TEXT NOT NULL
- `upstream_model` TEXT NOT NULL
- `capabilities_json` TEXT NOT NULL
- `enabled` INTEGER NOT NULL DEFAULT 1
- `priority` INTEGER NOT NULL DEFAULT 0
- `created_at` TEXT NOT NULL
- `updated_at` TEXT NOT NULL
- UNIQUE(`logical_model`, `provider_id`)

ISM4.8. `model_metadata_records` columns:

- `model_id` TEXT PK
- `models_dev_provider` TEXT NULL
- `mode` TEXT NULL
- `input_cost_per_token_nano` TEXT NULL
- `output_cost_per_token_nano` TEXT NULL
- `cache_read_input_cost_per_token_nano` TEXT NULL
- `cache_creation_input_cost_per_token_nano` TEXT NULL
- `output_cost_per_reasoning_token_nano` TEXT NULL
- `max_input_tokens` INTEGER NULL
- `max_output_tokens` INTEGER NULL
- `max_tokens` INTEGER NULL
- `raw_json` TEXT NOT NULL
- `source` TEXT NOT NULL
- `updated_at` TEXT NOT NULL

ISM4.9. `monoize_providers` columns:

- `id` TEXT PK
- `name` TEXT NOT NULL
- `provider_type` TEXT NOT NULL
- `max_retries` INTEGER NOT NULL DEFAULT 3
- `transforms` TEXT NOT NULL DEFAULT '[]'
- `api_type_overrides` TEXT NOT NULL DEFAULT '[]'
- `active_probe_enabled_override` INTEGER NULL
- `active_probe_interval_seconds_override` INTEGER NULL
- `active_probe_success_threshold_override` INTEGER NULL
- `active_probe_model_override` TEXT NULL
- `request_timeout_ms_override` INTEGER NULL
- `enabled` INTEGER NOT NULL DEFAULT 1
- `priority` INTEGER NOT NULL DEFAULT 0
- `created_at` TEXT NOT NULL
- `updated_at` TEXT NOT NULL

ISM4.10. `monoize_provider_models` columns:

- `id` TEXT PK
- `provider_id` TEXT NOT NULL
- `model_name` TEXT NOT NULL
- `redirect` TEXT NULL
- `multiplier` REAL NOT NULL DEFAULT 1.0
- `created_at` TEXT NOT NULL
- UNIQUE(`provider_id`, `model_name`)

ISM4.11. `monoize_channels` columns:

- `id` TEXT PK
- `provider_id` TEXT NOT NULL
- `name` TEXT NOT NULL
- `base_url` TEXT NOT NULL
- `api_key` TEXT NOT NULL
- `weight` INTEGER NOT NULL DEFAULT 1
- `enabled` INTEGER NOT NULL DEFAULT 1
- `passive_failure_threshold_override` INTEGER NULL
- `passive_cooldown_seconds_override` INTEGER NULL
- `passive_window_seconds_override` INTEGER NULL
- `passive_min_samples_override` INTEGER NULL
- `passive_failure_rate_threshold_override` REAL NULL
- `passive_rate_limit_cooldown_seconds_override` INTEGER NULL
- `request_timeout_ms_override` INTEGER NULL
- `created_at` TEXT NOT NULL
- `updated_at` TEXT NOT NULL

ISM4.10a. Migration `m20260403_000013_drop_orphan_channel_override_columns` MUST remove these unused `monoize_channels` columns from the effective schema:

- `passive_min_samples_override`
- `passive_failure_rate_threshold_override`
- `request_timeout_ms_override`

ISM4.12. `providers` columns:

- `id` TEXT PK
- `name` TEXT NOT NULL
- `provider_type` TEXT NOT NULL
- `base_url` TEXT NULL
- `auth_type` TEXT NULL
- `auth_value` TEXT NULL
- `auth_header_name` TEXT NULL
- `auth_query_name` TEXT NULL
- `capabilities_json` TEXT NULL
- `strategy_json` TEXT NULL
- `enabled` INTEGER NOT NULL DEFAULT 1
- `priority` INTEGER NOT NULL DEFAULT 0
- `weight` INTEGER NOT NULL DEFAULT 1
- `tag` TEXT NULL
- `groups_json` TEXT NOT NULL DEFAULT '[]'
- `balance` REAL NULL
- `created_at` TEXT NOT NULL
- `updated_at` TEXT NOT NULL

ISM4.13. `model_mappings` columns:

- `id` TEXT PK
- `provider_id` TEXT NOT NULL
- `logical_model` TEXT NOT NULL
- `upstream_model` TEXT NOT NULL
- `created_at` TEXT NOT NULL
- UNIQUE(`provider_id`, `logical_model`)

ISM4.14. `group_members` columns:

- `id` TEXT PK
- `group_provider_id` TEXT NOT NULL
- `member_provider_id` TEXT NOT NULL
- `weight` INTEGER NOT NULL DEFAULT 1
- `priority` INTEGER NOT NULL DEFAULT 0
- `created_at` TEXT NOT NULL
- UNIQUE(`group_provider_id`, `member_provider_id`)

ISM4.15. `state_records` columns:

- `tenant_id` TEXT NOT NULL
- `kind` TEXT NOT NULL
- `id` TEXT NOT NULL
- `value` TEXT NOT NULL
- `expires_at` INTEGER NULL
- PRIMARY KEY(`tenant_id`, `kind`, `id`)

ISM4.16. `file_bytes` columns:

- `tenant_id` TEXT NOT NULL
- `file_id` TEXT NOT NULL
- `bytes` BLOB NOT NULL
- PRIMARY KEY(`tenant_id`, `file_id`)

## 5. Unique constraints and indexes

ISM5.1. Required unique constraints:

- `users.username`
- `sessions.token`
- `model_registry_records(logical_model, provider_id)`
- `monoize_provider_models(provider_id, model_name)`
- `model_mappings(provider_id, logical_model)`
- `group_members(group_provider_id, member_provider_id)`

ISM5.2. Required indexes:

- `idx_sessions_user_id` on `sessions(user_id)`
- `idx_sessions_token` on `sessions(token)`
- `idx_api_keys_user_id` on `api_keys(user_id)`
- `idx_api_keys_key_hash` on `api_keys(key_hash)`
- `idx_billing_ledger_user_id` on `billing_ledger(user_id)`
- `idx_request_logs_user_id` on `request_logs(user_id)`
- `idx_request_logs_created_at` on `request_logs(created_at)`
- `idx_request_logs_model` on `request_logs(model)`
- `idx_mpm_provider_id` on `monoize_provider_models(provider_id)`
- `idx_mc_provider_id` on `monoize_channels(provider_id)`
- `idx_mm_provider_id` on `model_mappings(provider_id)`
- `idx_gm_group_provider_id` on `group_members(group_provider_id)`

## 6. Foreign keys

ISM6.1. Foreign keys MAY be defined where cross-database compatible.

ISM6.2. If defined, foreign key edges SHOULD follow:

- `sessions.user_id -> users.id`
- `api_keys.user_id -> users.id`
- `billing_ledger.user_id -> users.id`
- `request_logs.user_id -> users.id`
- `monoize_provider_models.provider_id -> monoize_providers.id`
- `monoize_channels.provider_id -> monoize_providers.id`
- `model_mappings.provider_id -> providers.id`
- `group_members.group_provider_id -> providers.id`
- `group_members.member_provider_id -> providers.id`
