import { createContext, useContext } from "react";
import type { TFunction } from "i18next";

import {
  SETTING_ENTRIES,
  SETTINGS_CATEGORIES,
  SETTINGS_GROUPS,
  type SettingEntry,
  type SettingsGroup,
  type SettingsCategoryId,
  type SettingsGroupId,
} from "./settings-categories";

const ENTRIES: readonly SettingEntry[] = SETTING_ENTRIES;

const ENTRY_IDS_BY_GROUP = new Map<SettingsGroupId, string[]>();
for (const entry of ENTRIES) {
  ENTRY_IDS_BY_GROUP.set(entry.group, [...(ENTRY_IDS_BY_GROUP.get(entry.group) ?? []), entry.id]);
}

export interface SettingsSearchResult {
  entryIds: ReadonlySet<string>;
  countByCategory: ReadonlyMap<SettingsCategoryId, number>;
}

export function normalizeSettingsQuery(query: string): string {
  return query.trim().toLocaleLowerCase();
}

/**
 * Matches entries whose search text contains every whitespace-separated token of
 * `normalizedQuery` (SSU-16). The search text covers translated labels, descriptions,
 * help text, group and category titles, plus raw API field names.
 */
export function matchSettingEntries(normalizedQuery: string, t: TFunction): SettingsSearchResult {
  const tokens = normalizedQuery.split(/\s+/);
  const entryIds = new Set<string>();
  const countByCategory = new Map<SettingsCategoryId, number>();

  for (const entry of ENTRIES) {
    const group: SettingsGroup = SETTINGS_GROUPS[entry.group];
    const category = SETTINGS_CATEGORIES.find((item) => item.id === group.category)!;
    const text = [
      entry.labelKey && t(entry.labelKey),
      entry.rawLabel,
      entry.descriptionKey && t(entry.descriptionKey),
      ...(entry.helpKeys ?? []).map((key) => t(key)),
      group.titleKey && t(group.titleKey),
      t(category.titleKey),
      ...entry.fields,
    ]
      .filter(Boolean)
      .join("\n")
      .toLocaleLowerCase();

    if (tokens.every((token) => text.includes(token))) {
      entryIds.add(entry.id);
      countByCategory.set(group.category, (countByCategory.get(group.category) ?? 0) + 1);
    }
  }

  return { entryIds, countByCategory };
}

/** Matching entry ids while searching; `null` in browse mode, where every entry renders. */
export const SettingsSearchContext = createContext<ReadonlySet<string> | null>(null);

export function useEntryVisible(id: string): boolean {
  const matches = useContext(SettingsSearchContext);
  return matches === null || matches.has(id);
}

export function useGroupVisible(group: SettingsGroupId): boolean {
  const matches = useContext(SettingsSearchContext);
  return matches === null || (ENTRY_IDS_BY_GROUP.get(group) ?? []).some((id) => matches.has(id));
}
