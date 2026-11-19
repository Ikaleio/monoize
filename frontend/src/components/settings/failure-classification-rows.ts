import type { FailureClassificationRule } from "@/lib/api";

export const CLASSIFICATION_RULE_LIMIT = 100;
export const FAILURE_CLASSES = ["persistent", "rate_limited", "transient"] as const;

export interface ClassificationRow {
  id: number;
  status_codes: string;
  keyword: string;
  class: FailureClassificationRule["class"];
}

export interface ClassificationRuleLocation {
  index: number;
  field?: keyof FailureClassificationRule;
}

export function classificationInputId(index: number, field: ClassificationRuleLocation["field"] = "status_codes") {
  return `failure-classification-${index}-${field}`;
}

let rowId = 0;

export function createClassificationRow(rule?: FailureClassificationRule): ClassificationRow {
  return {
    id: ++rowId,
    status_codes: rule?.status_codes.join(", ") ?? "",
    keyword: rule?.keyword ?? "",
    class: rule?.class ?? "persistent",
  };
}

function trimWhitespace(value: string): string {
  return value.replace(/^\p{White_Space}+|\p{White_Space}+$/gu, "");
}

function parseStatusCodes(text: string): number[] | null {
  if (!trimWhitespace(text)) return [];
  const codes = new Set<number>();
  for (const raw of text.split(",")) {
    const token = trimWhitespace(raw);
    if (!/^[+-]?\d+$/.test(token)) return null;
    const code = Number(token);
    if (code < 100 || code > 599) return null;
    codes.add(code);
  }
  return [...codes];
}

export type ClassificationRowErrors = Partial<Record<keyof FailureClassificationRule, boolean>>;

export interface ParsedClassificationDraft {
  rules: FailureClassificationRule[] | null;
  errors: ClassificationRowErrors[];
  rowCount: number;
}

export function parseClassificationRows(rows: readonly ClassificationRow[]): ParsedClassificationDraft {
  let valid = rows.length <= CLASSIFICATION_RULE_LIMIT;
  const rules: FailureClassificationRule[] = [];
  const errors = rows.map(({ status_codes, keyword, class: failureClass }): ClassificationRowErrors => {
    const codes = parseStatusCodes(status_codes);
    const trimmed = trimWhitespace(keyword);
    const invalidKeyword = !trimmed || [...trimmed].length > 200;
    const invalidClass = !FAILURE_CLASSES.includes(failureClass);
    if (codes === null || invalidKeyword || invalidClass) {
      valid = false;
      return { status_codes: codes === null, keyword: invalidKeyword, class: invalidClass };
    }
    rules.push({ status_codes: codes, keyword: trimmed, class: failureClass });
    return {};
  });
  return { rules: valid ? rules : null, errors, rowCount: rows.length };
}
