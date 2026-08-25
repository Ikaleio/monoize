import { useTranslation } from "react-i18next";

import { Field, FieldDescription, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import type { SystemSettings } from "@/lib/api";

const PROVIDER_TYPES = ["chat_completion", "responses", "messages", "gemini"] as const;

interface ExtraFieldsSectionProps {
  settings: SystemSettings;
  onChange: (updates: Partial<SystemSettings>) => void;
}

/**
 * Per-provider-type extra-field whitelist editor. Typing keeps raw
 * comma-separated entries in the draft; blur normalizes the entry (drops the
 * key entirely when the list is empty), matching the pre-redesign contract.
 */
export function ExtraFieldsSection({ settings, onChange }: ExtraFieldsSectionProps) {
  const { t } = useTranslation();

  return (
    <div className="flex flex-col gap-4">
      <div className="grid gap-6 sm:grid-cols-2">
        {PROVIDER_TYPES.map((providerType) => (
          <Field key={providerType}>
            <FieldLabel htmlFor={`extra-fields-${providerType}`} className="font-mono text-xs">
              {providerType}
            </FieldLabel>
            <Input
              id={`extra-fields-${providerType}`}
              value={(settings.monoize_extra_fields_whitelist?.[providerType] ?? []).join(", ")}
              onChange={(e) => {
                const fields = e.target.value
                  .split(",")
                  .map((s) => s.trim())
                  .filter(Boolean);
                onChange({
                  monoize_extra_fields_whitelist: {
                    ...settings.monoize_extra_fields_whitelist,
                    [providerType]: fields.length > 0 ? fields : undefined!,
                  },
                });
              }}
              onBlur={(e) => {
                const raw = settings.monoize_extra_fields_whitelist ?? {};
                const fields = e.target.value
                  .split(",")
                  .map((s) => s.trim())
                  .filter(Boolean);
                const next = { ...raw };
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
          </Field>
        ))}
      </div>
      <FieldDescription>{t("settings.extraFieldsWhitelistHelp")}</FieldDescription>
    </div>
  );
}
