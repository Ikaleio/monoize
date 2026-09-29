import { useTranslation } from "react-i18next";

import { Input } from "@/components/ui/input";
import type { SystemSettings } from "@/lib/api";
import { SettingRow, SettingsGroup } from "./settings-layout";

interface SiteSectionProps {
  settings: SystemSettings;
  onChange: (updates: Partial<SystemSettings>) => void;
}

/** Site identity fields: name, description, downstream API base URL, recharge origin. */
export function SiteSection({ settings, onChange }: SiteSectionProps) {
  const { t } = useTranslation();

  return (
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
  );
}
