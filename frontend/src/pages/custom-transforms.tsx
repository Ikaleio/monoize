import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  BookMarked,
  Braces,
  Code2,
  Copy,
  Pencil,
  Plus,
  Trash2,
  UserRound,
} from "lucide-react";
import CodeMirror from "@uiw/react-codemirror";
import { javascript } from "@codemirror/lang-javascript";
import { format as prettierFormat } from "prettier/standalone";
import * as prettierPluginBabel from "prettier/plugins/babel";
import * as prettierPluginEstree from "prettier/plugins/estree";
import { toast } from "sonner";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { Switch } from "@/components/ui/switch";
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
import { PageWrapper, motion, springs, transitions } from "@/components/ui/motion";
import { PageHeader } from "@/components/ui/page-header";
import { EmptyState } from "@/components/ui/empty-state";
import { useTheme } from "@/hooks/use-theme";
import {
  useCustomTransforms,
  createCustomTransformOptimistic,
  updateCustomTransformOptimistic,
  deleteCustomTransformOptimistic,
} from "@/lib/swr";
import type { CustomTransform } from "@/lib/api";
import skillMarkdown from "@/skills/monoize-custom-transform-design.skill.md?raw";

const CREATE_TEMPLATE = `/**
 * @monoize-transform
 * id: js:my-transform
 * name: My Transform
 * description: Describe what this transform does.
 * author: admin
 * phase: request
 * scopes: provider, global, api_key
 * visibility: admin
 */

// Optional: declare a config schema rendered in the chain editor.
// const configSchema = {
//   type: "object",
//   properties: {
//     example: { type: "string", title: "Example" }
//   }
// };

function transform(ctx) {
  // ctx.phase: "request" | "response"
  // ctx.kind: "request" | "response" | "stream"
  // ctx.data: URP payload (mutable JSON)
  // ctx.config: rule config object
  // ctx.state: per-request state, survives across stream events
  // Monoize.fetch(url, options): host HTTP bridge
}
`;

async function copyToClipboard(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}

export function CustomTransformsPage() {
  const { t, i18n } = useTranslation();
  const { data, isLoading } = useCustomTransforms();
  const transforms = useMemo(() => data ?? [], [data]);

  const [editorOpen, setEditorOpen] = useState(false);
  const [editTarget, setEditTarget] = useState<CustomTransform | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<CustomTransform | null>(null);

  const openCreate = () => {
    setEditTarget(null);
    setEditorOpen(true);
  };

  const openEdit = (item: CustomTransform) => {
    setEditTarget(item);
    setEditorOpen(true);
  };

  const copySkill = async () => {
    if (await copyToClipboard(skillMarkdown)) {
      toast.success(t("customTransforms.copySkillSuccess"));
    } else {
      toast.error(t("customTransforms.copyFailed"));
    }
  };

  const toggleEnabled = async (item: CustomTransform, enabled: boolean) => {
    await updateCustomTransformOptimistic(item.id, { enabled }, transforms, (error) =>
      toast.error(error.message)
    ).catch(() => undefined);
  };

  const handleDelete = async () => {
    if (!deleteTarget) return;
    try {
      await deleteCustomTransformOptimistic(deleteTarget.id, transforms, (error) =>
        toast.error(error.message)
      );
      toast.success(t("common.success"));
    } catch {
      // optimistic helper already rolled back and toasted
    } finally {
      setDeleteTarget(null);
    }
  };

  return (
    <PageWrapper>
      <div className="space-y-6">
        <PageHeader
          title={t("customTransforms.title")}
          description={t("customTransforms.description")}
          actions={
            <>
              <Button variant="outline" onClick={copySkill}>
                <BookMarked className="mr-2 h-4 w-4" />
                {t("customTransforms.copySkill")}
              </Button>
              <Button onClick={openCreate}>
                <Plus className="mr-2 h-4 w-4" />
                {t("customTransforms.create")}
              </Button>
            </>
          }
        />

        {isLoading ? (
          <CardGridSkeleton />
        ) : transforms.length === 0 ? (
          <EmptyState
            variant="card"
            icon={<Code2 className="h-10 w-10 text-muted-foreground" />}
            title={t("customTransforms.emptyTitle")}
            description={t("customTransforms.emptyDescription")}
            action={
              <Button onClick={openCreate}>
                <Plus className="mr-2 h-4 w-4" />
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
                whileHover={{ y: -3 }}
              >
                <TransformCard
                  item={item}
                  language={i18n.language}
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

        <AlertDialog
          open={deleteTarget !== null}
          onOpenChange={(open) => !open && setDeleteTarget(null)}
        >
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>{t("customTransforms.deleteTitle")}</AlertDialogTitle>
              <AlertDialogDescription>
                {t("customTransforms.deleteDescription", {
                  name: deleteTarget?.name,
                  id: deleteTarget?.id,
                })}
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
  language,
  onToggle,
  onEdit,
  onDelete,
}: {
  item: CustomTransform;
  language: string;
  onToggle: (enabled: boolean) => void;
  onEdit: () => void;
  onDelete: () => void;
}) {
  const { t } = useTranslation();
  const updatedAt = new Date(item.updated_at).toLocaleDateString(language);

  return (
    <Card className="flex h-full flex-col gap-3 p-5 transition-colors hover:border-foreground/15">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0 flex-1">
          <h3 className="truncate text-base font-semibold leading-tight">{item.name}</h3>
          <p className="mt-1 truncate font-mono text-xs text-muted-foreground">{item.id}</p>
        </div>
        <Switch
          checked={item.enabled}
          onCheckedChange={onToggle}
          aria-label={t("common.enabled")}
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
          <Badge key={phase} variant="outline" className="font-mono text-[11px]">
            {phase}
          </Badge>
        ))}
        {item.scopes.map((scope) => (
          <Badge key={scope} variant="outline" className="font-mono text-[11px] text-muted-foreground">
            {scope}
          </Badge>
        ))}
      </div>

      <div className="mt-auto flex items-center justify-between gap-2 border-t pt-3">
        <div className="flex min-w-0 items-center gap-1.5 text-xs text-muted-foreground">
          <UserRound className="h-3.5 w-3.5 shrink-0" />
          <span className="truncate">{item.author}</span>
          <span className="shrink-0">·</span>
          <span className="truncate">{t("customTransforms.updatedAt", { date: updatedAt })}</span>
        </div>
        <div className="flex shrink-0 items-center gap-1">
          <Button
            variant="ghost"
            size="icon"
            className="size-9 sm:size-8"
            aria-label={t("common.edit")}
            onClick={onEdit}
          >
            <Pencil className="h-4 w-4" />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            className="size-9 sm:size-8"
            aria-label={t("common.delete")}
            onClick={onDelete}
          >
            <Trash2 className="h-4 w-4 text-destructive" />
          </Button>
        </div>
      </div>
    </Card>
  );
}

