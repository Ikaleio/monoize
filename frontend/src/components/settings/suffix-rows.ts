export interface SuffixRow {
  id: number;
  suffix: string;
  effort: string;
}

let suffixRowId = 0;

export function createSuffixRow(suffix = "", effort = "high"): SuffixRow {
  return { id: ++suffixRowId, suffix, effort };
}

export function suffixMapToRows(map: Record<string, string> | undefined): SuffixRow[] {
  return Object.entries(map ?? {}).map(([suffix, effort]) => createSuffixRow(suffix, effort));
}

/** Blank rows stay in the draft rows but never reach the saved map (SSU-20c). */
export function suffixRowsToMap(rows: SuffixRow[]): Record<string, string> {
  const map: Record<string, string> = {};
  for (const row of rows) {
    const suffix = row.suffix.trim();
    if (suffix) map[suffix] = row.effort;
  }
  return map;
}

/** Ids of rows whose trimmed suffix repeats an earlier row. */
export function duplicateSuffixRowIds(rows: SuffixRow[]): ReadonlySet<number> {
  const seen = new Set<string>();
  const duplicates = new Set<number>();
  for (const row of rows) {
    const suffix = row.suffix.trim();
    if (!suffix) continue;
    if (seen.has(suffix)) duplicates.add(row.id);
    seen.add(suffix);
  }
  return duplicates;
}
