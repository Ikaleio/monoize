import { useTranslation } from "react-i18next";

import { Input } from "@/components/ui/input";
import type { SystemSettings } from "@/lib/api";
import { SettingRow, SettingsGroup } from "./settings-layout";

const PROVIDER_TYPES = ["chat_completion", "responses", "messages", "gemini"] as const;

interface ExtraFieldsSectionProps {
  settings: SystemSettings;
  onChange: (updates: Partial<SystemSettings>) => void;
}

function parseFieldList(raw: string) {
  return raw
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);
}

/**
 * Per-provider-type extra-field whitelist editor. Typing keeps raw
 * comma-separated entries in the draft; blur normalizes the entry (drops the
 * key entirely when the list is empty), matching the pre-redesign contract.
 */
export function ExtraFieldsSection({ settings, onChange }: ExtraFieldsSectionProps) {
  const { t } = useTranslation();

  return (
    <SettingsGroup id="extra" footer={t("settings.extraFieldsWhitelistHelp")}>
      {PROVIDER_TYPES.map((providerType) => (
        <SettingRow key={providerType} id={`extra_fields_${providerType}`}>
          <Input
            id={`extra_fields_${providerType}`}
            value={(settings.monoize_extra_fields_whitelist?.[providerType] ?? []).join(", ")}
            onChange={(e) => {
              const fields = parseFieldList(e.target.value);
              onChange({
                monoize_extra_fields_whitelist: {
                  ...settings.monoize_extra_fields_whitelist,
                  [providerType]: fields.length > 0 ? fields : undefined!,
                },
              });
            }}
            onBlur={(e) => {
              const fields = parseFieldList(e.target.value);
              const next = { ...(settings.monoize_extra_fields_whitelist ?? {}) };
              if (fields.length > 0) {
                next[providerType] = fields;
              } else {
                delete next[providerType];
              }
              onChange({ monoize_extra_fields_whitelist: next });
            }}
            placeholder={t("settings.extraFieldsWhitelistPlaceholder")}
            className="font-mono text-sm"
          />
        </SettingRow>
      ))}
    </SettingsGroup>
  );
}
