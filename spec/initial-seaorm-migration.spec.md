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
10. `monoize_channels`
11. `monoize_channel_models`
12. `state_records`
13. `file_bytes`

ISM2.3. `down()` MUST drop the same 13 tables in reverse dependency order.

## 3. Type mapping and key rules

ISM3.1. The migration MUST use `Table::create()` for every table.

ISM3.2. Column type rules MUST be:

- logical TEXT → `.text()`
- logical INTEGER → `.integer()`
- logical BIGINT → `.big_integer()`; physical type MUST be SQLite `INTEGER` and PostgreSQL `BIGINT`
- logical REAL → `.double()`; the physical type MUST be SQLite `REAL` and PostgreSQL `DOUBLE PRECISION` (`FLOAT8`)
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
- `created_at` TEXT NOT NULL
- `expires_at` TEXT NULL
- `last_used_at` TEXT NULL
- `enabled` INTEGER NOT NULL DEFAULT 1
- `quota_remaining` INTEGER NULL
- `quota_unlimited` INTEGER NOT NULL DEFAULT 0
- `model_limits_enabled` INTEGER NOT NULL DEFAULT 0
- `model_limits` TEXT NOT NULL DEFAULT '{}'
- `ip_whitelist` TEXT NOT NULL DEFAULT '[]'
- `token_group` TEXT NOT NULL DEFAULT 'default', removed by migration `m20260825_000042_groups_registry`
- `max_multiplier` TEXT NULL
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

ISM4.4a. `billing_ledger.user_id` is a historical identifier. The table MUST NOT define a foreign key from this column to `users`, because user deletion MUST preserve ledger history.

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
- `input_tokens` BIGINT NULL
- `output_tokens` BIGINT NULL
- `cache_read_tokens` BIGINT NULL
- `cache_creation_tokens` BIGINT NULL
- `tool_prompt_tokens` BIGINT NULL
- `reasoning_tokens` BIGINT NULL
- `accepted_prediction_tokens` BIGINT NULL
- `rejected_prediction_tokens` BIGINT NULL
- `provider_multiplier` TEXT NULL
- `charge_nano_usd` TEXT NULL
- `status` TEXT NOT NULL
- `usage_breakdown_json` TEXT NULL
- `billing_breakdown_json` TEXT NULL
- `error_code` TEXT NULL
- `error_message` TEXT NULL
- `error_http_status` BIGINT NULL
- `duration_ms` BIGINT NULL
- `ttfb_ms` BIGINT NULL
- `first_visible_output_ms` BIGINT NULL
- `last_visible_output_ms` BIGINT NULL
- `visible_generation_ms` BIGINT NULL
- `visible_output_tokens` BIGINT NULL
- `tps_mode` TEXT NULL
- `request_ip` TEXT NULL
- `reasoning_effort` TEXT NULL
- `tried_providers_json` TEXT NULL
- `request_kind` TEXT NULL
- `effective_provider_type` TEXT NULL
- `affinity_hit` INTEGER NULL
- `affinity_key_hash` TEXT NULL
- `affinity_target` TEXT NULL
- `created_at` TEXT NOT NULL
- `created_at_unix_ms` BIGINT NULL

ISM4.5a. The initial migration MUST create `request_logs` with exactly the 42 columns listed by ISM4.5. Later migrations MAY add nullable columns beyond these 42 via `ALTER TABLE ADD COLUMN` per `request-logs.spec.md` RL-S4 (currently `session_affinity_value`), and MAY drop columns per `request-logs.spec.md` RL-S12 (`first_visible_output_ms`, `last_visible_output_ms`, `visible_generation_ms`, `visible_output_tokens`, `tps_mode` are dropped by `m20260824_000040_drop_request_log_visible_tps`). `request_logs.user_id` is a historical identifier and MUST NOT have a foreign key to `users`. Deleting a user MUST preserve all request-log rows for that user.

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
- `max_input_tokens` BIGINT NULL
- `max_output_tokens` BIGINT NULL
- `max_tokens` BIGINT NULL
- `raw_json` TEXT NOT NULL
- `source` TEXT NOT NULL
- `updated_at` TEXT NOT NULL

ISM4.8a. `billing_rate_records` columns:

- `id` TEXT PK
- `source` TEXT NOT NULL
- `pricing_profile` TEXT NOT NULL
- `model_pattern` TEXT NULL
- `provider_type` TEXT NULL
- `rate_kind` TEXT NOT NULL
- `usage_class` TEXT NOT NULL
- `unit` TEXT NOT NULL
- `unit_price_nano_usd` TEXT NOT NULL
- `context_tier` TEXT NULL
- `service_tier` TEXT NULL
- `modality` TEXT NULL
- `cache_ttl` TEXT NULL
- `match_json` TEXT NOT NULL
- `priority` INTEGER NOT NULL DEFAULT 0
- `enabled` INTEGER NOT NULL DEFAULT 1
- `raw_json` TEXT NOT NULL
- `updated_at` TEXT NOT NULL

