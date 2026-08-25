import type { TransformRegistryItem, TransformRuleConfig } from "@/lib/api";

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

export type DraftError = { path: string; message: string };

export type TransformRuleValidationError = {
  field: string;
  message: string;
};

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
      const fields: Record<string, DraftValue> = {};
      for (const [subKey, subProperty] of Object.entries(properties)) {
        fields[subKey] = Object.prototype.hasOwnProperty.call(value, subKey)
          ? buildDraftValue(subProperty ?? undefined, value[subKey])
          : { kind: "unset" };
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
      : { kind: "unset" };
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
        ? { present: false, errors: [{ path, message: "is required" }] }
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
            errors: [
              { path, message: `must be at least ${property.minLength} characters` },
            ],
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
            ? { present: false, errors: [{ path, message: "is required" }] }
            : { present: false, errors: [] };
        }
        return { present: true, errors: [{ path, message: "must be a number" }] };
      }
      const parsed = Number(trimmed);
      if (!Number.isFinite(parsed)) {
        return { present: true, errors: [{ path, message: "must be a number" }] };
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
        return { present: true, errors: [{ path, message: "Invalid JSON" }] };
      }
      try {
        return finishScalar(property, JSON.parse(trimmed), path);
      } catch {
        return { present: true, errors: [{ path, message: "Invalid JSON" }] };
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
          message: `must have at least ${property.minItems} item(s)`,
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
          errors.push({ path: `${path}.${index}`, message: `duplicate key "${key}"` });
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
        message: `must be one of: ${property.enum.map(String).join(", ")}`,
      });
    }
    return errors;
  }

  switch (property.type) {
    case "string": {
      if (typeof value !== "string") {
        errors.push({ path, message: "must be a string" });
        return errors;
      }
      if (typeof property.minLength === "number" && value.length < property.minLength) {
        errors.push({
          path,
          message: `must be at least ${property.minLength} characters`,
        });
      }
      return errors;
    }
    case "boolean": {
      if (typeof value !== "boolean") {
        errors.push({ path, message: "must be a boolean" });
      }
      return errors;
    }
    case "integer":
    case "number": {
      if (typeof value !== "number" || !Number.isFinite(value)) {
        errors.push({ path, message: "must be a number" });
        return errors;
      }
      if (property.type === "integer" && !Number.isInteger(value)) {
        errors.push({ path, message: "must be an integer" });
      }
      if (typeof property.minimum === "number" && value < property.minimum) {
        errors.push({ path, message: `must be >= ${property.minimum}` });
      }
      if (typeof property.maximum === "number" && value > property.maximum) {
        errors.push({ path, message: `must be <= ${property.maximum}` });
      }
      return errors;
    }
    case "array": {
      if (!Array.isArray(value)) {
        errors.push({ path, message: "must be an array" });
        return errors;
      }
      if (typeof property.minItems === "number" && value.length < property.minItems) {
        errors.push({
          path,
          message: `must have at least ${property.minItems} item(s)`,
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
        errors.push({ path, message: "must be an object" });
        return errors;
      }
      const properties = property.properties;
      if (isRecord(properties)) {
        const requiredKeys = Array.isArray(property.required) ? property.required : [];
        for (const requiredKey of requiredKeys) {
          if (!Object.prototype.hasOwnProperty.call(value, requiredKey)) {
            errors.push({ path: `${path}.${requiredKey}`, message: "is required" });
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
      message: `phase "${rule.phase}" is not supported by transform "${rule.transform}"`,
    });
  }

  const schema = getSchemaObject(registryItem.config_schema);
  if (!schema || schema.type !== "object") {
    return errors;
  }

  if (!isRecord(rule.config)) {
    errors.push({
      field: "config",
      message: "config must be a JSON object",
    });
    return errors;
  }

  const required = Array.isArray(schema.required) ? schema.required : [];
  for (const key of required) {
    if (!Object.prototype.hasOwnProperty.call(rule.config, key) || rule.config[key] === undefined) {
      errors.push({
        field: key,
        message: "is required",
      });
    }
  }

  const properties = isRecord(schema.properties) ? schema.properties : {};
  for (const [key, rawProperty] of Object.entries(properties)) {
    if (!Object.prototype.hasOwnProperty.call(rule.config, key)) {
      continue;
    }
    const property = (rawProperty ?? {}) as JsonSchemaProperty;
    for (const error of validateValueAgainstProperty(property, rule.config[key], key)) {
      errors.push({ field: error.path, message: error.message });
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

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
