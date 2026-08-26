import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Save, Settings2 } from "lucide-react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Tabs } from "@/components/ui/tabs";
import {
  useProviders,
  useSettings,
  updateSettingsOptimistic,
  useTransformRegistry,
} from "@/lib/swr";
import type { SystemSettings } from "@/lib/api";
import { AnimatedButton, PageWrapper, motion, transitions } from "@/components/ui/motion";
import { PageHeader } from "@/components/ui/page-header";
import { TransformChainEditor } from "@/components/transforms/transform-chain-editor";
import { findFirstInvalidTransformRule } from "@/components/transforms/transform-schema";
import { CodexModelSelector } from "@/components/settings/codex-model-selector";
import { ModelRedirectsEditor } from "@/components/settings/model-redirects-editor";
import { SuffixMapEditor } from "@/components/settings/suffix-map-editor";
import {
  SETTINGS_CATEGORIES,
  type SettingsCategoryId,
} from "@/components/settings/settings-categories";
import { SettingsCategoryRail } from "@/components/settings/settings-category-rail";
import { SettingsCategoryPanel } from "@/components/settings/settings-category-panel";
import { SettingsPageSkeleton } from "@/components/settings/settings-skeleton";
import { SiteSection } from "@/components/settings/site-section";
import { AccessSection } from "@/components/settings/access-section";
import { AffinitySection } from "@/components/settings/affinity-section";
import { HealthSection } from "@/components/settings/health-section";
import { ExtraFieldsSection } from "@/components/settings/extra-fields-section";

export function SettingsPage() {
  const { t } = useTranslation();
  const { data: settings, isLoading, mutate } = useSettings();
  const {
    data: providers,
    error: providersError,
    isLoading: providersLoading,
    mutate: mutateProviders,
  } = useProviders();
  const { data: transformRegistry = [], isLoading: transformRegistryLoading } =
    useTransformRegistry();
  const [localSettings, setLocalSettings] = useState<SystemSettings | null>(null);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [activeCategory, setActiveCategory] = useState<SettingsCategoryId>("site");

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
      <PageWrapper>
        <SettingsPageSkeleton />
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

  const renderCategoryContent = (id: SettingsCategoryId) => {
    switch (id) {
      case "site":
        return <SiteSection settings={currentSettings} onChange={handleChange} />;
      case "access":
        return <AccessSection settings={currentSettings} onChange={handleChange} />;
      case "codex":
        return (
          <CodexModelSelector
            availableModelIds={availableCodexModelIds}
            selectedModelIds={currentSettings.codex_model_ids ?? []}
            isLoading={providersLoading}
            loadError={providersError}
            onRetry={() => void mutateProviders()}
            onChange={(codex_model_ids) => handleChange({ codex_model_ids })}
          />
        );
      case "suffix":
        return (
          <SuffixMapEditor
            value={currentSettings.reasoning_suffix_map}
            onChange={(map) => handleChange({ reasoning_suffix_map: map })}
          />
        );
      case "redirects":
        return (
          <ModelRedirectsEditor
            value={currentSettings.global_model_redirects ?? []}
            disabled={saving}
            onChange={(global_model_redirects) =>
              handleChange({ global_model_redirects })
            }
          />
        );
      case "transforms":
        return (
          <div className="flex flex-col gap-3">
            <div className="flex items-center gap-2">
              <Settings2 className="h-4 w-4 text-muted-foreground" />
              <h3 className="text-sm font-medium">{t("transforms.titleGlobal")}</h3>
            </div>
            <TransformChainEditor
              value={currentSettings.global_transforms ?? []}
              registry={globalTransformRegistry}
              loading={transformRegistryLoading}
              onChange={(next) => handleChange({ global_transforms: next })}
            />
            <p className="text-sm text-muted-foreground">
              {t("settings.globalTransformsHelp")}
            </p>
          </div>
        );
      case "affinity":
        return <AffinitySection settings={currentSettings} onChange={handleChange} />;
      case "health":
        return (
          <HealthSection
            settings={currentSettings}
            onChange={handleChange}
            availableModelIds={availableCodexModelIds}
            modelsLoading={providersLoading}
            modelsError={providersError}
            onRetryModels={() => void mutateProviders()}
          />
        );
      case "extra":
        return <ExtraFieldsSection settings={currentSettings} onChange={handleChange} />;
    }
  };

  return (
    <PageWrapper className="flex min-w-0 flex-col gap-6">
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

      <Tabs
        value={activeCategory}
        onValueChange={(value) => setActiveCategory(value as SettingsCategoryId)}
        className="flex min-w-0 flex-col gap-8"
      >
        <SettingsCategoryRail activeId={activeCategory} />
        {SETTINGS_CATEGORIES.map((category, index) => (
          <SettingsCategoryPanel key={category.id} category={category} index={index}>
            {renderCategoryContent(category.id)}
          </SettingsCategoryPanel>
        ))}
      </Tabs>
    </PageWrapper>
  );
}
