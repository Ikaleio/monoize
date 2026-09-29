import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import type { SystemSettings } from "@/lib/api";
import { SettingRow, SettingsGroup } from "./settings-layout";

interface AccessSectionProps {
  settings: SystemSettings;
  onChange: (updates: Partial<SystemSettings>) => void;
}

/** Registration policy plus session and API-key limits. */
export function AccessSection({ settings, onChange }: AccessSectionProps) {
  return (
    <>
      <SettingsGroup id="access.registration">
        <SettingRow id="registration_enabled" switchControl>
          <Switch
            id="registration_enabled"
            checked={settings.registration_enabled}
            onCheckedChange={(checked) => onChange({ registration_enabled: checked })}
          />
        </SettingRow>
        <SettingRow id="default_user_role">
          <Input
            id="default_user_role"
            value={settings.default_user_role}
            onChange={(e) => onChange({ default_user_role: e.target.value })}
          />
        </SettingRow>
      </SettingsGroup>

      <SettingsGroup id="access.session">
        <SettingRow id="captcha_enabled" switchControl>
          <Switch
            id="captcha_enabled"
            checked={settings.captcha_enabled}
            onCheckedChange={(checked) => onChange({ captcha_enabled: checked })}
          />
        </SettingRow>
        <SettingRow id="session_ttl_days">
          <Input
            id="session_ttl_days"
            type="number"
            min="1"
            value={settings.session_ttl_days}
            onChange={(e) => onChange({ session_ttl_days: parseInt(e.target.value) || 7 })}
          />
        </SettingRow>
        <SettingRow id="api_key_max_per_user">
          <Input
            id="api_key_max_per_user"
            type="number"
            min="1"
            value={settings.api_key_max_per_user}
            onChange={(e) =>
              onChange({ api_key_max_per_user: parseInt(e.target.value) || 10 })
            }
          />
        </SettingRow>
      </SettingsGroup>
    </>
  );
}
