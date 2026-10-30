import type { TFunction } from "i18next";
import type { TransformRegistryItem, TransformRuleConfig } from "@/lib/api";
import { resolveLocalizedText } from "./localized-text";

export type JsonSchemaProperty = {
  type?: string;
  enum?: unknown[];
  minimum?: number;
  maximum?: number;
  minLength?: number;
  minItems?: number;
  format?: string;
  title?: string;
  description?: string;
  /** Localized field label per transform-config-ui.spec.md TCU-1b. */
  "x-localized-title"?: Record<string, string>;
  /** Localized field description per TCU-1b / urp-transform-system.spec.md TF-8d. */
  "x-localized-description"?: Record<string, string>;
  default?: unknown;
  items?: JsonSchemaProperty;
  properties?: Record<string, JsonSchemaProperty>;
  required?: string[];
  additionalProperties?: boolean;
};

export type JsonSchemaObject = {
  type?: string;
  properties?: Record<string, JsonSchemaProperty>;
  required?: string[];
  additionalProperties?: boolean;
};

/** Widget selection result per transform-config-ui.spec.md TCU-3. */
export type WidgetKind =
  | "enum"
  | "boolean"
  | "integer"
  | "number"
  | "string-multiline"
  | "string"
  | "array"
  | "object-fields"
  | "object-map"
  | "json";

export type MapEntry = { key: string; value: DraftValue };

/**
 * In-progress editor state for one config value. Text-bearing kinds keep the
 * raw input buffer so intermediate keystrokes (e.g. "1e", "-") never destroy
 * user input; parsing happens at save time.
 */
export type DraftValue =
  | { kind: "unset" }
  | { kind: "string"; text: string }
  | { kind: "number"; text: string }
  | { kind: "boolean"; value: boolean }
  | { kind: "enum"; value: string }
  | { kind: "null" }
  | { kind: "json"; text: string }
  | { kind: "array"; items: DraftValue[] }
  | { kind: "object"; fields: Record<string, DraftValue>; extra: Record<string, unknown> }
  | { kind: "map"; entries: MapEntry[] };

/** Value-kind selector options of the typed JSON value editor (TCU-7). */
export type JsonValueKind = "string" | "number" | "boolean" | "null" | "json";

export const JSON_VALUE_KINDS: JsonValueKind[] = ["string", "number", "boolean", "null", "json"];

/** Validation error codes per transform-config-ui.spec.md TCU-5a. */
export type TransformValidationCode =
  | "required"
  | "minLength"
  | "minimum"
  | "maximum"
  | "minItems"
  | "enum"
  | "notString"
  | "notNumber"
  | "notInteger"
  | "notBoolean"
  | "notArray"
  | "notObject"
  | "invalidJson"
  | "duplicateKey"
  | "phaseUnsupported"
  | "configNotObject";

export type TransformValidationParams = Record<string, string | number>;

export type DraftError = {
  path: string;
  code: TransformValidationCode;
  params?: TransformValidationParams;
};

export type TransformRuleValidationError = {
  field: string;
  code: TransformValidationCode;
  params?: TransformValidationParams;
};

/** Localized reason text for one validation error (TCU-5a). */
export function formatTransformValidationError(
  error: { code: TransformValidationCode; params?: TransformValidationParams },
  t: TFunction
): string {
  if (error.code === "phaseUnsupported") {
    const phase = error.params?.phase;
    return t("transforms.validation.phaseUnsupported", {
      phase: t(phase === "response" ? "transforms.responseChain" : "transforms.requestChain"),
    });
  }
  return t(`transforms.validation.${error.code}`, error.params ?? {});
}

/**
 * Builds the host-form save-blocking message "{rule} · {field}: {reason}" for
 * the first error of an invalid rule (TCU-5c).
 */
export function formatTransformRuleError(
  invalid: { index: number; errors: TransformRuleValidationError[] },
  rules: TransformRuleConfig[],
  registry: TransformRegistryItem[],
  t: TFunction,
  language: string
): string {
  const rule = rules[invalid.index];
  const typeId = rule?.transform ?? "";
  const registryItem = registry.find((item) => item.type_id === typeId);
  const ruleName = registryItem
    ? resolveLocalizedText(registryItem.name, language, typeId)
    : typeId;
  const error = invalid.errors[0];
  if (!error) {
    return ruleName;
  }
  let field: string;
  if (error.code === "phaseUnsupported") {
    field = t("transforms.validation.fieldPhase");
  } else if (error.code === "configNotObject") {
    field = t("transforms.config");
  } else {
    const schema = registryItem ? getSchemaObject(registryItem.config_schema) : null;
    field = formatSchemaPath(schema, error.field, language);
  }
  return t("transforms.validation.ruleError", {
    rule: ruleName,
    field,
    reason: formatTransformValidationError(error, t),
  });
}

