import { useEffect, useId, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Link } from "react-router-dom";
import { ArrowDown, ArrowUp, Boxes, GripVertical, Pencil, Plus, Trash2 } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button, buttonVariants } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { FieldError } from "@/components/ui/field";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { toast } from "sonner";
import { AnimatedButton, PageWrapper } from "@/components/ui/motion";
import { QueryError } from "@/components/ui/query-error";
import { PageHeader } from "@/components/ui/page-header";
import { TablePageSkeleton } from "@/components/ui/page-skeleton";
import { EmptyState } from "@/components/ui/empty-state";
import { DataTableShell } from "@/components/ui/data-table-shell";
import {
  DataList,
  DataListActions,
  DataListBody,
  DataListCell,
  DataListHead,
  DataListHeader,
  DataListRow,
} from "@/components/ui/data-list";
import {
  useBillingPlans,
  useDashboardGroups,
  createGroupOptimistic,
  updateGroupOptimistic,
  reorderGroupsOptimistic,
  deleteGroupOptimistic,
} from "@/lib/swr";
import { DashboardApiError, type Group } from "@/lib/api";
import { toastApiError } from "@/lib/api-error";
import { normalizeMultiplier } from "@/lib/exact-decimal";
import { cn } from "@/lib/utils";

interface GroupFormState {
  name: string;
  description: string;
  billing_ratio: string;
  user_selectable: boolean;
}

type GroupFormField = "name" | "description" | "billing_ratio";

// GR §2.2 default: a new group is not user-selectable until an admin opts in.
const EMPTY_FORM: GroupFormState = {
  name: "",
  description: "",
  billing_ratio: "1",
  user_selectable: false,
};

function formFromGroup(group: Group): GroupFormState {
  return {
    name: group.name,
    description: group.description,
    billing_ratio: group.billing_ratio,
    user_selectable: group.user_selectable,
  };
}

function useFinePointer() {
  const [isFinePointer, setIsFinePointer] = useState(() =>
    typeof window === "undefined" ? false : window.matchMedia("(pointer: fine)").matches
  );

  useEffect(() => {
    const media = window.matchMedia("(pointer: fine)");
    const syncPointerState = () => setIsFinePointer(media.matches);
    syncPointerState();
    media.addEventListener("change", syncPointerState);
    return () => media.removeEventListener("change", syncPointerState);
  }, []);

  return isFinePointer;
}