ISM4.9. `monoize_providers` columns:

- `id` TEXT PK
- `name` TEXT NOT NULL
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

ISM4.10. `monoize_provider_models` MUST NOT be created by the baseline migration and MUST NOT exist after all migrations complete.

ISM4.11. `monoize_channels` columns:

- `id` TEXT PK
- `provider_id` TEXT NOT NULL
- `name` TEXT NOT NULL
- `provider_type` TEXT NOT NULL
- `base_url` TEXT NOT NULL
- `api_key` TEXT NOT NULL
- `weight` INTEGER NOT NULL DEFAULT 1
- `enabled` INTEGER NOT NULL DEFAULT 1
- `passive_failure_threshold_override` INTEGER NULL
- `passive_cooldown_seconds_override` INTEGER NULL
- `passive_window_seconds_override` INTEGER NULL
- `passive_rate_limit_cooldown_seconds_override` INTEGER NULL
- `active_probe_enabled_override` INTEGER NULL
- `active_probe_interval_seconds_override` INTEGER NULL
- `active_probe_success_threshold_override` INTEGER NULL
- `active_probe_model_override` TEXT NULL
- `affinity_enabled_override` INTEGER NULL
- `affinity_idle_ttl_seconds_override` INTEGER NULL
- `affinity_failback_mode_override` TEXT NULL
- `affinity_failback_delay_seconds_override` INTEGER NULL
- `created_at` TEXT NOT NULL
- `updated_at` TEXT NOT NULL

ISM4.11a. Migration `m20260729_000024_channel_affinity_overrides` MUST add the four Channel affinity override columns when they are absent on SQLite or PostgreSQL. Running the migration when a column already exists MUST NOT fail.

ISM4.10a. Migration `m20260403_000013_drop_orphan_channel_override_columns` MUST be idempotent when these obsolete columns are absent from a fresh baseline schema:

- `passive_min_samples_override`
- `passive_failure_rate_threshold_override`
- `request_timeout_ms_override`

ISM4.12. `monoize_channel_models` columns:

- `id` TEXT PK
- `channel_id` TEXT NOT NULL
- `model_name` TEXT NOT NULL
- `redirect` TEXT NULL
- `multiplier` TEXT NOT NULL DEFAULT `"1"`
- `created_at` TEXT NOT NULL
- UNIQUE(`channel_id`, `model_name`)

ISM4.12a. Migration `m20260809_000026_exact_multiplier_text` MUST replace `monoize_channel_models.multiplier`, `api_keys.max_multiplier`, and `request_logs.provider_multiplier` floating-point columns with TEXT decimal columns on SQLite and PostgreSQL. Existing finite values MUST be copied as decimal text. Runtime reads and writes after this migration MUST NOT use `REAL`, `DOUBLE PRECISION`, `f32`, or `f64` for these fields.

ISM4.12b. Migration `m20260809_000025_storage_ledger_integrity` MUST remove the cascading `billing_ledger.user_id` foreign key while preserving every ledger row. On PostgreSQL it MUST also convert request-log token/timing counters and model-metadata maximum-token counters from `INTEGER` to `BIGINT` without changing their values.

ISM4.12c. On SQLite and PostgreSQL, every backend-specific DDL/data-copy statement in each of migrations `m20260809_000025_storage_ledger_integrity` and `m20260809_000026_exact_multiplier_text` MUST execute inside one database transaction. Any failed statement MUST roll back every earlier statement from that migration.

ISM4.12d. Migration `m20260809_000029_sessions_expires_at_index` MUST create `idx_sessions_expires_at` on `sessions(expires_at)` on SQLite and PostgreSQL. Its down migration MUST remove that index.

ISM4.12e. Migration `m20260809_000031_request_logs_without_user_fk` requires source columns `id`, `user_id`, `model`, `is_stream`, `status`, and `created_at`. It MUST fail and roll back when any required source column is absent. Every other nullable ISM4.5 column MAY be absent and MUST be added with null values. For `input_tokens`, `output_tokens`, and `cache_read_tokens`, an existing canonical non-null value MUST win; a null or absent canonical value MUST fall back respectively to legacy `prompt_tokens`, `completion_tokens`, or `cached_tokens`; both absent MUST produce null. The output on SQLite and PostgreSQL MUST contain exactly the 42 ISM4.5 columns. Every other column and every ordinary request-log index outside the ISM5.2 set MUST be removed. PostgreSQL MUST preserve indexes owned by table constraints and MUST drop both `request_logs_user_id_fkey` and `fk_request_logs_user_id` with `IF EXISTS`. The migration MUST perform inspection, data changes, and DDL in one transaction, MUST be idempotent, and MUST preserve every row. The migration down operation MUST be a no-op.

