import { useContext, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { SearchX } from "lucide-react";
import { toast } from "sonner";

import { TableToolbarSearch } from "@/components/ui/data-table-shell";
import { EmptyState } from "@/components/ui/empty-state";
import { PageWrapper, motion, transitions } from "@/components/ui/motion";
import { PageHeader } from "@/components/ui/page-header";
import { QueryError } from "@/components/ui/query-error";
import { useReducedMotionPreference } from "@/hooks/use-reduced-motion";
import { DashboardScrollParentContext } from "@/lib/dashboard-scroll";
import {
  useProviders,
  useSettings,
  updateSettingsOptimistic,
  useTransformRegistry,
} from "@/lib/swr";
import type { SystemSettings } from "@/lib/api";
import { TransformChainEditor } from "@/components/transforms/transform-chain-editor";
import { findFirstInvalidTransformRule } from "@/components/transforms/transform-schema";
import { CodexModelSelector } from "@/components/settings/codex-model-selector";
import { ModelRedirectsEditor } from "@/components/settings/model-redirects-editor";
import { SuffixMapEditor } from "@/components/settings/suffix-map-editor";
import {
  SETTINGS_CATEGORIES,
  type SettingsCategory,
  type SettingsCategoryId,
} from "@/components/settings/settings-categories";
import {
  SettingsSearchContext,
  matchSettingEntries,
  normalizeSettingsQuery,
} from "@/components/settings/settings-search";
import {
  SettingBlock,
  SettingsBody,
  SettingsCategorySection,
  SettingsGroup,
} from "@/components/settings/settings-layout";
import { SettingsCategoryNav } from "@/components/settings/settings-category-nav";
import { SettingsSaveBar } from "@/components/settings/settings-save-bar";
import { SettingsPageSkeleton } from "@/components/settings/settings-skeleton";
import { SiteSection } from "@/components/settings/site-section";
import { AccessSection } from "@/components/settings/access-section";
import { AffinitySection } from "@/components/settings/affinity-section";
import { HealthSection } from "@/components/settings/health-section";
import { ExtraFieldsSection } from "@/components/settings/extra-fields-section";

export function SettingsPage() {
  const { t } = useTranslation();
  const { data: settings, isLoading, isValidating, mutate } = useSettings();
  const {
    data: providers,
    error: providersError,
    isLoading: providersLoading,
    mutate: mutateProviders,
  } = useProviders();
  const { data: transformRegistry = [], isLoading: transformRegistryLoading } =
    useTransformRegistry();
  const scrollParent = useContext(DashboardScrollParentContext);
  const reduceMotion = useReducedMotionPreference();
  const contentRef = useRef<HTMLDivElement>(null);
  const [localSettings, setLocalSettings] = useState<SystemSettings | null>(null);
  const [saving, setSaving] = useState(false);
  const [activeCategory, setActiveCategory] = useState<SettingsCategoryId>("site");
  const [query, setQuery] = useState("");

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

  const normalizedQuery = normalizeSettingsQuery(query);
  const search = useMemo(
    () => (normalizedQuery ? matchSettingEntries(normalizedQuery, t) : null),
    [normalizedQuery, t]
  );

  // SSU-10: keep the top of the content column visible after it is replaced.
  const revealContentTop = () => {
    const content = contentRef.current;
    if (!scrollParent || !content) return;
    const offset = content.getBoundingClientRect().top - scrollParent.getBoundingClientRect().top;
    if (offset < 0) scrollParent.scrollTo({ top: scrollParent.scrollTop + offset });
  };

  const handleQueryChange = (next: string) => {
    // Entering or leaving search mode replaces the whole content column.
    if (!normalizedQuery !== !normalizeSettingsQuery(next)) revealContentTop();
    setQuery(next);
  };

  const handleSelectCategory = (id: SettingsCategoryId) => {
    setActiveCategory(id);
    if (search) {
      document.getElementById(`settings-category-${id}`)?.scrollIntoView({
        block: "start",
        behavior: reduceMotion ? "auto" : "smooth",
      });
    } else {
      revealContentTop();
    }
  };

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
      toast.success(t("settings.saved"));
      mutate();
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t("settings.failedSave"));
    } finally {
      setSaving(false);
    }
  };

  const header = <PageHeader title={t("settings.title")} description={t("settings.description")} />;

  if (isLoading) {
    return (
      <PageWrapper>
        <SettingsPageSkeleton />
      </PageWrapper>
    );
  }

  if (!currentSettings) {
    return (
      <PageWrapper className="flex min-w-0 flex-col gap-6">
        {header}
        <QueryError onRetry={() => mutate()} retrying={isValidating} />
      </PageWrapper>
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
          <SettingsGroup id="codex">
            <SettingBlock id="codex_model_ids">
              <CodexModelSelector
                availableModelIds={availableCodexModelIds}
                selectedModelIds={currentSettings.codex_model_ids ?? []}
                isLoading={providersLoading}
                loadError={providersError}
                onRetry={() => void mutateProviders()}
                onChange={(codex_model_ids) => handleChange({ codex_model_ids })}
              />
            </SettingBlock>
          </SettingsGroup>
        );
      case "suffix":
        return (
          <SettingsGroup id="suffix">
            <SettingBlock id="reasoning_suffix_map">
              <SuffixMapEditor
                value={currentSettings.reasoning_suffix_map}
                onChange={(map) => handleChange({ reasoning_suffix_map: map })}
              />
            </SettingBlock>
          </SettingsGroup>
        );
      case "redirects":
        return (
          <SettingsGroup id="redirects">
            <SettingBlock id="global_model_redirects">
              <ModelRedirectsEditor
                value={currentSettings.global_model_redirects ?? []}
                disabled={saving}
                onChange={(global_model_redirects) => handleChange({ global_model_redirects })}
              />
            </SettingBlock>
          </SettingsGroup>
        );
      case "transforms":
        return (
          <SettingsGroup id="transforms" footer={t("settings.globalTransformsHelp")}>
            <SettingBlock id="global_transforms">
              <TransformChainEditor
                value={currentSettings.global_transforms ?? []}
                registry={globalTransformRegistry}
                loading={transformRegistryLoading}
                onChange={(next) => handleChange({ global_transforms: next })}
              />
            </SettingBlock>
          </SettingsGroup>
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

  const renderCategory = (category: SettingsCategory) => (
    <SettingsCategorySection key={category.id} category={category}>
      {renderCategoryContent(category.id)}
    </SettingsCategorySection>
  );

  let content;
  if (!search) {
    const category = SETTINGS_CATEGORIES.find((item) => item.id === activeCategory)!;
    content = (
      <motion.div
        key={category.id}
        initial={reduceMotion ? { opacity: 0 } : { opacity: 0, y: 8 }}
        animate={{ opacity: 1, y: 0 }}
        transition={transitions.normal}
      >
        {renderCategory(category)}
      </motion.div>
    );
  } else {
    content = (
      <div className="flex flex-col gap-4">
        <p aria-live="polite" className="text-sm text-muted-foreground">
          {t("settings.searchResultCount", { count: search.entryIds.size })}
        </p>
        {search.entryIds.size === 0 ? (
          <EmptyState
            icon={<SearchX className="size-6" aria-hidden="true" />}
            title={t("settings.searchNoMatchTitle")}
            description={t("settings.searchNoMatchDescription", { query: query.trim() })}
          />
        ) : (
          <div className="flex flex-col gap-10">
            {SETTINGS_CATEGORIES.filter((category) =>
              search.countByCategory.has(category.id)
            ).map(renderCategory)}
          </div>
        )}
      </div>
    );
  }

  return (
    <PageWrapper className="flex min-w-0 flex-col gap-6">
      {header}
      <SettingsBody
        aside={
          <>
            <TableToolbarSearch
              type="search"
              value={query}
              onChange={(event) => handleQueryChange(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Escape") handleQueryChange("");
              }}
              placeholder={t("settings.searchPlaceholder")}
              aria-label={t("settings.searchPlaceholder")}
              autoComplete="off"
              containerClassName="sm:w-full"
            />
            <SettingsCategoryNav
              activeId={activeCategory}
              matchCounts={search?.countByCategory ?? null}
              onSelect={handleSelectCategory}
            />
          </>
        }
      >
        <SettingsSearchContext.Provider value={search?.entryIds ?? null}>
          <div ref={contentRef} className="min-w-0">
            {content}
          </div>
        </SettingsSearchContext.Provider>
        <SettingsSaveBar
          open={localSettings !== null || saving}
          saving={saving}
          onDiscard={() => setLocalSettings(null)}
          onSave={handleSave}
        />
      </SettingsBody>
    </PageWrapper>
  );
}
