import type { TransformRegistryItem, TransformRuleConfig } from "@/lib/api";

export type JsonSchemaProperty = {
  type?: string;
  enum?: unknown[];
  minimum?: number;
  minLength?: number;
  title?: string;
  description?: string;
};

export type JsonSchemaObject = {
  type?: string;
  properties?: Record<string, JsonSchemaProperty>;
  required?: string[];
  additionalProperties?: boolean;
};

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
    const value = rule.config[key];
    const property = (rawProperty ?? {}) as JsonSchemaProperty;
    errors.push(...validateProperty(key, value, property));
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

function validateProperty(
  key: string,
  value: unknown,
  property: JsonSchemaProperty
): TransformRuleValidationError[] {
  const errors: TransformRuleValidationError[] = [];

  if (Array.isArray(property.enum)) {
    if (!property.enum.includes(value)) {
      errors.push({
        field: key,
        message: `must be one of: ${property.enum.map(String).join(", ")}`,
      });
    }
    return errors;
  }

  if (property.type === "string") {
    if (typeof value !== "string") {
      errors.push({ field: key, message: "must be a string" });
      return errors;
    }
    if (typeof property.minLength === "number" && value.length < property.minLength) {
      errors.push({ field: key, message: `must be at least ${property.minLength} characters` });
    }
    return errors;
  }

  if (property.type === "boolean") {
    if (typeof value !== "boolean") {
      errors.push({ field: key, message: "must be a boolean" });
    }
    return errors;
  }

  if (property.type === "number" || property.type === "integer") {
    if (typeof value !== "number" || !Number.isFinite(value)) {
      errors.push({ field: key, message: "must be a number" });
      return errors;
    }
    if (property.type === "integer" && !Number.isInteger(value)) {
      errors.push({ field: key, message: "must be an integer" });
    }
    if (typeof property.minimum === "number" && value < property.minimum) {
      errors.push({ field: key, message: `must be >= ${property.minimum}` });
    }
  }

  return errors;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
