import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Store } from "lucide-react";
import { ModelBadge } from "@/components/ModelBadge";
import { useMarketplaceModels } from "@/lib/swr";
import { PageWrapper, motion, transitions } from "@/components/ui/motion";
import { EmptyState } from "@/components/ui/empty-state";
import { PageHeader } from "@/components/ui/page-header";
import { TablePageSkeleton } from "@/components/ui/page-skeleton";
import { DataTableShell, TableToolbarSearch, VirtualTableCell, VirtualTableHeaderCell } from "@/components/ui/data-table-shell";
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
  const { data: records = [], isLoading } = useMarketplaceModels();
  const [search, setSearch] = useState("");

  const filtered = records.filter((r) =>
    r.model_id.toLowerCase().includes(search.toLowerCase())
  );

  if (isLoading) {
    return (
      <PageWrapper className="space-y-6">
        <TablePageSkeleton showToolbar />
      </PageWrapper>
    );
  }

  return (
    <PageWrapper className="space-y-6">
      <motion.div
        initial={{ opacity: 0, y: -10 }}
        animate={{ opacity: 1, y: 0 }}
        transition={transitions.normal}
      >
        <PageHeader title={t("modelMarketplace.title")} description={t("modelMarketplace.description")} />
      </motion.div>

      <motion.div
        initial={{ opacity: 0, y: 20 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ delay: 0.1, ...transitions.normal }}
      >
        <DataTableShell
          toolbar={(
            <>
              <div className="flex items-center gap-2 text-base font-semibold">
                <Store className="h-5 w-5" />
                {t("modelMarketplace.title")}
              </div>
              <TableToolbarSearch
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                placeholder={t("modelMarketplace.searchPlaceholder")}
              />
            </>
          )}
          isEmpty={filtered.length === 0}
          emptyState={(
            <EmptyState
              icon={<Store className="h-12 w-12" />}
              title={t("modelMarketplace.noModels")}
              description={t("modelMarketplace.noModelsDesc")}
            />
          )}
        >
              <TableVirtuoso
                style={{ height: "calc(100dvh - 280px)", minHeight: 400 }}
                data={filtered}
                components={{
                  Table: (props) => <table {...props} className="w-full caption-bottom text-sm" />,
                  TableHead: (props) => <thead {...props} className="[&_tr]:border-b" />,
                  TableRow: (props) => <tr {...props} className="border-b transition-colors hover:bg-muted/50" />,
                  TableBody: (props) => <tbody {...props} className="[&_tr:last-child]:border-0" />,
                }}
                fixedHeaderContent={() => (
                  <tr className="border-b bg-background">
                    <VirtualTableHeaderCell className="min-w-[200px]">
                      {t("modelMarketplace.modelId")}
                    </VirtualTableHeaderCell>
                    <VirtualTableHeaderCell>
                      {t("modelMarketplace.inputCost")}
                    </VirtualTableHeaderCell>
                    <VirtualTableHeaderCell>
                      {t("modelMarketplace.outputCost")}
                    </VirtualTableHeaderCell>
                    <VirtualTableHeaderCell>
                      {t("modelMarketplace.context")}
                    </VirtualTableHeaderCell>
                    <VirtualTableHeaderCell>
                      {t("modelMarketplace.maxOutput")}
                    </VirtualTableHeaderCell>
                  </tr>
                )}
                itemContent={(_index, record) => {
                  const inputCost = nanoToPerMillion(record.input_cost_per_token_nano);
                  const outputCost = nanoToPerMillion(record.output_cost_per_token_nano);

                  return (
                    <>
                      <VirtualTableCell>
                        <ModelBadge
                          model={record.model_id}
                          provider={record.models_dev_provider}
                          showDetails={false}
                        />
                      </VirtualTableCell>
                      <VirtualTableCell className="font-mono text-xs">
                        {inputCost === "-" ? "-" : `${inputCost} / 1M`}
                      </VirtualTableCell>
                      <VirtualTableCell className="font-mono text-xs">
                        {outputCost === "-" ? "-" : `${outputCost} / 1M`}
                      </VirtualTableCell>
                      <VirtualTableCell className="font-mono text-xs">
                        {formatTokens(record.max_tokens)}
                      </VirtualTableCell>
                      <VirtualTableCell className="font-mono text-xs">
                        {formatTokens(record.max_output_tokens)}
                      </VirtualTableCell>
                    </>
                  );
                }}
              />
        </DataTableShell>
      </motion.div>
    </PageWrapper>
  );
}
