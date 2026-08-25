# Dashboard Data Cache Specification

## 1. Scope

DC1. This specification applies to the browser-process SWR cache used by the dashboard.

DC2. An SWR key that contains authenticated data MUST be scoped to the currently authenticated principal by cache invalidation at every authentication transition.

## 2. Authentication transitions

DC3. After login or registration succeeds and before the new token becomes active, the client MUST delete every SWR cache entry without revalidation.

DC4. After logout starts clearing authentication state, the client MUST delete every SWR cache entry without revalidation even when the logout HTTP request fails.

DC5. When current-user refresh rejects the stored authentication state, the client MUST clear the token and every SWR cache entry. This deletion MUST include fixed keys, parameterized request-log and analytics keys, marketplace keys, and Provider-detail keys.

## 3. Mutation dependencies

DC6. A successful full settings mutation MUST revalidate `PUBLIC_SETTINGS`, `PRICING_PROFILE_PATTERNS`, and `PROVIDERS` after publishing the returned `SETTINGS` value.

DC7. A successful Provider create, update, or delete MUST revalidate `PROVIDERS`, `CONFIG`, and `MARKETPLACE_MODELS`. Create and delete MUST also revalidate `STATS`. Delete MUST remove the deleted Provider-detail key without revalidation. Provider mutations MUST NOT revalidate `DASHBOARD_GROUPS`: the group registry is a first-class resource and is not derived from provider rows.

DC8. A successful model-metadata create, update, delete, batch delete, or models.dev sync
MUST revalidate `MODEL_METADATA`, `MARKETPLACE_MODELS`, `PROVIDERS`, and `BILLING_RATES`.

DC9. A successful billing-rate create, update, delete, or catalog sync MUST revalidate `BILLING_RATES` and `PROVIDERS`.

DC10. A successful pricing-pattern mutation MUST publish the returned `PRICING_PROFILE_PATTERNS` value and revalidate `SETTINGS` and `PROVIDERS`.

DC11. A successful user create MUST revalidate `USERS` and `STATS`. A successful user update MUST revalidate those keys plus `ME`. A successful user delete MUST revalidate `USERS` and `STATS`. User mutations MUST NOT revalidate `DASHBOARD_GROUPS`.

DC11a. A successful group create, update, or reorder MUST revalidate `DASHBOARD_GROUPS`. A successful group delete MUST revalidate `DASHBOARD_GROUPS`, `USERS`, `API_KEYS`, `PROVIDERS`, `BILLING_PLANS`, and `ME` because the server-side deletion cascade rewrites group references in those entities (`groups-registry.spec.md` §3).

## 4. Global operations

DC12. Global revalidation MUST target every key currently present in the SWR cache. It MUST NOT rely on a fixed key list.

DC13. Global cache deletion MUST target every key currently present in the SWR cache and MUST disable revalidation for that operation.

DC14. Every dashboard consumer of one server resource MUST use that resource's exported canonical SWR key and hook. A page MUST NOT create an alias key for `MODEL_METADATA` or another exported resource because mutation invalidation would leave the alias stale.
