import type { RechargeOrderStatus } from "./api";

// Exact BigInt arithmetic for the recharge flow (recharge-system.spec.md
// RC-U2/RC-U6): no amount or rate passes through IEEE floats.

const NANO_PER_USD = 1_000_000_000n;

/** Parse a positive USD decimal string into nano-USD; null when invalid. */
export function parseUsdToNano(usd: string): bigint | null {
  const trimmed = usd.trim();
  if (!/^\d+(?:\.\d*)?$/.test(trimmed)) return null;
  const [wholeRaw, fracRaw = ""] = trimmed.split(".");
  if (fracRaw.length > 9) return null;
  const value =
    BigInt(wholeRaw) * NANO_PER_USD + BigInt(fracRaw.padEnd(9, "0") || "0");
  return value > 0n ? value : null;
}

interface ParsedRate {
  mantissa: bigint;
  scale: number;
}

/** Parse an RC-U5 `usd_rate` decimal string; null when invalid. */
function parseRate(rate: string): ParsedRate | null {
  const match = /^(\d{1,12})(?:\.(\d{1,9}))?$/.exec(rate.trim());
  if (!match) return null;
  const frac = match[2] ?? "";
  const mantissa = BigInt(match[1] + frac);
  if (mantissa <= 0n) return null;
  return { mantissa, scale: frac.length };
}

/**
 * RC-U6: pay_amount = ceil_to_scale(credit_usd * usd_rate, payScale),
 * returned as minor units (integer count of 10^-payScale currency units).
 */
export function payMinorUnits(
  creditNanoUsd: bigint,
  usdRate: string,
  payScale: number,
): bigint | null {
  if (creditNanoUsd <= 0n || payScale < 0 || payScale > 9) return null;
  const rate = parseRate(usdRate);
  if (!rate) return null;
  const divisor = 10n ** BigInt(9 + rate.scale - payScale);
  const product = creditNanoUsd * rate.mantissa;
  const ceil = product % divisor === 0n ? product / divisor : product / divisor + 1n;
  return ceil > 0n ? ceil : 1n;
}

/** Render minor units as an RC-U4 decimal string with exactly `scale` digits. */
export function formatMinorUnits(units: bigint, scale: number): string {
  if (scale === 0) return units.toString();
  const divisor = 10n ** BigInt(scale);
  const whole = units / divisor;
  const frac = (units % divisor).toString().padStart(scale, "0");
  return `${whole}.${frac}`;
}

/** Convenience: full RC-U6 preview from a USD input string; null when invalid. */
export function previewPayAmount(
  creditUsd: string,
  usdRate: string,
  payScale: number,
): string | null {
  const nano = parseUsdToNano(creditUsd);
  if (nano === null) return null;
  const units = payMinorUnits(nano, usdRate, payScale);
  return units === null ? null : formatMinorUnits(units, payScale);
}

/** RC-P3 version-1 capability matrix, keyed by adapter `type_id`. */
export const SUPPORTS_REFUND: Record<string, boolean> = {
  epay: false,
  stripe: true,
};

export const PAYMENT_TYPE_IDS = ["epay", "stripe"] as const;

/** RC-W5: default wallet-ledger kinds — every non-per-request kind. */
export const WALLET_LEDGER_KINDS = [
  "recharge",
  "recharge_refund",
  "admin_adjustment",
  "plan_grant",
  "sub_account_transfer_out",
  "sub_account_transfer_in",
  "sub_account_refund",
  "sub_account_debt_transfer",
  "sub_account_delete_settlement",
  "admin_sub_account_adjustment",
] as const;

export const ORDER_STATUSES: RechargeOrderStatus[] = [
  "pending",
  "succeeded",
  "failed",
  "expired",
  "refunded",
];

export type StatusVariant = "success" | "warning" | "info" | "destructive";

export const ORDER_STATUS_VARIANTS: Record<RechargeOrderStatus, StatusVariant> = {
  pending: "info",
  succeeded: "success",
  failed: "destructive",
  expired: "warning",
  refunded: "warning",
};
