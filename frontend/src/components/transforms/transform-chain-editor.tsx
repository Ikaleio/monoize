import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { ArrowDownUp, ArrowUpDown, GripVertical, Plus, Settings2, Trash2 } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Skeleton } from "@/components/ui/skeleton";
import { Switch } from "@/components/ui/switch";
import type { Phase, TransformRegistryItem, TransformRuleConfig } from "@/lib/api";
import { resolveLocalizedText } from "./localized-text";
import { TransformItemConfigDialog } from "./transform-item-config-dialog";

type TransformChainEditorProps = {
  value: TransformRuleConfig[];
  registry: TransformRegistryItem[];
  loading?: boolean;
  onChange: (next: TransformRuleConfig[]) => void;
};

export function TransformChainEditor({ value, registry, loading, onChange }: TransformChainEditorProps) {
  const requestRules = useMemo(
    () => value.filter((rule) => rule.phase === "request"),
    [value]
  );
  const responseRules = useMemo(
    () => value.filter((rule) => rule.phase === "response"),
    [value]
  );
  const registryMap = useMemo(
    () => new Map(registry.map((item) => [item.type_id, item])),
    [registry]
  );
  const [editing, setEditing] = useState<{ phase: Phase; index: number } | null>(null);

  if (loading) {
    return <TransformChainSkeleton />;
  }

  const updatePhaseRules = (phase: Phase, nextPhaseRules: TransformRuleConfig[]) => {
    const nextRequest = phase === "request" ? nextPhaseRules : requestRules;
    const nextResponse = phase === "response" ? nextPhaseRules : responseRules;
    onChange([...nextRequest, ...nextResponse]);
  };

  const editingRule = editing
    ? (editing.phase === "request" ? requestRules[editing.index] : responseRules[editing.index]) ?? null
    : null;
  const editingRegistry = editingRule ? registryMap.get(editingRule.transform) : undefined;

  return (
    <div className="space-y-4">
      <PhaseChainSection
        phase="request"
        rules={requestRules}
        registry={registry}
        registryMap={registryMap}
        onChange={(next) => updatePhaseRules("request", next)}
        onConfigure={(index) => setEditing({ phase: "request", index })}
      />
      <PhaseChainSection
        phase="response"
        rules={responseRules}
        registry={registry}
        registryMap={registryMap}
        onChange={(next) => updatePhaseRules("response", next)}
        onConfigure={(index) => setEditing({ phase: "response", index })}
      />

      <TransformItemConfigDialog
        open={Boolean(editingRule)}
        rule={editingRule}
        registryItem={editingRegistry}
        onOpenChange={(open) => {
          if (!open) {
            setEditing(null);
          }
        }}
        onSave={(nextRule) => {
          if (!editing) {
            return;
          }
          const current = editing.phase === "request" ? requestRules : responseRules;
          const next = current.map((rule, idx) => (idx === editing.index ? nextRule : rule));
          updatePhaseRules(editing.phase, next);
        }}
      />
    </div>
  );
}

function TransformChainSkeleton() {
  return (
    <div className="space-y-4">
      {[0, 1].map((section) => (
        <div key={section} className="space-y-3">
          <div className="flex items-center gap-2">
            <Skeleton className="h-4 w-4 rounded" />
            <Skeleton className="h-4 w-36" />
          </div>
          <div className="space-y-2 rounded-lg border p-3">
            <Skeleton className="h-12 w-full rounded-md" />
            <Skeleton className="h-12 w-full rounded-md" />
          </div>
        </div>
      ))}
    </div>
  );
}

type PhaseChainSectionProps = {
  phase: Phase;
  rules: TransformRuleConfig[];
  registry: TransformRegistryItem[];
  registryMap: Map<string, TransformRegistryItem>;
  onChange: (next: TransformRuleConfig[]) => void;
  onConfigure: (index: number) => void;
};

