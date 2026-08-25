import { useTranslation } from "react-i18next";
import { Braces, ChevronDown, ChevronUp, Copy, Plus, Trash2, X } from "lucide-react";
import { toast } from "sonner";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import {
  JSON_VALUE_KINDS,
  convertTypedJsonDraft,
  defaultDraftForProperty,
  resolveWidgetKind,
  typedJsonDraftKind,
  widgetTypeBadge,
  type DraftConfig,
  type DraftValue,
  type JsonSchemaObject,
  type JsonSchemaProperty,
  type JsonValueKind,
  type MapEntry,
} from "./transform-schema";

type SchemaFormFieldsProps = {
  schema: JsonSchemaObject | null;
  draftConfig: DraftConfig;
  errors: Record<string, string>;
  disabled?: boolean;
  onDraftChange: (key: string, draft: DraftValue) => void;
};

export function SchemaFormFields({
  schema,
  draftConfig,
  errors,
  disabled,
  onDraftChange,
}: SchemaFormFieldsProps) {
  const { t } = useTranslation();
  const properties = schema?.properties ?? {};
  const entries = Object.entries(properties);
  const required = Array.isArray(schema?.required) ? schema.required : [];

  if (entries.length === 0) {
    return (
      <p className="text-sm text-muted-foreground">
        {t("transforms.noConfigFields")}
      </p>
    );
  }

  return (
    <div className="space-y-5">
      {entries.map(([key, property]) => (
        <FieldRow
          key={key}
          fieldKey={key}
          property={property ?? {}}
          required={required.includes(key)}
          draft={draftConfig.drafts[key] ?? { kind: "unset" }}
          path={key}
          errors={errors}
          disabled={disabled}
          onChange={(draft) => onDraftChange(key, draft)}
        />
      ))}
    </div>
  );
}

type FieldRowProps = {
  fieldKey: string;
  property: JsonSchemaProperty;
  required: boolean;
  draft: DraftValue;
  path: string;
  errors: Record<string, string>;
  disabled?: boolean;
  onChange: (draft: DraftValue) => void;
};

function FieldRow({
  fieldKey,
  property,
  required,
  draft,
  path,
  errors,
  disabled,
  onChange,
}: FieldRowProps) {
  const { t } = useTranslation();
  const label = property.title ?? fieldKey;
  const badge = widgetTypeBadge(property);
  const error = errors[path];
  const isUnset = draft.kind === "unset";

  return (
    <div className="space-y-2">
      <div className="flex flex-wrap items-center gap-2">
        <Label className="text-sm font-medium">{label}</Label>
        <Badge variant="outline" className="font-mono text-[10px] uppercase">
          {badge}
        </Badge>
        {required ? (
          <Badge variant="secondary" className="text-[10px]">
            {t("transforms.requiredBadge")}
          </Badge>
        ) : (
          <Badge variant="outline" className="text-[10px] text-muted-foreground">
            {t("transforms.optionalBadge")}
          </Badge>
        )}
      </div>
      {property.description && (
        <p className="text-xs text-muted-foreground">{property.description}</p>
      )}
      {isUnset ? (
        <div className="flex flex-wrap items-center justify-between gap-2 rounded-md border border-dashed px-3 py-2">
          <span className="text-xs text-muted-foreground">
            {t("transforms.fieldNotSet")}
            {property.default !== undefined && (
              <span className="ml-1 font-mono">
                {t("transforms.fieldDefault", {
                  value: JSON.stringify(property.default),
                })}
              </span>
            )}
          </span>
          <Button
            type="button"
            size="sm"
            variant="outline"
            className="h-11 touch-manipulation sm:h-7"
            disabled={disabled}
            onClick={() => onChange(defaultDraftForProperty(property))}
          >
            {t("transforms.fieldSetValue")}
          </Button>
        </div>
      ) : (
        <div className="flex items-start gap-2">
          <div className="min-w-0 flex-1">
            <DraftValueEditor
              property={property}
              draft={draft}
              path={path}
              errors={errors}
              disabled={disabled}
              onChange={onChange}
            />
          </div>
          {!required && (
            <Button
              type="button"
              variant="ghost"
              size="icon"
              className="size-11 shrink-0 touch-manipulation text-muted-foreground sm:size-8"
              aria-label={t("transforms.fieldClear")}
              disabled={disabled}
              onClick={() => onChange({ kind: "unset" })}
            >
              <X className="h-4 w-4" />
            </Button>
          )}
        </div>
      )}
      {error && <p className="text-xs text-destructive">{error}</p>}
    </div>
  );
}

