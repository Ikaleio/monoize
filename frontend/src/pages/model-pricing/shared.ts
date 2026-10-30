import type { BillingExpr, BillingMode, ModelPriceRecord } from "@/lib/api";

// Mirrors the server rule (model-pricing.spec.md MP-U1): non-negative base-10
// decimal string, <= 12 integer digits, <= 9 fractional digits. Values are
// validated and submitted as strings; they never pass through Number().
const USD_DECIMAL_RE = /^(\d{1,12})(?:\.(\d{0,9}))?$|^\.(\d{1,9})$/;

export function isValidUsdDecimal(value: string): boolean {
  return USD_DECIMAL_RE.test(value.trim());
}

/**
 * Shared USD price text for the admin list and the marketplace (design.md §6):
 * the exact stored decimal, trailing zeros trimmed, at least two fractional
 * digits (`$5.00`, `$0.60306`). Null or empty renders as an em dash.
 */
export function formatUsdPerM(value: string | null | undefined): string {
  if (value == null || value.trim() === "") return "—";
  const trimmed = value.trim();
  const [whole, fraction = ""] = trimmed.split(".");
  const grouped = whole.replace(/\B(?=(\d{3})+(?!\d))/g, ",");
  const cleanFraction = fraction.replace(/0+$/, "").padEnd(2, "0");
  return `$${grouped}.${cleanFraction}`;
}

/** Fields a list or marketplace row needs to render its price cells. */
export interface PriceDisplaySource {
  billing_mode: BillingMode | null;
  input_usd_per_1m: string | null;
  output_usd_per_1m: string | null;
  per_request_usd: string | null;
  billing_expr: BillingExpr | null;
}

/** MP-R3 completeness: an incomplete enabled row is billed like a missing row. */
export function isPriceRecordComplete(record: ModelPriceRecord): boolean {
  if (record.billing_mode === "per_request") return !!record.per_request_usd;
  if (record.billing_mode === "tiered_expr") return (record.billing_expr?.tiers.length ?? 0) > 0;
  return !!record.input_usd_per_1m;
}

export const BILLING_MODES: BillingMode[] = [
  "per_token",
  "per_request",
  "tiered_expr",
];

export const PER_TOKEN_PRICE_FIELDS = [
  "input_usd_per_1m",
  "output_usd_per_1m",
  "cache_read_usd_per_1m",
  "cache_write_usd_per_1m",
  "cache_write_1h_usd_per_1m",
  "reasoning_usd_per_1m",
] as const;

export type PerTokenPriceField = (typeof PER_TOKEN_PRICE_FIELDS)[number];

export function priceFieldValue(
  record: ModelPriceRecord | null,
  field: PerTokenPriceField
): string {
  return record?.[field] ?? "";
}

export interface PricingEditorTarget {
  mode: "create" | "edit";
  modelId: string;
  record: ModelPriceRecord | null;
}
