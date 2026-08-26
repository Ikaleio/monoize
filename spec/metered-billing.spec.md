# Metered Billing Specification (DEPRECATED)

## 0. Status

- Product name: Monoize.
- Status: **DEPRECATED.** The pricing-profile and `billing_rate_records` rate-matrix
  model defined by earlier revisions of this file is superseded by
  `spec/model-pricing.spec.md`.
- Migration `m20260901_000049_model_prices_cutover` removes the legacy engine
  (`model-pricing.spec.md` §12). No legacy rate storage, API, or compatibility alias
  remains after that migration.

## 1. Rule mapping

The table below maps the deprecated rule groups to their replacements. The replacement
rules are normative; the legacy rules are not.

| Deprecated rule group | Replacement |
|---|---|
| MB-D (rate storage `billing_rate_records`) | `model-pricing.spec.md` §2.1 `model_prices` (table dropped at cutover, §12.2) |
| MB-P (pricing profiles, `pricing_profile_model_patterns`) | Removed. Exact `model_id` lookup, `model-pricing.spec.md` §3 |
| MB-R (rate selection, context/service-tier/modality dimensions) | `model-pricing.spec.md` §3 and §4.4 (`tiered_expr`); service tier and modality no longer select prices (MP-R6) |
| MB-T (token billing) | `model-pricing.spec.md` §4.1–§4.2 |
| MB-M (server-native meter billing) | `model-pricing.spec.md` §6 (`tool_prices`, fail-open MP-T8) |
| MB-C1..MB-C4 (charge formula, breakdown v2) | `model-pricing.spec.md` §4.5 and §8 (breakdown v3) |
| MB-C5 (missing usage, per-Channel `allow_missing_usage`) | `model-pricing.spec.md` §7 (`allow_free_when_missing_usage` + Provider override) |
| MB-C6 (post-delivery settlement failure) | Unchanged rule, restated below as MB-X1 |
| MB-A (billing-rate dashboard APIs) | `model-pricing.spec.md` §10 (legacy endpoints removed at cutover) |

## 2. Rules that remain in force

MB-X1. Once pass-through stream bytes have been delivered, a settlement error MUST NOT
be converted into a successful zero-charge snapshot. Monoize MUST finalize the request
log as an explicit billing failure containing the billing error code. The server MUST
NOT claim that an error response was delivered downstream after the terminal stream
event has already been sent.

MB-X2. Stream consumption, ledger, and negative-balance settlement rules live in
`user-billing-and-model-metadata.spec.md` §6 and are unaffected by this deprecation.
