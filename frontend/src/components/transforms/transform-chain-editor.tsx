import { Fragment, useMemo, useRef, useState, type DragEvent } from "react";
import { useTranslation } from "react-i18next";
import { Link } from "react-router-dom";
import {
  ArrowDownUp,
  ArrowUpDown,
  ChevronDown,
  ChevronUp,
  GripVertical,
  Plus,
  Settings2,
  Trash2,
} from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Skeleton } from "@/components/ui/skeleton";
import { StatusBadge } from "@/components/ui/status";
import { Switch } from "@/components/ui/switch";
import { useAuth } from "@/hooks/use-auth";
import type { Phase, TransformRegistryItem, TransformRuleConfig } from "@/lib/api";
import { resolveLocalizedText } from "./localized-text";
import { TransformItemConfigDialog } from "./transform-item-config-dialog";
import { transformRequiresConfig, validateTransformRule } from "./transform-schema";
import type { TransformLayer } from "./transform-layer";

type TransformChainEditorProps = {
  value: TransformRuleConfig[];
  registry: TransformRegistryItem[];
  loading?: boolean;
  onChange: (next: TransformRuleConfig[]) => void;
  layer: TransformLayer;
};

/** A rule with its position in the editor's full `value` array (TCU-10e). */
type IndexedRule = { rule: TransformRuleConfig; index: number };

type EditingState =
  | { kind: "existing"; index: number }
  | { kind: "pending"; rule: TransformRuleConfig };

const CUSTOM_TRANSFORM_PREFIX = "js:";

export function TransformChainEditor({
  value,
  registry,
  loading,
  onChange,
  layer,
}: TransformChainEditorProps) {
  const indexed = useMemo(() => value.map((rule, index) => ({ rule, index })), [value]);
  const requestRules = useMemo(
    () => indexed.filter((entry) => entry.rule.phase === "request"),
    [indexed]
  );
  const responseRules = useMemo(
    () => indexed.filter((entry) => entry.rule.phase === "response"),
    [indexed]
  );
  const registryMap = useMemo(
    () => new Map(registry.map((item) => [item.type_id, item])),
    [registry]
  );
  const [editing, setEditing] = useState<EditingState | null>(null);

  const addRule = (phase: Phase, item: TransformRegistryItem) => {
    const rule: TransformRuleConfig = {
      transform: item.type_id,
      enabled: true,
      models: null,
      phase,
      config: {},
    };
    // TCU-2d: a transform with required config joins the chain only on Save.
    if (transformRequiresConfig(item)) {
      setEditing({ kind: "pending", rule });
      return;
    }
    onChange([...value, rule]);
  };

  // Reorders one phase while keeping every rule of the other phase in its slot.
  const movePhaseRule = (phaseRules: IndexedRule[], from: number, to: number) => {
    if (from === to || from < 0 || to < 0 || from >= phaseRules.length || to >= phaseRules.length) {
      return;
    }
    const reordered = phaseRules.map((entry) => entry.rule);
    const [moved] = reordered.splice(from, 1);
    reordered.splice(to, 0, moved);
    const next = [...value];
    phaseRules.forEach((entry, position) => {
      next[entry.index] = reordered[position];
    });
    onChange(next);
  };

  const editingRule =
    editing?.kind === "pending"
      ? editing.rule
      : editing?.kind === "existing"
        ? (value[editing.index] ?? null)
        : null;
  const editingRegistry = editingRule ? registryMap.get(editingRule.transform) : undefined;

  const sectionProps = {
    registry,
    registryMap,
    onAdd: addRule,
    onToggle: (index: number, enabled: boolean) =>
      onChange(value.map((rule, idx) => (idx === index ? { ...rule, enabled } : rule))),
    onDelete: (index: number) => onChange(value.filter((_, idx) => idx !== index)),
    onMove: movePhaseRule,
    onConfigure: (index: number) => setEditing({ kind: "existing", index }),
  };

  return (
    <div className="space-y-4" data-transform-layer={layer}>
      <LayerOrderLine layer={layer} />
      {loading ? (
        <TransformChainSkeleton />
      ) : (
        <>
          <PhaseChainSection phase="request" rules={requestRules} {...sectionProps} />
          <PhaseChainSection phase="response" rules={responseRules} {...sectionProps} />
        </>
      )}

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
          if (editing?.kind === "pending") {
            onChange([...value, nextRule]);
          } else if (editing?.kind === "existing") {
            onChange(value.map((rule, idx) => (idx === editing.index ? nextRule : rule)));
          }
        }}
      />
    </div>
  );
}

const LAYER_LINKS: Record<TransformLayer, string> = {
  provider: "/dashboard/providers",
  global: "/dashboard/admin-settings?setting=global_transforms",
  apiKey: "/dashboard/tokens",
};

const LAYER_ORDER: TransformLayer[] = ["provider", "global", "apiKey"];