/** Field label per TCU-1b: localized title, then `title`, then the key. */
export function resolvePropertyLabel(
  property: JsonSchemaProperty | undefined,
  key: string,
  language: string
): string {
  const fallback = typeof property?.title === "string" && property.title ? property.title : key;
  const localized = property?.["x-localized-title"];
  return isRecord(localized) ? resolveLocalizedText(localized, language, fallback) : fallback;
}

/** Field description per TCU-1b; empty string when the schema has none. */
export function resolvePropertyDescription(
  property: JsonSchemaProperty | undefined,
  language: string
): string {
  const fallback = typeof property?.description === "string" ? property.description : "";
  const localized = property?.["x-localized-description"];
  return isRecord(localized) ? resolveLocalizedText(localized, language, fallback) : fallback;
}

function formatSchemaPath(
  schema: JsonSchemaObject | null,
  path: string,
  language: string
): string {
  const labels: string[] = [];
  let properties: Record<string, JsonSchemaProperty> | undefined = schema?.properties;
  let current: JsonSchemaProperty | undefined;
  for (const segment of path.split(".")) {
    const property = properties?.[segment];
    if (!property && /^\d+$/.test(segment)) {
      // Array item or key/value map row index.
      labels.push(`#${Number(segment) + 1}`);
      current = current?.type === "array" ? current.items : undefined;
    } else {
      labels.push(resolvePropertyLabel(property, segment, language));
      current = property;
    }
    properties = current?.properties;
  }
  return labels.join(" › ");
}

export function getSchemaObject(schema: Record<string, unknown>): JsonSchemaObject | null {
  if (!isRecord(schema)) {
    return null;
  }
  return schema as JsonSchemaObject;
}

export function isRequiredSchemaKey(schema: JsonSchemaObject | null, key: string): boolean {
  const required = schema?.required;
  return Array.isArray(required) && required.includes(key);
}

export function resolveWidgetKind(property?: JsonSchemaProperty | null): WidgetKind {
  if (!property) {
    return "json";
  }
  if (Array.isArray(property.enum) && property.enum.length > 0) {
    return "enum";
  }
  switch (property.type) {
    case "boolean":
      return "boolean";
    case "integer":
      return "integer";
    case "number":
      return "number";
    case "string":
      return property.format === "multiline" ? "string-multiline" : "string";
    case "array":
      return "array";
    case "object":
      return isRecord(property.properties) ? "object-fields" : "object-map";
    default:
      return "json";
  }
}

/** Type badge label per TCU-4 rule 2. */
export function widgetTypeBadge(
  property?: JsonSchemaProperty | null
): "string" | "number" | "integer" | "boolean" | "enum" | "array" | "object" | "json" {
  switch (resolveWidgetKind(property)) {
    case "enum":
      return "enum";
    case "boolean":
      return "boolean";
    case "integer":
      return "integer";
    case "number":
      return "number";
    case "string":
    case "string-multiline":
      return "string";
    case "array":
      return "array";
    case "object-fields":
    case "object-map":
      return "object";
    default:
      return "json";
  }
}

/** Initial value-kind inference for the typed JSON value editor (TCU-7a). */
export function buildTypedJsonDraft(value: unknown): DraftValue {
  if (typeof value === "string") {
    return { kind: "string", text: value };
  }
  if (typeof value === "number" && Number.isFinite(value)) {
    return { kind: "number", text: String(value) };
  }
  if (typeof value === "boolean") {
    return { kind: "boolean", value };
  }
  if (value === null) {
    return { kind: "null" };
  }
  return { kind: "json", text: JSON.stringify(value, null, 2) ?? "" };
}

/**
 * Builds the editor draft for one stored config value (TCU-8 rule 1). A value
 * whose JSON type does not fit the schema widget falls back to a typed JSON
 * draft so the stored data stays visible and editable instead of being
 * silently coerced.
 */
