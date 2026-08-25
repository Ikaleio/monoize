/**
 * Category model for the system settings page.
 *
 * Order and ids are part of the UI contract defined in
 * `spec/system-settings-ui.spec.md` (SSU-1): the horizontal rail renders these
 * entries in array order and numbers them 01..09.
 */
export const SETTINGS_CATEGORIES = [
  {
    id: "site",
    titleKey: "settings.siteInformation",
    descriptionKey: "settings.siteInfoDescription",
  },
  {
    id: "access",
    titleKey: "settings.accessControl",
    descriptionKey: "settings.accessControlDescription",
  },
  {
    id: "codex",
    titleKey: "settings.codexModels",
    descriptionKey: "settings.codexModelsDescription",
  },
  {
    id: "suffix",
    titleKey: "settings.reasoningSuffixMap",
    descriptionKey: "settings.reasoningSuffixMapDescription",
  },
  {
    id: "redirects",
    titleKey: "settings.globalModelRedirects",
    descriptionKey: "settings.globalModelRedirectsDescription",
  },
  {
    id: "transforms",
    titleKey: "settings.globalTransforms",
    descriptionKey: "settings.globalTransformsDescription",
  },
  {
    id: "affinity",
    titleKey: "settings.affinityRouting",
    descriptionKey: "settings.affinityRoutingDescription",
  },
  {
    id: "health",
    titleKey: "settings.healthMonitoring",
    descriptionKey: "settings.healthMonitoringDescription",
  },
  {
    id: "extra",
    titleKey: "settings.extraFieldsWhitelist",
    descriptionKey: "settings.extraFieldsWhitelistDescription",
  },
] as const;

export type SettingsCategory = (typeof SETTINGS_CATEGORIES)[number];
export type SettingsCategoryId = SettingsCategory["id"];

export function categoryIndexLabel(index: number): string {
  return String(index + 1).padStart(2, "0");
}
