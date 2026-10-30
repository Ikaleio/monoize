import { useMemo, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { BookMarked, Check, Code2, Pencil, Plus, Trash2, UserRound } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { Switch } from "@/components/ui/switch";
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
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { AnimatedButton, PageWrapper, motion, transitions } from "@/components/ui/motion";
import { QueryError } from "@/components/ui/query-error";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";
import { useAuth } from "@/hooks/use-auth";
import { formatDate } from "@/lib/format-time";
import { toastApiError } from "@/lib/api-error";
import { PageHeader } from "@/components/ui/page-header";
import { EmptyState } from "@/components/ui/empty-state";
import {
  useCustomTransforms,
  useCustomTransformReferences,
  updateCustomTransformOptimistic,
  deleteCustomTransformOptimistic,
} from "@/lib/swr";
import type { CustomTransform, CustomTransformReference } from "@/lib/api";
import skillMarkdown from "@/skills/monoize-custom-transform-design.skill.md?raw";
import { EditorDialog } from "./custom-transforms/editor-dialog";
import { ChainEntryLinks, TransformReferences } from "./custom-transforms/references";

export function CustomTransformsPage() {
  const { t } = useTranslation();
  const { user } = useAuth();
  const { data, error, isLoading, isValidating, mutate } = useCustomTransforms();
  const { data: referenceMap, error: referencesError } = useCustomTransformReferences();
  const skillCopy = useCopyToClipboard();
  const transforms = useMemo(() => data ?? [], [data]);

  const [editorOpen, setEditorOpen] = useState(false);
  const [editTarget, setEditTarget] = useState<CustomTransform | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<CustomTransform | null>(null);
  const [disableTarget, setDisableTarget] = useState<CustomTransform | null>(null);

  // Undefined while loading or after a failed request (CJS-UI-8).
  const referencesOf = (id: string): CustomTransformReference[] | undefined =>
    referenceMap ? (referenceMap[id] ?? []) : undefined;

  const openCreate = () => {
    setEditTarget(null);
    setEditorOpen(true);
  };

  const openEdit = (item: CustomTransform) => {
    setEditTarget(item);
    setEditorOpen(true);
  };

  const setEnabled = async (item: CustomTransform, enabled: boolean) => {
    await updateCustomTransformOptimistic(item.id, { enabled }, transforms).catch((toggleError) =>
      toastApiError(toggleError, t),
    );
  };

  const toggleEnabled = (item: CustomTransform, enabled: boolean) => {
    // CJS-UI-9: disabling a referenced transform asks first.
    if (!enabled && (referencesOf(item.id)?.length ?? 0) > 0) {
      setDisableTarget(item);
      return;
    }
    void setEnabled(item, enabled);
  };

  const handleDelete = async () => {
    if (!deleteTarget) return;
    await deleteCustomTransformOptimistic(deleteTarget.id, transforms).catch((deleteError) =>
      toastApiError(deleteError, t),
    );
  };

  const deleteReferences = deleteTarget ? referencesOf(deleteTarget.id) : undefined;
  const disableCount = disableTarget ? (referencesOf(disableTarget.id)?.length ?? 0) : 0;

  return (
    <PageWrapper>
      <div className="space-y-6">
        <PageHeader
          title={t("customTransforms.title")}
          description={t("customTransforms.description")}
          actions={
            <>
              <Button variant="outline" onClick={() => void skillCopy.copy(skillMarkdown)}>
                {skillCopy.copied ? (
                  <Check data-icon="inline-start" aria-hidden="true" />
                ) : (
                  <BookMarked data-icon="inline-start" aria-hidden="true" />
                )}
                {skillCopy.copied ? t("common.copied") : t("customTransforms.copySkill")}
              </Button>
              <AnimatedButton>
                <Button onClick={openCreate}>
                  <Plus data-icon="inline-start" aria-hidden="true" />
                  {t("customTransforms.create")}
                </Button>
              </AnimatedButton>
            </>
          }
        />

        {error ? <QueryError onRetry={mutate} retrying={isValidating} stale={data !== undefined} /> : null}

        {isLoading ? (
          <CardGridSkeleton />
        ) : data === undefined ? null : transforms.length === 0 ? (
          <EmptyState
            variant="card"
            icon={<Code2 className="size-10" aria-hidden="true" />}
            title={t("customTransforms.emptyTitle")}
            description={
              <>
                {t("customTransforms.emptyDescription")} <ChainEntryLinks />
              </>
            }
            action={
              <Button onClick={openCreate}>
                <Plus data-icon="inline-start" aria-hidden="true" />
                {t("customTransforms.create")}
              </Button>
            }
          />
        ) : (
          <div className="grid grid-cols-1 gap-4 md:grid-cols-2 xl:grid-cols-3">
            {transforms.map((item, index) => (
              <motion.div
                key={item.id}
                initial={{ opacity: 0, y: 12 }}
                animate={{ opacity: 1, y: 0 }}
                transition={{ ...transitions.normal, delay: index * 0.05 }}
              >
                <TransformCard
                  item={item}
                  references={
                    <TransformReferences
                      references={referencesOf(item.id)}
                      loading={referenceMap === undefined && !referencesError}
                      currentUserId={user?.id}
                    />
                  }
                  onToggle={(enabled) => toggleEnabled(item, enabled)}
                  onEdit={() => openEdit(item)}
                  onDelete={() => setDeleteTarget(item)}
                />
              </motion.div>
            ))}
          </div>
        )}

        <EditorDialog
          open={editorOpen}
          target={editTarget}
          currentTransforms={transforms}
          onOpenChange={(open) => {
            setEditorOpen(open);
            if (!open) setEditTarget(null);
          }}
        />

        <ConfirmDeleteDialog
          open={deleteTarget !== null}
          onOpenChange={(open) => {
            if (!open) setDeleteTarget(null);
          }}
          title={t("customTransforms.deleteTitle")}
          description={
            deleteReferences === undefined
              ? t("customTransforms.deleteDescription", {
                  name: deleteTarget?.name,
                  id: deleteTarget?.id,
                })
              : deleteReferences.length > 0
                ? t("customTransforms.deleteDescriptionUsed", {
                    name: deleteTarget?.name,
                    id: deleteTarget?.id,
                    count: deleteReferences.length,
                  })
                : t("customTransforms.deleteDescriptionUnused", {
                    name: deleteTarget?.name,
                    id: deleteTarget?.id,
                  })
          }
          onConfirm={handleDelete}
        />

        <AlertDialog
          open={disableTarget !== null}
          onOpenChange={(open) => {
            if (!open) setDisableTarget(null);
          }}
        >
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>{t("customTransforms.disableTitle")}</AlertDialogTitle>
              <AlertDialogDescription>
                {t("customTransforms.disableDescription", {
                  name: disableTarget?.name,
                  count: disableCount,
                })}
              </AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel>{t("common.cancel")}</AlertDialogCancel>
              <AlertDialogAction
                onClick={() => {
                  if (disableTarget) void setEnabled(disableTarget, false);
                }}
              >
                {t("customTransforms.disableConfirm")}
              </AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      </div>
    </PageWrapper>
  );
}

function CardGridSkeleton() {
  return (
    <div className="grid grid-cols-1 gap-4 md:grid-cols-2 xl:grid-cols-3">
      {[0, 1, 2].map((index) => (
        <Card key={index} className="p-5">
          <div className="space-y-3">
            <div className="flex items-start justify-between gap-3">
              <div className="min-w-0 flex-1 space-y-2">
                <Skeleton className="h-5 w-2/3" />
                <Skeleton className="h-3.5 w-1/2" />
              </div>
              <Skeleton className="h-6 w-10 rounded-full" />
            </div>
            <Skeleton className="h-4 w-full" />
            <Skeleton className="h-4 w-3/4" />
            <div className="flex gap-1.5 pt-1">
              <Skeleton className="h-5 w-16 rounded-full" />
              <Skeleton className="h-5 w-16 rounded-full" />
              <Skeleton className="h-5 w-16 rounded-full" />
            </div>
          </div>
        </Card>
      ))}
    </div>
  );
}

function TransformCard({
  item,
  references,
  onToggle,
  onEdit,
  onDelete,
}: {
  item: CustomTransform;
  references: ReactNode;
  onToggle: (enabled: boolean) => void;
  onEdit: () => void;
  onDelete: () => void;
}) {
  const { t } = useTranslation();

  return (
    <Card className="flex h-full flex-col gap-3 p-5">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0 flex-1">
          <h3 className="truncate text-base font-semibold leading-tight">{item.name}</h3>
          <p className="mt-1 truncate font-mono text-xs text-muted-foreground">{item.id}</p>
        </div>
        <Switch
          checked={item.enabled}
          onCheckedChange={onToggle}
          aria-label={t("common.enableItem", { name: item.name })}
        />
      </div>

      <p className="line-clamp-2 min-h-10 text-sm text-muted-foreground">{item.description}</p>

      <div className="flex flex-wrap items-center gap-1.5">
        <Badge variant={item.visibility === "user" ? "default" : "secondary"}>
          {item.visibility === "user"
            ? t("customTransforms.visibilityUser")
            : t("customTransforms.visibilityAdmin")}
        </Badge>
        {item.phases.map((phase) => (
          <Badge key={phase} variant="outline" className="font-mono">
            {phase}
          </Badge>
        ))}
        {item.scopes.map((scope) => (
          <Badge key={scope} variant="outline" className="font-mono text-muted-foreground">
            {scope}
          </Badge>
        ))}
      </div>

      <div className="flex items-center">{references}</div>

      <div className="mt-auto flex items-center justify-between gap-2 border-t pt-3">
        <div className="flex min-w-0 items-center gap-1.5 text-sm text-muted-foreground">
          <UserRound className="size-4 shrink-0" aria-hidden="true" />
          <span className="truncate">{item.author}</span>
          <span className="shrink-0">·</span>
          <span className="truncate tabular-nums">
            {t("customTransforms.updatedAt", { date: formatDate(item.updated_at) })}
          </span>
        </div>
        <div className="flex shrink-0 items-center gap-1">
          <Button
            variant="ghost"
            size="icon"
            className="size-11 touch-manipulation sm:size-9"
            aria-label={t("common.editItem", { name: item.name })}
            onClick={onEdit}
          >
            <Pencil />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            className="size-11 touch-manipulation sm:size-9"
            aria-label={t("common.deleteItem", { name: item.name })}
            onClick={onDelete}
          >
            <Trash2 className="text-error-foreground" />
          </Button>
        </div>
      </div>
    </Card>
  );
}
