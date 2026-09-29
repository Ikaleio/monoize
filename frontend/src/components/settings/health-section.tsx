import { useTranslation } from "react-i18next";

import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import type { SystemSettings } from "@/lib/api";
import type { SettingEntryId } from "./settings-categories";
import { SettingBlock, SettingRow, SettingsGroup } from "./settings-layout";
import { DashboardPerformanceTargets } from "./dashboard-performance-targets";

interface HealthSectionProps {
  settings: SystemSettings;
  onChange: (updates: Partial<SystemSettings>) => void;
  availableModelIds?: string[];
  modelsLoading?: boolean;
  modelsError?: unknown;
  onRetryModels?: () => void;
}

type BooleanSettingKey = {
  [K in keyof SystemSettings]: SystemSettings[K] extends boolean ? K : never;
}[keyof SystemSettings] &
  SettingEntryId;

interface NumberFieldSpec {
  id: SettingEntryId;
  value: number;
  min: number;
  max?: number;
  step?: string;
  parse: (raw: string) => number;
  apply: (parsed: number) => Partial<SystemSettings>;
}

/**
 * Health and runtime settings: active probe, passive breaker, request capture,
 * billing/runtime switches, and home dashboard performance targets.
 */
export function HealthSection({
  settings,
  onChange,
  availableModelIds = [],
  modelsLoading = false,
  modelsError,
  onRetryModels,
}: HealthSectionProps) {
  const { t } = useTranslation();

  const switchRow = (key: BooleanSettingKey) => (
    <SettingRow id={key} switchControl>
      <Switch
        id={key}
        checked={settings[key]}
        onCheckedChange={(checked) => onChange({ [key]: checked } as Partial<SystemSettings>)}
      />
    </SettingRow>
  );

  const numberRow = (spec: NumberFieldSpec) => (
    <SettingRow key={spec.id} id={spec.id}>
      <Input
        id={spec.id}
        type="number"
        min={spec.min}
        max={spec.max}
        step={spec.step}
        value={spec.value}
        onChange={(e) => onChange(spec.apply(spec.parse(e.target.value)))}
      />
    </SettingRow>
  );

  const passiveFields: NumberFieldSpec[] = [
    {
      id: "monoize_passive_failure_threshold",
      value: settings.monoize_passive_failure_threshold,
      min: 1,
      parse: (raw) => Math.max(1, parseInt(raw) || 3),
      apply: (parsed) => ({ monoize_passive_failure_threshold: parsed }),
    },
    {
      id: "monoize_passive_cooldown_seconds",
      value: settings.monoize_passive_cooldown_seconds,
      min: 1,
      parse: (raw) => Math.max(1, parseInt(raw) || 60),
      apply: (parsed) => ({ monoize_passive_cooldown_seconds: parsed }),
    },
    {
      id: "monoize_passive_window_seconds",
      value: settings.monoize_passive_window_seconds,
      min: 1,
      parse: (raw) => Math.max(1, parseInt(raw) || 30),
      apply: (parsed) => ({ monoize_passive_window_seconds: parsed }),
    },
    {
      id: "monoize_passive_min_samples",
      value: settings.monoize_passive_min_samples,
      min: 1,
      parse: (raw) => Math.max(1, parseInt(raw) || 20),
      apply: (parsed) => ({ monoize_passive_min_samples: parsed }),
    },
    {
      id: "monoize_passive_failure_rate_threshold",
      value: settings.monoize_passive_failure_rate_threshold,
      min: 0.01,
      max: 1,
      step: "0.01",
      parse: (raw) => Math.min(1, Math.max(0.01, parseFloat(raw) || 0.6)),
      apply: (parsed) => ({ monoize_passive_failure_rate_threshold: parsed }),
    },
    {
      id: "monoize_passive_rate_limit_cooldown_seconds",
      value: settings.monoize_passive_rate_limit_cooldown_seconds,
      min: 1,
      parse: (raw) => Math.max(1, parseInt(raw) || 15),
      apply: (parsed) => ({ monoize_passive_rate_limit_cooldown_seconds: parsed }),
    },
  ];

  return (
    <>
      <SettingsGroup id="health.probe">
        {switchRow("monoize_active_probe_enabled")}
        {numberRow({
          id: "monoize_active_probe_interval_seconds",
          value: settings.monoize_active_probe_interval_seconds,
          min: 1,
          parse: (raw) => Math.max(1, parseInt(raw) || 30),
          apply: (parsed) => ({ monoize_active_probe_interval_seconds: parsed }),
        })}
        {numberRow({
          id: "monoize_active_probe_success_threshold",
          value: settings.monoize_active_probe_success_threshold,
          min: 1,
          parse: (raw) => Math.max(1, parseInt(raw) || 1),
          apply: (parsed) => ({ monoize_active_probe_success_threshold: parsed }),
        })}
        <SettingRow id="monoize_active_probe_model">
          <Input
            id="monoize_active_probe_model"
            value={settings.monoize_active_probe_model ?? ""}
            onChange={(e) => onChange({ monoize_active_probe_model: e.target.value || null })}
            placeholder={t("settings.activeProbeModelPlaceholder")}
          />
        </SettingRow>
      </SettingsGroup>

      <SettingsGroup id="health.passive">{passiveFields.map(numberRow)}</SettingsGroup>

      <SettingsGroup id="health.capture">
        {switchRow("monoize_request_capture_enabled")}
        {switchRow("monoize_mask_sensitive_info")}
        {numberRow({
          id: "monoize_request_capture_max_total_bytes",
          value: Math.round(settings.monoize_request_capture_max_total_bytes / 1048576),
          min: 0,
          parse: (raw) => Math.max(0, parseInt(raw) || 0),
          // SSU-2a: edited in MiB, stored in bytes; 0 disables the budget.
          apply: (mib) => ({ monoize_request_capture_max_total_bytes: mib * 1048576 }),
        })}
      </SettingsGroup>

      <SettingsGroup id="health.runtime">
        {switchRow("monoize_enable_estimated_billing")}
        {switchRow("monoize_strip_cross_protocol_nested_extra")}
        {switchRow("allow_free_when_unpriced")}
        {switchRow("allow_free_when_missing_usage")}
        {numberRow({
          id: "monoize_request_timeout_ms",
          value: settings.monoize_request_timeout_ms,
          min: 1,
          parse: (raw) => Math.max(1, parseInt(raw) || 30000),
          apply: (parsed) => ({ monoize_request_timeout_ms: parsed }),
        })}
      </SettingsGroup>

      <SettingsGroup id="health.dashboard">
        <SettingBlock id="dashboard_performance_targets">
          <DashboardPerformanceTargets
            settings={settings}
            availableModelIds={availableModelIds}
            modelsLoading={modelsLoading}
            modelsError={modelsError}
            onRetryModels={onRetryModels ?? (() => undefined)}
            onChange={onChange}
          />
        </SettingBlock>
      </SettingsGroup>
    </>
  );
}
