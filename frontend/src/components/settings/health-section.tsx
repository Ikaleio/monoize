import { useTranslation } from "react-i18next";

import {
  Field,
  FieldContent,
  FieldDescription,
  FieldLabel,
} from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Separator } from "@/components/ui/separator";
import { Switch } from "@/components/ui/switch";
import type { SystemSettings } from "@/lib/api";
import { SettingsGroup } from "./settings-category-panel";

interface HealthSectionProps {
  settings: SystemSettings;
  onChange: (updates: Partial<SystemSettings>) => void;
}

interface NumberFieldSpec {
  id: string;
  labelKey: string;
  descriptionKey: string;
  value: number;
  min: number;
  max?: number;
  step?: string;
  parse: (raw: string) => number;
  apply: (parsed: number) => Partial<SystemSettings>;
}

/**
 * Health & runtime drawer: active probe, passive breaker, request capture,
 * and billing/runtime toggles grouped into kicker-labeled clusters with dense
 * numeric grids.
 */
export function HealthSection({ settings, onChange }: HealthSectionProps) {
  const { t } = useTranslation();

  const passiveFields: NumberFieldSpec[] = [
    {
      id: "passive_failure_count_threshold",
      labelKey: "settings.passiveFailureThreshold",
      descriptionKey: "settings.passiveFailureThresholdDescription",
      value: settings.monoize_passive_failure_threshold,
      min: 1,
      parse: (raw) => Math.max(1, parseInt(raw) || 3),
      apply: (parsed) => ({ monoize_passive_failure_threshold: parsed }),
    },
    {
      id: "passive_cooldown_seconds",
      labelKey: "settings.passiveCooldownSeconds",
      descriptionKey: "settings.passiveCooldownSecondsDescription",
      value: settings.monoize_passive_cooldown_seconds,
      min: 1,
      parse: (raw) => Math.max(1, parseInt(raw) || 60),
      apply: (parsed) => ({ monoize_passive_cooldown_seconds: parsed }),
    },
    {
      id: "passive_window_seconds",
      labelKey: "settings.passiveWindowSeconds",
      descriptionKey: "settings.passiveWindowSecondsDescription",
      value: settings.monoize_passive_window_seconds,
      min: 1,
      parse: (raw) => Math.max(1, parseInt(raw) || 30),
      apply: (parsed) => ({ monoize_passive_window_seconds: parsed }),
    },
    {
      id: "passive_min_samples",
      labelKey: "settings.passiveMinSamples",
      descriptionKey: "settings.passiveMinSamplesDescription",
      value: settings.monoize_passive_min_samples,
      min: 1,
      parse: (raw) => Math.max(1, parseInt(raw) || 20),
      apply: (parsed) => ({ monoize_passive_min_samples: parsed }),
    },
    {
      id: "passive_failure_rate_threshold",
      labelKey: "settings.passiveFailureRateThreshold",
      descriptionKey: "settings.passiveFailureRateThresholdDescription",
      value: settings.monoize_passive_failure_rate_threshold,
      min: 0.01,
      max: 1,
      step: "0.01",
      parse: (raw) => Math.min(1, Math.max(0.01, parseFloat(raw) || 0.6)),
      apply: (parsed) => ({ monoize_passive_failure_rate_threshold: parsed }),
    },
    {
      id: "passive_rate_limit_cooldown_seconds",
      labelKey: "settings.passiveRateLimitCooldownSeconds",
      descriptionKey: "settings.passiveRateLimitCooldownSecondsDescription",
      value: settings.monoize_passive_rate_limit_cooldown_seconds,
      min: 1,
      parse: (raw) => Math.max(1, parseInt(raw) || 15),
      apply: (parsed) => ({ monoize_passive_rate_limit_cooldown_seconds: parsed }),
    },
  ];

  return (
    <div className="flex flex-col gap-6">
      <SettingsGroup label={t("settings.groupActiveProbe")}>
        <Field orientation="horizontal">
          <FieldContent>
            <FieldLabel htmlFor="active_probe_enabled">
              {t("settings.activeProbeEnabled")}
            </FieldLabel>
            <FieldDescription>
              {t("settings.activeProbeEnabledDescription")}
            </FieldDescription>
          </FieldContent>
          <Switch
            id="active_probe_enabled"
            checked={settings.monoize_active_probe_enabled}
            onCheckedChange={(checked) =>
              onChange({ monoize_active_probe_enabled: checked })
            }
          />
        </Field>
        <div className="grid gap-6 sm:grid-cols-2 xl:grid-cols-3">
          <Field>
            <FieldLabel htmlFor="probe_interval_seconds">
              {t("settings.activeProbeIntervalSeconds")}
            </FieldLabel>
            <Input
              id="probe_interval_seconds"
              type="number"
              min="1"
              value={settings.monoize_active_probe_interval_seconds}
              onChange={(e) =>
                onChange({
                  monoize_active_probe_interval_seconds: Math.max(
                    1,
                    parseInt(e.target.value) || 30
                  ),
                })
              }
            />
            <FieldDescription>
              {t("settings.activeProbeIntervalSecondsDescription")}
            </FieldDescription>
          </Field>
          <Field>
            <FieldLabel htmlFor="probe_success_threshold">
              {t("settings.activeProbeSuccessThreshold")}
            </FieldLabel>
            <Input
              id="probe_success_threshold"
              type="number"
              min="1"
              value={settings.monoize_active_probe_success_threshold}
              onChange={(e) =>
                onChange({
                  monoize_active_probe_success_threshold: Math.max(
                    1,
                    parseInt(e.target.value) || 1
                  ),
                })
              }
            />
            <FieldDescription>
              {t("settings.activeProbeSuccessThresholdDescription")}
            </FieldDescription>
          </Field>
          <Field>
            <FieldLabel htmlFor="probe_model">{t("settings.activeProbeModel")}</FieldLabel>
            <Input
              id="probe_model"
              value={settings.monoize_active_probe_model ?? ""}
              onChange={(e) =>
                onChange({ monoize_active_probe_model: e.target.value || null })
              }
              placeholder={t("settings.activeProbeModelPlaceholder")}
            />
            <FieldDescription>
              {t("settings.activeProbeModelDescription")}
            </FieldDescription>
          </Field>
        </div>
      </SettingsGroup>

      <Separator />

      <SettingsGroup label={t("settings.groupPassiveBreaker")}>
        <div className="grid gap-6 sm:grid-cols-2 xl:grid-cols-3">
          {passiveFields.map((spec) => (
            <Field key={spec.id}>
              <FieldLabel htmlFor={spec.id}>{t(spec.labelKey)}</FieldLabel>
              <Input
                id={spec.id}
                type="number"
                min={spec.min}
                max={spec.max}
                step={spec.step}
                value={spec.value}
                onChange={(e) => onChange(spec.apply(spec.parse(e.target.value)))}
              />
              <FieldDescription>{t(spec.descriptionKey)}</FieldDescription>
            </Field>
          ))}
        </div>
      </SettingsGroup>

      <Separator />

      <SettingsGroup label={t("settings.groupRequestCapture")}>
        <div className="grid gap-6 lg:grid-cols-2 lg:items-start">
          <Field orientation="horizontal">
            <FieldContent>
              <FieldLabel htmlFor="request_capture_enabled">
                {t("settings.requestCaptureEnabled")}
              </FieldLabel>
              <FieldDescription>
                {t("settings.requestCaptureEnabledDescription")}
              </FieldDescription>
            </FieldContent>
            <Switch
              id="request_capture_enabled"
              checked={settings.monoize_request_capture_enabled}
              onCheckedChange={(checked) =>
                onChange({ monoize_request_capture_enabled: checked })
              }
            />
          </Field>
          <Field orientation="horizontal">
            <FieldContent>
              <FieldLabel htmlFor="mask_sensitive_info">
                {t("settings.maskSensitiveInfo")}
              </FieldLabel>
              <FieldDescription>
                {t("settings.maskSensitiveInfoDescription")}
              </FieldDescription>
            </FieldContent>
            <Switch
              id="mask_sensitive_info"
              checked={settings.monoize_mask_sensitive_info}
              onCheckedChange={(checked) =>
                onChange({ monoize_mask_sensitive_info: checked })
              }
            />
          </Field>
          <Field>
            <FieldLabel htmlFor="request_capture_retention_days">
              {t("settings.requestCaptureRetentionDays")}
            </FieldLabel>
            <Input
              id="request_capture_retention_days"
              type="number"
              min="1"
              value={settings.monoize_request_capture_retention_days}
              onChange={(e) =>
                onChange({
                  monoize_request_capture_retention_days: Math.max(
                    1,
                    parseInt(e.target.value) || 1
                  ),
                })
              }
            />
            <FieldDescription>
              {t("settings.requestCaptureRetentionDaysDescription")}
            </FieldDescription>
          </Field>
        </div>
      </SettingsGroup>

      <Separator />

      <SettingsGroup label={t("settings.groupRuntimeBehavior")}>
        <div className="grid gap-6 lg:grid-cols-2 lg:items-start">
          <Field orientation="horizontal">
            <FieldContent>
              <FieldLabel htmlFor="enable_estimated_billing">
                {t("settings.enableEstimatedBilling")}
              </FieldLabel>
              <FieldDescription>
                {t("settings.enableEstimatedBillingDescription")}
              </FieldDescription>
            </FieldContent>
            <Switch
              id="enable_estimated_billing"
              checked={settings.monoize_enable_estimated_billing}
              onCheckedChange={(checked) =>
                onChange({ monoize_enable_estimated_billing: checked })
              }
            />
          </Field>
          <Field orientation="horizontal">
            <FieldContent>
              <FieldLabel htmlFor="strip_cross_protocol_nested_extra">
                {t("settings.stripCrossProtocolNestedExtra")}
              </FieldLabel>
              <FieldDescription>
                {t("settings.stripCrossProtocolNestedExtraDescription")}
              </FieldDescription>
            </FieldContent>
            <Switch
              id="strip_cross_protocol_nested_extra"
              checked={settings.monoize_strip_cross_protocol_nested_extra}
              onCheckedChange={(checked) =>
                onChange({ monoize_strip_cross_protocol_nested_extra: checked })
              }
            />
          </Field>
          <Field>
            <FieldLabel htmlFor="request_timeout_ms">
              {t("settings.requestTimeoutMs")}
            </FieldLabel>
            <Input
              id="request_timeout_ms"
              type="number"
              min="1"
              value={settings.monoize_request_timeout_ms}
              onChange={(e) =>
                onChange({
                  monoize_request_timeout_ms: Math.max(
                    1,
                    parseInt(e.target.value) || 30000
                  ),
                })
              }
            />
            <FieldDescription>{t("settings.requestTimeoutMsDescription")}</FieldDescription>
          </Field>
        </div>
      </SettingsGroup>
    </div>
  );
}
