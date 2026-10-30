import { useMemo, useRef, useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import { AlertCircle, Braces, Check, Copy, History, WrapText } from "lucide-react";
import CodeMirror, {
  Decoration,
  EditorView,
  type Extension,
  type ReactCodeMirrorRef,
} from "@uiw/react-codemirror";
import { javascript } from "@codemirror/lang-javascript";
import { format as prettierFormat } from "prettier/standalone";
import * as prettierPluginBabel from "prettier/plugins/babel";
import * as prettierPluginEstree from "prettier/plugins/estree";
import { toast } from "sonner";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Toggle } from "@/components/ui/toggle";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { motion, springs } from "@/components/ui/motion";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";
import { useTheme } from "@/hooks/use-theme";
import { DashboardApiError, type CustomTransform } from "@/lib/api";
import { apiErrorMessage } from "@/lib/api-error";
import { formatDateTime } from "@/lib/format-time";
import { createCustomTransformOptimistic, updateCustomTransformOptimistic } from "@/lib/swr";
import { checkFrontmatter, splitLinePrefix } from "./frontmatter";

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

// custom-js-transforms.spec.md CJS-UI-5e
const DRAFT_STORAGE_KEY = "monoize-custom-transform-draft";

interface StoredDraft {
  source: string;
  saved_at: string;
}

