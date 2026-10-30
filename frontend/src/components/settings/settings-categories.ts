/**
 * Category, group, and entry model for the system settings page.
 *
 * Order and ids are part of the UI contract defined in
 * `spec/system-settings-ui.spec.md` (SSU-1, SSU-1a, SSU-1b). Entries are the single
 * source of every row label, description, and numeric constraint, so search and the
 * change list always match what the page renders. Entry ids are linked from other pages
 * through `?setting=<id>` and MUST NOT be renamed.
 */
export const SETTINGS_CATEGORIES = [
  {
    id: "site",
    titleKey: "settings.categorySite",
    descriptionKey: "settings.categorySiteDescription",
  },
  {
    id: "access",
    titleKey: "settings.categoryAccess",
    descriptionKey: "settings.categoryAccessDescription",
  },
  {
    id: "models",
    titleKey: "settings.categoryModels",
    descriptionKey: "settings.categoryModelsDescription",
  },
  {
    id: "transforms",
    titleKey: "settings.categoryTransforms",
    descriptionKey: "settings.categoryTransformsDescription",
  },
  {
    id: "routing",
    titleKey: "settings.categoryRouting",
    descriptionKey: "settings.categoryRoutingDescription",
  },
  {
    id: "billing",
    titleKey: "settings.categoryBilling",
    descriptionKey: "settings.categoryBillingDescription",
  },
  {
    id: "privacy",
    titleKey: "settings.categoryPrivacy",
    descriptionKey: "settings.categoryPrivacyDescription",
  },
  {
    id: "compatibility",
    titleKey: "settings.categoryCompatibility",
    descriptionKey: "settings.categoryCompatibilityDescription",
  },
] as const;

export type SettingsCategory = (typeof SETTINGS_CATEGORIES)[number];
export type SettingsCategoryId = SettingsCategory["id"];

export const SETTINGS_CATEGORY_IDS = SETTINGS_CATEGORIES.map((category) => category.id);

/**
 * A row group renders its entries as rows on one `Card`; an editor group (`editor`)
 * renders one self-framed editor without a `Card` (SSU-1a).
 */
export interface SettingsGroup {
  category: SettingsCategoryId;
  titleKey?: string;
  descriptionKey?: string;
  editor?: true;
}

export const SETTINGS_GROUPS = {
  site: { category: "site" },
  "site.dashboard": {
    category: "site",
    titleKey: "settings.dashboardPerformanceTitle",
    editor: true,
  },
  "access.registration": { category: "access", titleKey: "settings.registration" },
  "access.session": { category: "access", titleKey: "settings.sessionSecurity" },
  "models.codex": {
    category: "models",
    titleKey: "settings.codexModels",
    descriptionKey: "settings.codexModelsDescription",
    editor: true,
  },
  "models.suffix": {
    category: "models",
    titleKey: "settings.reasoningSuffixMap",
    descriptionKey: "settings.reasoningSuffixMapDescription",
    editor: true,
  },
  "models.redirects": {
    category: "models",
    titleKey: "settings.globalModelRedirects",
    descriptionKey: "settings.globalModelRedirectsDescription",
    editor: true,
  },
  transforms: { category: "transforms", editor: true },
  "routing.affinity": {
    category: "routing",
    titleKey: "settings.affinityRouting",
    descriptionKey: "settings.affinityRoutingDescription",
  },
  "routing.probe": { category: "routing", titleKey: "settings.groupActiveProbe" },
  "routing.passive": { category: "routing", titleKey: "settings.groupPassiveBreaker" },
  "routing.timeouts": { category: "routing", titleKey: "settings.groupTimeouts" },
  billing: { category: "billing" },
  privacy: { category: "privacy" },
  "compatibility.extra": {
    category: "compatibility",
    titleKey: "settings.extraFieldsWhitelist",
    descriptionKey: "settings.extraFieldsWhitelistDescription",
  },
  "compatibility.protocol": {
    category: "compatibility",
    titleKey: "settings.groupProtocolConversion",
  },
} as const satisfies Record<string, SettingsGroup>;

export type SettingsGroupId = keyof typeof SETTINGS_GROUPS;

