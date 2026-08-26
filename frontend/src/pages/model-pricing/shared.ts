import type { BillingMode, ModelPriceRecord } from "@/lib/api";

// Mirrors the server rule (model-pricing.spec.md MP-U1): non-negative base-10
// decimal string, <= 12 integer digits, <= 9 fractional digits. Values are
// validated and submitted as strings; they never pass through Number().
const USD_DECIMAL_RE = /^(\d{1,12})(?:\.(\d{0,9}))?$|^\.(\d{1,9})$/;

export function isValidUsdDecimal(value: string): boolean {
  return USD_DECIMAL_RE.test(value.trim());
}

/** Formats a stored USD decimal string for table display; null renders as a dash. */
export function formatUsdPerM(value: string | null | undefined): string {
  if (value == null || value.trim() === "") return "—";
  const trimmed = value.trim();
  const [whole, fraction = ""] = trimmed.split(".");
  const grouped = whole.replace(/\B(?=(\d{3})+(?!\d))/g, ",");
  const cleanFraction = fraction.replace(/0+$/, "");
  return `$${grouped}${cleanFraction ? `.${cleanFraction}` : ""}`;
}

export function formatRelativeTime(iso: string): string {
  const diff = Date.now() - new Date(iso).getTime();
  const mins = Math.floor(diff / 60_000);
  if (mins < 1) return "just now";
  if (mins < 60) return `${mins}m ago`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  if (days < 30) return `${days}d ago`;
  return new Date(iso).toLocaleDateString();
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

export interface PricingSheetTarget {
  mode: "create" | "edit";
  modelId: string;
  record: ModelPriceRecord | null;
}
