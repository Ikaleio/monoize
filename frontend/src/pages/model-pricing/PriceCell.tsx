import { useTranslation } from "react-i18next";
import type { BillingExprTier } from "@/lib/api";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { formatUsdPerM, type PriceDisplaySource } from "./shared";

type PriceSide = "input" | "output";

function tierPrice(tier: BillingExprTier | undefined, side: PriceSide): string | null {
  if (!tier) return null;
  // MP-R5: a missing output price falls back to the input price.
  return side === "input" ? tier.input_usd_per_1m : (tier.output_usd_per_1m ?? tier.input_usd_per_1m);
}

function formatTokenCount(tokens: number): string {
  if (tokens >= 1_000_000 && tokens % 1_000_000 === 0) return `${tokens / 1_000_000}M`;
  if (tokens >= 1_000 && tokens % 1_000 === 0) return `${tokens / 1_000}K`;
  return tokens.toLocaleString();
}

/** One line per tier: threshold, then input / output price. */
function TierLines({ tiers }: { tiers: BillingExprTier[] }) {
  const { t } = useTranslation();
  return (
    <ul className="space-y-0.5 tabular-nums">
      {tiers.map((tier, index) => {
        const previous = index > 0 ? tiers[index - 1].when_input_tokens_lte : null;
        const range =
          tier.when_input_tokens_lte != null
            ? t("modelPricing.tierRangeUpTo", { tokens: formatTokenCount(tier.when_input_tokens_lte) })
            : previous != null
              ? t("modelPricing.tierRangeAbove", { tokens: formatTokenCount(previous) })
              : t("modelPricing.tierRangeAll");
        return (
          <li key={index}>
            {range}: {formatUsdPerM(tierPrice(tier, "input"))} / {formatUsdPerM(tierPrice(tier, "output"))}
          </li>
        );
      })}
    </ul>
  );
}

/**
 * Input or output price cell shared by the admin price list (MP-UI3) and the
 * marketplace (model-marketplace.spec.md §4.2). per_request rows show the
 * request price on the input side; tiered rows show the first tier with a
 * muted tier count and a tooltip listing every tier and the Fast table.
 */
export function PriceCell({ record, side }: { record: PriceDisplaySource; side: PriceSide }) {
  const { t } = useTranslation();

  if (record.billing_mode === "per_request") {
    if (side === "output" || !record.per_request_usd) return <span className="tabular-nums">—</span>;
    return (
      <span className="tabular-nums">
        {t("modelPricing.perRequestPrice", { price: formatUsdPerM(record.per_request_usd) })}
      </span>
    );
  }

  const tiers = record.billing_mode === "tiered_expr" ? (record.billing_expr?.tiers ?? []) : [];
  const fastTiers = record.billing_expr?.service_tiers?.fast?.tiers ?? [];
  const price =
    record.billing_mode === "tiered_expr"
      ? tierPrice(tiers[0], side)
      : side === "input"
        ? record.input_usd_per_1m
        : (record.output_usd_per_1m ?? record.input_usd_per_1m);

  if (tiers.length <= 1 && fastTiers.length === 0) {
    return <span className="tabular-nums">{formatUsdPerM(price)}</span>;
  }

  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          type="button"
          className="rounded-sm tabular-nums underline-offset-4 hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
        >
          {formatUsdPerM(price)}
          {tiers.length > 1 ? (
            <span className="ml-1 text-xs text-muted-foreground">
              {t("modelPricing.tieredFrom", { count: tiers.length })}
            </span>
          ) : null}
        </button>
      </TooltipTrigger>
      <TooltipContent className="max-w-[min(24rem,calc(100vw-2rem))] space-y-2 text-xs">
        <p className="font-medium">{t("modelPricing.tierTooltipTitle")}</p>
        {tiers.length > 0 ? <TierLines tiers={tiers} /> : (
          <p className="tabular-nums">
            {formatUsdPerM(record.input_usd_per_1m)} / {formatUsdPerM(record.output_usd_per_1m ?? record.input_usd_per_1m)}
          </p>
        )}
        {fastTiers.length > 0 ? (
          <>
            <p className="font-medium">{t("modelPricing.serviceTierFast")}</p>
            <TierLines tiers={fastTiers} />
          </>
        ) : null}
      </TooltipContent>
    </Tooltip>
  );
}
