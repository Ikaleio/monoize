import { useTranslation } from "react-i18next";

import { Input } from "@/components/ui/input";
import type { SystemSettings } from "@/lib/api";
import { DashboardPerformanceTargets } from "./dashboard-performance-targets";
import { SettingBlock, SettingRow, SettingsGroup } from "./settings-layout";

interface SiteSectionProps {
  settings: SystemSettings;
  onChange: (updates: Partial<SystemSettings>) => void;
  availableModelIds: string[];
  modelsLoading: boolean;
  modelsError?: unknown;
  onRetryModels: () => void;
}

/** Site identity, public URLs, and the home dashboard performance targets. */
export function SiteSection({
  settings,
  onChange,
  availableModelIds,
  modelsLoading,
  modelsError,
  onRetryModels,
}: SiteSectionProps) {
  const { t } = useTranslation();

  return (
    <>
      <SettingsGroup id="site">
        <SettingRow id="site_name">
          <Input
            id="site_name"
            value={settings.site_name}
            onChange={(e) => onChange({ site_name: e.target.value })}
          />
        </SettingRow>
        <SettingRow id="site_description">
          <Input
            id="site_description"
            value={settings.site_description}
            onChange={(e) => onChange({ site_description: e.target.value })}
          />
        </SettingRow>
        <SettingRow id="api_base_url">
          <Input
            id="api_base_url"
            value={settings.api_base_url}
            onChange={(e) => onChange({ api_base_url: e.target.value })}
            placeholder={t("settings.apiBaseUrlPlaceholder")}
          />
        </SettingRow>
        <SettingRow id="recharge_public_origin">
          <Input
            id="recharge_public_origin"
            value={settings.recharge_public_origin}
            onChange={(e) => onChange({ recharge_public_origin: e.target.value })}
            placeholder={t("settings.rechargePublicOriginPlaceholder")}
          />
        </SettingRow>
      </SettingsGroup>

      <SettingsGroup id="site.dashboard">
        <SettingBlock id="dashboard_performance_targets">
          <DashboardPerformanceTargets
            settings={settings}
            availableModelIds={availableModelIds}
            modelsLoading={modelsLoading}
            modelsError={modelsError}
            onRetryModels={onRetryModels}
            onChange={onChange}
          />
        </SettingBlock>
      </SettingsGroup>
    </>
  );
}
