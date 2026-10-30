import { useTranslation } from "react-i18next";

import { NumberField } from "@/components/ui/number-field";
import { Switch } from "@/components/ui/switch";
import { getSettingEntry, type SettingEntryId } from "./settings-categories";
import { useSettingsRowState } from "./settings-context";
import { SettingRow } from "./settings-layout";

interface SwitchRowProps {
  id: SettingEntryId;
  checked: boolean;
  onCheckedChange: (checked: boolean) => void;
}

export function SwitchRow({ id, checked, onCheckedChange }: SwitchRowProps) {
  return (
    <SettingRow id={id} switchControl>
      <Switch id={id} checked={checked} onCheckedChange={onCheckedChange} />
    </SettingRow>
  );
}

interface NumberRowProps {
  id: SettingEntryId;
  value: number;
  onValueChange: (value: number) => void;
}

/** Numeric row; minimum and unit come from the entry model (SSU-2b). */
export function NumberRow({ id, value, onValueChange }: NumberRowProps) {
  const { t } = useTranslation();
  const { reportValidity } = useSettingsRowState();
  const entry = getSettingEntry(id);

  return (
    <SettingRow id={id}>
      <NumberField
        id={id}
        value={value}
        min={entry.min}
        unit={entry.unitKey ? t(entry.unitKey) : undefined}
        onValueChange={(next) => {
          if (next !== null) onValueChange(next);
        }}
        onValidityChange={(valid) => reportValidity(id, valid)}
      />
    </SettingRow>
  );
}
