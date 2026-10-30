import type { TFunction } from "i18next";

import type { ModelRedirectRule, SystemSettings } from "@/lib/api";
import {
  SETTING_ENTRIES,
  SETTINGS_CATEGORIES,
  SETTINGS_GROUPS,
  findSettingEntry,
  type SettingEntry,
  type SettingEntryId,
  type SettingsGroup,
} from "./settings-categories";

const ENTRIES: readonly SettingEntry[] = SETTING_ENTRIES;
const EXTRA_FIELDS_PREFIX = "extra_fields_";
const BYTES_PER_MIB = 1048576;

export function isBlankRedirect(rule: ModelRedirectRule): boolean {
  return !rule.pattern.trim() || !rule.replace.trim();
}

function isDeepEqual(a: unknown, b: unknown): boolean {
  if (Object.is(a, b)) return true;
  if (typeof a !== "object" || typeof b !== "object" || a === null || b === null) return false;
  if (Array.isArray(a) !== Array.isArray(b)) return false;
  const aRecord = a as Record<string, unknown>;
  const bRecord = b as Record<string, unknown>;
  const aKeys = Object.keys(aRecord);
  return (
    aKeys.length === Object.keys(bRecord).length &&
    aKeys.every((key) => key in bRecord && isDeepEqual(aRecord[key], bRecord[key]))
  );
}

export function settingsDeepEqual(a: SystemSettings, b: SystemSettings): boolean {
  return isDeepEqual(a, b);
}

/**
 * SSU-22: top-level keys whose draft value differs from the server snapshot. Keys edited
 * elsewhere (`tool_prices`, price sync) are never sent from a stale draft.
 */
export function settingsPatch(server: SystemSettings, draft: SystemSettings): Partial<SystemSettings> {
  const patch: Record<string, unknown> = {};
  for (const key of Object.keys(draft) as (keyof SystemSettings)[]) {
    if (key === "updated_at") continue;
    if (!isDeepEqual(draft[key], server[key])) patch[key] = draft[key];
  }
  return patch as Partial<SystemSettings>;
}

/** One entry's value with SSU-20a normalization applied. */
function normalizedValue(entry: SettingEntry, settings: SystemSettings): unknown {
  if (entry.id.startsWith(EXTRA_FIELDS_PREFIX)) {
    return settings.monoize_extra_fields_whitelist?.[entry.fields[1]] ?? [];
  }
  if (entry.id === "global_model_redirects") {
    return (settings.global_model_redirects ?? []).filter((rule) => !isBlankRedirect(rule));
  }
  if (entry.id === "monoize_active_probe_model") {
    return settings.monoize_active_probe_model || null;
  }
  return entry.fields.map((field) => settings[field as keyof SystemSettings] ?? null);
}

function formatScalar(value: unknown, unit: string | undefined, t: TFunction): string {
  if (value === null || value === undefined || value === "") return "—";
  if (typeof value === "boolean") return t(value ? "settings.valueOn" : "settings.valueOff");
  if (typeof value === "number") return unit ? `${value} ${unit}` : String(value);
  return String(value);
}

function formatValue(entry: SettingEntry, value: unknown, t: TFunction): string {
  // Whitelists are short identifiers, so the change list can show them verbatim.
  if (entry.id.startsWith(EXTRA_FIELDS_PREFIX)) {
    const fields = value as string[];
    return fields.length > 0 ? fields.join(", ") : "—";
  }
  const count = (size: number) => t("settings.valueItemCount", { count: size });
  if (entry.id === "global_model_redirects") return count((value as unknown[]).length);
  if (entry.id === "monoize_active_probe_model") return formatScalar(value, undefined, t);
  if (entry.id === "dashboard_performance_targets") {
    return count((value as unknown[][]).reduce((total, ids) => total + (ids ?? []).length, 0));
  }

  const [single] = value as unknown[];
  if (Array.isArray(single)) return count(single.length);
  if (typeof single === "object" && single !== null) return count(Object.keys(single).length);
  if (entry.id === "monoize_request_capture_max_total_bytes") {
    return formatScalar(Math.round((single as number) / BYTES_PER_MIB), t("settings.unitMib"), t);
  }
  return formatScalar(single, entry.unitKey ? t(entry.unitKey) : undefined, t);
}

