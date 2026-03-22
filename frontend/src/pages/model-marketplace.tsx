import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Search, Store } from "lucide-react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Badge } from "@/components/ui/badge";
import { Skeleton } from "@/components/ui/skeleton";
import { ModelBadge } from "@/components/ModelBadge";
import { useModelMetadata } from "@/lib/swr";
import { PageWrapper, motion, transitions } from "@/components/ui/motion";
import { TableVirtuoso } from "react-virtuoso";

function nanoToPerMillion(nano?: string | null): string {
  if (!nano) return "-";
  const n = Number(nano);
  if (!Number.isFinite(n)) return "-";
  const perM = n / 1000;
  if (perM === 0) return "$0";
  if (perM < 0.0001) return `$${perM.toFixed(6)}`;
  return `$${perM.toFixed(4)}`;
}

function formatTokens(tokens?: number | null): string {
  if (tokens == null) return "-";
  if (tokens >= 1_000_000) return `${(tokens / 1_000_000).toFixed(1)}M`;
  if (tokens >= 1_000) return `${(tokens / 1_000).toFixed(0)}K`;
  return tokens.toString();
}

export function ModelMarketplacePage() {
  const { t } = useTranslation();
  const { data: records = [], isLoading } = useModelMetadata();
  const [search, setSearch] = useState("");

  const filtered = records.filter((r) =>
    r.model_id.toLowerCase().includes(search.toLowerCase())
  );

  if (isLoading) {
    return (
      <div className="space-y-6">
        <Skeleton className="h-8 w-48" />
        <Skeleton className="h-12 w-full" />
        <Skeleton className="h-64 w-full" />
      </div>
    );
  }

  return (
    <PageWrapper className="space-y-6">
      <motion.div
        initial={{ opacity: 0, y: -10 }}
        animate={{ opacity: 1, y: 0 }}
        transition={transitions.normal}
        className="flex flex-wrap items-center justify-between gap-4"
      >
        <div className="min-w-0">
          <h1 className="text-3xl font-bold tracking-tight">{t("modelMarketplace.title")}</h1>
          <p className="text-muted-foreground">{t("modelMarketplace.description")}</p>
        </div>
      </motion.div>

      <motion.div
        initial={{ opacity: 0, y: 20 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ delay: 0.1, ...transitions.normal }}
      >
        <Card>
          <CardHeader className="pb-3">
            <div className="flex items-center justify-between">
              <CardTitle className="flex items-center gap-2">
                <Store className="h-5 w-5" />
                {t("modelMarketplace.title")}
              </CardTitle>
              <div className="relative w-64">
                <Search className="absolute left-2.5 top-2.5 h-4 w-4 text-muted-foreground" />
                <Input
                  value={search}
                  onChange={(e) => setSearch(e.target.value)}
                  placeholder={t("modelMarketplace.searchPlaceholder")}
                  className="pl-9"
                />
              </div>
            </div>
          </CardHeader>
          <CardContent>
            {filtered.length === 0 ? (
              <div className="flex flex-col items-center justify-center py-12 text-center">
                <Store className="h-12 w-12 text-muted-foreground/30 mb-4" />
                <p className="text-lg font-medium text-muted-foreground">{t("modelMarketplace.noModels")}</p>
                <p className="text-sm text-muted-foreground/70 mt-1">{t("modelMarketplace.noModelsDesc")}</p>
              </div>
            ) : (
              <TableVirtuoso
                style={{ height: "calc(100vh - 280px)", minHeight: 400 }}
                data={filtered}
                components={{
                  Table: (props) => <table {...props} className="w-full caption-bottom text-sm" />,
                  TableHead: (props) => <thead {...props} className="[&_tr]:border-b" />,
                  TableRow: (props) => <tr {...props} className="border-b transition-colors hover:bg-muted/50" />,
                  TableBody: (props) => <tbody {...props} className="[&_tr:last-child]:border-0" />,
                }}
                fixedHeaderContent={() => (
                  <tr className="border-b bg-background">
                    <th className="h-10 px-4 text-left align-middle font-medium text-muted-foreground min-w-[200px]">
                      {t("modelMarketplace.modelId")}
                    </th>
                    <th className="h-10 px-4 text-left align-middle font-medium text-muted-foreground">
                      {t("modelMarketplace.provider")}
                    </th>
                    <th className="h-10 px-4 text-left align-middle font-medium text-muted-foreground">
                      {t("modelMarketplace.mode")}
                    </th>
                    <th className="h-10 px-4 text-left align-middle font-medium text-muted-foreground">
                      {t("modelMarketplace.inputCost")}
                    </th>
                    <th className="h-10 px-4 text-left align-middle font-medium text-muted-foreground">
                      {t("modelMarketplace.outputCost")}
                    </th>
                    <th className="h-10 px-4 text-left align-middle font-medium text-muted-foreground">
                      {t("modelMarketplace.context")}
                    </th>
                    <th className="h-10 px-4 text-left align-middle font-medium text-muted-foreground">
                      {t("modelMarketplace.maxOutput")}
                    </th>
                  </tr>
                )}
                itemContent={(_index, record) => {
                  const inputCost = nanoToPerMillion(record.input_cost_per_token_nano);
                  const outputCost = nanoToPerMillion(record.output_cost_per_token_nano);

                  return (
                    <>
                      <td className="p-4 align-middle">
                        <ModelBadge
                          model={record.model_id}
                          provider={record.models_dev_provider}
                          showDetails={false}
                        />
                      </td>
                      <td className="p-4 align-middle text-xs text-muted-foreground">
                        {record.models_dev_provider?.toLowerCase() ?? "-"}
                      </td>
                      <td className="p-4 align-middle">
                        <Badge variant="secondary">{record.mode ?? "-"}</Badge>
                      </td>
                      <td className="p-4 align-middle font-mono text-xs">
                        {inputCost === "-" ? "-" : `${inputCost} / 1M`}
                      </td>
                      <td className="p-4 align-middle font-mono text-xs">
                        {outputCost === "-" ? "-" : `${outputCost} / 1M`}
                      </td>
                      <td className="p-4 align-middle font-mono text-xs">
                        {formatTokens(record.max_tokens)}
                      </td>
                      <td className="p-4 align-middle font-mono text-xs">
                        {formatTokens(record.max_output_tokens)}
                      </td>
                    </>
                  );
                }}
              />
            )}
          </CardContent>
        </Card>
      </motion.div>
    </PageWrapper>
  );
}