type DraftValueEditorProps = {
  property?: JsonSchemaProperty;
  draft: DraftValue;
  path: string;
  errors: Record<string, string>;
  disabled?: boolean;
  onChange: (draft: DraftValue) => void;
};

function DraftValueEditor({
  property,
  draft,
  path,
  errors,
  disabled,
  onChange,
}: DraftValueEditorProps) {
  const widget = resolveWidgetKind(property);

  if (widget === "enum" && draft.kind === "enum") {
    const options = (property?.enum ?? []).map((member) => String(member));
    return (
      <Select
        value={draft.value}
        disabled={disabled}
        onValueChange={(next) => onChange({ kind: "enum", value: next })}
      >
        <SelectTrigger className="w-full">
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

  if (widget === "boolean" && draft.kind === "boolean") {
    return (
      <div className="flex h-9 items-center gap-2">
        <Switch
          checked={draft.value}
          disabled={disabled}
          onCheckedChange={(next) => onChange({ kind: "boolean", value: next })}
        />
        <span className="font-mono text-xs text-muted-foreground">
          {String(draft.value)}
        </span>
      </div>
    );
  }

  if ((widget === "number" || widget === "integer") && draft.kind === "number") {
    return (
      <Input
        type="number"
        inputMode={widget === "integer" ? "numeric" : "decimal"}
        disabled={disabled}
        value={draft.text}
        min={typeof property?.minimum === "number" ? property.minimum : undefined}
        max={typeof property?.maximum === "number" ? property.maximum : undefined}
        step={widget === "integer" ? 1 : "any"}
        onChange={(e) => onChange({ kind: "number", text: e.target.value })}
      />
    );
  }

  if (widget === "string" && draft.kind === "string") {
    return (
      <Input
        value={draft.text}
        disabled={disabled}
        onChange={(e) => onChange({ kind: "string", text: e.target.value })}
      />
    );
  }

  if (widget === "string-multiline" && draft.kind === "string") {
    return (
      <Textarea
        value={draft.text}
        rows={4}
        disabled={disabled}
        onChange={(e) => onChange({ kind: "string", text: e.target.value })}
      />
    );
  }

  if (widget === "array" && draft.kind === "array") {
    return (
      <ArrayItemsEditor
        property={property}
        draft={draft}
        path={path}
        errors={errors}
        disabled={disabled}
        onChange={onChange}
      />
    );
  }

  if (widget === "object-fields" && draft.kind === "object") {
    return (
      <ObjectFieldsEditor
        property={property}
        draft={draft}
        path={path}
        errors={errors}
        disabled={disabled}
        onChange={onChange}
      />
    );
  }

  if (widget === "object-map" && draft.kind === "map") {
    return (
      <KeyValueMapEditor
        draft={draft}
        path={path}
        errors={errors}
        disabled={disabled}
        onChange={onChange}
      />
    );
  }

  // A schema/value type mismatch (or an untyped "json" property) falls back to
  // the typed JSON value editor so no stored value is ever hidden or coerced.
  return <TypedJsonValueEditor draft={draft} disabled={disabled} onChange={onChange} />;
}

type ArrayItemsEditorProps = {
  property?: JsonSchemaProperty;
  draft: Extract<DraftValue, { kind: "array" }>;
  path: string;
  errors: Record<string, string>;
  disabled?: boolean;
  onChange: (draft: DraftValue) => void;
};

function ArrayItemsEditor({
  property,
  draft,
  path,
  errors,
  disabled,
  onChange,
}: ArrayItemsEditorProps) {
  const { t } = useTranslation();
  const items = draft.items;

  const setItems = (next: DraftValue[]) => onChange({ kind: "array", items: next });

  const move = (from: number, to: number) => {
    if (to < 0 || to >= items.length) {
      return;
    }
    const next = [...items];
    const [item] = next.splice(from, 1);
    next.splice(to, 0, item);
    setItems(next);
  };

  return (
    <div className="space-y-2 rounded-md border p-2">
      {items.length === 0 && (
        <p className="px-1 py-1 text-xs text-muted-foreground">
          {t("transforms.arrayEmpty")}
        </p>
      )}
      {items.map((item, index) => (
        <div
          key={index}
          className="flex items-start gap-2 rounded-md border bg-muted/30 p-2"
        >
          <div className="min-w-0 flex-1">
            <DraftValueEditor
              property={property?.items}
              draft={item}
              path={`${path}.${index}`}
              errors={errors}
              disabled={disabled}
              onChange={(next) =>
                setItems(items.map((entry, idx) => (idx === index ? next : entry)))
              }
            />
            {errors[`${path}.${index}`] && (
              <p className="mt-1 text-xs text-destructive">{errors[`${path}.${index}`]}</p>
            )}
          </div>
          <div className="flex shrink-0 flex-col gap-1 sm:flex-row">
            <Button
              type="button"
              variant="ghost"
              size="icon"
              className="size-11 touch-manipulation sm:size-7"
              aria-label={t("transforms.arrayMoveUp")}
              disabled={disabled || index === 0}
              onClick={() => move(index, index - 1)}
            >
              <ChevronUp className="h-3.5 w-3.5" />
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              className="size-11 touch-manipulation sm:size-7"
              aria-label={t("transforms.arrayMoveDown")}
              disabled={disabled || index === items.length - 1}
              onClick={() => move(index, index + 1)}
            >
              <ChevronDown className="h-3.5 w-3.5" />
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              className="size-11 touch-manipulation text-destructive hover:text-destructive sm:size-7"
              aria-label={t("transforms.arrayRemoveItem")}
              disabled={disabled}
              onClick={() => setItems(items.filter((_, idx) => idx !== index))}
            >
              <Trash2 className="h-3.5 w-3.5" />
            </Button>
          </div>
        </div>
      ))}
      <Button
        type="button"
        size="sm"
        variant="outline"
        className="h-11 touch-manipulation sm:h-7"
        disabled={disabled}
        onClick={() => setItems([...items, defaultDraftForProperty(property?.items)])}
      >
        <Plus className="mr-1 h-3.5 w-3.5" />
        {t("transforms.arrayAddItem")}
      </Button>
    </div>
  );
}

type ObjectFieldsEditorProps = {
  property?: JsonSchemaProperty;
  draft: Extract<DraftValue, { kind: "object" }>;
  path: string;
  errors: Record<string, string>;
  disabled?: boolean;
  onChange: (draft: DraftValue) => void;
};

function ObjectFieldsEditor({
  property,
  draft,
  path,
  errors,
  disabled,
  onChange,
}: ObjectFieldsEditorProps) {
  const properties = property?.properties ?? {};
  const required =
    property && Array.isArray(property.required) ? property.required : [];

  return (
    <div className="space-y-4 rounded-md border p-3">
      {Object.entries(properties).map(([subKey, subProperty]) => (
        <FieldRow
          key={subKey}
          fieldKey={subKey}
          property={subProperty ?? {}}
          required={required.includes(subKey)}
          draft={draft.fields[subKey] ?? { kind: "unset" }}
          path={`${path}.${subKey}`}
          errors={errors}
          disabled={disabled}
          onChange={(next) =>
            onChange({
              kind: "object",
              fields: { ...draft.fields, [subKey]: next },
              extra: draft.extra,
            })
          }
        />
      ))}
    </div>
  );
}

type KeyValueMapEditorProps = {
  draft: Extract<DraftValue, { kind: "map" }>;
  path: string;
  errors: Record<string, string>;
  disabled?: boolean;
  onChange: (draft: DraftValue) => void;
};

function KeyValueMapEditor({
  draft,
  path,
  errors,
  disabled,
  onChange,
}: KeyValueMapEditorProps) {
  const { t } = useTranslation();
  const entries = draft.entries;

  const setEntries = (next: MapEntry[]) => onChange({ kind: "map", entries: next });

  return (
    <div className="space-y-2 rounded-md border p-2">
      {entries.length === 0 && (
        <p className="px-1 py-1 text-xs text-muted-foreground">
          {t("transforms.mapEmpty")}
        </p>
      )}
      {entries.map((entry, index) => (
        <div
          key={index}
          className="space-y-2 rounded-md border bg-muted/30 p-2"
        >
          <div className="flex items-center gap-2">
            <Input
              value={entry.key}
              placeholder={t("transforms.mapKeyPlaceholder")}
              className="font-mono text-xs"
              disabled={disabled}
              onChange={(e) =>
                setEntries(
                  entries.map((item, idx) =>
                    idx === index ? { ...item, key: e.target.value } : item
                  )
                )
              }
            />
            <Button
              type="button"
              variant="ghost"
              size="icon"
              className="size-11 shrink-0 touch-manipulation text-destructive hover:text-destructive sm:size-8"
              aria-label={t("transforms.mapRemoveEntry")}
              disabled={disabled}
              onClick={() => setEntries(entries.filter((_, idx) => idx !== index))}
            >
              <Trash2 className="h-3.5 w-3.5" />
            </Button>
          </div>
          <TypedJsonValueEditor
            draft={entry.value}
            disabled={disabled}
            onChange={(next) =>
              setEntries(
                entries.map((item, idx) =>
                  idx === index ? { ...item, value: next } : item
                )
              )
            }
          />
          {errors[`${path}.${index}`] && (
            <p className="text-xs text-destructive">{errors[`${path}.${index}`]}</p>
          )}
        </div>
      ))}
      <Button
        type="button"
        size="sm"
        variant="outline"
        className="h-11 touch-manipulation sm:h-7"
        disabled={disabled}
        onClick={() =>
          setEntries([...entries, { key: "", value: { kind: "string", text: "" } }])
        }
      >
        <Plus className="mr-1 h-3.5 w-3.5" />
        {t("transforms.mapAddEntry")}
      </Button>
    </div>
  );
}

type TypedJsonValueEditorProps = {
  draft: DraftValue;
  disabled?: boolean;
  onChange: (draft: DraftValue) => void;
};

function TypedJsonValueEditor({ draft, disabled, onChange }: TypedJsonValueEditorProps) {
  const { t } = useTranslation();
  const kind = typedJsonDraftKind(draft);

  const kindLabel = (value: JsonValueKind): string => {
    switch (value) {
      case "string":
        return t("transforms.kindString");
      case "number":
        return t("transforms.kindNumber");
      case "boolean":
        return t("transforms.kindBoolean");
      case "null":
        return t("transforms.kindNull");
      case "json":
        return t("transforms.kindJson");
    }
  };

  const formatJson = () => {
    if (draft.kind !== "json") {
      return;
    }
    try {
      const parsed = JSON.parse(draft.text.trim());
      onChange({ kind: "json", text: JSON.stringify(parsed, null, 2) });
    } catch {
      toast.error(t("transforms.validationInvalidJson"));
    }
  };

  const copyJson = async () => {
    if (draft.kind !== "json") {
      return;
    }
    try {
      await navigator.clipboard.writeText(draft.text);
      toast.success(t("transforms.jsonCopied"));
    } catch {
      toast.error(t("transforms.jsonCopyFailed"));
    }
  };

  return (
    <div className="space-y-2">
      <div className="flex items-center gap-2">
        <Select
          value={kind}
          disabled={disabled}
          onValueChange={(next) =>
            onChange(convertTypedJsonDraft(draft, next as JsonValueKind))
          }
        >
          <SelectTrigger className="h-8 w-[130px] text-xs">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {JSON_VALUE_KINDS.map((option) => (
              <SelectItem key={option} value={option}>
                {kindLabel(option)}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        {draft.kind === "json" && (
          <div className="flex items-center gap-1">
            <Button
              type="button"
              variant="ghost"
              size="icon"
              className="size-11 touch-manipulation sm:size-8"
              aria-label={t("transforms.jsonFormat")}
              disabled={disabled}
              onClick={formatJson}
            >
              <Braces className="h-3.5 w-3.5" />
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              className="size-11 touch-manipulation sm:size-8"
              aria-label={t("transforms.jsonCopy")}
              onClick={copyJson}
            >
              <Copy className="h-3.5 w-3.5" />
            </Button>
          </div>
        )}
      </div>

      {draft.kind === "string" && (
        <Input
          value={draft.text}
          disabled={disabled}
          placeholder={t("transforms.stringValuePlaceholder")}
          onChange={(e) => onChange({ kind: "string", text: e.target.value })}
        />
      )}
      {draft.kind === "number" && (
        <Input
          type="number"
          inputMode="decimal"
          value={draft.text}
          disabled={disabled}
          step="any"
          onChange={(e) => onChange({ kind: "number", text: e.target.value })}
        />
      )}
      {draft.kind === "boolean" && (
        <div className="flex h-9 items-center gap-2">
          <Switch
            checked={draft.value}
            disabled={disabled}
            onCheckedChange={(next) => onChange({ kind: "boolean", value: next })}
          />
          <span className="font-mono text-xs text-muted-foreground">
            {String(draft.value)}
          </span>
        </div>
      )}
      {draft.kind === "null" && (
        <p className="rounded-md border border-dashed px-3 py-2 font-mono text-xs text-muted-foreground">
          null
        </p>
      )}
      {draft.kind === "json" && (
        <Textarea
          value={draft.text}
          rows={4}
          disabled={disabled}
          placeholder={t("transforms.jsonValuePlaceholder")}
          className="font-mono text-xs"
          onChange={(e) => onChange({ kind: "json", text: e.target.value })}
        />
      )}
    </div>
  );
}