function readDraft(): StoredDraft | null {
  try {
    const raw = sessionStorage.getItem(DRAFT_STORAGE_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as Partial<StoredDraft>;
    if (typeof parsed.source !== "string" || typeof parsed.saved_at !== "string") return null;
    return parsed.source === CREATE_TEMPLATE ? null : { source: parsed.source, saved_at: parsed.saved_at };
  } catch {
    return null;
  }
}

function writeDraft(source: string) {
  try {
    if (source === CREATE_TEMPLATE) {
      sessionStorage.removeItem(DRAFT_STORAGE_KEY);
    } else {
      const draft: StoredDraft = { source, saved_at: new Date().toISOString() };
      sessionStorage.setItem(DRAFT_STORAGE_KEY, JSON.stringify(draft));
    }
  } catch {
    // Storage full or unavailable: the draft is a convenience, editing continues.
  }
}

function removeDraft() {
  try {
    sessionStorage.removeItem(DRAFT_STORAGE_KEY);
  } catch {
    // Ignore unavailable storage.
  }
}

const errorLineDecoration = Decoration.line({ class: "cm-error-line" });
const errorLineTheme = EditorView.baseTheme({
  ".cm-error-line": {
    backgroundColor: "hsl(var(--error-border) / 0.35)",
    boxShadow: "inset 3px 0 0 hsl(var(--error-foreground))",
  },
});

function errorLinesExtension(lines: number[]): Extension {
  return [
    errorLineTheme,
    EditorView.decorations.compute(["doc"], (state) =>
      Decoration.set(
        lines
          .filter((line) => line >= 1 && line <= state.doc.lines)
          .map((line) => errorLineDecoration.range(state.doc.line(line).from)),
      ),
    ),
  ];
}

interface EditorIssue {
  line: number | null;
  message: string;
}

export function EditorDialog({
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
  const editorRef = useRef<ReactCodeMirrorRef>(null);
  const [source, setSource] = useState("");
  const [saving, setSaving] = useState(false);
  const [issues, setIssues] = useState<EditorIssue[]>([]);
  const [wrap, setWrap] = useState(false);
  const [pendingDraft, setPendingDraft] = useState<StoredDraft | null>(null);
  const initialSource = target?.source ?? CREATE_TEMPLATE;

  // Reset the dialog state when it opens for a different target.
  const [lastKey, setLastKey] = useState<string | null>(null);
  const openKey = open ? (target?.id ?? "__create__") : null;
  if (openKey !== lastKey) {
    setLastKey(openKey);
    if (openKey !== null) {
      setSource(initialSource);
      setIssues([]);
      // Wrap defaults on below the `sm` breakpoint (CJS-UI-5 item 3).
      setWrap(window.matchMedia("(max-width: 639px)").matches);
      setPendingDraft(target ? null : readDraft());
    }
  }

  const codeCopy = useCopyToClipboard();

  // Create mode keeps the buffer in sessionStorage once any stored draft has
  // been restored or discarded, so a reload or crash does not lose it.
  const persistDraft = open && target === null && pendingDraft === null;

  const updateSource = (next: string) => {
    setSource(next);
    setIssues([]);
    if (persistDraft) writeDraft(next);
  };

  const handleOpenChange = (next: boolean) => {
    // Closing is an explicit discard (the dirty guard has already confirmed it).
    if (!next && persistDraft) removeDraft();
    onOpenChange(next);
  };

  const restoreDraft = () => {
    if (!pendingDraft) return;
    setSource(pendingDraft.source);
    setIssues([]);
    setPendingDraft(null);
  };

  const discardDraft = () => {
    removeDraft();
    setPendingDraft(null);
  };

  const formatCode = async () => {
    try {
      const formatted = await prettierFormat(source, {
        parser: "babel",
        plugins: [prettierPluginBabel, prettierPluginEstree],
      });
      updateSource(formatted);
    } catch (error) {
      const message = error instanceof Error ? error.message.split("\n")[0] : String(error);
      toast.error(t("customTransforms.formatError", { message }));
    }
  };

  const jumpToLine = (line: number) => {
    const view = editorRef.current?.view;
    if (!view) return;
    const position = view.state.doc.line(Math.min(Math.max(line, 1), view.state.doc.lines)).from;
    view.dispatch({
      selection: { anchor: position },
      effects: EditorView.scrollIntoView(position, { y: "center" }),
    });
    view.focus();
  };

  const describeSaveError = (error: unknown): EditorIssue => {
    if (error instanceof DashboardApiError && error.code === "invalid_custom_transform") {
      // Script diagnostics are shown verbatim (CJS-UI-5 item 1).
      return splitLinePrefix(error.message);
    }
    if (error instanceof DashboardApiError && error.code === "custom_transform_exists") {
      return { line: checkFrontmatter(source).idLine, message: t("customTransforms.errorExists") };
    }
    return { line: null, message: apiErrorMessage(error, t) };
  };

  const save = async (event: FormEvent) => {
    event.preventDefault();
    if (saving || source.trim().length === 0) return;
    setIssues([]);
    const check = checkFrontmatter(source, target?.id);
    if (check.issues.length > 0) {
      setIssues(
        check.issues.map((issue) => ({
          line: issue.line,
          message: t(`customTransforms.issues.${issue.code}`, issue.params),
        })),
      );
      return;
    }
    setSaving(true);
    try {
      if (target) {
        await updateCustomTransformOptimistic(target.id, { source }, currentTransforms);
      } else {
        await createCustomTransformOptimistic(source);
        removeDraft();
      }
      toast.success(t("customTransforms.saveSuccess"));
      onOpenChange(false);
    } catch (error) {
      setIssues([describeSaveError(error)]);
    } finally {
      setSaving(false);
    }
  };

  const errorLines = issues.flatMap((issue) => (issue.line === null ? [] : [issue.line]));
  const errorLinesKey = errorLines.join(",");
  const extensions = useMemo(
    () => [
      javascript(),
      ...(wrap ? [EditorView.lineWrapping] : []),
      errorLinesExtension(errorLinesKey ? errorLinesKey.split(",").map(Number) : []),
    ],
    [wrap, errorLinesKey],
  );

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent
        dirty={source !== initialSource}
        className="h-[calc(100dvh-2rem)] sm:h-[85dvh] sm:max-w-5xl"
      >
        <DialogHeader>
          <div className="flex flex-wrap items-baseline gap-x-2 gap-y-1">
            <DialogTitle>
              {target
                ? t("customTransforms.editorEditTitle")
                : t("customTransforms.editorCreateTitle")}
            </DialogTitle>
            {target ? (
              <code className="rounded bg-muted px-1.5 py-0.5 font-mono text-xs">{target.id}</code>
            ) : null}
          </div>
          {target ? (
            <p className="text-xs text-muted-foreground">{t("customTransforms.idImmutable")}</p>
          ) : null}
          <DialogDescription>{t("customTransforms.editorDescription")}</DialogDescription>
        </DialogHeader>

        <form onSubmit={(event) => void save(event)} className="flex min-h-0 flex-1 flex-col gap-3">
          {pendingDraft ? (
            <Alert>
              <History aria-hidden="true" />
              <AlertTitle>
                {t("customTransforms.draftFound", { date: formatDateTime(pendingDraft.saved_at) })}
              </AlertTitle>
              <AlertDescription>
                <div className="mt-2 flex flex-wrap gap-2">
                  <Button type="button" size="sm" onClick={restoreDraft}>
                    {t("customTransforms.draftRestore")}
                  </Button>
                  <Button type="button" size="sm" variant="outline" onClick={discardDraft}>
                    {t("customTransforms.draftDiscard")}
                  </Button>
                </div>
              </AlertDescription>
            </Alert>
          ) : null}

          {issues.length > 0 ? (
            <Alert variant="destructive">
              <AlertCircle aria-hidden="true" />
              <AlertTitle>{t("customTransforms.saveFailed")}</AlertTitle>
              <AlertDescription>
                <ul className="max-h-32 space-y-1 overflow-y-auto">
                  {issues.map((issue, index) => (
                    <li key={index} className="flex gap-2">
                      {issue.line !== null ? (
                        <button
                          type="button"
                          className="shrink-0 font-medium underline underline-offset-2"
                          onClick={() => jumpToLine(issue.line!)}
                        >
                          {t("customTransforms.lineLabel", { line: issue.line })}
                        </button>
                      ) : null}
                      <span className="min-w-0 whitespace-pre-wrap break-words">{issue.message}</span>
                    </li>
                  ))}
                </ul>
              </AlertDescription>
            </Alert>
          ) : null}

          <div className="flex flex-wrap items-center justify-end gap-2">
            <Toggle
              variant="outline"
              size="sm"
              pressed={wrap}
              onPressedChange={setWrap}
            >
              <WrapText aria-hidden="true" />
              {t("customTransforms.wrapLines")}
            </Toggle>
            <Button type="button" variant="outline" size="sm" onClick={() => void codeCopy.copy(source)}>
              {codeCopy.copied ? (
                <Check data-icon="inline-start" aria-hidden="true" />
              ) : (
                <Copy data-icon="inline-start" aria-hidden="true" />
              )}
              {codeCopy.copied ? t("common.copied") : t("customTransforms.copyCode")}
            </Button>
            <Button type="button" variant="outline" size="sm" onClick={() => void formatCode()}>
              <Braces data-icon="inline-start" aria-hidden="true" />
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
              ref={editorRef}
              value={source}
              height="100%"
              className="h-full"
              theme={resolvedTheme}
              extensions={extensions}
              onChange={updateSource}
              basicSetup={{
                lineNumbers: true,
                foldGutter: true,
                highlightActiveLine: true,
                autocompletion: false,
              }}
            />
          </motion.div>

          <DialogFooter>
            <Button type="submit" disabled={saving || source.trim().length === 0}>
              {saving ? t("common.saving") : t("common.save")}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
