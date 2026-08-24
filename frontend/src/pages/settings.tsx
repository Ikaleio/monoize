import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Plus, Save, Settings2, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import {
  Field,
  FieldContent,
  FieldDescription,
  FieldGroup,
  FieldLabel,
} from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Separator } from "@/components/ui/separator";
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import {
  useProviders,
  useSettings,
  updateSettingsOptimistic,
  useTransformRegistry,
} from "@/lib/swr";
import type { SystemSettings } from "@/lib/api";
import { AnimatedButton, PageWrapper, motion, transitions, StaggerList, StaggerItem } from "@/components/ui/motion";
import { PageHeader } from "@/components/ui/page-header";
import { TablePageSkeleton } from "@/components/ui/page-skeleton";
import { TransformChainEditor } from "@/components/transforms/transform-chain-editor";
import { findFirstInvalidTransformRule } from "@/components/transforms/transform-schema";
import { toast } from "sonner";
import { CodexModelSelector } from "@/components/settings/codex-model-selector";
import { ModelRedirectsEditor } from "@/components/settings/model-redirects-editor";

const EFFORT_VALUES = ["none", "minimum", "low", "medium", "high", "xhigh", "max"] as const;

interface SuffixRow {
  id: number;
  suffix: string;
  effort: string;
}

let suffixRowId = 0;

function mapToRows(map: Record<string, string> | undefined): SuffixRow[] {
  return Object.entries(map ?? {}).map(([suffix, effort]) => ({
    id: ++suffixRowId,
    suffix,
    effort,
  }));
}

function rowsToMap(rows: SuffixRow[]): Record<string, string> {
  const map: Record<string, string> = {};
  for (const row of rows) {
    if (row.suffix) map[row.suffix] = row.effort;
  }
  return map;
}

