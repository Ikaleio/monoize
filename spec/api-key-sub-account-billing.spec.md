# API Key Sub-Account Billing Specification

## 0. Status

- **Purpose:** Define per-API-key independent balance (sub-account) billing, replacing the legacy per-call quota system.
- **Scope:** Applies to `api_keys` table, forwarding handlers, dashboard API key endpoints, and billing execution paths.
- **Replaces:** `quota_remaining` and `quota_unlimited` fields from `api-token-management.spec.md`.

## 1. Motivation

The legacy quota system counted API key usage by request count (decrement-by-1 per call), regardless of actual cost. This spec replaces it with a sub-account model where each API key optionally holds its own nano-dollar balance. When enabled, charges deduct from the API key's balance using the same token-based pricing as user-level billing.

## 2. Data model changes

### 2.1 New fields on `api_keys`

| Column                     | Type          | Default   | Description                                              |
|----------------------------|---------------|-----------|----------------------------------------------------------|
| `sub_account_enabled`      | `INTEGER`     | `0`       | `0` = inherit user balance (default). `1` = use own balance. |
| `sub_account_balance_nano` | `TEXT`         | `"0"`     | Signed integer nano-dollar string. Only meaningful when `sub_account_enabled = 1`. |

### 2.2 Removed fields from `api_keys`

| Column              | Disposition                                              |
|---------------------|----------------------------------------------------------|
| `quota_remaining`   | Removed. Migration MUST drop this column.                |
| `quota_unlimited`   | Removed. Migration MUST drop this column.                |

### 2.3 Precision and storage

SA-P1. Sub-account balance MUST use the same nano-dollar precision as user balance: `1 USD = 1_000_000_000 nano_usd`.

SA-P2. `sub_account_balance_nano` MUST be stored as a `TEXT` column containing a signed integer string, matching the `users.balance_nano_usd` convention.

SA-P3. Balance arithmetic MUST use checked integer operations. Overflow MUST return `500 internal_error`.

## 3. Billing flow changes

### 3.1 Balance eligibility check (pre-forward)

SA-BE1. When `sub_account_enabled = 1` on the authenticated API key:
- The pre-forward balance check MUST verify `sub_account_balance_nano > 0` instead of checking the owning user's balance.
- If `sub_account_balance_nano <= 0`, server MUST return HTTP `402` with code `insufficient_balance`.

SA-BE2. When `sub_account_enabled = 0` (default):
- The pre-forward balance check MUST use the owning user's balance, exactly as the current `ensure_balance_before_forward` works.

SA-BE3. The `ensure_quota_before_forward` function MUST be removed entirely.

### 3.2 Charge deduction (post-response)

SA-CH1. When `sub_account_enabled = 1`:
- The charge MUST deduct from `api_keys.sub_account_balance_nano`, NOT from `users.balance_nano_usd`.
- The charge calculation (token-based pricing with multipliers) MUST be identical to user-level billing as defined in `user-billing-and-model-metadata.spec.md` §5.

SA-CH2. On successful sub-account deduction, server MUST append a ledger row with:
- `user_id` = owning user's ID
- `kind = "api_key_charge"`
- `delta_nano_usd` (negative value)
- `balance_after_nano_usd` = the API key's balance after deduction
- `meta_json` MUST include all fields from regular `request_charge` entries, including `request_id`, plus `api_key_id`.

SA-CH3. A request admitted while `sub_account_balance_nano > 0` MUST settle its complete final charge with checked integer subtraction even when the resulting balance is zero or negative. The charge and ledger row MUST commit in the same transaction. A later request MUST fail the SA-BE1 admission gate while the persisted balance is non-positive.

SA-CH4. When `sub_account_enabled = 0`:
- Charge deduction MUST use the owning user's balance, identical to current behavior.

### 3.3 Concurrency control

SA-CC1. Sub-account balance mutations MUST execute on the write pool (same as user balance mutations per `user-billing-and-model-metadata.spec.md` §6a).