function EditorDialog({
  open,
  target,
  currentTransforms,
  onOpenChange,
}: {
  open: boolean;
  target: CustomTransform | null;
  currentTransforms: CustomTransform[];
  onOpenChange: (open: boolean) => void;
}) {
  const { t } = useTranslation();
  const { resolvedTheme } = useTheme();
  const [source, setSource] = useState("");
  const [saving, setSaving] = useState(false);
  // Reset the buffer when the dialog opens for a different target.
  const [lastKey, setLastKey] = useState<string | null>(null);
  const openKey = open ? (target?.id ?? "__create__") : null;
  if (openKey !== lastKey) {
    setLastKey(openKey);
    if (openKey !== null) {
      setSource(target?.source ?? CREATE_TEMPLATE);
    }
  }

  const copyCode = async () => {
    if (await copyToClipboard(source)) {
      toast.success(t("customTransforms.copyCodeSuccess"));
    } else {
      toast.error(t("customTransforms.copyFailed"));
    }
  };

  const formatCode = async () => {
    try {
      const formatted = await prettierFormat(source, {
        parser: "babel",
        plugins: [prettierPluginBabel, prettierPluginEstree],
      });
      setSource(formatted);
    } catch (error) {
      const message = error instanceof Error ? error.message.split("\n")[0] : String(error);
      toast.error(t("customTransforms.formatError", { message }));
    }
  };

  const save = async () => {
    if (saving) return;
    setSaving(true);
    try {
      if (target) {
        await updateCustomTransformOptimistic(target.id, { source }, currentTransforms);
      } else {
        await createCustomTransformOptimistic(source);
      }
      toast.success(t("common.success"));
      onOpenChange(false);
    } catch (error) {
      // Keep the dialog open with the buffer intact (CJS-UI-5) and surface
      // the server-side validation detail.
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      setSaving(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="flex max-h-[90dvh] flex-col sm:max-w-3xl">
        <DialogHeader>
          <DialogTitle>
            {target
              ? t("customTransforms.editorEditTitle")
              : t("customTransforms.editorCreateTitle")}
          </DialogTitle>
          <DialogDescription>{t("customTransforms.editorDescription")}</DialogDescription>
        </DialogHeader>

        <div className="flex items-center justify-end gap-2">
          <Button type="button" variant="outline" size="sm" onClick={copyCode}>
            <Copy className="mr-1.5 h-3.5 w-3.5" />
            {t("customTransforms.copyCode")}
          </Button>
          <Button type="button" variant="outline" size="sm" onClick={formatCode}>
            <Braces className="mr-1.5 h-3.5 w-3.5" />
            {t("customTransforms.format")}
          </Button>
        </div>

        <motion.div
          initial={{ opacity: 0, y: 8 }}
          animate={{ opacity: 1, y: 0 }}
          transition={springs.smooth}
          className="min-h-0 flex-1 overflow-hidden rounded-md border"
        >
          <CodeMirror
            value={source}
            height="420px"
            theme={resolvedTheme}
            extensions={[javascript()]}
            onChange={setSource}
            basicSetup={{
              lineNumbers: true,
              foldGutter: true,
              highlightActiveLine: true,
              autocompletion: false,
            }}
          />
        </motion.div>

        <DialogFooter>
          <Button type="button" onClick={save} disabled={saving || source.trim().length === 0}>
            {saving ? t("common.saving") : t("common.save")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