export function GroupsPage() {
  const { t } = useTranslation();
  const formId = useId();
  const { data, error, isLoading, isValidating, mutate } = useDashboardGroups();
  const groups = useMemo(() => data ?? [], [data]);
  const { data: plans = [] } = useBillingPlans();

  // `editing === null` with `dialogOpen` means create mode.
  const [dialogOpen, setDialogOpen] = useState(false);
  const [editTarget, setEditTarget] = useState<Group | null>(null);
  const [form, setForm] = useState<GroupFormState>(EMPTY_FORM);
  const [initialForm, setInitialForm] = useState<GroupFormState>(EMPTY_FORM);
  const [errors, setErrors] = useState<Partial<Record<GroupFormField, string>>>({});
  const [deleteTarget, setDeleteTarget] = useState<Group | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [saving, setSaving] = useState(false);
  const [reordering, setReordering] = useState(false);
  const [draggingGroupId, setDraggingGroupId] = useState<string | null>(null);
  const canDrag = useFinePointer();

  const dirty = dialogOpen && JSON.stringify(form) !== JSON.stringify(initialForm);
  const fieldId = (field: GroupFormField) => `${formId}-${field}`;

  // GR-X4: plans whose only eligible group is the delete target block the delete.
  const blockingPlans = deleteTarget
    ? plans.filter((plan) => plan.group_ids.length === 1 && plan.group_ids[0] === deleteTarget.id)
    : [];

  const openDialog = (group: Group | null) => {
    const next = group ? formFromGroup(group) : EMPTY_FORM;
    setEditTarget(group);
    setForm(next);
    setInitialForm(next);
    setErrors({});
    setDialogOpen(true);
  };

  const updateField = <K extends keyof GroupFormState>(key: K, value: GroupFormState[K]) => {
    setForm((previous) => ({ ...previous, [key]: value }));
    setErrors((previous) => ({ ...previous, [key]: undefined }));
  };

  const focusField = (field: GroupFormField) => document.getElementById(fieldId(field))?.focus();

  const validateForm = () => {
    const next: Partial<Record<GroupFormField, string>> = {};
    const name = form.name.trim();
    if (!name || name.length > 64) next.name = t("groups.invalidName");
    const description = form.description.trim();
    if (description.length > 256) next.description = t("groups.invalidDescription");
    const billingRatio = normalizeMultiplier(form.billing_ratio);
    if (billingRatio === null) next.billing_ratio = t("groups.invalidBillingRatio");
    setErrors(next);
    const firstInvalid = (["name", "description", "billing_ratio"] as const).find((field) => next[field]);
    if (firstInvalid || billingRatio === null) {
      if (firstInvalid) focusField(firstInvalid);
      return null;
    }
    return { name, description, billing_ratio: billingRatio, user_selectable: form.user_selectable };
  };

  const handleServerError = (saveError: Error) => {
    if (saveError instanceof DashboardApiError && saveError.code === "group_name_exists") {
      setErrors({ name: t("apiErrors.group_name_exists") });
      focusField("name");
      return;
    }
    if (saveError instanceof DashboardApiError && saveError.code === "invalid_group_name") {
      setErrors({ name: t("groups.invalidName") });
      focusField("name");
      return;
    }
    toastApiError(saveError, t, t(editTarget ? "groups.updateFailed" : "groups.createFailed"));
  };

  const handleSubmit = async () => {
    if (saving) return;
    const validated = validateForm();
    if (!validated) return;
    setSaving(true);
    try {
      if (editTarget) {
        await updateGroupOptimistic(editTarget.id, validated, groups, handleServerError);
        toast.success(t("groups.updateSuccess"));
      } else {
        // GR-A3a: omitting sort_order appends the group after the current last one.
        await createGroupOptimistic(validated, groups, handleServerError);
        toast.success(t("groups.createSuccess"));
      }
      setDialogOpen(false);
    } catch {
      // optimistic helper already rolled back; the error is shown on the field or as a toast
    } finally {
      setSaving(false);
    }
  };

  const handleDelete = async () => {
    if (!deleteTarget || blockingPlans.length > 0) return;
    setDeleting(true);
    try {
      await deleteGroupOptimistic(deleteTarget.id, groups, (deleteError) =>
        toastApiError(deleteError, t, t("groups.deleteFailed"))
      );
      setDeleteTarget(null);
    } catch {
      // toast already shown
    } finally {
      setDeleting(false);
    }
  };

  const toggleUserSelectable = async (group: Group, userSelectable: boolean) => {
    await updateGroupOptimistic(
      group.id,
      { user_selectable: userSelectable },
      groups,
      (toggleError) => toastApiError(toggleError, t, t("groups.updateFailed"))
    ).catch(() => undefined);
  };

  const applyReorder = async (orderedGroups: Group[]) => {
    if (reordering) return;
    setReordering(true);
    try {
      await reorderGroupsOptimistic(
        orderedGroups.map((group) => group.id),
        groups,
        (reorderError) => toastApiError(reorderError, t, t("groups.reorderFailed"))
      );
    } catch {
      // optimistic helper already rolled back and toasted
    } finally {
      setDraggingGroupId(null);
      setReordering(false);
    }
  };

  const moveGroup = async (from: number, to: number) => {
    if (to < 0 || to >= groups.length || from === to || reordering) return;
    const next = [...groups];
    const [group] = next.splice(from, 1);
    next.splice(to, 0, group);
    await applyReorder(next);
  };

  const handleDrop = async (targetGroupId: string) => {
    if (!draggingGroupId || draggingGroupId === targetGroupId || reordering) {
      setDraggingGroupId(null);
      return;
    }
    const next = [...groups];
    const from = next.findIndex((group) => group.id === draggingGroupId);
    const to = next.findIndex((group) => group.id === targetGroupId);
    if (from < 0 || to < 0) {
      setDraggingGroupId(null);
      return;
    }
    const [group] = next.splice(from, 1);
    next.splice(to, 0, group);
    await applyReorder(next);
  };

  const describedBy = (field: GroupFormField, helpId?: string) =>
    [errors[field] ? `${fieldId(field)}-error` : null, helpId].filter(Boolean).join(" ") || undefined;

  const createButton = (
    <Button onClick={() => openDialog(null)}>
      <Plus data-icon="inline-start" aria-hidden="true" />
      {t("groups.create")}
    </Button>
  );

  if (isLoading) {
    return (
      <PageWrapper>
        <TablePageSkeleton rows={5} columns={5} />
      </PageWrapper>
    );
  }

  return (
    <PageWrapper className="space-y-6">
      <PageHeader
        title={t("groups.title")}
        description={t("groups.description")}
        actions={<AnimatedButton>{createButton}</AnimatedButton>}
      />

      {error ? <QueryError onRetry={mutate} retrying={isValidating} stale={data !== undefined} /> : null}

      {data !== undefined ? (
      <DataTableShell
        toolbar={
          <p className="ml-auto text-sm tabular-nums text-muted-foreground">
            {t("groups.count", { count: groups.length })}
          </p>
        }
        isEmpty={groups.length === 0}
        emptyState={
          <EmptyState
            icon={<Boxes className="size-10" aria-hidden="true" />}
            title={t("groups.emptyTitle")}
            description={t("groups.emptyDescription")}
            action={createButton}
          />
        }
      >
        <DataList columns="5.5rem minmax(0,1fr) 6rem 9rem 12.5rem">
          <DataListHeader>
            <DataListHead>{t("groups.position")}</DataListHead>
            <DataListHead>{t("groups.nameAndDescription")}</DataListHead>
            <DataListHead align="end">{t("groups.billingRatio")}</DataListHead>
            <DataListHead>{t("groups.userSelectable")}</DataListHead>
            <DataListHead align="end">{t("common.actions")}</DataListHead>
          </DataListHeader>
          <DataListBody aria-label={t("groups.title")}>
            {groups.map((group, index) => (
              <DataListRow
                key={group.id}
                className={cn(draggingGroupId === group.id && "bg-muted/50 opacity-60")}
                onDragOver={(event) => {
                  if (canDrag && !reordering) event.preventDefault();
                }}
                onDrop={(event) => {
                  event.preventDefault();
                  if (canDrag) void handleDrop(group.id);
                }}
              >
                <DataListCell label={t("groups.position")}>
                  <span className="inline-flex items-center gap-1">
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon"
                      className="hidden size-9 cursor-grab touch-manipulation active:cursor-grabbing [@media(pointer:fine)]:inline-flex"
                      draggable={canDrag && !reordering}
                      disabled={!canDrag || reordering}
                      aria-label={t("groups.dragToReorderItem", { name: group.name })}
                      title={t("groups.dragToReorder")}
                      onDragStart={(event) => {
                        event.dataTransfer.effectAllowed = "move";
                        event.dataTransfer.setData("text/plain", group.id);
                        setDraggingGroupId(group.id);
                      }}
                      onDragEnd={() => setDraggingGroupId(null)}
                    >
                      <GripVertical />
                    </Button>
                    <span className="min-w-6 tabular-nums text-muted-foreground">{index + 1}</span>
                  </span>
                </DataListCell>
                <DataListCell primary>
                  <div className="flex min-w-0 items-center gap-2">
                    <span className="truncate font-medium" title={group.name}>
                      {group.name}
                    </span>
                    {group.is_default && (
                      <Badge variant="secondary">{t("groups.defaultBadge")}</Badge>
                    )}
                  </div>
                  <p
                    className="mt-0.5 truncate text-muted-foreground"
                    title={group.description || undefined}
                  >
                    {group.description || "—"}
                  </p>
                </DataListCell>
                <DataListCell label={t("groups.billingRatio")} align="end">
                  <span className="tabular-nums">{group.billing_ratio}×</span>
                </DataListCell>
                <DataListCell label={t("groups.userSelectable")}>
                  <Switch
                    className="align-middle"
                    checked={group.user_selectable}
                    aria-label={t("groups.userSelectableItem", { name: group.name })}
                    onCheckedChange={(checked) => toggleUserSelectable(group, checked)}
                  />
                </DataListCell>
                <DataListActions>
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    className="size-11 touch-manipulation sm:size-9"
                    disabled={index === 0 || reordering}
                    aria-label={t("groups.moveUpItem", { name: group.name })}
                    title={t("groups.moveUp")}
                    onClick={() => void moveGroup(index, index - 1)}
                  >
                    <ArrowUp />
                  </Button>
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    className="size-11 touch-manipulation sm:size-9"
                    disabled={index === groups.length - 1 || reordering}
                    aria-label={t("groups.moveDownItem", { name: group.name })}
                    title={t("groups.moveDown")}
                    onClick={() => void moveGroup(index, index + 1)}
                  >
                    <ArrowDown />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon"
                    className="size-11 touch-manipulation sm:size-9"
                    aria-label={t("common.editItem", { name: group.name })}
                    onClick={() => openDialog(group)}
                  >
                    <Pencil />
                  </Button>
                  {/* Disabled buttons ignore pointer events, so the reason lives on a wrapper. */}
                  <span
                    className="inline-flex"
                    title={group.is_default ? t("groups.cannotDeleteDefault") : undefined}
                  >
                    <Button
                      variant="ghost"
                      size="icon"
                      className="size-11 touch-manipulation sm:size-9"
                      aria-label={t("common.deleteItem", { name: group.name })}
                      disabled={group.is_default}
                      onClick={() => setDeleteTarget(group)}
                    >
                      <Trash2 className="text-error-foreground" />
                    </Button>
                  </span>
                </DataListActions>
              </DataListRow>
            ))}
          </DataListBody>
        </DataList>
      </DataTableShell>
      ) : null}

      <Dialog open={dialogOpen} onOpenChange={(open) => !saving && setDialogOpen(open)}>
        <DialogContent dirty={dirty}>
          <DialogHeader>
            <DialogTitle>{t(editTarget ? "groups.edit" : "groups.create")}</DialogTitle>
            <DialogDescription>
              {editTarget
                ? editTarget.is_default
                  ? t("groups.editDefaultDescription")
                  : editTarget.name
                : t("groups.createDescription")}
            </DialogDescription>
          </DialogHeader>
          <form
            noValidate
            onSubmit={(event) => {
              event.preventDefault();
              void handleSubmit();
            }}
          >
            <div className="grid gap-4 py-4">
              <div className="grid gap-2">
                <Label htmlFor={fieldId("name")}>
                  {t("groups.name")}
                  <span aria-hidden="true" className="text-error-foreground">*</span>
                </Label>
                <Input
                  id={fieldId("name")}
                  value={form.name}
                  maxLength={64}
                  aria-required
                  aria-invalid={errors.name ? true : undefined}
                  aria-describedby={describedBy("name")}
                  onChange={(event) => updateField("name", event.target.value)}
                  placeholder={t("groups.namePlaceholder")}
                />
                {errors.name ? <FieldError id={`${fieldId("name")}-error`}>{errors.name}</FieldError> : null}
              </div>
              <div className="grid gap-2">
                <Label htmlFor={fieldId("description")}>{t("groups.descriptionLabel")}</Label>
                <Textarea
                  id={fieldId("description")}
                  value={form.description}
                  maxLength={256}
                  rows={2}
                  aria-invalid={errors.description ? true : undefined}
                  aria-describedby={describedBy("description", `${fieldId("description")}-help`)}
                  onChange={(event) => updateField("description", event.target.value)}
                  placeholder={t("groups.descriptionPlaceholder")}
                />
                <p id={`${fieldId("description")}-help`} className="text-xs text-muted-foreground">
                  {t("groups.descriptionHelp")}
                </p>
                {errors.description ? (
                  <FieldError id={`${fieldId("description")}-error`}>{errors.description}</FieldError>
                ) : null}
              </div>
              <div className="grid gap-2">
                <Label htmlFor={fieldId("billing_ratio")}>
                  {t("groups.billingRatio")}
                  <span aria-hidden="true" className="text-error-foreground">*</span>
                </Label>
                <Input
                  id={fieldId("billing_ratio")}
                  inputMode="decimal"
                  value={form.billing_ratio}
                  aria-required
                  aria-invalid={errors.billing_ratio ? true : undefined}
                  aria-describedby={describedBy("billing_ratio", `${fieldId("billing_ratio")}-help`)}
                  onChange={(event) => updateField("billing_ratio", event.target.value)}
                  placeholder="1"
                  className="font-mono"
                />
                <p id={`${fieldId("billing_ratio")}-help`} className="text-xs text-muted-foreground">
                  {t("groups.billingRatioHelp")}
                </p>
                {errors.billing_ratio ? (
                  <FieldError id={`${fieldId("billing_ratio")}-error`}>{errors.billing_ratio}</FieldError>
                ) : null}
              </div>
              <div className="flex items-center justify-between gap-4 rounded-lg border p-3">
                <div>
                  <Label htmlFor={`${formId}-user-selectable`}>{t("groups.userSelectable")}</Label>
                  <p id={`${formId}-user-selectable-help`} className="mt-1 text-xs text-muted-foreground">
                    {t("groups.userSelectableHelp")}
                  </p>
                </div>
                <Switch
                  id={`${formId}-user-selectable`}
                  aria-describedby={`${formId}-user-selectable-help`}
                  checked={form.user_selectable}
                  onCheckedChange={(checked) => updateField("user_selectable", checked)}
                />
              </div>
            </div>
            <DialogFooter>
              <Button type="button" variant="outline" disabled={saving} onClick={() => setDialogOpen(false)}>
                {t("common.cancel")}
              </Button>
              <Button type="submit" disabled={saving}>
                {saving ? t("common.saving") : t("common.save")}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>

      <AlertDialog
        open={deleteTarget !== null}
        onOpenChange={(open) => {
          if (!open && !deleting) setDeleteTarget(null);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("groups.deleteTitle")}</AlertDialogTitle>
            <AlertDialogDescription>
              {t("groups.deleteDescription", { name: deleteTarget?.name })}
            </AlertDialogDescription>
          </AlertDialogHeader>
          {blockingPlans.length > 0 ? (
            <div role="alert" className="space-y-2 rounded-md border border-error-border bg-error-soft p-3 text-sm">
              <p className="text-error-foreground">{t("groups.deleteBlockedByPlans")}</p>
              <ul className="list-disc pl-5">
                {blockingPlans.map((plan) => (
                  <li key={plan.id}>{plan.name}</li>
                ))}
              </ul>
              <Link to="/dashboard/plans" className="font-medium underline underline-offset-4">
                {t("groups.goToPlans")}
              </Link>
            </div>
          ) : null}
          <AlertDialogFooter>
            <AlertDialogCancel disabled={deleting}>{t("common.cancel")}</AlertDialogCancel>
            <AlertDialogAction
              className={buttonVariants({ variant: "destructive" })}
              disabled={deleting || blockingPlans.length > 0}
              onClick={(event) => {
                event.preventDefault();
                void handleDelete();
              }}
            >
              {t("common.delete")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </PageWrapper>
  );
}