SA-CC2. The charge path MUST use a single atomic transaction: lock participating rows → read current balance → checked subtraction → update balance → write ledger → commit.

SA-CC3. On PostgreSQL, every mutation involving both a user and an API key MUST acquire row locks in deterministic order: first `users`, then `api_keys`, each with `SELECT ... FOR UPDATE`. The transaction MUST recompute each new balance from the locked values. It MUST NOT update a balance from a value read before the transaction.

SA-CC4. If an admitted sub-account request settles after another transaction has disabled and consolidated that sub-account, settlement MUST deduct the charge from the locked owning user balance and write `request_charge`. It MUST NOT recreate an ignored negative balance on the disabled key, and it MUST NOT omit the charge.

SA-CC5. If an admitted sub-account request settles after another transaction has deleted and consolidated that API key, settlement MUST deduct the charge from the locked owning user balance and write `request_charge`. Deleting an API key MUST NOT cancel the charge of an already admitted request.

## 4. Balance transfer

### 4.1 User-to-key transfer

SA-TX1. Endpoint: `POST /api/dashboard/tokens/{key_id}/transfer`

SA-TX2. Authorization: The authenticated user MUST own the API key. Admin/super-admin users MAY transfer to any key.

SA-TX3. Request body:
```json
{
  "amount_nano_usd": "string",   // positive integer nano-dollar string (required if amount_usd not provided)
  "amount_usd": "string"         // positive decimal USD string (required if amount_nano_usd not provided)
}
```

SA-TX4. If both `amount_nano_usd` and `amount_usd` are provided, server MUST use `amount_nano_usd`.

SA-TX5. Transfer MUST execute atomically in a single transaction:
1. Lock the owning user and target API key in the SA-CC3 order.
2. Verify `sub_account_enabled = 1` on the locked target key.
3. Verify the locked owning user has sufficient balance (unless user is `balance_unlimited`).
4. Deduct `amount` from the locked `users.balance_nano_usd`.
5. Add `amount` to the locked `api_keys.sub_account_balance_nano`.
6. Write two ledger entries:
   - `kind = "sub_account_transfer_out"`, negative delta, on user
   - `kind = "sub_account_transfer_in"`, positive delta, on user (with `api_key_id` in meta)

SA-TX6. If the owning user is `balance_unlimited = true`, the user balance deduction step MUST be skipped (unlimited users can fund sub-accounts without draining their own balance). The transfer MUST still credit the API key sub-account and write the `sub_account_transfer_in` ledger entry.

SA-TX7. Transfer to a key with `sub_account_enabled = 0` MUST be rejected with HTTP `400` and code `invalid_request`.

SA-TX8. Transfer amount MUST be positive. Zero or negative amounts MUST be rejected with HTTP `400` and code `invalid_request`.

SA-TX9. Response:
```json
{
  "success": true,
  "api_key_balance_nano_usd": "string",
  "user_balance_nano_usd": "string"
}
```

### 4.2 Admin direct adjustment

SA-ADM1. Admin users MAY directly set `sub_account_balance_nano` via `PUT /api/dashboard/tokens/{key_id}` with field `sub_account_balance_nano_usd: string`.

SA-ADM2. If the new balance is **lower** than the current balance, the difference MUST be refunded to the owning user's balance atomically:
1. Let `refund = old_balance - new_balance`.
2. Set `api_keys.sub_account_balance_nano = new_balance`.
3. Add `refund` to `users.balance_nano_usd`.
4. Write a ledger entry with `kind = "sub_account_refund"` (positive delta on user, with `api_key_id` in meta).

SA-ADM3. If the new balance is **higher** than the current balance, only the API key balance is updated (admin top-up does not deduct from user). Write a ledger entry with `kind = "admin_sub_account_adjustment"`.

SA-ADM4. If the owning user has `balance_unlimited = true`, the refund credit step (SA-ADM2 step 3) MUST be skipped, but the sub-account balance MUST still be reduced and the ledger entry MUST still be written.

## 5. API surface changes

