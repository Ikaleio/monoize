import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { RefreshCw, SearchX } from "lucide-react";

import { GroupMultiSelect } from "@/components/groups/GroupPicker";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { EmptyState } from "@/components/ui/empty-state";
import { FieldDescription, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Separator } from "@/components/ui/separator";
import { Skeleton } from "@/components/ui/skeleton";
import { useDashboardGroups } from "@/lib/swr";
import type { SystemSettings } from "@/lib/api";

interface DashboardPerformanceTargetsProps {
  settings: SystemSettings;
  availableModelIds: string[];
  modelsLoading: boolean;
  modelsError?: unknown;
  onRetryModels: () => void;
  onChange: (updates: Partial<SystemSettings>) => void;
}

function uniqueIds(ids: string[]) {
  return Array.from(new Set(ids.map((id) => id.trim()).filter(Boolean)));
}

/**
 * Admin controls for which groups and models appear on the user dashboard
 * performance panel (dashboard-home-overview.spec.md DH-9a).
 */
export function DashboardPerformanceTargets({
  settings,
  availableModelIds,
  modelsLoading,
  modelsError,
  onRetryModels,
  onChange,
}: DashboardPerformanceTargetsProps) {
  const { t } = useTranslation();
  const { data: groups, isLoading: groupsLoading } = useDashboardGroups();
  const [query, setQuery] = useState("");

  const selected = useMemo(
    () => uniqueIds(settings.dashboard_performance_model_ids ?? []),
    [settings.dashboard_performance_model_ids]
  );
  const available = useMemo(
    () => uniqueIds(availableModelIds).sort(),
    [availableModelIds]
  );
  const selectedSet = useMemo(() => new Set(selected), [selected]);
  const modelIds = useMemo(
    () => [...selected, ...available.filter((id) => !selectedSet.has(id))],
    [available, selected, selectedSet]
  );
  const normalizedQuery = query.trim().toLocaleLowerCase();
  const filtered = modelIds.filter((id) =>
    id.toLocaleLowerCase().includes(normalizedQuery)
  );

  const toggleModel = (modelId: string, checked: boolean) => {
    onChange({
      dashboard_performance_model_ids: checked
        ? [...selected, modelId]
        : selected.filter((id) => id !== modelId),
    });
  };

  return (
    <div className="flex flex-col gap-4">
      <div>
        <FieldLabel>{t("settings.dashboardPerformanceTitle")}</FieldLabel>
        <FieldDescription>
          {t("settings.dashboardPerformanceDescription")}
        </FieldDescription>
      </div>

      <div className="space-y-2">
        <FieldLabel>{t("settings.dashboardPerformanceGroups")}</FieldLabel>
        <GroupMultiSelect
          value={settings.dashboard_performance_group_ids ?? []}
          groups={groups ?? []}
          loading={groupsLoading}
          onChange={(dashboard_performance_group_ids) =>
            onChange({ dashboard_performance_group_ids })
          }
        />
      </div>

      <Separator />

      <div className="space-y-3">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <FieldLabel>{t("settings.dashboardPerformanceModels")}</FieldLabel>
          <Badge variant="secondary">
            {t("settings.dashboardPerformanceModelsSelected", {
              count: selected.length,
              defaultValue: "{{count}} selected",
            })}
          </Badge>
        </div>

        <Input
          type="search"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          placeholder={t("settings.dashboardPerformanceModelsSearch", "Search models")}
          autoComplete="off"
        />

        {modelsError ? (
          <Alert variant="destructive">
            <AlertTitle>{t("settings.dashboardPerformanceModelsLoadFailed", "Failed to load models")}</AlertTitle>
            <AlertDescription className="flex items-center justify-between gap-2">
              <span className="text-xs">
                {modelsError instanceof Error ? modelsError.message : String(modelsError)}
              </span>
              <Button type="button" size="sm" variant="outline" onClick={onRetryModels}>
                <RefreshCw className="mr-1 h-3.5 w-3.5" />
                {t("common.retry", "Retry")}
              </Button>
            </AlertDescription>
          </Alert>
        ) : modelsLoading ? (
          <div className="space-y-2">
            <Skeleton className="h-8 w-full" />
            <Skeleton className="h-8 w-full" />
            <Skeleton className="h-8 w-3/4" />
          </div>
        ) : filtered.length === 0 ? (
          <EmptyState
            icon={<SearchX className="h-6 w-6 text-muted-foreground" />}
            title={t("settings.dashboardPerformanceModelsEmpty", "No models")}
            description={t(
              "settings.dashboardPerformanceModelsEmptyDescription",
              "Enable a Channel with models, or clear the search."
            )}
            className="py-6"
          />
        ) : (
          <div className="max-h-64 space-y-1 overflow-auto rounded-md border p-2">
            {filtered.map((modelId) => {
              const checked = selectedSet.has(modelId);
              const unavailable = !available.includes(modelId);
              return (
                <label
                  key={modelId}
                  className="flex cursor-pointer items-center gap-2 rounded-md px-2 py-1.5 hover:bg-muted/50"
                >
                  <Checkbox
                    checked={checked}
                    onCheckedChange={(value) => toggleModel(modelId, value === true)}
                  />
                  <span className="min-w-0 flex-1 truncate font-mono text-xs">{modelId}</span>
                  {unavailable ? (
                    <Badge variant="outline" className="shrink-0 text-[10px]">
                      {t("common.unavailable", "Unavailable")}
                    </Badge>
                  ) : null}
                </label>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}
