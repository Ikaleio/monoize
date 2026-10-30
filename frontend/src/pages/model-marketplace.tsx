import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { SearchX, Store } from "lucide-react";
import { ModelIcon } from "@/components/ModelIcon";
import { Button } from "@/components/ui/button";
import { CopyButton } from "@/components/ui/copy-button";
import {
  DataList,
  DataListBody,
  DataListCell,
  DataListHead,
  DataListHeader,
  DataListRow,
} from "@/components/ui/data-list";
import { DataTableShell, TableToolbarSearch } from "@/components/ui/data-table-shell";
import { EmptyState } from "@/components/ui/empty-state";
import { PageWrapper } from "@/components/ui/motion";
import { PageHeader } from "@/components/ui/page-header";
import { TablePageSkeleton } from "@/components/ui/page-skeleton";
import { QueryError } from "@/components/ui/query-error";
import { useCurrentUser, useDashboardGroups, useMarketplaceModels } from "@/lib/swr";
import { PriceCell } from "./model-pricing/PriceCell";

function formatTokens(tokens?: number | null): string {
  if (tokens == null) return "—";
  if (tokens >= 1_000_000) return `${Number((tokens / 1_000_000).toFixed(1))}M`;
  if (tokens >= 1_000) return `${Math.round(tokens / 1_000)}K`;
  return tokens.toString();
}

/** Read-only model catalog (model-marketplace.spec.md §4). */
export function ModelMarketplacePage() {
  const { t } = useTranslation();
  const { data, error, isLoading, isValidating, mutate } = useMarketplaceModels();
  const records = useMemo(() => data ?? [], [data]);
  const { data: user } = useCurrentUser();
  const { data: groups = [] } = useDashboardGroups();
  const [search, setSearch] = useState("");
  const isAdmin = user?.role === "admin" || user?.role === "super_admin";
  const ownGroup =
    groups.find((group) => group.id === user?.group_id) ?? groups.find((group) => group.is_default);

  const filtered = useMemo(() => {
    const query = search.trim().toLocaleLowerCase();
    if (!query) return records;
    return records.filter((record) => record.model_id.toLocaleLowerCase().includes(query));
  }, [records, search]);

  if (isLoading && !error && data === undefined) {
    return (
      <PageWrapper>
        <TablePageSkeleton rows={6} columns={6} />
      </PageWrapper>
    );
  }

  return (
    <PageWrapper className="space-y-6">
      <PageHeader title={t("modelMarketplace.title")} description={t("modelMarketplace.description")} />

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
                  placeholder={t("modelMarketplace.searchPlaceholder")}
                  aria-label={t("modelMarketplace.searchPlaceholder")}
                  autoComplete="off"
                  containerClassName="sm:w-72"
                />
                <p className="text-sm tabular-nums text-muted-foreground">
                  {t("modelMarketplace.resultCount", { filtered: filtered.length, total: records.length })}
                </p>
                {!isAdmin && records.some((record) => record.price_ratio !== "1") ? (
                  <p className="basis-full text-sm text-muted-foreground">
                    {t("modelMarketplace.ratioNote")}
                  </p>
                ) : null}
              </>
            ) : undefined
          }
          isEmpty={records.length === 0}
          emptyState={
            <EmptyState
              icon={<Store className="size-10" aria-hidden="true" />}
              title={t("modelMarketplace.noModels")}
              description={t("modelMarketplace.noModelsDesc")}
            />
          }
        >
          {filtered.length === 0 ? (
            <EmptyState
              variant="inline"
              icon={<SearchX className="size-8" aria-hidden="true" />}
              title={t("modelMarketplace.noMatches")}
              description={t("modelMarketplace.noMatchesDesc")}
              action={
                <Button variant="outline" onClick={() => setSearch("")}>
                  {t("modelMarketplace.clearSearch")}
                </Button>
              }
            />
          ) : (
            <DataList columns="minmax(0,1fr) 8rem 10rem 10rem 6rem 6rem">
              <DataListHeader>
                <DataListHead>{t("modelMarketplace.modelId")}</DataListHead>
                <DataListHead>{t("modelMarketplace.mode")}</DataListHead>
                <DataListHead align="end">{t("modelMarketplace.inputPerMillion")}</DataListHead>
                <DataListHead align="end">{t("modelMarketplace.outputPerMillion")}</DataListHead>
                <DataListHead align="end">{t("modelMarketplace.context")}</DataListHead>
                <DataListHead align="end">{t("modelMarketplace.maxOutput")}</DataListHead>
              </DataListHeader>
              <DataListBody aria-label={t("modelMarketplace.title")}>
                {filtered.map((record) => (
                  <DataListRow key={record.model_id}>
                    <DataListCell primary>
                      <div className="flex min-w-0 items-center gap-2">
                        <span aria-hidden="true" className="inline-flex shrink-0">
                          <ModelIcon model={record.model_id} provider={record.models_dev_provider} className="size-4" />
                        </span>
                        <span className="truncate font-mono font-medium" title={record.model_id}>
                          {record.model_id}
                        </span>
                        <CopyButton
                          value={record.model_id}
                          label={t("modelMarketplace.copyModelId", { model: record.model_id })}
                        />
                      </div>
                      <p className="truncate pl-6 text-muted-foreground">
                        {[
                          record.models_dev_provider,
                          record.groups.length > 0
                            ? t("modelMarketplace.groupsLine", { groups: record.groups.join(", ") })
                            : null,
                        ]
                          .filter(Boolean)
                          .join(" · ") || "—"}
                      </p>
                      {!isAdmin && ownGroup && record.groups.length > 0 && !record.groups.includes(ownGroup.name) ? (
                        <p className="truncate pl-6 text-muted-foreground">
                          {t("modelMarketplace.requiresKeyGroup", { group: record.groups[0] })}
                        </p>
                      ) : null}
                      {!isAdmin && record.price_ratio !== "1" ? (
                        <p className="truncate pl-6 text-muted-foreground">
                          {t("modelMarketplace.rowRatio", {
                            group:
                              ownGroup && record.groups.includes(ownGroup.name)
                                ? ownGroup.name
                                : record.groups[0],
                            ratio: record.price_ratio,
                          })}
                        </p>
                      ) : null}
                    </DataListCell>
                    <DataListCell label={t("modelMarketplace.mode")}>{record.mode || "—"}</DataListCell>
                    <DataListCell label={t("modelMarketplace.inputPerMillion")} align="end">
                      <PriceCell record={record} side="input" />
                    </DataListCell>
                    <DataListCell label={t("modelMarketplace.outputPerMillion")} align="end">
                      <PriceCell record={record} side="output" />
                    </DataListCell>
                    <DataListCell label={t("modelMarketplace.context")} align="end">
                      <span className="tabular-nums">{formatTokens(record.max_tokens)}</span>
                    </DataListCell>
                    <DataListCell label={t("modelMarketplace.maxOutput")} align="end">
                      <span className="tabular-nums">{formatTokens(record.max_output_tokens)}</span>
                    </DataListCell>
                  </DataListRow>
                ))}
              </DataListBody>
            </DataList>
          )}
        </DataTableShell>
      ) : null}
    </PageWrapper>
  );
}