### 5.1 API key data model (read)

SA-API1. The API key read model MUST replace `quota_remaining` and `quota_unlimited` with:
- `sub_account_enabled: boolean`
- `sub_account_balance_nano_usd: string`
- `sub_account_balance_usd: string` (computed from nano, same precision rules as user balance)

### 5.2 Create API key

SA-API2. `POST /api/dashboard/tokens` MUST accept optional fields:
- `sub_account_enabled: boolean` (default `false`)
- `sub_account_balance_nano_usd: string` (default `"0"`)

SA-API3. When `sub_account_enabled` is set to `true` during creation, the initial balance is `"0"` unless explicitly provided. Only an admin MAY provide an initial balance. The explicit initial balance MUST be a canonical non-negative nano-dollar integer string. A non-zero initial balance with `sub_account_enabled = false` MUST be rejected. A non-zero initial balance MUST append `kind = "admin_sub_account_adjustment"` in the same transaction as key creation.

### 5.3 Update API key

SA-API4. `PUT /api/dashboard/tokens/{key_id}` MUST accept optional fields:
- `sub_account_enabled: boolean`
- `sub_account_balance_nano_usd: string` (admin only)

SA-API5. Non-admin users MUST NOT be able to set `sub_account_balance_nano_usd` directly. They MUST use the transfer endpoint (§4.1).

SA-API5a. An update MUST reject a body that combines `sub_account_enabled = false` with `sub_account_balance_nano_usd`. Disabling consolidates the locked current balance under SA-API6; it MUST NOT depend on a caller-supplied replacement balance. An update that leaves `sub_account_enabled = false` MUST reject a non-zero replacement balance.

SA-API6. Disabling sub-account (`sub_account_enabled: false`) on a key with a non-zero signed balance MUST consolidate the complete balance into the owning user atomically:
1. Lock the owning user and API key in the SA-CC3 order and let `settlement = sub_account_balance_nano` from the locked key.
2. Set `api_keys.sub_account_balance_nano = "0"`.
3. Set `api_keys.sub_account_enabled = 0`.
4. Add the signed `settlement` to `users.balance_nano_usd` using checked integer arithmetic. A positive value returns credit; a negative value transfers debt.
5. Write a ledger entry with `kind = "sub_account_refund"` for a positive settlement or `kind = "sub_account_debt_transfer"` for a negative settlement. `delta_nano_usd` MUST equal `settlement`, `balance_after_nano_usd` MUST equal the locked user's resulting balance, and `meta_json` MUST include `api_key_id`.
6. Commit all steps in one transaction. Failure of any step MUST leave the key enabled with its original balance and MUST leave the user and ledger unchanged.

SA-API6a. If the owning user has `balance_unlimited = true`, the finite user balance mutation in SA-API6 step 4 MUST be skipped, but the sub-account balance MUST still be zeroed and the signed settlement ledger entry MUST still be written with `balance_after_nano_usd = null`.

SA-API6b. Disabling sub-account on a key with zero balance MUST succeed without writing a refund ledger entry.

### 5.4 Delete API key

SA-DEL1. Single-key and batch-key deletion MUST run in a transaction. For every selected key, the transaction MUST lock the owning user before the API key as required by SA-CC3.

SA-DEL1a. Batch deletion MUST lock all distinct owning users with one ordered set query and then lock all selected API keys with one ordered set query. It MUST NOT issue one user-lock query per user or one key-lock query per key.

SA-DEL2. Before deleting a sub-account key, the transaction MUST add its complete signed `sub_account_balance_nano` to the owning finite user's balance with checked integer arithmetic. This rule applies to positive, zero, and negative key balances. The transaction MUST NOT erase sub-account debt by deleting the key.

SA-DEL3. For each deleted sub-account key with a non-zero balance, the transaction MUST append `kind = "sub_account_delete_settlement"` with `delta_nano_usd` equal to the signed key balance, `balance_after_nano_usd` equal to the resulting finite user balance, and `meta_json.api_key_id` equal to the deleted key ID. For an unlimited user, the finite balance update is skipped and `balance_after_nano_usd` is null, but the settlement ledger row MUST still be appended.

