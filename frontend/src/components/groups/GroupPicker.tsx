import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import { Reorder } from "framer-motion";
import { GripVertical, Plus, X } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Skeleton } from "@/components/ui/skeleton";
import { cn } from "@/lib/utils";
import type { Group } from "@/lib/api";

function useGroupIndex(groups: Group[]): Map<string, Group> {
  return useMemo(() => new Map(groups.map((g) => [g.id, g])), [groups]);
}

function GroupOptionLabel({ group }: { group: Group }) {
  const { t } = useTranslation();
  return (
    <span className="flex min-w-0 items-center gap-2">
      <span className="truncate font-medium">{group.name}</span>
      {group.description && (
        <span className="hidden truncate text-xs text-muted-foreground sm:inline">
          {group.description}
        </span>
      )}
      {group.is_default && (
        <Badge variant="outline" className="shrink-0 text-[10px]">
          {t("groupPicker.defaultBadge")}
        </Badge>
      )}
    </span>
  );
}

interface GroupSingleSelectProps {
  id?: string;
  value: string;
  groups: Group[];
  loading: boolean;
  disabled?: boolean;
  onChange: (groupId: string) => void;
}

/** Single-select group picker (users have exactly one group, GR-U1). */
export function GroupSingleSelect({
  id,
  value,
  groups,
  loading,
  disabled,
  onChange,
}: GroupSingleSelectProps) {
  const { t } = useTranslation();
  const index = useGroupIndex(groups);
  const known = value && index.has(value);

  if (loading) {
    return <Skeleton className="h-9 w-full rounded-md" />;
  }

  return (
    <Select value={known ? value : undefined} onValueChange={onChange} disabled={disabled}>
      <SelectTrigger id={id} className="w-full">
        <SelectValue placeholder={t("groupPicker.selectPlaceholder")} />
      </SelectTrigger>
      <SelectContent>
        {groups.map((group) => (
          <SelectItem key={group.id} value={group.id} textValue={group.name}>
            <GroupOptionLabel group={group} />
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}

interface GroupMultiSelectProps {
  value: string[];
  groups: Group[];
  loading: boolean;
  /** Enables drag-and-drop reordering; position 1 has the highest routing priority. */
  sortable?: boolean;
  disabled?: boolean;
  /** Restricts which registry rows are offered as addable options. */
  optionFilter?: (group: Group) => boolean;
  onChange: (groupIds: string[]) => void;
}

interface SelectedRowContentProps {
  group: Group | undefined;
  groupId: string;
  position: number | null;
  disabled?: boolean;
  onRemove: () => void;
}

function SelectedRowContent({ group, groupId, position, disabled, onRemove }: SelectedRowContentProps) {
  const { t } = useTranslation();
  return (
    <>
      {position !== null && (
        <>
          <GripVertical className="h-4 w-4 shrink-0 cursor-grab text-muted-foreground active:cursor-grabbing" />
          <span className="w-5 shrink-0 text-center font-mono text-xs text-muted-foreground">
            {position}
          </span>
        </>
      )}
      <span className="flex min-w-0 flex-1 items-center gap-2">
        <span className="truncate text-sm font-medium">
          {group ? group.name : t("groupPicker.unknownGroup")}
        </span>
        {group?.description && (
          <span className="hidden truncate text-xs text-muted-foreground sm:inline">
            {group.description}
          </span>
        )}
        {!group && (
          <span className="truncate font-mono text-xs text-muted-foreground">{groupId}</span>
        )}
        {group?.is_default && (
          <Badge variant="outline" className="shrink-0 text-[10px]">
            {t("groupPicker.defaultBadge")}
          </Badge>
        )}
      </span>
      <Button
        type="button"
        variant="ghost"
        size="icon"
        className="h-7 w-7 shrink-0"
        aria-label={t("common.delete")}
        disabled={disabled}
        onClick={onRemove}
      >
        <X className="h-3.5 w-3.5" />
      </Button>
    </>
  );
}

/**
 * Multi-select group picker for API keys, providers, and billing plans.
 * When `sortable`, the selected list is drag-reorderable and its order is the
 * routing priority order (TM-GRP-2 / R-GRP-2).
 */
export function GroupMultiSelect({
  value,
  groups,
  loading,
  sortable = false,
  disabled,
  optionFilter,
  onChange,
}: GroupMultiSelectProps) {
  const { t } = useTranslation();
  const index = useGroupIndex(groups);
  const available = useMemo(
    () =>
      groups.filter(
        (group) => !value.includes(group.id) && (optionFilter ? optionFilter(group) : true)
      ),
    [groups, value, optionFilter]
  );

  if (loading) {
    return (
      <div className="space-y-2">
        <Skeleton className="h-9 w-full rounded-md" />
        <Skeleton className="h-7 w-40 rounded-md" />
      </div>
    );
  }

  const removeId = (id: string) => onChange(value.filter((entry) => entry !== id));
  const rowClass =
    "flex items-center gap-2 rounded-md border bg-card px-2.5 py-1.5 select-none";

  return (
    <div className="space-y-2">
      {value.length > 0 &&
        (sortable && !disabled ? (
          <Reorder.Group
            axis="y"
            values={value}
            onReorder={onChange}
            className="flex list-none flex-col gap-1.5"
          >
            {value.map((id, position) => (
              <Reorder.Item key={id} value={id} className={cn(rowClass, "cursor-grab active:cursor-grabbing")}>
                <SelectedRowContent
                  group={index.get(id)}
                  groupId={id}
                  position={position + 1}
                  disabled={disabled}
                  onRemove={() => removeId(id)}
                />
              </Reorder.Item>
            ))}
          </Reorder.Group>
        ) : (
          <div className="flex flex-col gap-1.5">
            {value.map((id, position) => (
              <div key={id} className={rowClass}>
                <SelectedRowContent
                  group={index.get(id)}
                  groupId={id}
                  position={sortable ? position + 1 : null}
                  disabled={disabled}
                  onRemove={() => removeId(id)}
                />
              </div>
            ))}
          </div>
        ))}
      {sortable && value.length > 1 && (
        <p className="text-xs text-muted-foreground">{t("groupPicker.dragHint")}</p>
      )}
      {available.length > 0 && !disabled ? (
        <div className="flex flex-wrap gap-1.5">
          {available.map((group) => (
            <Button
              key={group.id}
              type="button"
              variant="outline"
              size="sm"
              className="h-7 max-w-full rounded-md px-2.5 text-xs"
              onClick={() => onChange([...value, group.id])}
            >
              <Plus className="mr-1 h-3 w-3 shrink-0" />
              <span className="truncate">{group.name}</span>
              {group.description && (
                <span className="ml-1.5 hidden max-w-[12rem] truncate font-normal text-muted-foreground sm:inline">
                  {group.description}
                </span>
              )}
            </Button>
          ))}
        </div>
      ) : value.length === 0 ? (
        <p className="text-xs text-muted-foreground">{t("groupPicker.noOptions")}</p>
      ) : null}
    </div>
  );
}