function SuffixMapEditor({
  value,
  onChange,
}: {
  value: Record<string, string> | undefined;
  onChange: (map: Record<string, string>) => void;
}) {
  const { t } = useTranslation();
  const [rows, setRows] = useState<SuffixRow[]>(() => mapToRows(value));
  const prevValueRef = useRef(value);

  useEffect(() => {
    if (prevValueRef.current !== value) {
      prevValueRef.current = value;
      setRows(mapToRows(value));
    }
  }, [value]);

  const commit = useCallback(
    (updated: SuffixRow[]) => {
      setRows(updated);
      onChange(rowsToMap(updated));
    },
    [onChange]
  );

  return (
    <div className="space-y-4">
      {rows.map((row, idx) => (
        <div key={row.id} className="flex items-center gap-2">
          <Input
            defaultValue={row.suffix}
            placeholder={t("settings.suffix")}
            className="flex-1 transition-all"
            onBlur={(e) => {
              const updated = rows.map((r, i) =>
                i === idx ? { ...r, suffix: e.target.value } : r
              );
              commit(updated);
            }}
          />
          <Select
            value={row.effort}
            onValueChange={(val) => {
              const updated = rows.map((r, i) =>
                i === idx ? { ...r, effort: val } : r
              );
              commit(updated);
            }}
          >
            <SelectTrigger className="w-[140px]">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {EFFORT_VALUES.map((v) => (
                <SelectItem key={v} value={v}>
                  {v}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Button
            variant="ghost"
            size="icon"
            className="size-11 touch-manipulation sm:size-9"
            aria-label={t("common.delete")}
            onClick={() => commit(rows.filter((_, i) => i !== idx))}
          >
            <X className="h-4 w-4" />
          </Button>
        </div>
      ))}
      <Button
        variant="outline"
        size="sm"
        onClick={() => {
          setRows([...rows, { id: ++suffixRowId, suffix: "", effort: "high" }]);
        }}
      >
        <Plus className="mr-2 h-4 w-4" />
        {t("settings.addSuffix")}
      </Button>
      <p className="text-sm text-muted-foreground">
        {t("settings.effortValues")}
      </p>
    </div>
  );
}

export function SettingsPage() {
  const { t } = useTranslation();
  const { data: settings, isLoading, mutate } = useSettings();
  const {
    data: providers,
    error: providersError,
    isLoading: providersLoading,
    mutate: mutateProviders,
  } = useProviders();
  const { data: transformRegistry = [] } = useTransformRegistry();
  const [localSettings, setLocalSettings] = useState<SystemSettings | null>(null);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);

  // Use local state if user has made changes, otherwise use SWR data
  const currentSettings = localSettings ?? settings;
  const globalTransformRegistry = transformRegistry.filter((item) =>
    item.supported_scopes.includes("global")
  );
  const availableCodexModelIds = useMemo(() => {
    const modelIds = new Set<string>();
    for (const provider of providers ?? []) {
      if (!provider.enabled) continue;
      for (const channel of provider.channels) {
        if (!channel.enabled || channel.weight <= 0) continue;
        for (const modelId of Object.keys(channel.models)) {
          modelIds.add(modelId);
        }
      }
    }
    return Array.from(modelIds).sort();
  }, [providers]);

  const handleChange = (updates: Partial<SystemSettings>) => {
    if (!currentSettings) return;
    setLocalSettings({ ...currentSettings, ...updates });
  };

  const handleSave = async () => {
    if (!currentSettings) return;
    const invalidRule = findFirstInvalidTransformRule(
      currentSettings.global_transforms ?? [],
      globalTransformRegistry
    );
    if (invalidRule) {
      const firstError = invalidRule.errors[0];
      toast.error(t("transforms.validationRuleInvalid", {
        index: invalidRule.index + 1,
        reason: `${firstError.field} ${firstError.message}`,
      }));
      return;
    }
    setSaving(true);
    try {
      const settingsToSave = {
        ...currentSettings,
        global_model_redirects: (currentSettings.global_model_redirects ?? []).filter(
          (rule) => rule.pattern.trim() && rule.replace.trim()
        ),
      };
      await updateSettingsOptimistic(settingsToSave);
      setLocalSettings(null); // Clear local state to use SWR data
      setSaved(true);
      setTimeout(() => setSaved(false), 2000);
      mutate();
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t("settings.failedSave"));
    } finally {
      setSaving(false);
    }
  };

  const hasChanges = localSettings !== null;

  if (isLoading) {
    return (
      <PageWrapper className="space-y-6">
        <TablePageSkeleton />
      </PageWrapper>
    );
  }

  if (!currentSettings) {
    return (
      <div className="py-8 text-center text-muted-foreground">
        {t("settings.failedLoad")}
      </div>
    );
  }

  return (
    <PageWrapper className="space-y-6">
      <motion.div
        initial={{ opacity: 0, y: -10 }}
        animate={{ opacity: 1, y: 0 }}
        transition={transitions.normal}
      >
        <PageHeader title={t("settings.title")} description={t("settings.description")} actions={(<AnimatedButton>
          <Button onClick={handleSave} disabled={saving || !hasChanges}>
            <Save className="mr-2 h-4 w-4" />
            {saving ? t("common.saving") : saved ? t("common.saved") : t("common.saveChanges")}
          </Button>
        </AnimatedButton>)} />
      </motion.div>

      <StaggerList className="grid min-w-0 gap-6 [&>*]:min-w-0">
        <StaggerItem>
          <Card>
            <CardHeader>
              <CardTitle>{t("settings.siteInformation")}</CardTitle>
              <CardDescription>{t("settings.siteInfoDescription")}</CardDescription>
            </CardHeader>
            <CardContent className="space-y-4">
              <div className="space-y-2">
                <Label htmlFor="site_name">{t("settings.siteName")}</Label>
                <Input
                  id="site_name"
                  value={currentSettings.site_name}
                  onChange={(e) => handleChange({ site_name: e.target.value })}
                  className="transition-all"
                />
              </div>
              <div className="space-y-2">
                <Label htmlFor="site_description">{t("settings.siteDescription")}</Label>
                <Input
                  id="site_description"
                  value={currentSettings.site_description}
                  onChange={(e) => handleChange({ site_description: e.target.value })}
                  className="transition-all"
                />
              </div>
              <div className="space-y-2">
                <Label htmlFor="api_base_url">{t("settings.apiBaseUrl")}</Label>
                <Input
                  id="api_base_url"
                  value={currentSettings.api_base_url}
                  onChange={(e) => handleChange({ api_base_url: e.target.value })}
                  placeholder={t("settings.apiBaseUrlPlaceholder")}
                  className="transition-all"
                />
                <p className="text-sm text-muted-foreground">
                  {t("settings.apiBaseUrlDescription")}
                </p>
              </div>
            </CardContent>
          </Card>
        </StaggerItem>

        <StaggerItem>
          <Card>
            <CardHeader>
              <CardTitle>{t("settings.codexModels")}</CardTitle>
              <CardDescription>{t("settings.codexModelsDescription")}</CardDescription>
            </CardHeader>
            <CardContent>
              <CodexModelSelector
                availableModelIds={availableCodexModelIds}
                selectedModelIds={currentSettings.codex_model_ids ?? []}
                isLoading={providersLoading}
                loadError={providersError}
                onRetry={() => void mutateProviders()}
                onChange={(codex_model_ids) => handleChange({ codex_model_ids })}
              />
            </CardContent>
          </Card>
        </StaggerItem>

        <StaggerItem>
          <Card>
            <CardHeader>
              <CardTitle>{t("settings.registration")}</CardTitle>
              <CardDescription>{t("settings.registrationDescription")}</CardDescription>
            </CardHeader>
            <CardContent className="space-y-4">
              <div className="flex items-center justify-between">
                <div className="space-y-0.5">
                  <Label>{t("settings.allowRegistration")}</Label>
                  <p className="text-sm text-muted-foreground">
                    {t("settings.allowRegistrationDescription")}
                  </p>
                </div>
                <Switch
                  checked={currentSettings.registration_enabled}
                  onCheckedChange={(checked) =>
                    handleChange({ registration_enabled: checked })
                  }
                />
              </div>
              <Separator />
              <div className="space-y-2">
                <Label htmlFor="default_role">{t("settings.defaultUserRole")}</Label>
                <Input
                  id="default_role"
                  value={currentSettings.default_user_role}
                  onChange={(e) =>
                    handleChange({ default_user_role: e.target.value })
                  }
                  className="transition-all"
                />
                <p className="text-sm text-muted-foreground">
                  {t("settings.defaultUserRoleDescription")}
                </p>
              </div>
            </CardContent>
          </Card>
        </StaggerItem>

        <StaggerItem>
          <Card>
            <CardHeader>
              <CardTitle>{t("settings.sessionSecurity")}</CardTitle>
              <CardDescription>{t("settings.sessionSecurityDescription")}</CardDescription>
            </CardHeader>
            <CardContent className="space-y-4">
              <div className="flex items-center justify-between gap-4">
                <div className="space-y-0.5">
                  <Label>{t("settings.captchaEnabled")}</Label>
                  <p className="text-sm text-muted-foreground">
                    {t("settings.captchaEnabledDescription")}
                  </p>
                </div>
                <Switch
                  checked={currentSettings.captcha_enabled}
                  onCheckedChange={(checked) =>
                    handleChange({ captcha_enabled: checked })
                  }
                />
              </div>
              <Separator />
              <div className="space-y-2">
                <Label htmlFor="session_ttl">{t("settings.sessionDuration")}</Label>
                <Input
                  id="session_ttl"
                  type="number"
                  min="1"
                  value={currentSettings.session_ttl_days}
                  onChange={(e) =>
                    handleChange({
                      session_ttl_days: parseInt(e.target.value) || 7,
                    })
                  }
                  className="transition-all"
                />
                <p className="text-sm text-muted-foreground">
                  {t("settings.sessionDurationDescription")}
                </p>
              </div>
              <Separator />
              <div className="space-y-2">
                <Label htmlFor="max_api_keys">{t("settings.maxApiKeys")}</Label>
                <Input
                  id="max_api_keys"
                  type="number"
                  min="1"
                  value={currentSettings.api_key_max_per_user}
                  onChange={(e) =>
                    handleChange({
                      api_key_max_per_user: parseInt(e.target.value) || 10,
                    })
                  }
                  className="transition-all"
                />
                <p className="text-sm text-muted-foreground">
                  {t("settings.maxApiKeysDescription")}
                </p>
              </div>
            </CardContent>
          </Card>
        </StaggerItem>

        <StaggerItem>
          <Card>
            <CardHeader>
              <CardTitle>{t("settings.reasoningSuffixMap")}</CardTitle>
              <CardDescription>{t("settings.reasoningSuffixMapDescription")}</CardDescription>
            </CardHeader>
            <CardContent>
              <SuffixMapEditor
                value={currentSettings.reasoning_suffix_map}
                onChange={(map) => handleChange({ reasoning_suffix_map: map })}
              />
            </CardContent>
          </Card>
        </StaggerItem>

        <StaggerItem>
          <Card>
            <CardHeader>
              <CardTitle>{t("settings.globalModelRedirects")}</CardTitle>
              <CardDescription>
                {t("settings.globalModelRedirectsDescription")}
              </CardDescription>
            </CardHeader>
            <CardContent>
              <ModelRedirectsEditor
                value={currentSettings.global_model_redirects ?? []}
                disabled={saving}
                onChange={(global_model_redirects) =>
                  handleChange({ global_model_redirects })
                }
              />
            </CardContent>
          </Card>
        </StaggerItem>

        <StaggerItem>
          <Card>
            <CardHeader>
              <CardTitle>{t("settings.globalTransforms")}</CardTitle>
              <CardDescription>{t("settings.globalTransformsDescription")}</CardDescription>
            </CardHeader>
            <CardContent className="space-y-3">
              <div className="flex items-center gap-2">
                <Settings2 className="h-4 w-4 text-muted-foreground" />
                <h3 className="text-sm font-medium">{t("transforms.titleGlobal")}</h3>
              </div>
              <TransformChainEditor
                value={currentSettings.global_transforms ?? []}
                registry={globalTransformRegistry}
                onChange={(next) => handleChange({ global_transforms: next })}
              />
              <p className="text-sm text-muted-foreground">
                {t("settings.globalTransformsHelp")}
              </p>
            </CardContent>
          </Card>
        </StaggerItem>

        <StaggerItem>
          <Card>
            <CardHeader>
              <CardTitle>{t("settings.affinityRouting")}</CardTitle>
              <CardDescription>{t("settings.affinityRoutingDescription")}</CardDescription>
            </CardHeader>
            <CardContent>
              <FieldGroup>
                <Field orientation="horizontal">
                  <FieldContent>
                    <FieldLabel htmlFor="affinity_enabled">
                      {t("settings.affinityEnabled")}
                    </FieldLabel>
                    <FieldDescription>
                      {t("settings.affinityEnabledDescription")}
                    </FieldDescription>
                  </FieldContent>
                  <Switch
                    id="affinity_enabled"
                    checked={currentSettings.monoize_affinity_enabled}
                    onCheckedChange={(checked) =>
                      handleChange({ monoize_affinity_enabled: checked })
                    }
                  />
                </Field>
                <Separator />
                <div className="grid gap-5 md:grid-cols-2">
                  <Field>
                    <FieldLabel htmlFor="affinity_failback_mode">
                      {t("settings.affinityFailbackMode")}
                    </FieldLabel>
                    <Select
                      value={currentSettings.monoize_affinity_failback_mode}
                      onValueChange={(value: SystemSettings["monoize_affinity_failback_mode"]) =>
                        handleChange({ monoize_affinity_failback_mode: value })
                      }
                    >
                      <SelectTrigger id="affinity_failback_mode">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectGroup>
                          <SelectItem value="sticky">
                            {t("settings.affinitySticky")}
                          </SelectItem>
                          <SelectItem value="prefer_higher_priority">
                            {t("settings.affinityPreferHigherPriority")}
                          </SelectItem>
                        </SelectGroup>
                      </SelectContent>
                    </Select>
                    <FieldDescription>
                      {t("settings.affinityFailbackModeDescription")}
                    </FieldDescription>
                  </Field>
                  <Field>
                    <FieldLabel htmlFor="affinity_idle_ttl_seconds">
                      {t("settings.affinityIdleTtlSeconds")}
                    </FieldLabel>
                    <Input
                      id="affinity_idle_ttl_seconds"
                      type="number"
                      min="1"
                      value={currentSettings.monoize_affinity_idle_ttl_seconds}
                      onChange={(event) =>
                        handleChange({
                          monoize_affinity_idle_ttl_seconds: Math.max(
                            1,
                            parseInt(event.target.value) || 1
                          ),
                        })
                      }
                    />
                    <FieldDescription>
                      {t("settings.affinityIdleTtlSecondsDescription")}
                    </FieldDescription>
                  </Field>
                  <Field className="md:col-span-2">
                    <FieldLabel htmlFor="affinity_failback_delay_seconds">
                      {t("settings.affinityFailbackDelaySeconds")}
                    </FieldLabel>
                    <Input
                      id="affinity_failback_delay_seconds"
                      type="number"
                      min="0"
                      value={currentSettings.monoize_affinity_failback_delay_seconds}
                      onChange={(event) =>
                        handleChange({
                          monoize_affinity_failback_delay_seconds: Math.max(
                            0,
                            parseInt(event.target.value) || 0
                          ),
                        })
                      }
                    />
                    <FieldDescription>
                      {t("settings.affinityFailbackDelaySecondsDescription")}
                    </FieldDescription>
                  </Field>
                </div>
              </FieldGroup>
            </CardContent>
          </Card>
        </StaggerItem>

        <StaggerItem>
          <Card>
            <CardHeader>
              <CardTitle>{t("settings.healthMonitoring")}</CardTitle>
              <CardDescription>{t("settings.healthMonitoringDescription")}</CardDescription>
            </CardHeader>
            <CardContent className="space-y-4">
              <div className="flex items-center justify-between">
                <div className="space-y-0.5">
                  <Label>{t("settings.activeProbeEnabled")}</Label>
                  <p className="text-sm text-muted-foreground">
                    {t("settings.activeProbeEnabledDescription")}
                  </p>
                </div>
                <Switch
                  checked={currentSettings.monoize_active_probe_enabled}
                  onCheckedChange={(checked) =>
                    handleChange({ monoize_active_probe_enabled: checked })
                  }
                />
              </div>
              <Separator />
              <div className="flex items-center justify-between">
                <div className="space-y-0.5">
                  <Label>{t("settings.enableEstimatedBilling")}</Label>
                  <p className="text-sm text-muted-foreground">
                    {t("settings.enableEstimatedBillingDescription")}
                  </p>
                </div>
                <Switch
                  checked={currentSettings.monoize_enable_estimated_billing}
                  onCheckedChange={(checked) =>
                    handleChange({ monoize_enable_estimated_billing: checked })
                  }
                />
              </div>
              <Separator />
              <div className="flex items-center justify-between">
                <div className="space-y-0.5">
                  <Label>{t("settings.stripCrossProtocolNestedExtra")}</Label>
                  <p className="text-sm text-muted-foreground">
                    {t("settings.stripCrossProtocolNestedExtraDescription")}
                  </p>
                </div>
                <Switch
                  checked={currentSettings.monoize_strip_cross_protocol_nested_extra}
                  onCheckedChange={(checked) =>
                    handleChange({ monoize_strip_cross_protocol_nested_extra: checked })
                  }
                />
              </div>
              <Separator />
              <div className="flex items-center justify-between">
                <div className="space-y-0.5">
                  <Label>{t("settings.requestCaptureEnabled")}</Label>
                  <p className="text-sm text-muted-foreground">
                    {t("settings.requestCaptureEnabledDescription")}
                  </p>
                </div>
                <Switch
                  checked={currentSettings.monoize_request_capture_enabled}
                  onCheckedChange={(checked) =>
                    handleChange({ monoize_request_capture_enabled: checked })
                  }
                />
              </div>
              <div className="space-y-2">
                <Label htmlFor="request_capture_retention_days">{t("settings.requestCaptureRetentionDays")}</Label>
                <Input
                  id="request_capture_retention_days"
                  type="number"
                  min="1"
                  value={currentSettings.monoize_request_capture_retention_days}
                  onChange={(e) =>
                    handleChange({
                      monoize_request_capture_retention_days: Math.max(1, parseInt(e.target.value) || 1),
                    })
                  }
                  className="transition-all"
                />
                <p className="text-sm text-muted-foreground">
                  {t("settings.requestCaptureRetentionDaysDescription")}
                </p>
              </div>
              <Separator />
              <div className="space-y-2">
                <Label htmlFor="probe_interval_seconds">{t("settings.activeProbeIntervalSeconds")}</Label>
                <Input
                  id="probe_interval_seconds"
                  type="number"
                  min="1"
                  value={currentSettings.monoize_active_probe_interval_seconds}
                  onChange={(e) =>
                    handleChange({
                      monoize_active_probe_interval_seconds: Math.max(1, parseInt(e.target.value) || 30),
                    })
                  }
                  className="transition-all"
                />
                <p className="text-sm text-muted-foreground">
                  {t("settings.activeProbeIntervalSecondsDescription")}
                </p>
              </div>
              <div className="space-y-2">
                <Label htmlFor="probe_success_threshold">{t("settings.activeProbeSuccessThreshold")}</Label>
                <Input
                  id="probe_success_threshold"
                  type="number"
                  min="1"
                  value={currentSettings.monoize_active_probe_success_threshold}
                  onChange={(e) =>
                    handleChange({
                      monoize_active_probe_success_threshold: Math.max(1, parseInt(e.target.value) || 1),
                    })
                  }
                  className="transition-all"
                />
                <p className="text-sm text-muted-foreground">
                  {t("settings.activeProbeSuccessThresholdDescription")}
                </p>
              </div>
              <div className="space-y-2">
                <Label htmlFor="probe_model">{t("settings.activeProbeModel")}</Label>
                <Input
                  id="probe_model"
                  value={currentSettings.monoize_active_probe_model ?? ""}
                  onChange={(e) =>
                    handleChange({ monoize_active_probe_model: e.target.value || null })
                  }
                  placeholder={t("settings.activeProbeModelPlaceholder")}
                  className="transition-all"
                />
                <p className="text-sm text-muted-foreground">
                  {t("settings.activeProbeModelDescription")}
                </p>
              </div>
              <Separator />
              <div className="space-y-2">
                <Label htmlFor="passive_failure_count_threshold">{t("settings.passiveFailureThreshold")}</Label>
                <Input
                  id="passive_failure_count_threshold"
                  type="number"
                  min="1"
                  value={currentSettings.monoize_passive_failure_threshold}
                  onChange={(e) =>
                    handleChange({
                      monoize_passive_failure_threshold: Math.max(1, parseInt(e.target.value) || 3),
                    })
                  }
                  className="transition-all"
                />
                <p className="text-sm text-muted-foreground">
                  {t("settings.passiveFailureThresholdDescription")}
                </p>
              </div>
              <div className="space-y-2">
                <Label htmlFor="passive_cooldown_seconds">{t("settings.passiveCooldownSeconds")}</Label>
                <Input
                  id="passive_cooldown_seconds"
                  type="number"
                  min="1"
                  value={currentSettings.monoize_passive_cooldown_seconds}
                  onChange={(e) =>
                    handleChange({
                      monoize_passive_cooldown_seconds: Math.max(1, parseInt(e.target.value) || 60),
                    })
                  }
                  className="transition-all"
                />
                <p className="text-sm text-muted-foreground">
                  {t("settings.passiveCooldownSecondsDescription")}
                </p>
              </div>
              <div className="space-y-2">
                <Label htmlFor="passive_window_seconds">{t("settings.passiveWindowSeconds")}</Label>
                <Input
                  id="passive_window_seconds"
                  type="number"
                  min="1"
                  value={currentSettings.monoize_passive_window_seconds}
                  onChange={(e) =>
                    handleChange({
                      monoize_passive_window_seconds: Math.max(1, parseInt(e.target.value) || 30),
                    })
                  }
                  className="transition-all"
                />
                <p className="text-sm text-muted-foreground">
                  {t("settings.passiveWindowSecondsDescription")}
                </p>
              </div>
              <div className="space-y-2">
                <Label htmlFor="passive_min_samples">{t("settings.passiveMinSamples")}</Label>
                <Input
                  id="passive_min_samples"
                  type="number"
                  min="1"
                  value={currentSettings.monoize_passive_min_samples}
                  onChange={(e) =>
                    handleChange({
                      monoize_passive_min_samples: Math.max(1, parseInt(e.target.value) || 20),
                    })
                  }
                  className="transition-all"
                />
                <p className="text-sm text-muted-foreground">
                  {t("settings.passiveMinSamplesDescription")}
                </p>
              </div>
              <div className="space-y-2">
                <Label htmlFor="passive_failure_rate_threshold">{t("settings.passiveFailureRateThreshold")}</Label>
                <Input
                  id="passive_failure_rate_threshold"
                  type="number"
                  min="0.01"
                  max="1"
                  step="0.01"
                  value={currentSettings.monoize_passive_failure_rate_threshold}
                  onChange={(e) =>
                    handleChange({
                      monoize_passive_failure_rate_threshold: Math.min(
                        1,
                        Math.max(0.01, parseFloat(e.target.value) || 0.6)
                      ),
                    })
                  }
                  className="transition-all"
                />
                <p className="text-sm text-muted-foreground">
                  {t("settings.passiveFailureRateThresholdDescription")}
                </p>
              </div>
              <div className="space-y-2">
                <Label htmlFor="passive_rate_limit_cooldown_seconds">{t("settings.passiveRateLimitCooldownSeconds")}</Label>
                <Input
                  id="passive_rate_limit_cooldown_seconds"
                  type="number"
                  min="1"
                  value={currentSettings.monoize_passive_rate_limit_cooldown_seconds}
                  onChange={(e) =>
                    handleChange({
                      monoize_passive_rate_limit_cooldown_seconds: Math.max(1, parseInt(e.target.value) || 15),
                    })
                  }
                  className="transition-all"
                />
                <p className="text-sm text-muted-foreground">
                  {t("settings.passiveRateLimitCooldownSecondsDescription")}
                </p>
              </div>
              <div className="space-y-2">
                <Label htmlFor="request_timeout_ms">{t("settings.requestTimeoutMs")}</Label>
                <Input
                  id="request_timeout_ms"
                  type="number"
                  min="1"
                  value={currentSettings.monoize_request_timeout_ms}
                  onChange={(e) =>
                    handleChange({
                      monoize_request_timeout_ms: Math.max(1, parseInt(e.target.value) || 30000),
                    })
                  }
                  className="transition-all"
                />
                <p className="text-sm text-muted-foreground">
                  {t("settings.requestTimeoutMsDescription")}
                </p>
              </div>
            </CardContent>
          </Card>
        </StaggerItem>

        <StaggerItem>
          <Card>
            <CardHeader>
              <CardTitle>{t("settings.extraFieldsWhitelist")}</CardTitle>
              <CardDescription>{t("settings.extraFieldsWhitelistDescription")}</CardDescription>
            </CardHeader>
            <CardContent className="space-y-4">
              {(["chat_completion", "responses", "messages", "gemini"] as const).map((providerType) => (
                <div key={providerType} className="space-y-1">
                  <Label className="font-mono text-xs">{providerType}</Label>
                  <Input
                    value={(currentSettings.monoize_extra_fields_whitelist?.[providerType] ?? []).join(", ")}
                    onChange={(e) => {
                      const fields = e.target.value
                        .split(",")
                        .map((s) => s.trim())
                        .filter(Boolean);
                      handleChange({
                        monoize_extra_fields_whitelist: {
                          ...currentSettings.monoize_extra_fields_whitelist,
                          [providerType]: fields.length > 0 ? fields : undefined!,
                        },
                      });
                    }}
                    onBlur={(e) => {
                      const raw = currentSettings.monoize_extra_fields_whitelist ?? {};
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
                      handleChange({ monoize_extra_fields_whitelist: next });
                    }}
                    placeholder={t("settings.extraFieldsWhitelistPlaceholder")}
                    className="font-mono text-sm transition-all"
                  />
                </div>
              ))}
              <p className="text-sm text-muted-foreground">
                {t("settings.extraFieldsWhitelistHelp")}
              </p>
            </CardContent>
          </Card>
        </StaggerItem>
      </StaggerList>
    </PageWrapper>
  );
}
