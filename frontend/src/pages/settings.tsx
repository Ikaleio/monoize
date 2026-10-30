import { useCallback, useContext, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useSearchParams } from "react-router-dom";
import { SearchX } from "lucide-react";
import { toast } from "sonner";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { buttonVariants } from "@/components/ui/button";
import { TableToolbarSearch } from "@/components/ui/data-table-shell";
import { EmptyState } from "@/components/ui/empty-state";
import { PageWrapper, motion, transitions } from "@/components/ui/motion";
import { PageHeader } from "@/components/ui/page-header";
import { QueryError } from "@/components/ui/query-error";
import { useReducedMotionPreference } from "@/hooks/use-reduced-motion";
import { useSearchParamState } from "@/hooks/use-search-param-state";
import { useUnsavedChangesGuard } from "@/hooks/use-unsaved-changes-guard";
import { apiErrorParam, toastApiError } from "@/lib/api-error";
import { DashboardScrollParentContext } from "@/lib/dashboard-scroll";
import {
  useProviders,
  useSettings,
  patchSettingsOptimistic,
  useTransformRegistry,
} from "@/lib/swr";
import type { SystemSettings } from "@/lib/api";
import { TransformChainEditor } from "@/components/transforms/transform-chain-editor";
import { revealTransformRule } from "@/components/transforms/transform-layer";
import {
  findFirstInvalidTransformRule,
  formatTransformRuleError,
} from "@/components/transforms/transform-schema";
import { ModelMultiSelect } from "@/components/models/model-multi-select";
import { ModelRedirectsEditor } from "@/components/settings/model-redirects-editor";
import { SuffixMapEditor } from "@/components/settings/suffix-map-editor";
import {
  duplicateSuffixRowIds,
  suffixMapToRows,
  suffixRowsToMap,
  type SuffixRow,
} from "@/components/settings/suffix-rows";
import { isRedirectPatternInvalid } from "@/components/settings/redirect-pattern";
import {
  SETTINGS_CATEGORIES,
  SETTINGS_CATEGORY_IDS,
  categoryOfGroup,
  findSettingEntry,
  getSettingEntry,
  type SettingEntryId,
  type SettingsCategory,
  type SettingsCategoryId,
} from "@/components/settings/settings-categories";
import {
  SettingsRowStateContext,
  type SettingsFieldError,
} from "@/components/settings/settings-context";
import {
  diffSettings,
  highRiskChanges,
  isBlankRedirect,
  resolveSettingParam,
  settingEntryLabel,
  settingsDeepEqual,
  settingsPatch,
  type HighRiskChange,
} from "@/components/settings/settings-diff";
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
import { RoutingSection } from "@/components/settings/routing-section";
import { BillingSection } from "@/components/settings/billing-section";
import { PrivacySection } from "@/components/settings/privacy-section";
import { CompatibilitySection } from "@/components/settings/compatibility-section";

const HIGHLIGHT_MS = 2000;