/** Read-only execution-order line with the current layer emphasized (TCU-13). */
function LayerOrderLine({ layer }: { layer: TransformLayer }) {
  const { t } = useTranslation();
  const { user } = useAuth();
  const canOpenLayers = user?.role === "admin" || user?.role === "super_admin";
  const labels: Record<TransformLayer, string> = {
    provider: t("transforms.layerProvider"),
    global: t("transforms.layerGlobal"),
    apiKey: t("transforms.layerApiKey"),
  };

  return (
    <p className="text-xs text-muted-foreground">
      {t("transforms.executionOrderLabel")}
      {LAYER_ORDER.map((entry, position) => (
        <Fragment key={entry}>
          {position > 0 && <span aria-hidden="true">{" → "}</span>}
          {entry === layer ? (
            <span aria-current="true" className="font-medium text-foreground">
              {labels[entry]}
            </span>
          ) : canOpenLayers ? (
            <Link
              to={LAYER_LINKS[entry]}
              className="underline-offset-4 hover:text-foreground hover:underline"
            >
              {labels[entry]}
            </Link>
          ) : (
            <span>{labels[entry]}</span>
          )}
        </Fragment>
      ))}
    </p>
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
  rules: IndexedRule[];
  registry: TransformRegistryItem[];
  registryMap: Map<string, TransformRegistryItem>;
  onAdd: (phase: Phase, item: TransformRegistryItem) => void;
  onToggle: (index: number, enabled: boolean) => void;
  onDelete: (index: number) => void;
  onMove: (phaseRules: IndexedRule[], from: number, to: number) => void;
  onConfigure: (index: number) => void;
};

function PhaseChainSection({
  phase,
  rules,
  registry,
  registryMap,
  onAdd,
  onToggle,
  onDelete,
  onMove,
  onConfigure,
}: PhaseChainSectionProps) {
  const { t, i18n } = useTranslation();
  // Drag state is section-local, so a drop in the other phase section is never accepted.
  const [draggingPosition, setDraggingPosition] = useState<number | null>(null);
  const available = useMemo(
    () => registry.filter((item) => item.supported_phases.includes(phase)),
    [registry, phase]
  );

  const title = phase === "request" ? t("transforms.requestChain") : t("transforms.responseChain");
  const PhaseIcon = phase === "request" ? ArrowUpDown : ArrowDownUp;

  const startDrag = (event: DragEvent<HTMLElement>, position: number) => {
    event.dataTransfer.effectAllowed = "move";
    // Firefox starts a drag only when data is set.
    event.dataTransfer.setData("text/plain", "");
    const row = event.currentTarget.closest<HTMLElement>("[data-transform-rule-index]");
    if (row) {
      event.dataTransfer.setDragImage(row, 16, 16);
    }
    setDraggingPosition(position);
  };

  const listRef = useRef<HTMLDivElement>(null);
  // Row keys include the rule index, so a move remounts the rows; keep keyboard focus on
  // the moved rule, falling back to the opposite arrow when it reached the end.
  const moveByButton = (from: number, to: number) => {
    onMove(rules, from, to);
    const direction = to < from ? "up" : "down";
    requestAnimationFrame(() => {
      const row = listRef.current?.querySelectorAll<HTMLElement>(":scope > [data-transform-rule-index]")[to];
      const same = row?.querySelector<HTMLButtonElement>(`[data-move="${direction}"]`);
      const other = row?.querySelector<HTMLButtonElement>(`[data-move="${direction === "up" ? "down" : "up"}"]`);
      (same && !same.disabled ? same : other)?.focus();
    });
  };

  return (
    <div className="space-y-3">
      <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
        <div className="flex items-center gap-2">
          <PhaseIcon className="h-4 w-4 text-muted-foreground" aria-hidden="true" />
          <h3 className="text-sm font-medium">{title}</h3>
          <Badge variant="secondary" className="text-xs">{rules.length}</Badge>
        </div>
        <AddTransformCombobox
          items={available}
          language={i18n.language}
          onSelect={(item) => onAdd(phase, item)}
        />
      </div>
      <div ref={listRef} className="space-y-2 rounded-lg border p-3">
        {rules.length === 0 && (
          <p className="py-2 text-sm text-muted-foreground">{t("transforms.emptyChain")}</p>
        )}
        {rules.map(({ rule, index }, position) => {
          const registryItem = registryMap.get(rule.transform);
          const name = registryItem
            ? resolveLocalizedText(registryItem.name, i18n.language, registryItem.type_id)
            : rule.transform;
          const needsConfig = registryItem
            ? validateTransformRule(rule, registryItem).length > 0
            : false;
          const modelsSummary =
            !rule.models || rule.models.length === 0
              ? t("transforms.modelsAllModels")
              : rule.models.join(", ");
          return (
            <div
              key={`${phase}-${index}-${rule.transform}`}
              data-transform-rule-index={index}
              className="flex flex-wrap items-center gap-3 rounded-md border bg-muted/30 px-3 py-2 sm:flex-nowrap"
              onDragOver={(event) => {
                if (draggingPosition === null) {
                  return;
                }
                event.preventDefault();
                event.dataTransfer.dropEffect = "move";
              }}
              onDrop={(event) => {
                if (draggingPosition === null) {
                  return;
                }
                event.preventDefault();
                onMove(rules, draggingPosition, position);
                setDraggingPosition(null);
              }}
            >
              <span
                draggable
                aria-hidden="true"
                className="flex shrink-0 cursor-grab touch-none items-center text-muted-foreground"
                onDragStart={(event) => startDrag(event, position)}
                onDragEnd={() => setDraggingPosition(null)}
              >
                <GripVertical className="h-4 w-4" />
              </span>
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-2">
                  <span className="truncate text-sm font-medium">{name}</span>
                  <RuleStatusBadge
                    registered={Boolean(registryItem)}
                    custom={rule.transform.startsWith(CUSTOM_TRANSFORM_PREFIX)}
                    needsConfig={needsConfig}
                  />
                </div>
                <p className="truncate text-xs text-muted-foreground">
                  <span className="font-mono">{rule.transform}</span>
                  {" · "}
                  {modelsSummary}
                </p>
              </div>

              <div className="flex w-full items-center justify-end gap-1 sm:w-auto">
                <Switch
                  checked={rule.enabled}
                  aria-label={t("transforms.enableRule", { name })}
                  onCheckedChange={(enabled) => onToggle(index, enabled)}
                />
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  className="size-11 touch-manipulation sm:size-7"
                  aria-label={t("transforms.arrayMoveUp")}
                  data-move="up"
                  disabled={position === 0}
                  onClick={() => moveByButton(position, position - 1)}
                >
                  <ChevronUp aria-hidden="true" />
                </Button>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  className="size-11 touch-manipulation sm:size-7"
                  aria-label={t("transforms.arrayMoveDown")}
                  data-move="down"
                  disabled={position === rules.length - 1}
                  onClick={() => moveByButton(position, position + 1)}
                >
                  <ChevronDown aria-hidden="true" />
                </Button>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  className="size-11 touch-manipulation sm:size-7"
                  aria-label={t("transforms.configure")}
                  data-transform-configure=""
                  onClick={() => onConfigure(index)}
                >
                  <Settings2 aria-hidden="true" />
                </Button>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  className="size-11 touch-manipulation sm:size-7"
                  aria-label={t("common.delete")}
                  onClick={() => onDelete(index)}
                >
                  <Trash2 className="text-error-foreground" aria-hidden="true" />
                </Button>
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}

/** Row status badge per TCU-12a. */
function RuleStatusBadge({
  registered,
  custom,
  needsConfig,
}: {
  registered: boolean;
  custom: boolean;
  needsConfig: boolean;
}) {
  const { t } = useTranslation();
  if (!registered) {
    return custom ? (
      <StatusBadge variant="warning">
        {t("transforms.customUnavailable")}
      </StatusBadge>
    ) : (
      <Badge variant="destructive">{t("transforms.unknown")}</Badge>
    );
  }
  return needsConfig ? (
    <StatusBadge variant="warning">{t("transforms.needsConfig")}</StatusBadge>
  ) : null;
}

/** Searchable add-transform list grouped into built-in and custom items (TCU-2b–TCU-2d). */
function AddTransformCombobox({
  items,
  language,
  onSelect,
}: {
  items: TransformRegistryItem[];
  language: string;
  onSelect: (item: TransformRegistryItem) => void;
}) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const builtin = items.filter((item) => !item.type_id.startsWith(CUSTOM_TRANSFORM_PREFIX));
  const custom = items.filter((item) => item.type_id.startsWith(CUSTOM_TRANSFORM_PREFIX));
  const groups = [
    { heading: t("transforms.groupBuiltin"), entries: builtin },
    { heading: t("transforms.groupCustom"), entries: custom },
  ].filter((group) => group.entries.length > 0);

  return (
    // `modal` keeps wheel scrolling inside the list when the editor sits in a modal dialog.
    <Popover open={open} onOpenChange={setOpen} modal>
      <PopoverTrigger asChild>
        <Button
          type="button"
          size="sm"
          variant="outline"
          className="w-full sm:w-auto"
          disabled={items.length === 0}
        >
          <Plus data-icon="inline-start" aria-hidden="true" />
          {t("transforms.addTransform")}
        </Button>
      </PopoverTrigger>
      <PopoverContent className="w-[min(320px,calc(100vw-2rem))] p-0" align="end">
        <Command>
          <CommandInput placeholder={t("transforms.searchTransforms")} />
          <CommandList>
            <CommandEmpty>{t("transforms.noMatchingTransform")}</CommandEmpty>
            {groups.map((group) => (
              <CommandGroup key={group.heading} heading={group.heading}>
                {group.entries.map((item) => {
                  const name = resolveLocalizedText(item.name, language, item.type_id);
                  const description = resolveLocalizedText(item.description, language, "");
                  return (
                    <CommandItem
                      key={item.type_id}
                      value={item.type_id}
                      keywords={description ? [name, description] : [name]}
                      onSelect={() => {
                        setOpen(false);
                        onSelect(item);
                      }}
                    >
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
                    </CommandItem>
                  );
                })}
              </CommandGroup>
            ))}
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  );
}
