import { useTranslation } from "react-i18next";

import {
  Field,
  FieldContent,
  FieldDescription,
  FieldLabel,
} from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import type { SystemSettings } from "@/lib/api";
import { SettingsGroup } from "./settings-category-panel";

interface AccessSectionProps {
  settings: SystemSettings;
  onChange: (updates: Partial<SystemSettings>) => void;
}

/**
 * Merged access-control drawer: registration policy plus session/API-key
 * limits, laid out as two side-by-side subgroups on `lg`.
 */
export function AccessSection({ settings, onChange }: AccessSectionProps) {
  const { t } = useTranslation();

  return (
    <div className="grid gap-8 lg:grid-cols-2 lg:gap-12">
      <SettingsGroup label={t("settings.registration")}>
        <Field orientation="horizontal">
          <FieldContent>
            <FieldLabel htmlFor="registration_enabled">
              {t("settings.allowRegistration")}
            </FieldLabel>
            <FieldDescription>
              {t("settings.allowRegistrationDescription")}
            </FieldDescription>
          </FieldContent>
          <Switch
            id="registration_enabled"
            checked={settings.registration_enabled}
            onCheckedChange={(checked) => onChange({ registration_enabled: checked })}
          />
        </Field>
        <Field>
          <FieldLabel htmlFor="default_role">{t("settings.defaultUserRole")}</FieldLabel>
          <Input
            id="default_role"
            value={settings.default_user_role}
            onChange={(e) => onChange({ default_user_role: e.target.value })}
          />
          <FieldDescription>{t("settings.defaultUserRoleDescription")}</FieldDescription>
        </Field>
      </SettingsGroup>

      <SettingsGroup label={t("settings.sessionSecurity")}>
        <Field orientation="horizontal">
          <FieldContent>
            <FieldLabel htmlFor="captcha_enabled">
              {t("settings.captchaEnabled")}
            </FieldLabel>
            <FieldDescription>
              {t("settings.captchaEnabledDescription")}
            </FieldDescription>
          </FieldContent>
          <Switch
            id="captcha_enabled"
            checked={settings.captcha_enabled}
            onCheckedChange={(checked) => onChange({ captcha_enabled: checked })}
          />
        </Field>
        <div className="grid gap-6 sm:grid-cols-2">
          <Field>
            <FieldLabel htmlFor="session_ttl">{t("settings.sessionDuration")}</FieldLabel>
            <Input
              id="session_ttl"
              type="number"
              min="1"
              value={settings.session_ttl_days}
              onChange={(e) =>
                onChange({ session_ttl_days: parseInt(e.target.value) || 7 })
              }
            />
            <FieldDescription>{t("settings.sessionDurationDescription")}</FieldDescription>
          </Field>
          <Field>
            <FieldLabel htmlFor="max_api_keys">{t("settings.maxApiKeys")}</FieldLabel>
            <Input
              id="max_api_keys"
              type="number"
              min="1"
              value={settings.api_key_max_per_user}
              onChange={(e) =>
                onChange({ api_key_max_per_user: parseInt(e.target.value) || 10 })
              }
            />
            <FieldDescription>{t("settings.maxApiKeysDescription")}</FieldDescription>
          </Field>
        </div>
      </SettingsGroup>
    </div>
  );
}