export function SettingsPage() {
  const { t, i18n } = useTranslation();
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
  // SSU-20: draft rows, because blank and duplicate rows cannot be stored in the map.
  const [suffixRows, setSuffixRows] = useState<SuffixRow[] | null>(null);
  const [saving, setSaving] = useState(false);
  const [activeCategory, setActiveCategory] = useSearchParamState<SettingsCategoryId>(
    "category",
    "site",
    SETTINGS_CATEGORY_IDS
  );
  const [searchParams, setSearchParams] = useSearchParams();
  const settingParam = searchParams.get("setting");
  const [query, setQuery] = useState("");
  const [fieldError, setFieldError] = useState<SettingsFieldError | null>(null);
  const [invalidEntryIds, setInvalidEntryIds] = useState<ReadonlySet<string>>(new Set());
  const [jump, setJump] = useState<{
    id: SettingEntryId;
    focus: boolean;
    transformRuleIndex?: number;
  } | null>(null);
  const [highlight, setHighlight] = useState<{ id: string } | null>(null);
  const [pendingRisks, setPendingRisks] = useState<HighRiskChange[] | null>(null);
  // Remounting the content on discard and save drops raw NumberField text and blur state.
  const [draftGeneration, setDraftGeneration] = useState(0);

  // Use local state if user has made changes, otherwise use SWR data
  const currentSettings = localSettings ?? settings;
  const serverSuffixMap = currentSettings?.reasoning_suffix_map;
  const initialSuffixRows = useMemo(() => suffixMapToRows(serverSuffixMap), [serverSuffixMap]);
  const renderedSuffixRows = suffixRows ?? initialSuffixRows;
  const changes = useMemo(
    () =>
      settings && (localSettings || suffixRows)
        ? diffSettings(settings, localSettings ?? settings, suffixRows ? suffixRows.length : null, t)
        : [],
    [settings, localSettings, suffixRows, t]
  );
  const dirty = changes.length > 0;
  const guard = useUnsavedChangesGuard(dirty);
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

  // SSU-9b: the category param commits asynchronously through the router, so the jump
  // stays pending until the target row is in the DOM.
  useEffect(() => {
    if (!jump) return;
    const row = document.getElementById(`setting-${jump.id}`);
    if (!row) return;
    if (jump.transformRuleIndex !== undefined) {
      revealTransformRule("global", jump.transformRuleIndex);
    } else {
      row.scrollIntoView({ block: "center", behavior: reduceMotion ? "auto" : "smooth" });
    }
    if (jump.focus) {
      row.querySelector<HTMLElement>("input, textarea, button")?.focus({ preventScroll: true });
    }
    setHighlight({ id: jump.id });
    setJump(null);
  }, [jump, activeCategory, normalizedQuery, isLoading, reduceMotion]);

  useEffect(() => {
    if (!highlight) return;
    const timer = window.setTimeout(() => setHighlight(null), HIGHLIGHT_MS);
    return () => window.clearTimeout(timer);
  }, [highlight]);

  useEffect(() => {
    if (!settingParam) return;
    const entry = findSettingEntry(settingParam);
    const category = entry ? categoryOfGroup(entry.group) : null;
    setSearchParams(
      (current) => {
        const next = new URLSearchParams(current);
        next.delete("setting");
        if (category === "site") next.delete("category");
        else if (category) next.set("category", category);
        return next;
      },
      { replace: true }
    );
    if (entry) setJump({ id: entry.id as SettingEntryId, focus: false });
  }, [settingParam, setSearchParams]);

  const jumpToSetting = (id: SettingEntryId, focus = false, transformRuleIndex?: number) => {
    if (search && !search.entryIds.has(id)) setQuery("");
    setActiveCategory(categoryOfGroup(getSettingEntry(id).group));
    setJump({ id, focus, transformRuleIndex });
  };

  const reportValidity = useCallback((entryId: string, valid: boolean) => {
    setInvalidEntryIds((previous) => {
      if (previous.has(entryId) !== valid) return previous;
      const next = new Set(previous);
      if (valid) next.delete(entryId);
      else next.add(entryId);
      return next;
    });
  }, []);

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
    setFieldError(null);
    const next = { ...currentSettings, ...updates };
    setLocalSettings(settings && settingsDeepEqual(next, settings) ? null : next);
  };

  const handleSuffixRowsChange = (rows: SuffixRow[]) => {
    setSuffixRows(rows);
    handleChange({ reasoning_suffix_map: suffixRowsToMap(rows) });
  };

  const resetDraft = () => {
    setLocalSettings(null);
    setSuffixRows(null);
    setFieldError(null);
    setDraftGeneration((generation) => generation + 1);
  };

  const persist = async () => {
    if (!currentSettings || !settings) return;
    setSaving(true);
    // SSU-28: the server indexes the rules it received, not the draft rows.
    const sentRedirectRows = (currentSettings.global_model_redirects ?? []).flatMap((rule, row) =>
      isBlankRedirect(rule) ? [] : [row]
    );
    const patch = settingsPatch(settings, {
      ...currentSettings,
      global_model_redirects: (currentSettings.global_model_redirects ?? []).filter(
        (rule) => !isBlankRedirect(rule)
      ),
    });
    try {
      // A draft of only blank rows has nothing to send.
      if (Object.keys(patch).length > 0) await patchSettingsOptimistic(patch);
      resetDraft();
      toast.success(t("settings.saved"));
      mutate();
    } catch (error) {
      const param = apiErrorParam(error);
      const resolved = param ? resolveSettingParam(param) : null;
      if (!resolved) {
        toastApiError(error, t, t("settings.failedSave"));
      } else {
        const redirectIndex =
          resolved.redirectIndex === undefined ? undefined : sentRedirectRows[resolved.redirectIndex];
        const message =
          redirectIndex === undefined
            ? t("settings.invalidSettingValue", {
                label: settingEntryLabel(getSettingEntry(resolved.entryId), t),
              })
            : t("settings.invalidRedirectRule", { index: redirectIndex + 1 });
        setFieldError({ entryId: resolved.entryId, message, redirectIndex });
        toast.error(message);
        // The redirect editor focuses its own row.
        jumpToSetting(resolved.entryId, redirectIndex === undefined);
      }
    } finally {
      setSaving(false);
    }
  };

  const handleSave = () => {
    if (!currentSettings || !settings) return;
    const transformRules = currentSettings.global_transforms ?? [];
    const invalidRule = findFirstInvalidTransformRule(transformRules, globalTransformRegistry);
    if (invalidRule) {
      const message = formatTransformRuleError(
        invalidRule,
        transformRules,
        globalTransformRegistry,
        t,
        i18n.language
      );
      toast.error(message);
      setFieldError({ entryId: "global_transforms", message });
      jumpToSetting("global_transforms", false, invalidRule.index);
      return;
    }
    const risks = highRiskChanges(settings, currentSettings, t);
    if (risks.length > 0) {
      setPendingRisks(risks);
      return;
    }
    void persist();
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
        return (
          <SiteSection
            settings={currentSettings}
            onChange={handleChange}
            availableModelIds={availableCodexModelIds}
            modelsLoading={providersLoading}
            modelsError={providersError}
            onRetryModels={() => void mutateProviders()}
          />
        );
      case "access":
        return <AccessSection settings={currentSettings} onChange={handleChange} />;
      case "models":
        return (
          <>
            <SettingsGroup id="models.codex">
              <SettingBlock id="codex_model_ids">
                <div className="flex flex-col gap-4">
                  <ModelMultiSelect
                    value={currentSettings.codex_model_ids ?? []}
                    options={availableCodexModelIds}
                    loading={providersLoading}
                    error={providersError}
                    onRetry={() => mutateProviders()}
                    onChange={(codex_model_ids) => handleChange({ codex_model_ids })}
                    label={t("settings.codexModelsLegend")}
                  />
                  <p className="text-pretty text-sm leading-6 text-muted-foreground">
                    {t("settings.codexModelsCompatibilityHelp")}
                  </p>
                </div>
              </SettingBlock>
            </SettingsGroup>
            <SettingsGroup id="models.suffix">
              <SettingBlock id="reasoning_suffix_map">
                <SuffixMapEditor rows={renderedSuffixRows} onRowsChange={handleSuffixRowsChange} />
              </SettingBlock>
            </SettingsGroup>
            <SettingsGroup id="models.redirects">
              <SettingBlock id="global_model_redirects">
                <ModelRedirectsEditor
                  value={currentSettings.global_model_redirects ?? []}
                  disabled={saving}
                  emptyText={t("settings.globalModelRedirectsEmpty")}
                  serverErrorIndex={fieldError?.redirectIndex}
                  onChange={(global_model_redirects) => handleChange({ global_model_redirects })}
                />
              </SettingBlock>
            </SettingsGroup>
          </>
        );
      case "transforms":
        return (
          <SettingsGroup id="transforms" footer={t("settings.globalTransformsHelp")}>
            <SettingBlock id="global_transforms">
              <TransformChainEditor
                layer="global"
                value={currentSettings.global_transforms ?? []}
                registry={globalTransformRegistry}
                loading={transformRegistryLoading}
                onChange={(next) => handleChange({ global_transforms: next })}
              />
            </SettingBlock>
          </SettingsGroup>
        );
      case "routing":
        return <RoutingSection settings={currentSettings} onChange={handleChange} />;
      case "billing":
        return <BillingSection settings={currentSettings} onChange={handleChange} />;
      case "privacy":
        return <PrivacySection settings={currentSettings} onChange={handleChange} />;
      case "compatibility":
        return <CompatibilitySection settings={currentSettings} onChange={handleChange} />;
    }
  };

  const renderCategory = (category: SettingsCategory) => (
    <SettingsCategorySection key={category.id} category={category}>
      {renderCategoryContent(category.id)}
    </SettingsCategorySection>
  );

  const hasDuplicateSuffix = duplicateSuffixRowIds(renderedSuffixRows).size > 0;
  const errorCategories = (() => {
    const ids = new Set<SettingsCategoryId>();
    for (const id of invalidEntryIds) {
      const entry = findSettingEntry(id);
      if (entry) ids.add(categoryOfGroup(entry.group));
    }
    if (fieldError) {
      const entry = findSettingEntry(fieldError.entryId);
      if (entry) ids.add(categoryOfGroup(entry.group));
    }
    if (
      hasDuplicateSuffix ||
      (currentSettings.global_model_redirects ?? []).some((rule) =>
        isRedirectPatternInvalid(rule.pattern)
      )
    ) {
      ids.add("models");
    }
    return ids;
  })();
  const blockedReason =
    invalidEntryIds.size > 0 || hasDuplicateSuffix ? t("settings.saveBlockedInvalid") : null;

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
              errorCategories={errorCategories}
              onSelect={handleSelectCategory}
            />
          </>
        }
      >
        <SettingsSearchContext.Provider value={search?.entryIds ?? null}>
          <SettingsRowStateContext.Provider
            value={{ highlightId: highlight?.id ?? null, fieldError, reportValidity }}
          >
            <div key={draftGeneration} ref={contentRef} className="min-w-0">
              {content}
            </div>
          </SettingsRowStateContext.Provider>
        </SettingsSearchContext.Provider>
        <SettingsSaveBar
          open={dirty || saving}
          saving={saving}
          changes={changes}
          blockedReason={blockedReason}
          onDiscard={resetDraft}
          onSave={handleSave}
          onJump={(id) => jumpToSetting(id)}
        />
      </SettingsBody>
      <AlertDialog
        open={pendingRisks !== null}
        onOpenChange={(open) => {
          if (!open) setPendingRisks(null);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("settings.riskConfirmTitle")}</AlertDialogTitle>
            <AlertDialogDescription>{t("settings.riskConfirmDescription")}</AlertDialogDescription>
          </AlertDialogHeader>
          <ul className="flex flex-col gap-3 text-sm">
            {(pendingRisks ?? []).map((risk) => (
              <li key={risk.entryId} className="flex flex-col gap-0.5">
                <span className="font-medium">{risk.label}</span>
                <span className="text-muted-foreground">{risk.risk}</span>
              </li>
            ))}
          </ul>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("settings.riskConfirmBack")}</AlertDialogCancel>
            <AlertDialogAction
              className={buttonVariants({ variant: "destructive" })}
              onClick={() => void persist()}
            >
              {t("settings.riskConfirmSave")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
      {guard}
    </PageWrapper>
  );
}
