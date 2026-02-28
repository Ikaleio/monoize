# State Store Runtime Specification

## 0. Status

- **Purpose:** Define runtime expiration behavior for in-memory and database-backed state records.
- **Scope:** Applies to `StateStore` implementations in `src/store.rs`.

## 1. Data model

SSR1. A state record consists of:
- `id: string`
- `value: JSON`
- `expires_at: integer?` (Unix timestamp seconds, nullable)

SSR2. `expires_at = null` means the record does not expire.

## 2. Expiration semantics

SSR3. `StateStore::get(tenant_id, kind, id)` MUST return `None` when the target record exists but is expired (`expires_at <= now_unix_seconds`).

SSR4. `StateStore::list(tenant_id, kind)` MUST exclude expired records from returned results.

SSR5. `StateStore::delete(tenant_id, kind, id)` remains explicit deletion and is independent from expiration filtering.

## 3. MemoryStateStore

SSR-M1. `MemoryStateStore::put` MUST store `expires_at` as provided.

SSR-M2. `MemoryStateStore::get` MUST apply SSR3 before returning a cloned record.

SSR-M3. `MemoryStateStore::list` MUST apply SSR4 when iterating records.

## 4. DbStateStore

SSR-D1. `DbStateStore::put` MUST persist `expires_at` as provided.

SSR-D2. `DbStateStore::get` MUST apply SSR3 using the persisted `expires_at` value.

SSR-D3. `DbStateStore::list` MUST apply SSR4 for each returned row.
