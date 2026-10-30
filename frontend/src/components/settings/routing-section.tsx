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
import type { SystemSettings } from "@/lib/api";
import { SettingRow, SettingsGroup } from "./settings-layout";
import { NumberRow, SwitchRow } from "./settings-rows";

interface RoutingSectionProps {
  settings: SystemSettings;
  onChange: (updates: Partial<SystemSettings>) => void;
}

/** Routing reliability: affinity, active probe, passive breaker, and timeouts. */
export function RoutingSection({ settings, onChange }: RoutingSectionProps) {
  const { t } = useTranslation();

  return (
    <>
      <SettingsGroup id="routing.affinity">
        <SwitchRow
          id="monoize_affinity_enabled"
          checked={settings.monoize_affinity_enabled}
          onCheckedChange={(monoize_affinity_enabled) => onChange({ monoize_affinity_enabled })}
        />
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
        <NumberRow
          id="monoize_affinity_idle_ttl_seconds"
          value={settings.monoize_affinity_idle_ttl_seconds}
          onValueChange={(monoize_affinity_idle_ttl_seconds) =>
            onChange({ monoize_affinity_idle_ttl_seconds })
          }
        />
        <NumberRow
          id="monoize_affinity_failback_delay_seconds"
          value={settings.monoize_affinity_failback_delay_seconds}
          onValueChange={(monoize_affinity_failback_delay_seconds) =>
            onChange({ monoize_affinity_failback_delay_seconds })
          }
        />
      </SettingsGroup>

      <SettingsGroup id="routing.probe">
        <SwitchRow
          id="monoize_active_probe_enabled"
          checked={settings.monoize_active_probe_enabled}
          onCheckedChange={(monoize_active_probe_enabled) =>
            onChange({ monoize_active_probe_enabled })
          }
        />
        <NumberRow
          id="monoize_active_probe_interval_seconds"
          value={settings.monoize_active_probe_interval_seconds}
          onValueChange={(monoize_active_probe_interval_seconds) =>
            onChange({ monoize_active_probe_interval_seconds })
          }
        />
        <NumberRow
          id="monoize_active_probe_success_threshold"
          value={settings.monoize_active_probe_success_threshold}
          onValueChange={(monoize_active_probe_success_threshold) =>
            onChange({ monoize_active_probe_success_threshold })
          }
        />
        <SettingRow id="monoize_active_probe_model">
          <Input
            id="monoize_active_probe_model"
            value={settings.monoize_active_probe_model ?? ""}
            onChange={(e) => onChange({ monoize_active_probe_model: e.target.value || null })}
            placeholder={t("settings.activeProbeModelPlaceholder")}
          />
        </SettingRow>
      </SettingsGroup>

      <SettingsGroup id="routing.passive">
        <NumberRow
          id="monoize_passive_failure_threshold"
          value={settings.monoize_passive_failure_threshold}
          onValueChange={(monoize_passive_failure_threshold) =>
            onChange({ monoize_passive_failure_threshold })
          }
        />
        <NumberRow
          id="monoize_passive_cooldown_seconds"
          value={settings.monoize_passive_cooldown_seconds}
          onValueChange={(monoize_passive_cooldown_seconds) =>
            onChange({ monoize_passive_cooldown_seconds })
          }
        />
        <NumberRow
          id="monoize_passive_window_seconds"
          value={settings.monoize_passive_window_seconds}
          onValueChange={(monoize_passive_window_seconds) =>
            onChange({ monoize_passive_window_seconds })
          }
        />
        <NumberRow
          id="monoize_passive_rate_limit_cooldown_seconds"
          value={settings.monoize_passive_rate_limit_cooldown_seconds}
          onValueChange={(monoize_passive_rate_limit_cooldown_seconds) =>
            onChange({ monoize_passive_rate_limit_cooldown_seconds })
          }
        />
      </SettingsGroup>

      <SettingsGroup id="routing.timeouts">
        <NumberRow
          id="monoize_request_timeout_ms"
          value={settings.monoize_request_timeout_ms}
          onValueChange={(monoize_request_timeout_ms) => onChange({ monoize_request_timeout_ms })}
        />
        <NumberRow
          id="monoize_stream_idle_timeout_ms"
          value={settings.monoize_stream_idle_timeout_ms}
          onValueChange={(monoize_stream_idle_timeout_ms) =>
            onChange({ monoize_stream_idle_timeout_ms })
          }
        />
      </SettingsGroup>
    </>
  );
}