export function buildDraftValue(
  property: JsonSchemaProperty | undefined,
  value: unknown
): DraftValue {
  const widget = resolveWidgetKind(property);
  switch (widget) {
    case "enum":
      return { kind: "enum", value: String(value) };
    case "boolean":
      return typeof value === "boolean"
        ? { kind: "boolean", value }
        : buildTypedJsonDraft(value);
    case "integer":
    case "number":
      return typeof value === "number" && Number.isFinite(value)
        ? { kind: "number", text: String(value) }
        : buildTypedJsonDraft(value);
    case "string":
    case "string-multiline":
      return typeof value === "string"
        ? { kind: "string", text: value }
        : buildTypedJsonDraft(value);
    case "array":
      return Array.isArray(value)
        ? { kind: "array", items: value.map((item) => buildDraftValue(property?.items, item)) }
        : buildTypedJsonDraft(value);
    case "object-fields": {
      if (!isRecord(value)) {
        return buildTypedJsonDraft(value);
      }
      const properties = property?.properties ?? {};
      const requiredKeys = property && Array.isArray(property.required) ? property.required : [];
      const fields: Record<string, DraftValue> = {};
      for (const [subKey, subProperty] of Object.entries(properties)) {
        fields[subKey] = Object.prototype.hasOwnProperty.call(value, subKey)
          ? buildDraftValue(subProperty ?? undefined, value[subKey])
          : absentDraft(subProperty ?? undefined, requiredKeys.includes(subKey));
      }
      const extra: Record<string, unknown> = {};
      for (const [subKey, subValue] of Object.entries(value)) {
        if (!Object.prototype.hasOwnProperty.call(properties, subKey)) {
          extra[subKey] = subValue;
        }
      }
      return { kind: "object", fields, extra };
    }
    case "object-map": {
      if (!isRecord(value)) {
        return buildTypedJsonDraft(value);
      }
      return {
        kind: "map",
        entries: Object.entries(value).map(([key, entryValue]) => ({
          key,
          value: buildTypedJsonDraft(entryValue),
        })),
      };
    }
    default:
      return buildTypedJsonDraft(value);
  }
}

/**
 * Initial draft for a key absent from the stored config (TCU-8 rules 2 and 3):
 * required keys open as an active editor, optional keys as unset.
 */
function absentDraft(property: JsonSchemaProperty | undefined, required: boolean): DraftValue {
  return required ? defaultDraftForProperty(property) : { kind: "unset" };
}

/** Draft created when the user activates an unset field or appends an array row. */
export function defaultDraftForProperty(property?: JsonSchemaProperty | null): DraftValue {
  const widget = resolveWidgetKind(property);
  switch (widget) {
    case "enum": {
      const members = property && Array.isArray(property.enum) ? property.enum : [];
      const fallback = members.length > 0 ? String(members[0]) : "";
      return {
        kind: "enum",
        value:
          property && property.default !== undefined ? String(property.default) : fallback,
      };
    }
    case "boolean":
      return {
        kind: "boolean",
        value: typeof property?.default === "boolean" ? property.default : false,
      };
    case "integer":
    case "number":
      return {
        kind: "number",
        text: typeof property?.default === "number" ? String(property.default) : "",
      };
    case "string":
    case "string-multiline":
      return {
        kind: "string",
        text: typeof property?.default === "string" ? property.default : "",
      };
    case "array":
      return { kind: "array", items: [] };
    case "object-fields": {
      const properties = property?.properties ?? {};
      const required = property && Array.isArray(property.required) ? property.required : [];
      const fields: Record<string, DraftValue> = {};
      for (const [subKey, subProperty] of Object.entries(properties)) {
        fields[subKey] = required.includes(subKey)
          ? defaultDraftForProperty(subProperty ?? undefined)
          : { kind: "unset" };
      }
      return { kind: "object", fields, extra: {} };
    }
    case "object-map":
      return { kind: "map", entries: [] };
    default:
      // TCU-7a: activation of an unset typed JSON field starts in string kind.
      return { kind: "string", text: "" };
  }
}

/** Returns the value-kind selector position for a typed JSON draft (TCU-7a). */
export function typedJsonDraftKind(draft: DraftValue): JsonValueKind {
  switch (draft.kind) {
    case "string":
      return "string";
    case "number":
      return "number";
    case "boolean":
      return "boolean";
    case "null":
      return "null";
    default:
      return "json";
  }
}

