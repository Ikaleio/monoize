/**
 * Resolves one display string out of a localized-text object per
 * transform-config-ui.spec.md TCU-2.
 *
 * Resolution order: exact language key, base language before the first "-",
 * "en", the lexicographically smallest key, then the provided fallback
 * (normally the transform type_id).
 */
export function resolveLocalizedText(
  map: Record<string, string> | null | undefined,
  language: string,
  fallback: string
): string {
  if (!map || typeof map !== "object") {
    return fallback;
  }
  const exact = map[language];
  if (typeof exact === "string" && exact.length > 0) {
    return exact;
  }
  const separator = language.indexOf("-");
  if (separator > 0) {
    const base = map[language.slice(0, separator)];
    if (typeof base === "string" && base.length > 0) {
      return base;
    }
  }
  if (typeof map.en === "string" && map.en.length > 0) {
    return map.en;
  }
  const keys = Object.keys(map)
    .filter((key) => typeof map[key] === "string" && map[key].length > 0)
    .sort();
  if (keys.length > 0) {
    return map[keys[0]];
  }
  return fallback;
}
