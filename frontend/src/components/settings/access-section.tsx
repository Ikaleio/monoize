import { TriangleAlert } from "lucide-react";
import { useTranslation } from "react-i18next";

import { StatusAlert } from "@/components/ui/status";
import type { SystemSettings } from "@/lib/api";
import { SettingsGroup } from "./settings-layout";
import { NumberRow, SwitchRow } from "./settings-rows";

interface AccessSectionProps {
  settings: SystemSettings;
  onChange: (updates: Partial<SystemSettings>) => void;
}

/** Registration policy plus session and API-key limits. */
export function AccessSection({ settings, onChange }: AccessSectionProps) {
  const { t } = useTranslation();

  return (
    <>
      {settings.registration_enabled && !settings.captcha_enabled ? (
        <StatusAlert
          variant="warning"
          icon={<TriangleAlert className="size-4" aria-hidden="true" />}
          title={t("settings.openRegistrationWarningTitle")}
          description={t("settings.openRegistrationWarningDescription")}
        />
      ) : null}

      <SettingsGroup id="access.registration">
        <SwitchRow
          id="registration_enabled"
          checked={settings.registration_enabled}
          onCheckedChange={(registration_enabled) => onChange({ registration_enabled })}
        />
      </SettingsGroup>

      <SettingsGroup id="access.session">
        <SwitchRow
          id="captcha_enabled"
          checked={settings.captcha_enabled}
          onCheckedChange={(captcha_enabled) => onChange({ captcha_enabled })}
        />
        <NumberRow
          id="session_ttl_days"
          value={settings.session_ttl_days}
          onValueChange={(session_ttl_days) => onChange({ session_ttl_days })}
        />
        <NumberRow
          id="api_key_max_per_user"
          value={settings.api_key_max_per_user}
          onValueChange={(api_key_max_per_user) => onChange({ api_key_max_per_user })}
        />
      </SettingsGroup>
    </>
  );
}