export function settingEntryLabel(entry: SettingEntry, t: TFunction): string {
  if (entry.labelKey) return t(entry.labelKey);
  if (entry.rawLabel) return entry.rawLabel;
  const group: SettingsGroup = SETTINGS_GROUPS[entry.group];
  if (group.titleKey) return t(group.titleKey);
  return t(SETTINGS_CATEGORIES.find((category) => category.id === group.category)!.titleKey);
}

export interface SettingChange {
  entryId: SettingEntryId;
  label: string;
  oldValue: string;
  newValue: string;
}

/**
 * Entries that differ from the server (SSU-20a), in entry order. `suffixRowCount` is the
 * draft suffix row count, or null when the rows were not edited: blank and duplicate rows
 * never reach the map, yet they still count as a change.
 */
export function diffSettings(
  server: SystemSettings,
  draft: SystemSettings,
  suffixRowCount: number | null,
  t: TFunction
): SettingChange[] {
  const changes: SettingChange[] = [];
  for (const entry of ENTRIES) {
    const before = normalizedValue(entry, server);
    const after = normalizedValue(entry, draft);
    const label = settingEntryLabel(entry, t);
    const entryId = entry.id as SettingEntryId;
    if (entry.id === "reasoning_suffix_map" && suffixRowCount !== null) {
      const serverCount = Object.keys(server.reasoning_suffix_map ?? {}).length;
      if (suffixRowCount !== serverCount || !isDeepEqual(before, after)) {
        changes.push({
          entryId,
          label,
          oldValue: t("settings.valueItemCount", { count: serverCount }),
          newValue: t("settings.valueItemCount", { count: suffixRowCount }),
        });
      }
      continue;
    }
    if (!isDeepEqual(before, after)) {
      changes.push({
        entryId,
        label,
        oldValue: formatValue(entry, before, t),
        newValue: formatValue(entry, after, t),
      });
    }
  }
  return changes;
}

const HIGH_RISK_CHANGES = [
  { id: "registration_enabled", riskyValue: true, riskKey: "settings.riskRegistration" },
  { id: "captcha_enabled", riskyValue: false, riskKey: "settings.riskCaptcha" },
  { id: "monoize_mask_sensitive_info", riskyValue: false, riskKey: "settings.riskMask" },
  { id: "allow_free_when_unpriced", riskyValue: true, riskKey: "settings.riskFreeUnpriced" },
  { id: "allow_free_when_missing_usage", riskyValue: true, riskKey: "settings.riskFreeMissingUsage" },
  { id: "monoize_request_capture_enabled", riskyValue: true, riskKey: "settings.riskCapture" },
] as const;

export interface HighRiskChange {
  entryId: SettingEntryId;
  label: string;
  risk: string;
}

/** SSU-22a: a change counts only when the server value differs. */
export function highRiskChanges(
  server: SystemSettings,
  draft: SystemSettings,
  t: TFunction
): HighRiskChange[] {
  return HIGH_RISK_CHANGES.filter(
    ({ id, riskyValue }) => draft[id] === riskyValue && server[id] !== riskyValue
  ).map(({ id, riskKey }) => ({
    entryId: id,
    label: settingEntryLabel(findSettingEntry(id)!, t),
    risk: t(riskKey),
  }));
}

export interface ResolvedSettingParam {
  entryId: SettingEntryId;
  /** Row in the rules sent to the server, when the param names a redirect rule. */
  redirectIndex?: number;
}

/** SSU-28: maps a backend `param` to the entry that edits it. */
export function resolveSettingParam(param: string): ResolvedSettingParam | null {
  const redirect = /^global_model_redirects\[(\d+)\]/.exec(param);
  if (redirect) {
    return { entryId: "global_model_redirects", redirectIndex: Number(redirect[1]) };
  }
  const entry = ENTRIES.find((item) => item.id === param || item.fields.includes(param));
  return entry ? { entryId: entry.id as SettingEntryId } : null;
}