SA-DEL4. Batch deletion MUST compute settlements in deterministic `(user_id, api_key_id)` order in memory from the locked values. Finite user balances MUST be updated through bind-safe CASE chunks, and settlement ledger rows MUST be inserted through bind-safe multi-row chunks. The number of database round trips MAY grow by fixed chunk count but MUST NOT grow by one round trip per deleted key.

SA-DEL4. The balance consolidation, ledger append, and key deletion MUST commit together. Any parse error, overflow, update error, ledger error, or delete error MUST roll back the entire single-key or batch-key operation.

## 6. Auth context changes

SA-AUTH1. `AuthResult` MUST replace `quota_remaining: Option<i32>` and `quota_unlimited: bool` with:
- `sub_account_enabled: bool`
- `sub_account_balance_nano: i128` (loaded at auth time for pre-forward check)

SA-AUTH2. The API key cache MUST include `sub_account_enabled` and `sub_account_balance_nano`.

SA-AUTH3. After a sub-account charge, the API key cache entry MUST be invalidated.

## 7. Migration

SA-MIG1. Migration name: `m20260328_000001_api_key_sub_account_billing`.

SA-MIG2. Migration MUST:
1. Add column `sub_account_enabled INTEGER NOT NULL DEFAULT 0`.
2. Add column `sub_account_balance_nano TEXT NOT NULL DEFAULT '0'`.
3. Drop column `quota_remaining`.
4. Drop column `quota_unlimited`.

SA-MIG3. Data migration for existing keys:
- Keys with `quota_unlimited = 1`: set `sub_account_enabled = 0` (inherit user balance — equivalent behavior).
- Keys with `quota_unlimited = 0` and `quota_remaining IS NOT NULL`: set `sub_account_enabled = 0` (per-call quota has no direct nano-dollar equivalent; default to inherit).

## 8. Ledger entry kinds

| Kind                           | Direction | Description                                              |
|--------------------------------|-----------|----------------------------------------------------------|
| `request_charge`               | non-positive | Request fallback charged to prepaid balance after plan allocation |
| `api_key_charge`               | non-positive | Request fallback charged to API key sub-account after plan allocation |
| `admin_adjustment`             | either    | Admin adjustment of user balance (existing)              |
| `plan_purchase`                | non-positive | Plan purchase from prepaid balance; zero for unlimited users |
| `sub_account_transfer_out`     | negative  | User balance deducted for transfer to API key            |
| `sub_account_transfer_in`      | positive  | API key sub-account credited from user transfer          |
| `sub_account_refund`           | positive  | Positive sub-account balance returned to user             |
| `sub_account_debt_transfer`    | negative  | Negative sub-account balance transferred to user          |
| `sub_account_delete_settlement`| either    | Signed sub-account balance consolidated before key delete |
| `admin_sub_account_adjustment` | positive  | Admin direct increase of API key sub-account balance     |
| `recharge`                     | positive  | Wallet credited by a succeeded recharge order (`recharge-system.spec.md` RC-L2) |
| `recharge_refund`              | negative  | Wallet debited by a refunded recharge order (`recharge-system.spec.md` RC-L2) |

## 9. Error codes

| Condition                                          | HTTP | Code                  | Message                                                   |
|----------------------------------------------------|------|-----------------------|-----------------------------------------------------------|
| Sub-account balance ≤ 0 at pre-forward             | 402  | `insufficient_balance`| `"insufficient balance"`                                  |
| Transfer to key with sub_account_enabled = 0       | 400  | `invalid_request`     | `"sub-account not enabled on this key"`                   |
| Transfer amount ≤ 0                                | 400  | `invalid_request`     | `"transfer amount must be positive"`                      |
| User insufficient balance for transfer             | 402  | `insufficient_balance`| `"insufficient balance for transfer"`                     |
