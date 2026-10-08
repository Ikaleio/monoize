import { useTranslation } from "react-i18next";

import { GroupMultiSelect } from "@/components/groups/GroupPicker";
import { ModelMultiSelect } from "@/components/models/model-multi-select";
import { FieldDescription, FieldLabel } from "@/components/ui/field";
import { Separator } from "@/components/ui/separator";
import { useDashboardGroups } from "@/lib/swr";
import type { SystemSettings } from "@/lib/api";

interface DashboardPerformanceTargetsProps {
  settings: SystemSettings;
  availableModelIds: string[];
  modelsLoading: boolean;
  modelsError?: unknown;
  onRetryModels: () => unknown;
  onChange: (updates: Partial<SystemSettings>) => void;
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

  return (
    <div className="flex flex-col gap-4">
      <FieldDescription>{t("settings.dashboardPerformanceDescription")}</FieldDescription>

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

      <div className="space-y-2">
        <FieldLabel>{t("settings.dashboardPerformanceModels")}</FieldLabel>
        <ModelMultiSelect
          value={settings.dashboard_performance_model_ids ?? []}
          options={availableModelIds}
          loading={modelsLoading}
          error={modelsError}
          onRetry={onRetryModels}
          onChange={(dashboard_performance_model_ids) =>
            onChange({ dashboard_performance_model_ids })
          }
          label={t("settings.dashboardPerformanceModels")}
        />
      </div>
    </div>
  );
}