ISM4.12f. Migration `m20260823_000033_billing_ledger_delta_dedupe` MUST add nullable TEXT column `idempotency_key` to `billing_ledger` and one partial unique index over that column restricted to non-null values, identically on SQLite and PostgreSQL (`primary-replica-deployment.spec.md` SC1). Existing rows keep NULL. The down migration MUST drop the index then the column.

ISM4.12g. Migration `m20260823_000034_channel_egress_proxy` MUST add nullable TEXT column `proxy_url` to `monoize_channels` on SQLite and PostgreSQL; existing rows MUST read as NULL (follow-global) (`primary-replica-deployment.spec.md` SC3). The down migration MUST drop the column.
ISM4.12h. Migration `m20260823_000035_channel_extra_headers` MUST add nullable TEXT column `extra_headers` to `monoize_channels` on SQLite and PostgreSQL; existing rows MUST read as NULL (no extra headers) (`channel-management.spec.md` CP-INV-15). The down migration MUST drop the column.
ISM4.12i. Migration `m20260823_000036_channel_session_affinity_auto` MUST add nullable INTEGER column `session_affinity_auto` to `monoize_channels` on SQLite and PostgreSQL; existing rows MUST read as NULL (disabled) (`channel-management.spec.md` CM-AFF-2). The down migration MUST drop the column.

ISM4.12j. Migration `m20260825_000043_channel_allow_missing_usage` MUST add INTEGER column `allow_missing_usage` with `NOT NULL DEFAULT 0` to `monoize_channels` on SQLite and PostgreSQL. Existing rows MUST read as `false`. The down migration MUST drop the column.

ISM4.12k. Migration `m20260826_000046_channel_allow_unpriced_server_tools` MUST add INTEGER column `allow_unpriced_server_tools` with `NOT NULL DEFAULT 0` to `monoize_channels` on SQLite and PostgreSQL. Existing rows MUST read as `false`. The down migration MUST drop the column.

ISM4.12l. Migration `m20260826_000048_model_prices` MUST, on SQLite and PostgreSQL: create `model_prices` and `price_sync_runs` per `model-pricing.spec.md` §2; add TEXT column `billing_ratio` with `NOT NULL DEFAULT '1'` to `monoize_groups`; add nullable INTEGER columns `allow_free_when_unpriced_override` and `allow_free_when_missing_usage_override` to `monoize_providers`. Existing rows MUST read as ratio `'1'` and override NULL. It MUST NOT drop or alter any existing column. The down migration MUST drop the two tables and the three added columns.

ISM4.12j. Migration `m20260823_000037_billing_plan_cron_schedule` MUST replace `billing_plans.period_seconds` with `billing_plans.schedule` (`billing-plan-subscriptions.spec.md` BP-D5). Existing `users.next_grant_at` values MUST be left unchanged. The down migration MUST restore `period_seconds`.

ISM4.12k. Migration `m20260825_000042_groups_registry` MUST create `monoize_groups`, seed exactly one default group, backfill `users.group_id`, `api_keys.use_user_group`, `api_keys.group_ids`, `monoize_providers.group_ids`, and `billing_plans.group_ids` from the legacy label columns, and drop `users.allowed_groups`, `api_keys.allowed_groups`, `api_keys.token_group`, `monoize_providers.groups`, and `billing_plans.allowed_groups` (`groups-registry.spec.md` §4).

ISM4.13. Legacy `providers`, `model_mappings`, and `group_members` tables MUST NOT be created.

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
- `monoize_channel_models(channel_id, model_name)`

ISM5.2. Required indexes:

- `idx_sessions_user_id` on `sessions(user_id)`
- `idx_sessions_token` on `sessions(token)`
- `idx_sessions_expires_at` on `sessions(expires_at)`
- `idx_api_keys_user_id` on `api_keys(user_id)`
- `idx_api_keys_key` on `api_keys(key)`
- `idx_billing_ledger_user_id` on `billing_ledger(user_id)`
- `idx_request_logs_user_created_at` on `request_logs(user_id, created_at_unix_ms DESC)`
- `idx_request_logs_created_at` on `request_logs(created_at_unix_ms DESC)`
- `idx_request_logs_model` on `request_logs(model)`
- `idx_request_logs_legacy_created_at` on `request_logs(created_at)` where `created_at_unix_ms IS NULL`
- `idx_mc_provider_id` on `monoize_channels(provider_id)`
- `idx_mcm_channel_id` on `monoize_channel_models(channel_id)`

## 6. Foreign keys

ISM6.1. Foreign keys MAY be defined where cross-database compatible.

ISM6.2. If defined, foreign key edges SHOULD follow:

- `sessions.user_id -> users.id`
- `api_keys.user_id -> users.id`
- `monoize_channels.provider_id -> monoize_providers.id`
- `monoize_channel_models.channel_id -> monoize_channels.id`

ISM6.3. `request_logs.user_id -> users.id` MUST NOT be defined. `request_logs.user_id` MUST accept an identifier that has no current `users` row.
