import { useTranslation } from "react-i18next";

import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import type { SystemSettings } from "@/lib/api";
import { SettingRow, SettingsGroup } from "./settings-layout";

interface AffinitySectionProps {
  settings: SystemSettings;
  onChange: (updates: Partial<SystemSettings>) => void;
}

/** Routing affinity: master switch, recovery policy, and timers. */
export function AffinitySection({ settings, onChange }: AffinitySectionProps) {
  const { t } = useTranslation();

  return (
    <SettingsGroup id="affinity">
      <SettingRow id="monoize_affinity_enabled" switchControl>
        <Switch
          id="monoize_affinity_enabled"
          checked={settings.monoize_affinity_enabled}
          onCheckedChange={(checked) => onChange({ monoize_affinity_enabled: checked })}
        />
      </SettingRow>
      <SettingRow id="monoize_affinity_failback_mode">
        <Select
          value={settings.monoize_affinity_failback_mode}
          onValueChange={(value: SystemSettings["monoize_affinity_failback_mode"]) =>
            onChange({ monoize_affinity_failback_mode: value })
          }
        >
          <SelectTrigger id="monoize_affinity_failback_mode">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectGroup>
              <SelectItem value="sticky">{t("settings.affinitySticky")}</SelectItem>
              <SelectItem value="prefer_higher_priority">
                {t("settings.affinityPreferHigherPriority")}
              </SelectItem>
            </SelectGroup>
          </SelectContent>
        </Select>
      </SettingRow>
      <SettingRow id="monoize_affinity_idle_ttl_seconds">
        <Input
          id="monoize_affinity_idle_ttl_seconds"
          type="number"
          min="1"
          value={settings.monoize_affinity_idle_ttl_seconds}
          onChange={(event) =>
            onChange({
              monoize_affinity_idle_ttl_seconds: Math.max(1, parseInt(event.target.value) || 1),
            })
          }
        />
      </SettingRow>
      <SettingRow id="monoize_affinity_failback_delay_seconds">
        <Input
          id="monoize_affinity_failback_delay_seconds"
          type="number"
          min="0"
          value={settings.monoize_affinity_failback_delay_seconds}
          onChange={(event) =>
            onChange({
              monoize_affinity_failback_delay_seconds: Math.max(
                0,
                parseInt(event.target.value) || 0
              ),
            })
          }
        />
      </SettingRow>
    </SettingsGroup>
  );
}
