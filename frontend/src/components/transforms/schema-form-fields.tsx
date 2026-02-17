import { useTranslation } from "react-i18next";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import type { JsonSchemaObject, JsonSchemaProperty } from "./transform-schema";

type SchemaFormFieldsProps = {
  schema: JsonSchemaObject | null;
  config: Record<string, unknown>;
  errors: Record<string, string>;
  rawJsonInputs: Record<string, string>;
  disabled?: boolean;
  onFieldChange: (key: string, value: unknown) => void;
  onRawJsonInputChange: (key: string, value: string) => void;
  onRawJsonInputCommit: (key: string) => void;
};

export function SchemaFormFields({
  schema,
  config,
  errors,
  rawJsonInputs,
  disabled,
  onFieldChange,
  onRawJsonInputChange,
  onRawJsonInputCommit,
}: SchemaFormFieldsProps) {
  const { t } = useTranslation();
  const properties = schema?.properties ?? {};
  const entries = Object.entries(properties);

  if (entries.length === 0) {
    return (
      <p className="text-sm text-muted-foreground">
        {t("transforms.noConfigFields")}
      </p>
    );
  }

  return (
    <div className="space-y-4">
      {entries.map(([key, property]) => {
        const schemaProperty = property ?? {};
        const label = schemaProperty.title ?? key;
        const error = errors[key];
        return (
          <div key={key} className="space-y-2">
            <Label className="text-sm font-medium">
              {label}
            </Label>
            {schemaProperty.description && (
              <p className="text-xs text-muted-foreground">{schemaProperty.description}</p>
            )}
            <SchemaPropertyField
              field={key}
              property={schemaProperty}
              value={config[key]}
              rawJsonInputs={rawJsonInputs}
              disabled={disabled}
              onFieldChange={onFieldChange}
              onRawJsonInputChange={onRawJsonInputChange}
              onRawJsonInputCommit={onRawJsonInputCommit}
            />
            {error && <p className="text-xs text-destructive">{error}</p>}
          </div>
        );
      })}
    </div>
  );
}

type SchemaPropertyFieldProps = {
  field: string;
  property: JsonSchemaProperty;
  value: unknown;
  rawJsonInputs: Record<string, string>;
  disabled?: boolean;
  onFieldChange: (key: string, value: unknown) => void;
  onRawJsonInputChange: (key: string, value: string) => void;
  onRawJsonInputCommit: (key: string) => void;
};

function SchemaPropertyField({
  field,
  property,
  value,
  rawJsonInputs,
  disabled,
  onFieldChange,
  onRawJsonInputChange,
  onRawJsonInputCommit,
}: SchemaPropertyFieldProps) {
  if (Array.isArray(property.enum) && property.enum.length > 0) {
    const options = property.enum.map((entry) => String(entry));
    const selected = typeof value === "string" ? value : options[0];
    return (
      <Select
        value={selected}
        disabled={disabled}
        onValueChange={(next) => onFieldChange(field, next)}
      >
        <SelectTrigger>
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          {options.map((option) => (
            <SelectItem key={option} value={option}>
              {option}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    );
  }

  if (property.type === "boolean") {
    return (
      <div className="flex items-center gap-2">
        <Switch
          checked={Boolean(value)}
          disabled={disabled}
          onCheckedChange={(next) => onFieldChange(field, next)}
        />
        <span className="text-sm text-muted-foreground">{String(Boolean(value))}</span>
      </div>
    );
  }

  if (property.type === "number" || property.type === "integer") {
    const numericValue = typeof value === "number" ? String(value) : "";
    return (
      <Input
        type="number"
        disabled={disabled}
        value={numericValue}
        min={typeof property.minimum === "number" ? property.minimum : undefined}
        step={property.type === "integer" ? 1 : "any"}
        onChange={(e) => {
          const next = e.target.value;
          if (next === "") {
            onFieldChange(field, undefined);
            return;
          }
          const parsed = Number(next);
          onFieldChange(field, Number.isFinite(parsed) ? parsed : undefined);
        }}
      />
    );
  }

  if (property.type === "string") {
    return (
      <Input
        value={typeof value === "string" ? value : ""}
        disabled={disabled}
        onChange={(e) => onFieldChange(field, e.target.value)}
      />
    );
  }

  const rawValue =
    rawJsonInputs[field] ??
    JSON.stringify(value === undefined ? null : value, null, 2);
  return (
    <Textarea
      value={rawValue}
      rows={4}
      disabled={disabled}
      className="font-mono text-xs"
      onChange={(e) => onRawJsonInputChange(field, e.target.value)}
      onBlur={() => onRawJsonInputCommit(field)}
    />
  );
}