/**
 * Switches the typed JSON value editor to another value kind, carrying the
 * previous input across when it can be represented in the target kind.
 */
export function convertTypedJsonDraft(draft: DraftValue, kind: JsonValueKind): DraftValue {
  switch (kind) {
    case "string": {
      if (draft.kind === "string") {
        return draft;
      }
      if (draft.kind === "number") {
        return { kind: "string", text: draft.text };
      }
      return { kind: "string", text: "" };
    }
    case "number": {
      if (draft.kind === "number") {
        return draft;
      }
      if (draft.kind === "string" && Number.isFinite(Number(draft.text.trim())) && draft.text.trim() !== "") {
        return { kind: "number", text: draft.text.trim() };
      }
      return { kind: "number", text: "" };
    }
    case "boolean":
      return draft.kind === "boolean" ? draft : { kind: "boolean", value: false };
    case "null":
      return { kind: "null" };
    case "json": {
      switch (draft.kind) {
        case "json":
          return draft;
        case "string":
          return { kind: "json", text: JSON.stringify(draft.text) };
        case "number":
          return { kind: "json", text: draft.text.trim() === "" ? "" : draft.text.trim() };
        case "boolean":
          return { kind: "json", text: String(draft.value) };
        case "null":
          return { kind: "json", text: "null" };
        default:
          return { kind: "json", text: "" };
      }
    }
  }
}

export type DraftConfig = {
  drafts: Record<string, DraftValue>;
  /** Keys present in the stored config but absent from the schema (TCU-9 rule 5). */
  extra: Record<string, unknown>;
};

export function buildDraftConfig(
  schema: JsonSchemaObject | null,
  config: Record<string, unknown>
): DraftConfig {
  const properties = schema?.properties ?? {};
  const drafts: Record<string, DraftValue> = {};
  for (const [key, property] of Object.entries(properties)) {
    drafts[key] = Object.prototype.hasOwnProperty.call(config, key)
      ? buildDraftValue(property ?? undefined, config[key])
      : absentDraft(property ?? undefined, isRequiredSchemaKey(schema, key));
  }
  const extra: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(config)) {
    if (!Object.prototype.hasOwnProperty.call(properties, key)) {
      extra[key] = value;
    }
  }
  return { drafts, extra };
}

export type SerializedDraft = {
  present: boolean;
  value?: unknown;
  errors: DraftError[];
};

/**
 * Turns one draft back into a JSON value per TCU-9, validating it against the
 * schema property (TCU-5). `required` controls unset/empty semantics.
 */
