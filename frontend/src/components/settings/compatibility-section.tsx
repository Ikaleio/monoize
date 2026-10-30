import { useTranslation } from "react-i18next";

import { ListField } from "@/components/ui/list-field";
import type { SystemSettings } from "@/lib/api";
import { SettingRow, SettingsGroup } from "./settings-layout";
import { SwitchRow } from "./settings-rows";

const PROVIDER_TYPES = ["chat_completion", "responses", "messages", "gemini"] as const;

interface CompatibilitySectionProps {
  settings: SystemSettings;
  onChange: (updates: Partial<SystemSettings>) => void;
}

/** Per-type extra-field whitelist and cross-protocol passthrough stripping. */
export function CompatibilitySection({ settings, onChange }: CompatibilitySectionProps) {
  const { t } = useTranslation();

  const updateWhitelist = (providerType: string, fields: string[]) => {
    const next = { ...settings.monoize_extra_fields_whitelist };
    // SSU-2c: an empty list removes the sub-key, leaving only built-in defaults.
    if (fields.length > 0) {
      next[providerType] = fields;
    } else {
      delete next[providerType];
    }
    onChange({ monoize_extra_fields_whitelist: next });
  };

  return (
    <>
      <SettingsGroup id="compatibility.extra" footer={t("settings.extraFieldsWhitelistHelp")}>
        {PROVIDER_TYPES.map((providerType) => (
          <SettingRow key={providerType} id={`extra_fields_${providerType}`}>
            <ListField
              id={`extra_fields_${providerType}`}
              value={settings.monoize_extra_fields_whitelist?.[providerType] ?? []}
              onValueChange={(fields) => updateWhitelist(providerType, fields)}
              placeholder={t("settings.extraFieldsWhitelistPlaceholder")}
              className="font-mono text-sm"
            />
          </SettingRow>
        ))}
      </SettingsGroup>

      <SettingsGroup id="compatibility.protocol">
        <SwitchRow
          id="monoize_strip_cross_protocol_nested_extra"
          checked={settings.monoize_strip_cross_protocol_nested_extra}
          onCheckedChange={(monoize_strip_cross_protocol_nested_extra) =>
            onChange({ monoize_strip_cross_protocol_nested_extra })
          }
        />
      </SettingsGroup>
    </>
  );
}
