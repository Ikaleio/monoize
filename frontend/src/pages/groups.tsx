import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Boxes, Pencil, Plus, Trash2 } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
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
import { PageWrapper, motion, transitions } from "@/components/ui/motion";
import { PageHeader } from "@/components/ui/page-header";
import { TablePageSkeleton } from "@/components/ui/page-skeleton";
import { EmptyState } from "@/components/ui/empty-state";
import {
  useDashboardGroups,
  createGroupOptimistic,
  updateGroupOptimistic,
  deleteGroupOptimistic,
} from "@/lib/swr";
import type { Group } from "@/lib/api";

interface GroupFormState {
  name: string;
  description: string;
  user_selectable: boolean;
  sort_order: string;
}

const EMPTY_FORM: GroupFormState = {
  name: "",
  description: "",
  user_selectable: true,
  sort_order: "0",
};

function formFromGroup(group: Group): GroupFormState {
  return {
    name: group.name,
    description: group.description,
    user_selectable: group.user_selectable,
    sort_order: String(group.sort_order),
  };
}

export function GroupsPage() {
  const { t } = useTranslation();
  const { data, isLoading } = useDashboardGroups();
  const groups = useMemo(() => data ?? [], [data]);

  const [createOpen, setCreateOpen] = useState(false);
  const [form, setForm] = useState<GroupFormState>(EMPTY_FORM);
  const [editTarget, setEditTarget] = useState<Group | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<Group | null>(null);
  const [saving, setSaving] = useState(false);

  const openCreate = () => {
    setForm({ ...EMPTY_FORM, sort_order: String(groups.length) });
    setCreateOpen(true);
  };

  const validateForm = (): { name: string; description: string; sort_order: number } | null => {
    const name = form.name.trim();
    if (!name || name.length > 64) {
      toast.error(t("groups.invalidName"));
      return null;
    }
    const description = form.description.trim();
    if (description.length > 256) {
      toast.error(t("groups.invalidDescription"));
      return null;
    }
    const sortOrder = Number(form.sort_order.trim() || "0");
    if (!Number.isInteger(sortOrder)) {
      toast.error(t("groups.invalidSortOrder"));
      return null;
    }
    return { name, description, sort_order: sortOrder };
  };

  const handleCreate = async () => {
    const validated = validateForm();
    if (!validated || saving) return;
    setSaving(true);
    try {
      await createGroupOptimistic(
        { ...validated, user_selectable: form.user_selectable },
        groups,
        (error) => toast.error(error.message)
      );
      setCreateOpen(false);
      toast.success(t("common.success"));
    } catch {
      // optimistic helper already rolled back and toasted
    } finally {
      setSaving(false);
    }
  };

  const handleUpdate = async () => {
    if (!editTarget) return;
    const validated = validateForm();
    if (!validated || saving) return;
    setSaving(true);
    try {
      await updateGroupOptimistic(
        editTarget.id,
        { ...validated, user_selectable: form.user_selectable },
        groups,
        (error) => toast.error(error.message)
      );
      setEditTarget(null);
      toast.success(t("common.success"));
    } catch {
      // optimistic helper already rolled back and toasted
    } finally {
      setSaving(false);
    }
  };

  const handleDelete = async () => {
    if (!deleteTarget) return;
    try {
      await deleteGroupOptimistic(deleteTarget.id, groups, (error) =>
        toast.error(error.message)
      );
      toast.success(t("common.success"));
    } catch {
      // optimistic helper already rolled back and toasted
    } finally {
      setDeleteTarget(null);
    }
  };

  const toggleUserSelectable = async (group: Group, userSelectable: boolean) => {
    await updateGroupOptimistic(
      group.id,
      { user_selectable: userSelectable },
      groups,
      (error) => toast.error(error.message)
    ).catch(() => undefined);
  };

  const renderForm = (onSubmit: () => void) => (
    <>
      <div className="grid gap-4 py-4">
        <div className="grid gap-2">
          <Label htmlFor="group-name">{t("groups.name")}</Label>
          <Input
            id="group-name"
            value={form.name}
            maxLength={64}
            onChange={(e) => setForm({ ...form, name: e.target.value })}
            placeholder={t("groups.namePlaceholder")}
          />
        </div>
        <div className="grid gap-2">
          <Label htmlFor="group-description">{t("groups.descriptionLabel")}</Label>
          <Textarea
            id="group-description"
            value={form.description}
            maxLength={256}
            rows={2}
            onChange={(e) => setForm({ ...form, description: e.target.value })}
            placeholder={t("groups.descriptionPlaceholder")}
          />
          <p className="text-xs text-muted-foreground">{t("groups.descriptionHelp")}</p>
        </div>
        <div className="grid gap-2">
          <Label htmlFor="group-sort-order">{t("groups.sortOrder")}</Label>
          <Input
            id="group-sort-order"
            inputMode="numeric"
            value={form.sort_order}
            onChange={(e) => setForm({ ...form, sort_order: e.target.value })}
            placeholder="0"
          />
          <p className="text-xs text-muted-foreground">{t("groups.sortOrderHelp")}</p>
        </div>
        <div className="flex items-center justify-between rounded-lg border p-3">
          <div>
            <Label htmlFor="group-user-selectable">{t("groups.userSelectable")}</Label>
            <p className="mt-1 text-xs text-muted-foreground">
              {t("groups.userSelectableHelp")}
            </p>
          </div>
          <Switch
            id="group-user-selectable"
            checked={form.user_selectable}
            onCheckedChange={(checked) => setForm({ ...form, user_selectable: checked })}
          />
        </div>
      </div>
      <DialogFooter>
        <Button type="button" onClick={onSubmit} disabled={saving}>
          {saving ? t("common.loading") : t("common.save")}
        </Button>
      </DialogFooter>
    </>
  );

  return (
    <PageWrapper>
      <motion.div
        initial={{ opacity: 0, y: 12 }}
        animate={{ opacity: 1, y: 0 }}
        transition={transitions.normal}
        className="space-y-6"
      >
        <PageHeader
          title={t("groups.title")}
          description={t("groups.description")}
          actions={
            <Button onClick={openCreate}>
              <Plus className="mr-2 h-4 w-4" />
              {t("groups.create")}
            </Button>
          }
        />

        {isLoading ? (
          <TablePageSkeleton />
        ) : groups.length === 0 ? (
          <EmptyState
            variant="card"
            icon={<Boxes className="h-10 w-10 text-muted-foreground" />}
            title={t("groups.emptyTitle")}
            description={t("groups.emptyDescription")}
          />
        ) : (
          <div className="overflow-x-auto rounded-lg border">
            <table className="w-full min-w-[48rem] text-sm">
              <thead className="bg-muted/50 text-left text-muted-foreground">
                <tr>
                  <th className="px-4 py-2.5 font-medium">{t("groups.name")}</th>
                  <th className="px-4 py-2.5 font-medium">{t("groups.descriptionLabel")}</th>
                  <th className="px-4 py-2.5 font-medium">{t("groups.userSelectable")}</th>
                  <th className="px-4 py-2.5 font-medium">{t("groups.sortOrder")}</th>
                  <th className="px-4 py-2.5" />
                </tr>
              </thead>
              <tbody>
                {groups.map((group) => (
                  <tr key={group.id} className="border-t transition-colors hover:bg-accent/40">
                    <td className="px-4 py-3">
                      <div className="flex items-center gap-2">
                        <span className="font-medium">{group.name}</span>
                        {group.is_default && (
                          <Badge variant="secondary">{t("groups.defaultBadge")}</Badge>
                        )}
                      </div>
                    </td>
                    <td className="max-w-[20rem] px-4 py-3">
                      <span className="block truncate text-muted-foreground">
                        {group.description || "—"}
                      </span>
                    </td>
                    <td className="px-4 py-3">
                      <Switch
                        checked={group.user_selectable}
                        onCheckedChange={(checked) => toggleUserSelectable(group, checked)}
                      />
                    </td>
                    <td className="px-4 py-3 tabular-nums">{group.sort_order}</td>
                    <td className="px-4 py-3">
                      <div className="flex items-center justify-end gap-1">
                        <Button
                          variant="ghost"
                          size="icon"
                          className="size-11 touch-manipulation sm:size-9"
                          aria-label={t("common.edit")}
                          onClick={() => {
                            setForm(formFromGroup(group));
                            setEditTarget(group);
                          }}
                        >
                          <Pencil className="h-4 w-4" />
                        </Button>
                        <Button
                          variant="ghost"
                          size="icon"
                          className="size-11 touch-manipulation sm:size-9"
                          aria-label={t("common.delete")}
                          disabled={group.is_default}
                          title={group.is_default ? t("groups.cannotDeleteDefault") : undefined}
                          onClick={() => setDeleteTarget(group)}
                        >
                          <Trash2 className="h-4 w-4 text-destructive" />
                        </Button>
                      </div>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}

        {/* Create dialog */}
        <Dialog open={createOpen} onOpenChange={setCreateOpen}>
          <DialogContent>
            <DialogHeader>
              <DialogTitle>{t("groups.create")}</DialogTitle>
              <DialogDescription>{t("groups.createDescription")}</DialogDescription>
            </DialogHeader>
            {renderForm(handleCreate)}
          </DialogContent>
        </Dialog>

        {/* Edit dialog */}
        <Dialog open={editTarget !== null} onOpenChange={(open) => !open && setEditTarget(null)}>
          <DialogContent>
            <DialogHeader>
              <DialogTitle>{t("groups.edit")}</DialogTitle>
              <DialogDescription>
                {editTarget?.is_default
                  ? t("groups.editDefaultDescription")
                  : editTarget?.name}
              </DialogDescription>
            </DialogHeader>
            {renderForm(handleUpdate)}
          </DialogContent>
        </Dialog>

        {/* Delete confirm */}
        <AlertDialog
          open={deleteTarget !== null}
          onOpenChange={(open) => !open && setDeleteTarget(null)}
        >
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>{t("groups.deleteTitle")}</AlertDialogTitle>
              <AlertDialogDescription>
                {t("groups.deleteDescription", { name: deleteTarget?.name })}
              </AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel>{t("common.cancel")}</AlertDialogCancel>
              <AlertDialogAction
                className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
                onClick={handleDelete}
              >
                {t("common.delete")}
              </AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      </motion.div>
    </PageWrapper>
  );
}