export function serializeDraftValue(
  property: JsonSchemaProperty | undefined,
  draft: DraftValue,
  path: string,
  required: boolean
): SerializedDraft {
  const widget = resolveWidgetKind(property);
  switch (draft.kind) {
    case "unset":
      return required
        ? { present: false, errors: [{ path, code: "required" }] }
        : { present: false, errors: [] };
    case "string": {
      if (
        draft.text === "" &&
        (widget === "string" || widget === "string-multiline")
      ) {
        // TCU-9 rules 3 and 4: empty plain-text input means unset when
        // optional; when required it saves "" unless minLength forbids it.
        if (!required) {
          return { present: false, errors: [] };
        }
        if (property && typeof property.minLength === "number" && property.minLength >= 1) {
          return {
            present: true,
            value: "",
            errors: [{ path, code: "minLength", params: { min: property.minLength } }],
          };
        }
        return { present: true, value: "", errors: [] };
      }
      return finishScalar(property, draft.text, path);
    }
    case "number": {
      const trimmed = draft.text.trim();
      if (trimmed === "") {
        if (widget === "number" || widget === "integer") {
          return required
            ? { present: false, errors: [{ path, code: "required" }] }
            : { present: false, errors: [] };
        }
        return { present: true, errors: [{ path, code: "notNumber" }] };
      }
      const parsed = Number(trimmed);
      if (!Number.isFinite(parsed)) {
        return { present: true, errors: [{ path, code: "notNumber" }] };
      }
      return finishScalar(property, parsed, path);
    }
    case "boolean":
      return finishScalar(property, draft.value, path);
    case "enum": {
      const members = property && Array.isArray(property.enum) ? property.enum : [];
      const match = members.find((member) => String(member) === draft.value);
      return finishScalar(property, match !== undefined ? match : draft.value, path);
    }
    case "null":
      return finishScalar(property, null, path);
    case "json": {
      const trimmed = draft.text.trim();
      if (trimmed === "") {
        return { present: true, errors: [{ path, code: "invalidJson" }] };
      }
      try {
        return finishScalar(property, JSON.parse(trimmed), path);
      } catch {
        return { present: true, errors: [{ path, code: "invalidJson" }] };
      }
    }
    case "array": {
      const errors: DraftError[] = [];
      const values: unknown[] = [];
      draft.items.forEach((item, index) => {
        const serialized = serializeDraftValue(
          property?.items,
          item,
          `${path}.${index}`,
          true
        );
        errors.push(...serialized.errors);
        if (serialized.present) {
          values.push(serialized.value);
        }
      });
      if (
        property &&
        typeof property.minItems === "number" &&
        values.length < property.minItems
      ) {
        errors.push({
          path,
          code: "minItems",
          params: { min: property.minItems },
        });
      }
      return { present: true, value: values, errors };
    }
    case "object": {
      const errors: DraftError[] = [];
      const result: Record<string, unknown> = { ...draft.extra };
      const properties = property?.properties ?? {};
      const requiredKeys = property && Array.isArray(property.required) ? property.required : [];
      for (const [subKey, subProperty] of Object.entries(properties)) {
        const subDraft = draft.fields[subKey] ?? { kind: "unset" };
        const serialized = serializeDraftValue(
          subProperty ?? undefined,
          subDraft,
          `${path}.${subKey}`,
          requiredKeys.includes(subKey)
        );
        errors.push(...serialized.errors);
        if (serialized.present) {
          result[subKey] = serialized.value;
        }
      }
      return { present: true, value: result, errors };
    }
    case "map": {
      const errors: DraftError[] = [];
      const result: Record<string, unknown> = {};
      const seen = new Set<string>();
      draft.entries.forEach((entry, index) => {
        const key = entry.key.trim();
        if (key === "") {
          // TCU-6: rows with an empty key are excluded from the saved value.
          return;
        }
        if (seen.has(key)) {
          errors.push({ path: `${path}.${index}`, code: "duplicateKey", params: { key } });
          return;
        }
        seen.add(key);
        const serialized = serializeDraftValue(
          undefined,
          entry.value,
          `${path}.${index}`,
          true
        );
        errors.push(...serialized.errors);
        if (serialized.present) {
          result[key] = serialized.value;
        }
      });
      return { present: true, value: result, errors };
    }
  }
}

export type SerializedConfig = {
  config: Record<string, unknown>;
  errors: DraftError[];
};

export function serializeDraftConfig(
  schema: JsonSchemaObject | null,
  draftConfig: DraftConfig
): SerializedConfig {
  const properties = schema?.properties ?? {};
  const config: Record<string, unknown> = { ...draftConfig.extra };
  const errors: DraftError[] = [];
  for (const [key, property] of Object.entries(properties)) {
    const draft = draftConfig.drafts[key] ?? { kind: "unset" };
    const serialized = serializeDraftValue(
      property ?? undefined,
      draft,
      key,
      isRequiredSchemaKey(schema, key)
    );
    errors.push(...serialized.errors);
    if (serialized.present) {
      config[key] = serialized.value;
    }
  }
  return { config, errors };
}

function finishScalar(
  property: JsonSchemaProperty | undefined,
  value: unknown,
  path: string
): SerializedDraft {
  return {
    present: true,
    value,
    errors: validateValueAgainstProperty(property, value, path),
  };
}