function PhaseChainSection({
  phase,
  rules,
  registry,
  registryMap,
  onChange,
  onConfigure,
}: PhaseChainSectionProps) {
  const { t, i18n } = useTranslation();
  const [draggingIndex, setDraggingIndex] = useState<number | null>(null);
  const available = useMemo(
    () => registry.filter((item) => item.supported_phases.includes(phase)),
    [registry, phase]
  );
  const [addType, setAddType] = useState<string>(available[0]?.type_id ?? "");
  const selectedAddType = available.some((item) => item.type_id === addType)
    ? addType
    : (available[0]?.type_id ?? "");
  const selectedAddItem = available.find((item) => item.type_id === selectedAddType);
  const selectedAddName = selectedAddItem
    ? resolveLocalizedText(selectedAddItem.name, i18n.language, selectedAddItem.type_id)
    : "";

  const title = phase === "request" ? t("transforms.requestChain") : t("transforms.responseChain");
  const PhaseIcon = phase === "request" ? ArrowUpDown : ArrowDownUp;

  const addRule = () => {
    if (!selectedAddType) {
      return;
    }
    const nextRule: TransformRuleConfig = {
      transform: selectedAddType,
      enabled: true,
      models: null,
      phase,
      config: {},
    };
    onChange([...rules, nextRule]);
  };

  const reorder = (from: number, to: number) => {
    if (from === to || from < 0 || to < 0 || from >= rules.length || to >= rules.length) {
      return;
    }
    const next = [...rules];
    const [item] = next.splice(from, 1);
    next.splice(to, 0, item);
    onChange(next);
  };

  return (
    <div className="space-y-3">
      <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
        <div className="flex items-center gap-2">
          <PhaseIcon className="h-4 w-4 text-muted-foreground" />
          <h3 className="text-sm font-medium">{title}</h3>
          <Badge variant="secondary" className="text-xs">{rules.length}</Badge>
        </div>
        <div className="flex w-full min-w-0 flex-col gap-2 sm:w-auto sm:flex-row sm:items-center">
          <Select value={selectedAddType} onValueChange={setAddType} disabled={available.length === 0}>
            <SelectTrigger
              className="h-9 min-w-0 w-full sm:w-[260px]"
              aria-label={t("transforms.selectTransform")}
            >
              <SelectValue placeholder={t("transforms.selectTransform")}>
                {selectedAddName ? (
                  <span className="block min-w-0 truncate text-left">{selectedAddName}</span>
                ) : undefined}
              </SelectValue>
            </SelectTrigger>
            <SelectContent>
              <SelectGroup>
                {available.map((item) => {
                  const name = resolveLocalizedText(item.name, i18n.language, item.type_id);
                  const description = resolveLocalizedText(item.description, i18n.language, "");
                  return (
                    <SelectItem key={item.type_id} value={item.type_id}>
                      <span className="flex min-w-0 flex-col gap-0.5">
                        <span className="truncate text-sm">{name}</span>
                        <span className="truncate font-mono text-[10px] text-muted-foreground">
                          {item.type_id}
                        </span>
                        {description && (
                          <span className="max-w-[280px] truncate text-xs text-muted-foreground">
                            {description}
                          </span>
                        )}
                      </span>
                    </SelectItem>
                  );
                })}
              </SelectGroup>
            </SelectContent>
          </Select>
          <Button
            type="button"
            size="sm"
            variant="outline"
            className="w-full sm:w-auto"
            onClick={addRule}
            disabled={!selectedAddType}
          >
            <Plus data-icon="inline-start" />
            {t("transforms.add")}
          </Button>
        </div>
      </div>
      <div className="rounded-lg border p-3 space-y-2">
        {rules.length === 0 && (
          <p className="text-sm text-muted-foreground py-2">{t("transforms.emptyChain")}</p>
        )}
        {rules.map((rule, index) => {
          const registryItem = registryMap.get(rule.transform);
          const unknown = !registryItem;
          const name = registryItem
            ? resolveLocalizedText(registryItem.name, i18n.language, registryItem.type_id)
            : rule.transform;
          const modelsSummary =
            !rule.models || rule.models.length === 0
              ? t("transforms.modelsAllModels")
              : rule.models.join(", ");
          return (
            <div
              key={`${phase}-${index}-${rule.transform}`}
              className="flex items-center gap-3 rounded-md border bg-muted/30 px-3 py-2"
              draggable
              onDragStart={() => setDraggingIndex(index)}
              onDragOver={(e) => e.preventDefault()}
              onDrop={() => {
                if (draggingIndex === null) {
                  return;
                }
                reorder(draggingIndex, index);
                setDraggingIndex(null);
              }}
              onDragEnd={() => setDraggingIndex(null)}
            >
              <GripVertical className="h-4 w-4 text-muted-foreground cursor-grab" />
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-2">
                  <span className="truncate text-sm font-medium">{name}</span>
                  {unknown && (
                    <Badge variant="destructive">
                      {t("transforms.unknown")}
                    </Badge>
                  )}
                </div>
                <p className="truncate text-xs text-muted-foreground">
                  <span className="font-mono">{rule.transform}</span>
                  {" · "}
                  {modelsSummary}
                </p>
              </div>

              <div className="flex items-center gap-1">
                <Switch
                  checked={rule.enabled}
                  onCheckedChange={(enabled) =>
                    onChange(rules.map((entry, idx) => (idx === index ? { ...entry, enabled } : entry)))
                  }
                />
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  className="size-11 touch-manipulation sm:size-7"
                  aria-label={t("transforms.configure", { defaultValue: "Configure" })}
                  onClick={() => onConfigure(index)}
                >
                  <Settings2 className="h-3.5 w-3.5" />
                </Button>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  className="size-11 touch-manipulation sm:size-7 text-destructive hover:text-destructive"
                  aria-label={t("common.delete")}
                  onClick={() => onChange(rules.filter((_, idx) => idx !== index))}
                >
                  <Trash2 className="h-3.5 w-3.5" />
                </Button>
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
