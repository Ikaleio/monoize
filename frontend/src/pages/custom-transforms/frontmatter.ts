// Client mirror of the backend frontmatter parser (src/custom_transforms/frontmatter.rs,
// custom-js-transforms.spec.md CJS-FM-1..6, CJS-ID-2, CJS-VAL-7) used as a save pre-check.

export const CUSTOM_TRANSFORM_ID_PATTERN = /^js:[a-z0-9]+(-[a-z0-9]+)*$/;
const ID_MAX_LENGTH = 64;
const KEYS = ["id", "name", "description", "author", "phase", "scopes", "visibility"] as const;
type FrontmatterKey = (typeof KEYS)[number];
const BOUNDED: Partial<Record<FrontmatterKey, number>> = {
  name: 100,
  description: 500,
  author: 100,
};
const SCOPES: Record<string, true> = { provider: true, global: true, api_key: true };

export type FrontmatterIssueCode =
  | "missingBlock"
  | "unterminated"
  | "missingMarker"
  | "malformedLine"
  | "unknownKey"
  | "duplicateKey"
  | "missingKey"
  | "invalidId"
  | "tooLong"
  | "invalidPhase"
  | "invalidScopes"
  | "invalidVisibility"
  | "idChanged";

export interface FrontmatterIssue {
  /** 1-based source line, or null when the issue has no single line. */
  line: number | null;
  code: FrontmatterIssueCode;
  params?: Record<string, string | number>;
}

export interface FrontmatterCheck {
  issues: FrontmatterIssue[];
  /** Line of the `id` key when present. */
  idLine: number | null;
}

function normalizeLine(line: string): string {
  let out = line.trimStart();
  if (out.startsWith("*")) out = out.slice(1);
  if (out.startsWith(" ")) out = out.slice(1);
  return out.trimEnd();
}

export function checkFrontmatter(source: string, expectedId?: string): FrontmatterCheck {
  const trimmed = source.trimStart();
  const startLine = source.slice(0, source.length - trimmed.length).split("\n").length;
  const issues: FrontmatterIssue[] = [];

  const opener = trimmed.startsWith("/**") ? 3 : trimmed.startsWith("/*") ? 2 : 0;
  if (opener === 0) {
    return { issues: [{ line: startLine, code: "missingBlock" }], idLine: null };
  }
  const afterOpener = trimmed.slice(opener);
  const end = afterOpener.indexOf("*/");
  if (end < 0) {
    return { issues: [{ line: null, code: "unterminated" }], idLine: null };
  }

  const lines = afterOpener
    .slice(0, end)
    .split("\n")
    .map((raw, index) => ({ line: startLine + index, text: normalizeLine(raw) }))
    .filter((entry) => entry.text.length > 0);
  const [first, ...rest] = lines;
  if (first?.text !== "@monoize-transform") {
    return {
      issues: [{ line: first?.line ?? startLine, code: "missingMarker" }],
      idLine: null,
    };
  }

  const values = new Map<FrontmatterKey, { value: string; line: number }>();
  for (const { line, text } of rest) {
    const colon = text.indexOf(":");
    if (colon < 0) {
      issues.push({ line, code: "malformedLine" });
      continue;
    }
    const key = text.slice(0, colon).trim();
    const value = text.slice(colon + 1).trim();
    if (!(KEYS as readonly string[]).includes(key)) {
      issues.push({ line, code: "unknownKey", params: { key } });
      continue;
    }
    if (values.has(key as FrontmatterKey)) {
      issues.push({ line, code: "duplicateKey", params: { key } });
      continue;
    }
    values.set(key as FrontmatterKey, { value, line });
  }

  for (const key of ["id", "name", "description", "author"] as const) {
    const entry = values.get(key);
    if (!entry || entry.value.length === 0) {
      issues.push({ line: entry?.line ?? null, code: "missingKey", params: { key } });
      continue;
    }
    const max = BOUNDED[key];
    if (max !== undefined && [...entry.value].length > max) {
      issues.push({ line: entry.line, code: "tooLong", params: { key, max } });
    }
  }

  const id = values.get("id");
  if (id && id.value.length > 0) {
    if (id.value.length > ID_MAX_LENGTH || !CUSTOM_TRANSFORM_ID_PATTERN.test(id.value)) {
      issues.push({ line: id.line, code: "invalidId" });
    } else if (expectedId !== undefined && id.value !== expectedId) {
      issues.push({ line: id.line, code: "idChanged", params: { id: expectedId } });
    }
  }

  const phase = values.get("phase");
  if (phase && !["request", "response", "both"].includes(phase.value)) {
    issues.push({ line: phase.line, code: "invalidPhase" });
  }

  const scopes = values.get("scopes");
  if (scopes) {
    const entries = scopes.value.split(",").map((entry) => entry.trim());
    const unknown = entries.some((entry) => !Object.hasOwn(SCOPES, entry));
    if (unknown || new Set(entries).size !== entries.length) {
      issues.push({ line: scopes.line, code: "invalidScopes" });
    }
  }

  const visibility = values.get("visibility");
  if (visibility && visibility.value !== "admin" && visibility.value !== "user") {
    issues.push({ line: visibility.line, code: "invalidVisibility" });
  }

  return { issues, idLine: id?.line ?? null };
}

/** Splits a CJS-VAL-7 `line <N>: ` prefix off a server detail message. */
export function splitLinePrefix(detail: string): { line: number | null; message: string } {
  const match = /^line (\d+): /.exec(detail);
  if (!match) return { line: null, message: detail };
  return { line: Number(match[1]), message: detail.slice(match[0].length) };
}