/** Schema conformance checks per TCU-5, shared by the dialog and host forms. */
export function validateValueAgainstProperty(
  property: JsonSchemaProperty | undefined,
  value: unknown,
  path: string
): DraftError[] {
  if (!property) {
    return [];
  }
  const errors: DraftError[] = [];

  if (Array.isArray(property.enum) && property.enum.length > 0) {
    if (!property.enum.includes(value)) {
      errors.push({
        path,
        code: "enum",
        params: { values: property.enum.map(String).join(", ") },
      });
    }
    return errors;
  }

  switch (property.type) {
    case "string": {
      if (typeof value !== "string") {
        errors.push({ path, code: "notString" });
        return errors;
      }
      if (typeof property.minLength === "number" && value.length < property.minLength) {
        errors.push({ path, code: "minLength", params: { min: property.minLength } });
      }
      return errors;
    }
    case "boolean": {
      if (typeof value !== "boolean") {
        errors.push({ path, code: "notBoolean" });
      }
      return errors;
    }
    case "integer":
    case "number": {
      if (typeof value !== "number" || !Number.isFinite(value)) {
        errors.push({ path, code: "notNumber" });
        return errors;
      }
      if (property.type === "integer" && !Number.isInteger(value)) {
        errors.push({ path, code: "notInteger" });
      }
      if (typeof property.minimum === "number" && value < property.minimum) {
        errors.push({ path, code: "minimum", params: { min: property.minimum } });
      }
      if (typeof property.maximum === "number" && value > property.maximum) {
        errors.push({ path, code: "maximum", params: { max: property.maximum } });
      }
      return errors;
    }
    case "array": {
      if (!Array.isArray(value)) {
        errors.push({ path, code: "notArray" });
        return errors;
      }
      if (typeof property.minItems === "number" && value.length < property.minItems) {
        errors.push({
          path,
          code: "minItems",
          params: { min: property.minItems },
        });
      }
      value.forEach((item, index) => {
        errors.push(
          ...validateValueAgainstProperty(property.items, item, `${path}.${index}`)
        );
      });
      return errors;
    }
    case "object": {
      if (!isRecord(value)) {
        errors.push({ path, code: "notObject" });
        return errors;
      }
      const properties = property.properties;
      if (isRecord(properties)) {
        const requiredKeys = Array.isArray(property.required) ? property.required : [];
        for (const requiredKey of requiredKeys) {
          if (!Object.prototype.hasOwnProperty.call(value, requiredKey)) {
            errors.push({ path: `${path}.${requiredKey}`, code: "required" });
          }
        }
        for (const [subKey, subProperty] of Object.entries(properties)) {
          if (Object.prototype.hasOwnProperty.call(value, subKey)) {
            errors.push(
              ...validateValueAgainstProperty(
                subProperty ?? undefined,
                value[subKey],
                `${path}.${subKey}`
              )
            );
          }
        }
      }
      return errors;
    }
    default:
      return errors;
  }
}

/** True when a newly added rule must be configured before it joins a chain (TCU-2d). */
export function transformRequiresConfig(registryItem: TransformRegistryItem): boolean {
  const schema = getSchemaObject(registryItem.config_schema);
  return Array.isArray(schema?.required) && schema.required.length > 0;
}

export function validateTransformRule(
  rule: TransformRuleConfig,
  registryItem?: TransformRegistryItem
): TransformRuleValidationError[] {
  if (!registryItem) {
    return [];
  }

  const errors: TransformRuleValidationError[] = [];
  if (!registryItem.supported_phases.includes(rule.phase)) {
    errors.push({
      field: "phase",
      code: "phaseUnsupported",
      params: { phase: rule.phase },
    });
  }

  const schema = getSchemaObject(registryItem.config_schema);
  if (!schema || schema.type !== "object") {
    return errors;
  }

  if (!isRecord(rule.config)) {
    errors.push({ field: "config", code: "configNotObject" });
    return errors;
  }

  const required = Array.isArray(schema.required) ? schema.required : [];
  for (const key of required) {
    if (!Object.prototype.hasOwnProperty.call(rule.config, key) || rule.config[key] === undefined) {
      errors.push({ field: key, code: "required" });
    }
  }

  const properties = isRecord(schema.properties) ? schema.properties : {};
  for (const [key, rawProperty] of Object.entries(properties)) {
    if (!Object.prototype.hasOwnProperty.call(rule.config, key)) {
      continue;
    }
    const property = (rawProperty ?? {}) as JsonSchemaProperty;
    for (const error of validateValueAgainstProperty(property, rule.config[key], key)) {
      errors.push({ field: error.path, code: error.code, params: error.params });
    }
  }

  return errors;
}

export function findFirstInvalidTransformRule(
  rules: TransformRuleConfig[],
  registry: TransformRegistryItem[]
): { index: number; errors: TransformRuleValidationError[] } | null {
  const map = new Map(registry.map((item) => [item.type_id, item]));
  for (let i = 0; i < rules.length; i += 1) {
    const rule = rules[i];
    const item = map.get(rule.transform);
    const errors = validateTransformRule(rule, item);
    if (errors.length > 0) {
      return { index: i, errors };
    }
  }
  return null;
}

/** Shared plain-object guard for untyped config JSON in this module family. */
export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