export interface SettingEntry {
  /** Row DOM id suffix; also the control id for single-control rows. */
  id: string;
  group: SettingsGroupId;
  /** i18n key of the visible label. */
  labelKey?: string;
  /** Untranslated label, rendered in the mono face (protocol type ids). */
  rawLabel?: string;
  descriptionKey?: string;
  /**
   * Extra searchable text keys: editor help text, and for editor entries the group
   * description, which is the only prose describing the editor.
   */
  helpKeys?: readonly string[];
  /** i18n key of search synonyms that appear in neither label nor description (SSU-16a). */
  keywordsKey?: string;
  /** Lower bound of a numeric entry (SSU-2b). */
  min?: number;
  /** i18n key of the unit suffix of a numeric entry (SSU-2b). */
  unitKey?: string;
  /** API field names, matched verbatim by search. */
  fields: readonly string[];
}

function extraFieldsEntry<K extends string>(subKey: K) {
  return {
    id: `extra_fields_${subKey}` as const,
    group: "compatibility.extra",
    rawLabel: subKey,
    helpKeys: ["settings.extraFieldsWhitelistHelp"],
    fields: ["monoize_extra_fields_whitelist", subKey],
  } as const;
}

export const SETTING_ENTRIES = [
  {
    id: "site_name",
    group: "site",
    labelKey: "settings.siteName",
    descriptionKey: "settings.siteNameDescription",
    keywordsKey: "settings.keywordsSiteName",
    fields: ["site_name"],
  },
  {
    id: "site_description",
    group: "site",
    labelKey: "settings.siteDescription",
    descriptionKey: "settings.siteDescriptionDescription",
    keywordsKey: "settings.keywordsSiteDescription",
    fields: ["site_description"],
  },
  {
    id: "api_base_url",
    group: "site",
    labelKey: "settings.apiBaseUrl",
    descriptionKey: "settings.apiBaseUrlDescription",
    fields: ["api_base_url"],
  },
  {
    id: "recharge_public_origin",
    group: "site",
    labelKey: "settings.rechargePublicOrigin",
    descriptionKey: "settings.rechargePublicOriginDescription",
    fields: ["recharge_public_origin"],
  },
  {
    id: "dashboard_performance_targets",
    group: "site.dashboard",
    helpKeys: [
      "settings.dashboardPerformanceDescription",
      "settings.dashboardPerformanceGroups",
      "settings.dashboardPerformanceModels",
    ],
    fields: ["dashboard_performance_group_ids", "dashboard_performance_model_ids"],
  },
  {
    id: "registration_enabled",
    group: "access.registration",
    labelKey: "settings.allowRegistration",
    descriptionKey: "settings.allowRegistrationDescription",
    keywordsKey: "settings.keywordsRegistration",
    fields: ["registration_enabled"],
  },
  {
    id: "captcha_enabled",
    group: "access.session",
    labelKey: "settings.captchaEnabled",
    descriptionKey: "settings.captchaEnabledDescription",
    keywordsKey: "settings.keywordsCaptcha",
    fields: ["captcha_enabled"],
  },
  {
    id: "session_ttl_days",
    group: "access.session",
    labelKey: "settings.sessionDuration",
    descriptionKey: "settings.sessionDurationDescription",
    keywordsKey: "settings.keywordsSession",
    min: 1,
    unitKey: "settings.unitDays",
    fields: ["session_ttl_days"],
  },
  {
    id: "api_key_max_per_user",
    group: "access.session",
    labelKey: "settings.maxApiKeys",
    descriptionKey: "settings.maxApiKeysDescription",
    keywordsKey: "settings.keywordsApiKeyLimit",
    min: 1,
    fields: ["api_key_max_per_user"],
  },
  {
    id: "codex_model_ids",
    group: "models.codex",
    helpKeys: ["settings.codexModelsDescription", "settings.codexModelsCompatibilityHelp"],
    fields: ["codex_model_ids"],
  },
  {
    id: "reasoning_suffix_map",
    group: "models.suffix",
    helpKeys: ["settings.reasoningSuffixMapDescription", "settings.effortValues"],
    keywordsKey: "settings.keywordsReasoning",
    fields: ["reasoning_suffix_map"],
  },
  {
    id: "global_model_redirects",
    group: "models.redirects",
    helpKeys: ["settings.globalModelRedirectsDescription", "settings.globalModelRedirectsHelp"],
    keywordsKey: "settings.keywordsRedirects",
    fields: ["global_model_redirects"],
  },
  {
    id: "global_transforms",
    group: "transforms",
    helpKeys: ["settings.categoryTransformsDescription", "settings.globalTransformsHelp"],
    fields: ["global_transforms"],
  },
  {
    id: "monoize_affinity_enabled",
    group: "routing.affinity",
    labelKey: "settings.affinityEnabled",
    descriptionKey: "settings.affinityEnabledDescription",
    keywordsKey: "settings.keywordsAffinity",
    fields: ["monoize_affinity_enabled"],
  },
  {
    id: "monoize_affinity_failback_mode",
    group: "routing.affinity",
    labelKey: "settings.affinityFailbackMode",
    descriptionKey: "settings.affinityFailbackModeDescription",
    fields: ["monoize_affinity_failback_mode"],
  },
  {
    id: "monoize_affinity_idle_ttl_seconds",
    group: "routing.affinity",
    labelKey: "settings.affinityIdleTtlSeconds",
    descriptionKey: "settings.affinityIdleTtlSecondsDescription",
    min: 1,
    unitKey: "settings.unitSeconds",
    fields: ["monoize_affinity_idle_ttl_seconds"],
  },
  {
    id: "monoize_affinity_failback_delay_seconds",
    group: "routing.affinity",
    labelKey: "settings.affinityFailbackDelaySeconds",
    descriptionKey: "settings.affinityFailbackDelaySecondsDescription",
    min: 0,
    unitKey: "settings.unitSeconds",
    fields: ["monoize_affinity_failback_delay_seconds"],
  },
  {
    id: "monoize_active_probe_enabled",
    group: "routing.probe",
    labelKey: "settings.activeProbeEnabled",
    descriptionKey: "settings.activeProbeEnabledDescription",
    keywordsKey: "settings.keywordsProbe",
    fields: ["monoize_active_probe_enabled"],
  },
  {
    id: "monoize_active_probe_interval_seconds",
    group: "routing.probe",
    labelKey: "settings.activeProbeIntervalSeconds",
    descriptionKey: "settings.activeProbeIntervalSecondsDescription",
    keywordsKey: "settings.keywordsProbe",
    min: 1,
    unitKey: "settings.unitSeconds",
    fields: ["monoize_active_probe_interval_seconds"],
  },
  {
    id: "monoize_active_probe_success_threshold",
    group: "routing.probe",
    labelKey: "settings.activeProbeSuccessThreshold",
    descriptionKey: "settings.activeProbeSuccessThresholdDescription",
    keywordsKey: "settings.keywordsProbe",
    min: 1,
    unitKey: "settings.unitTimes",
    fields: ["monoize_active_probe_success_threshold"],
  },
  {
    id: "monoize_active_probe_model",
    group: "routing.probe",
    labelKey: "settings.activeProbeModel",
    descriptionKey: "settings.activeProbeModelDescription",
    keywordsKey: "settings.keywordsProbe",
    fields: ["monoize_active_probe_model"],
  },
  {
    id: "monoize_passive_failure_threshold",
    group: "routing.passive",
    labelKey: "settings.passiveFailureThreshold",
    descriptionKey: "settings.passiveFailureThresholdDescription",
    keywordsKey: "settings.keywordsBreaker",
    min: 1,
    unitKey: "settings.unitTimes",
    fields: ["monoize_passive_failure_threshold"],
  },
  {
    id: "monoize_passive_cooldown_seconds",
    group: "routing.passive",
    labelKey: "settings.passiveCooldownSeconds",
    descriptionKey: "settings.passiveCooldownSecondsDescription",
    keywordsKey: "settings.keywordsBreaker",
    min: 1,
    unitKey: "settings.unitSeconds",
    fields: ["monoize_passive_cooldown_seconds"],
  },
  {
    id: "monoize_passive_window_seconds",
    group: "routing.passive",
    labelKey: "settings.passiveWindowSeconds",
    descriptionKey: "settings.passiveWindowSecondsDescription",
    keywordsKey: "settings.keywordsBreaker",
    min: 1,
    unitKey: "settings.unitSeconds",
    fields: ["monoize_passive_window_seconds"],
  },
  {
    id: "monoize_passive_rate_limit_cooldown_seconds",
    group: "routing.passive",
    labelKey: "settings.passiveRateLimitCooldownSeconds",
    descriptionKey: "settings.passiveRateLimitCooldownSecondsDescription",
    keywordsKey: "settings.keywordsBreaker",
    min: 1,
    unitKey: "settings.unitSeconds",
    fields: ["monoize_passive_rate_limit_cooldown_seconds"],
  },
  {
    id: "monoize_request_timeout_ms",
    group: "routing.timeouts",
    labelKey: "settings.requestTimeoutMs",
    descriptionKey: "settings.requestTimeoutMsDescription",
    keywordsKey: "settings.keywordsTimeout",
    min: 1,
    unitKey: "settings.unitMilliseconds",
    fields: ["monoize_request_timeout_ms"],
  },
  {
    id: "monoize_stream_idle_timeout_ms",
    group: "routing.timeouts",
    labelKey: "settings.streamIdleTimeoutMs",
    descriptionKey: "settings.streamIdleTimeoutMsDescription",
    keywordsKey: "settings.keywordsTimeout",
    min: 1,
    unitKey: "settings.unitMilliseconds",
    fields: ["monoize_stream_idle_timeout_ms"],
  },
  {
    id: "monoize_enable_estimated_billing",
    group: "billing",
    labelKey: "settings.enableEstimatedBilling",
    descriptionKey: "settings.enableEstimatedBillingDescription",
    keywordsKey: "settings.keywordsBilling",
    fields: ["monoize_enable_estimated_billing"],
  },
  {
    id: "allow_free_when_unpriced",
    group: "billing",
    labelKey: "settings.allowFreeWhenUnpriced",
    descriptionKey: "settings.allowFreeWhenUnpricedDescription",
    keywordsKey: "settings.keywordsBilling",
    fields: ["allow_free_when_unpriced"],
  },
  {
    id: "allow_free_when_missing_usage",
    group: "billing",
    labelKey: "settings.allowFreeWhenMissingUsage",
    descriptionKey: "settings.allowFreeWhenMissingUsageDescription",
    keywordsKey: "settings.keywordsBilling",
    fields: ["allow_free_when_missing_usage"],
  },
  {
    id: "monoize_request_capture_enabled",
    group: "privacy",
    labelKey: "settings.requestCaptureEnabled",
    descriptionKey: "settings.requestCaptureEnabledDescription",
    keywordsKey: "settings.keywordsCapture",
    fields: ["monoize_request_capture_enabled"],
  },
  {
    id: "monoize_request_capture_max_total_bytes",
    group: "privacy",
    labelKey: "settings.requestCaptureMaxTotalMib",
    descriptionKey: "settings.requestCaptureMaxTotalMibDescription",
    keywordsKey: "settings.keywordsCapture",
    min: 0,
    unitKey: "settings.unitMib",
    fields: ["monoize_request_capture_max_total_bytes"],
  },
  {
    id: "monoize_mask_sensitive_info",
    group: "privacy",
    labelKey: "settings.maskSensitiveInfo",
    descriptionKey: "settings.maskSensitiveInfoDescription",
    keywordsKey: "settings.keywordsMask",
    fields: ["monoize_mask_sensitive_info"],
  },
  extraFieldsEntry("chat_completion"),
  extraFieldsEntry("responses"),
  extraFieldsEntry("messages"),
  extraFieldsEntry("gemini"),
  {
    id: "monoize_strip_cross_protocol_nested_extra",
    group: "compatibility.protocol",
    labelKey: "settings.stripCrossProtocolNestedExtra",
    descriptionKey: "settings.stripCrossProtocolNestedExtraDescription",
    fields: ["monoize_strip_cross_protocol_nested_extra"],
  },
] as const satisfies readonly SettingEntry[];

export type SettingEntryId = (typeof SETTING_ENTRIES)[number]["id"];

const ENTRY_BY_ID = new Map<string, SettingEntry>(
  SETTING_ENTRIES.map((entry) => [entry.id, entry])
);

export function getSettingEntry(id: SettingEntryId): SettingEntry {
  return ENTRY_BY_ID.get(id)!;
}

export function findSettingEntry(id: string): SettingEntry | undefined {
  return ENTRY_BY_ID.get(id);
}

export function categoryOfGroup(group: SettingsGroupId): SettingsCategoryId {
  return SETTINGS_GROUPS[group].category;
}
