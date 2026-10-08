import { useContext, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { CircleDollarSign, Lock, Pencil, Plus, SearchX, Trash2 } from "lucide-react";
import { Virtuoso } from "react-virtuoso";
import { toast } from "sonner";
import { ModelBadge } from "@/components/ModelBadge";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
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
import { StatusBadge } from "@/components/ui/status";
import type { ModelPriceRecord } from "@/lib/api";
import { DashboardScrollParentContext } from "@/lib/dashboard-scroll";
import { formatDateTime } from "@/lib/format-time";
import { deleteModelPriceOptimistic, useModelPrices } from "@/lib/swr";
import { PricingEditorDialog } from "./PricingEditorDialog";
import { formatUsdPerM, type PricingEditorTarget } from "./shared";

function modeLabel(record: ModelPriceRecord): string {
  if (record.billing_mode === "per_request") return "per_request";
  if (record.billing_mode === "tiered_expr") return "tiered";
  return "per_token";
}

/** MP-UI3 model price list. */
export function ModelPricingTab() {
  const { t } = useTranslation();
  const { data, error, isLoading, isValidating, mutate } = useModelPrices();
  const records = useMemo(() => data ?? [], [data]);
  const scrollParent = useContext(DashboardScrollParentContext);
  const [search, setSearch] = useState("");
  const [editorTarget, setEditorTarget] = useState<PricingEditorTarget | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<ModelPriceRecord | null>(null);

  const needle = search.trim().toLowerCase();
  const filtered = useMemo(
    () => records.filter((record) => record.model_id.toLowerCase().includes(needle)),
    [records, needle],
  );

  const openCreate = () => setEditorTarget({ mode: "create", modelId: "", record: null });

  const confirmDelete = async () => {
    if (!deleteTarget) return;
    await deleteModelPriceOptimistic(deleteTarget.model_id, records, (deleteError) =>
      toast.error(t("modelPricing.deleteFailed", "Failed to delete model price"), {
        description: deleteError.message,
      }),
    ).catch(() => undefined);
  };

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
        records={records}
      />
      <ConfirmDeleteDialog
        open={deleteTarget !== null}
        onOpenChange={(open) => {
          if (!open) setDeleteTarget(null);
        }}
        title={t("modelPricing.deleteTitle", "Delete model price")}
        description={t("modelPricing.deleteConfirm", { model: deleteTarget?.model_id })}
        onConfirm={confirmDelete}
      />

      <div className="space-y-6">
        {error ? <QueryError onRetry={mutate} retrying={isValidating} stale={data !== undefined} /> : null}

        {data !== undefined ? (
          <DataTableShell
            toolbar={
              records.length > 0 ? (
                <>
                  <TableToolbarSearch
                    type="search"
                    value={search}
                    onChange={(event) => setSearch(event.target.value)}
                    placeholder={t("modelPricing.searchPlaceholder", "Search models...")}
                    aria-label={t("modelPricing.searchPlaceholder", "Search models...")}
                    autoComplete="off"
                  />
                  <Button onClick={openCreate}>
                    <Plus data-icon="inline-start" aria-hidden="true" />
                    {t("modelPricing.addPrice", "Add Price")}
                  </Button>
                </>
              ) : undefined
            }
            isEmpty={records.length === 0}
            emptyState={
              <EmptyState
                icon={<CircleDollarSign className="size-10" aria-hidden="true" />}
                title={t("modelPricing.noPrices", "No model prices yet")}
                description={t(
                  "modelPricing.noPricesDesc",
                  "Sync from an upstream source or add prices manually.",
                )}
                action={
                  <Button onClick={openCreate}>
                    <Plus data-icon="inline-start" aria-hidden="true" />
                    {t("modelPricing.addPrice", "Add Price")}
                  </Button>
                }
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
              <DataList columns="minmax(0,1.6fr) 7rem 6.5rem 6.5rem 7rem 7.5rem 10rem 5rem">
                <DataListHeader>
                  <DataListHead>{t("modelPricing.model", "Model")}</DataListHead>
                  <DataListHead>{t("modelPricing.mode", "Mode")}</DataListHead>
                  <DataListHead align="end">{t("modelPricing.inputPrice", "Input $/1M")}</DataListHead>
                  <DataListHead align="end">{t("modelPricing.outputPrice", "Output $/1M")}</DataListHead>
                  <DataListHead>{t("modelPricing.source", "Source")}</DataListHead>
                  <DataListHead>{t("modelPricing.status", "Status")}</DataListHead>
                  <DataListHead>{t("modelPricing.updated", "Updated")}</DataListHead>
                  <DataListHead align="end">{t("common.actions", "Actions")}</DataListHead>
                </DataListHeader>
                {scrollParent && (
                  <Virtuoso
                    customScrollParent={scrollParent}
                    data={filtered}
                    context={{ label: t("modelPricing.tabs.modelPricing", "Model Pricing") }}
                    computeItemKey={(_index, record) => record.model_id}
                    components={virtualDataListComponents}
                    itemContent={(_index, record) => (
                      <>
                        <DataListCell primary>
                          <ModelBadge model={record.model_id} showDetails={false} />
                        </DataListCell>
                        <DataListCell label={t("modelPricing.mode", "Mode")}>
                          <span className="inline-flex items-center gap-1.5">
                            <span className="font-mono">{modeLabel(record)}</span>
                            {record.billing_expr?.service_tiers?.fast ? (
                              <Badge variant="outline">
                                {t("modelPricing.serviceTierFastBadge", "fast")}
                              </Badge>
                            ) : null}
                          </span>
                        </DataListCell>
                        <DataListCell label={t("modelPricing.inputPrice", "Input $/1M")} align="end">
                          <span className="tabular-nums">
                            {record.billing_mode === "per_request"
                              ? formatUsdPerM(record.per_request_usd)
                              : formatUsdPerM(record.input_usd_per_1m)}
                          </span>
                        </DataListCell>
                        <DataListCell label={t("modelPricing.outputPrice", "Output $/1M")} align="end">
                          <span className="tabular-nums">
                            {record.billing_mode === "per_request"
                              ? "—"
                              : formatUsdPerM(record.output_usd_per_1m)}
                          </span>
                        </DataListCell>
                        <DataListCell label={t("modelPricing.source", "Source")}>
                          <span className="font-mono text-muted-foreground">{record.source}</span>
                        </DataListCell>
                        <DataListCell label={t("modelPricing.status", "Status")}>
                          <span className="inline-flex items-center gap-1.5">
                            {record.enabled ? (
                              <StatusBadge variant="success">{t("common.enabled")}</StatusBadge>
                            ) : (
                              <Badge variant="secondary">{t("common.disabled")}</Badge>
                            )}
                            {record.locked_fields.length > 0 ? (
                              <Badge
                                variant="outline"
                                className="gap-1"
                                title={record.locked_fields.join(", ")}
                              >
                                <Lock className="size-3" aria-hidden="true" />
                                {record.locked_fields.length}
                              </Badge>
                            ) : null}
                          </span>
                        </DataListCell>
                        <DataListCell label={t("modelPricing.updated", "Updated")}>
                          <span className="tabular-nums text-muted-foreground">
                            {formatDateTime(record.updated_at)}
                          </span>
                        </DataListCell>
                        <DataListActions>
                          <Button
                            variant="ghost"
                            size="icon"
                            className="size-11 touch-manipulation sm:size-9"
                            aria-label={t("common.editItem", { name: record.model_id })}
                            onClick={() =>
                              setEditorTarget({ mode: "edit", modelId: record.model_id, record })
                            }
                          >
                            <Pencil />
                          </Button>
                          <Button
                            variant="ghost"
                            size="icon"
                            className="size-11 touch-manipulation sm:size-9"
                            aria-label={t("common.deleteItem", { name: record.model_id })}
                            onClick={() => setDeleteTarget(record)}
                          >
                            <Trash2 className="text-error-foreground" />
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
