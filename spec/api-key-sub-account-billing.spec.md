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
- `meta_json` MUST include all fields from regular `request_charge` entries plus `api_key_id`.

SA-CH3. If deduction would make `sub_account_balance_nano` negative, server MUST return HTTP `402` with code `insufficient_balance` and MUST NOT write the deduction.

SA-CH4. When `sub_account_enabled = 0`:
- Charge deduction MUST use the owning user's balance, identical to current behavior.

### 3.3 Concurrency control

SA-CC1. Sub-account balance mutations MUST execute on the write pool (same as user balance mutations per `user-billing-and-model-metadata.spec.md` §6a).

SA-CC2. The charge path MUST use a single atomic transaction: read current balance → verify sufficient → update balance → write ledger → commit.

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
1. Verify `sub_account_enabled = 1` on the target key.
2. Verify owning user has sufficient balance (unless user is `balance_unlimited`).
3. Deduct `amount` from `users.balance_nano_usd`.
4. Add `amount` to `api_keys.sub_account_balance_nano`.
5. Write two ledger entries:
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

SA-API3. When `sub_account_enabled` is set to `true` during creation, the initial balance is `"0"` unless explicitly provided (admin only may provide initial balance).

### 5.3 Update API key

SA-API4. `PUT /api/dashboard/tokens/{key_id}` MUST accept optional fields:
- `sub_account_enabled: boolean`
- `sub_account_balance_nano_usd: string` (admin only)

SA-API5. Non-admin users MUST NOT be able to set `sub_account_balance_nano_usd` directly. They MUST use the transfer endpoint (§4.1).

SA-API6. Disabling sub-account (`sub_account_enabled: false`) on a key with non-zero positive balance MUST auto-refund the remaining balance to the owning user atomically:
1. Let `refund = sub_account_balance_nano` (current balance).
2. Set `api_keys.sub_account_balance_nano = "0"`.
3. Set `api_keys.sub_account_enabled = 0`.
4. Add `refund` to `users.balance_nano_usd`.
5. Write a ledger entry with `kind = "sub_account_refund"` (positive delta on user, with `api_key_id` in meta).

SA-API6a. If the owning user has `balance_unlimited = true`, the refund credit step (SA-API6 step 4) MUST be skipped, but the sub-account balance MUST still be zeroed and the ledger entry MUST still be written.

SA-API6b. Disabling sub-account on a key with zero balance MUST succeed without writing a refund ledger entry.

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
| `request_charge`               | negative  | Charge against user balance (existing, unchanged)        |
| `api_key_charge`               | negative  | Charge against API key sub-account balance               |
| `admin_adjustment`             | either    | Admin adjustment of user balance (existing)              |
| `sub_account_transfer_out`     | negative  | User balance deducted for transfer to API key            |
| `sub_account_transfer_in`      | positive  | API key sub-account credited from user transfer          |
| `sub_account_refund`           | positive  | Remaining sub-account balance returned to user           |
| `admin_sub_account_adjustment` | positive  | Admin direct increase of API key sub-account balance     |

## 9. Error codes

| Condition                                          | HTTP | Code                  | Message                                                   |
|----------------------------------------------------|------|-----------------------|-----------------------------------------------------------|
| Sub-account balance ≤ 0 at pre-forward             | 402  | `insufficient_balance`| `"insufficient balance"`                                  |
| Sub-account balance would go negative at charge    | 402  | `insufficient_balance`| `"insufficient balance"`                                  |
| Transfer to key with sub_account_enabled = 0       | 400  | `invalid_request`     | `"sub-account not enabled on this key"`                   |
| Transfer amount ≤ 0                                | 400  | `invalid_request`     | `"transfer amount must be positive"`                      |
| User insufficient balance for transfer             | 402  | `insufficient_balance`| `"insufficient balance for transfer"`                     |
