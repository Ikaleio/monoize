import { useContext, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { CircleDollarSign, SearchX } from "lucide-react";
import { Virtuoso } from "react-virtuoso";
import { ModelBadge } from "@/components/ModelBadge";
import { Button } from "@/components/ui/button";
import {
  DataList,
  DataListActions,
  DataListCell,
  DataListHead,
  DataListHeader,
} from "@/components/ui/data-list";
import { virtualDataListComponents } from "@/components/ui/data-list-virtual";
import { DataTableShell, TableToolbarSearch } from "@/components/ui/data-table-shell";
import { EmptyState } from "@/components/ui/empty-state";
import { TablePageSkeleton } from "@/components/ui/page-skeleton";
import { QueryError } from "@/components/ui/query-error";
import { DashboardScrollParentContext } from "@/lib/dashboard-scroll";
import { useModelPrices, useUnpricedModels } from "@/lib/swr";
import { PricingEditorDialog } from "./PricingEditorDialog";
import type { PricingEditorTarget } from "./shared";

/** MP-UI4 list of routable models without a price row. */
export function UnpricedModelsTab() {
  const { t } = useTranslation();
  const { data, error, isLoading, isValidating, mutate } = useUnpricedModels();
  const models = useMemo(() => data ?? [], [data]);
  const { data: priceRecords = [] } = useModelPrices();
  const scrollParent = useContext(DashboardScrollParentContext);
  const [search, setSearch] = useState("");
  const [editorTarget, setEditorTarget] = useState<PricingEditorTarget | null>(null);

  const needle = search.trim().toLowerCase();
  const filtered = useMemo(
    () => models.filter((model) => model.toLowerCase().includes(needle)),
    [models, needle],
  );

  if (isLoading) {
    return <TablePageSkeleton showToolbar />;
  }

  return (
    <>
      <PricingEditorDialog
        target={editorTarget}
        onOpenChange={(open) => {
          if (!open) setEditorTarget(null);
        }}
        records={priceRecords}
      />

      <div className="space-y-6">
        {error ? <QueryError onRetry={mutate} retrying={isValidating} stale={data !== undefined} /> : null}

        {data !== undefined ? (
          <DataTableShell
            toolbar={
              models.length > 0 ? (
                <>
                  <TableToolbarSearch
                    type="search"
                    value={search}
                    onChange={(event) => setSearch(event.target.value)}
                    placeholder={t("modelPricing.searchPlaceholder", "Search models...")}
                    aria-label={t("modelPricing.searchPlaceholder", "Search models...")}
                    autoComplete="off"
                  />
                  <p className="text-sm tabular-nums text-muted-foreground">
                    {t("modelPricing.unpricedCount", "{{count}} models", { count: filtered.length })}
                  </p>
                </>
              ) : undefined
            }
            isEmpty={models.length === 0}
            emptyState={
              <EmptyState
                icon={<CircleDollarSign className="size-10" aria-hidden="true" />}
                title={t("modelPricing.allPriced", "All routable models are priced")}
                description={t(
                  "modelPricing.allPricedDesc",
                  "Every model available for routing resolves to a price row.",
                )}
              />
            }
          >
            {filtered.length === 0 ? (
              <EmptyState
                variant="inline"
                icon={<SearchX className="size-8" aria-hidden="true" />}
                title={t("common.noMatchesTitle")}
                description={t("common.noMatchesDescription", { query: search.trim() })}
              />
            ) : (
              <DataList columns="minmax(0,1fr) 2.5rem">
                <DataListHeader>
                  <DataListHead>{t("modelPricing.model", "Model")}</DataListHead>
                  <DataListHead align="end">{t("common.actions", "Actions")}</DataListHead>
                </DataListHeader>
                {scrollParent && (
                  <Virtuoso
                    customScrollParent={scrollParent}
                    data={filtered}
                    context={{ label: t("modelPricing.tabs.unpricedModels", "Unpriced Models") }}
                    computeItemKey={(_index, model) => model}
                    components={virtualDataListComponents}
                    itemContent={(_index, model) => (
                      <>
                        <DataListCell primary>
                          <ModelBadge model={model} showDetails={false} />
                        </DataListCell>
                        <DataListActions>
                          <Button
                            variant="ghost"
                            size="icon"
                            className="size-11 touch-manipulation sm:size-9"
                            title={t("modelPricing.setPrice", "Set price")}
                            aria-label={`${t("modelPricing.setPrice", "Set price")}: ${model}`}
                            onClick={() => setEditorTarget({ mode: "create", modelId: model, record: null })}
                          >
                            <CircleDollarSign />
                          </Button>
                        </DataListActions>
                      </>
                    )}
                  />
                )}
              </DataList>
            )}
          </DataTableShell>
        ) : null}
      </div>
    </>
  );
}
