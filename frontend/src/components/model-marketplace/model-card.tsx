import { ModelIcon } from "@/components/ModelIcon";
import { getModelCardSpan } from "@/components/model-marketplace/model-grid";
import { Badge } from "@/components/ui/badge";
import {
  Card,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Separator } from "@/components/ui/separator";
import { formatUsdDecimal } from "@/lib/exact-decimal";
import type { MarketplaceModelRecord } from "@/lib/api";
import { useTranslation } from "react-i18next";

function formatTokens(tokens?: number | null): string {
  if (tokens == null) return "—";
  if (tokens >= 1_000_000) {
    const millions = tokens / 1_000_000;
    return `${Number.isInteger(millions) ? millions : millions.toFixed(1)}M`;
  }
  if (tokens >= 1_000) return `${Math.round(tokens / 1_000)}K`;
  return tokens.toString();
}

function formatPrice(price: string | null): string {
  return price == null ? "—" : `${formatUsdDecimal(price, 3)} / 1M`;
}

interface ModelMarketplaceCardProps {
  index: number;
  record: MarketplaceModelRecord;
}

export function ModelMarketplaceCard({
  index,
  record,
}: ModelMarketplaceCardProps) {
  const { t } = useTranslation();
  const titleId = `marketplace-model-${index}`;

  return (
    <article className={getModelCardSpan(index)} aria-labelledby={titleId}>
      <Card className="flex h-full flex-col overflow-hidden">
        <CardHeader>
          <div className="flex items-center justify-between gap-4">
            <div className="flex size-10 shrink-0 items-center justify-center rounded-md border bg-secondary text-secondary-foreground">
              <ModelIcon
                model={record.model_id}
                provider={record.models_dev_provider}
                className="size-5"
              />
            </div>
            <Badge variant="outline" className="text-sm font-normal">
              {record.mode || "—"}
            </Badge>
          </div>
          <div className="flex min-w-0 flex-col gap-2">
            <CardTitle
              id={titleId}
              className="break-words font-mono text-base leading-relaxed"
            >
              {record.model_id}
            </CardTitle>
            <CardDescription className="text-sm leading-relaxed">
              {t("modelMarketplace.provider")}:{" "}
              {record.models_dev_provider || "—"}
            </CardDescription>
          </div>
        </CardHeader>

        <CardContent className="mt-auto grid grid-cols-2 gap-4">
          <div className="flex min-w-0 flex-col gap-2">
            <p className="text-sm text-muted-foreground">
              {t("modelMarketplace.inputCost")}
            </p>
            <p className="break-words text-base font-semibold tracking-tight">
              {formatPrice(record.input_usd_per_1m)}
            </p>
          </div>
          <div className="flex min-w-0 flex-col gap-2">
            <p className="text-sm text-muted-foreground">
              {t("modelMarketplace.outputCost")}
            </p>
            <p className="break-words text-base font-semibold tracking-tight">
              {formatPrice(record.output_usd_per_1m)}
            </p>
          </div>
        </CardContent>

        <Separator />

        <CardFooter className="p-6 pt-4">
          <dl className="grid w-full grid-cols-2 gap-4">
            <div className="flex min-w-0 flex-col gap-1">
              <dt className="text-sm text-muted-foreground">
                {t("modelMarketplace.context")}
              </dt>
              <dd className="font-mono text-sm font-medium">
                {formatTokens(record.max_tokens)}
              </dd>
            </div>
            <div className="flex min-w-0 flex-col gap-1">
              <dt className="text-sm text-muted-foreground">
                {t("modelMarketplace.maxOutput")}
              </dt>
              <dd className="font-mono text-sm font-medium">
                {formatTokens(record.max_output_tokens)}
              </dd>
            </div>
          </dl>
        </CardFooter>
      </Card>
    </article>
  );
}
