import { useId, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import type { ReasoningUIPart, UIMessage } from "ai";
import {
  Brush,
  Check,
  ChevronRight,
  Copy,
  Download,
  FileText,
  Pencil,
  RefreshCcw,
  Trash2,
} from "lucide-react";
import { AnimatePresence, useReducedMotion } from "framer-motion";
import { Streamdown } from "streamdown";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { motion, transitions } from "@/components/ui/motion";
import {
  reasoningPartKind,
  type ReasoningPartKind,
} from "@/components/playground/responses-sse";
import { cn } from "@/lib/utils";

function messageText(message: UIMessage): string {
  return message.parts
    .filter((part): part is Extract<typeof part, { type: "text" }> => part.type === "text")
    .map((part) => part.text)
    .join("\n\n");
}

function ActionButton({
  label,
  onClick,
  disabled,
  destructive,
  children,
}: {
  label: string;
  onClick: () => void;
  disabled?: boolean;
  destructive?: boolean;
  children: React.ReactNode;
}) {
  return (
    <Button
      variant="ghost"
      size="icon"
      aria-label={label}
      title={label}
      onClick={onClick}
      disabled={disabled}
      className={cn(
        "size-11 touch-manipulation text-muted-foreground sm:size-7",
        destructive ? "hover:text-destructive" : "hover:text-foreground",
      )}
    >
      {children}
    </Button>
  );
}

function MessageImage({
  url,
  alt,
  onEditImage,
}: {
  url: string;
  alt: string;
  onEditImage: (url: string) => void;
}) {
  const { t } = useTranslation();

  const download = () => {
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.download = "playground-image.png";
    anchor.click();
  };

  return (
    <div className="group/image relative w-fit max-w-full">
      <img
        src={url}
        alt={alt}
        className="max-h-96 max-w-full rounded-xl border object-contain"
      />
      <div className="mt-1 flex items-center gap-0.5">
        <ActionButton label={t("playground.download")} onClick={download}>
          <Download className="h-3.5 w-3.5" />
        </ActionButton>
        <ActionButton
          label={t("playground.editImage")}
          onClick={() => onEditImage(url)}
        >
          <Brush className="h-3.5 w-3.5" />
        </ActionButton>
      </div>
    </div>
  );
}

interface ReasoningSection {
  kind: ReasoningPartKind;
  parts: ReasoningUIPart[];
}

/**
 * Groups a message's non-empty reasoning parts into at most two labeled
 * sections (raw `content` vs `summary`, PG-RD2), ordered by the first part of
 * each kind in `parts` order.
 */
function collectReasoningSections(message: UIMessage): ReasoningSection[] {
  const sections: ReasoningSection[] = [];
  for (const part of message.parts) {
    if (part.type !== "reasoning" || part.text.trim().length === 0) continue;
    const kind = reasoningPartKind(part.id);
    let section = sections.find((candidate) => candidate.kind === kind);
    if (!section) {
      section = { kind, parts: [] };
      sections.push(section);
    }
    section.parts.push(part);
  }
  return sections;
}

function ReasoningBlock({
  label,
  text,
  streaming,
}: {
  label: string;
  text: string;
  streaming: boolean;
}) {
  // null = automatic control (open while streaming); a manual toggle takes
  // over for the remaining lifetime of this rendered message (PG-RD2b).
  const [userOpen, setUserOpen] = useState<boolean | null>(null);
  const shouldReduceMotion = useReducedMotion();
  const panelId = useId();
  const open = userOpen ?? streaming;

  return (
    <div className="rounded-lg border border-dashed bg-muted/30">
      <button
        type="button"
        onClick={() => setUserOpen(!open)}
        aria-expanded={open}
        aria-controls={panelId}
        className="flex w-full items-center gap-1.5 px-3 py-2 text-xs font-medium text-muted-foreground transition-colors hover:text-foreground"
      >
        <motion.span
          animate={{ rotate: open ? 90 : 0 }}
          transition={shouldReduceMotion ? { duration: 0 } : transitions.fast}
          className="inline-flex"
        >
          <ChevronRight className="h-3.5 w-3.5" />
        </motion.span>
        {label}
        {streaming && (
          <motion.span
            animate={{ opacity: [0.3, 1, 0.3] }}
            transition={{ duration: 1.2, repeat: Infinity }}
            className="ml-0.5 inline-block h-1.5 w-1.5 rounded-full bg-muted-foreground"
            aria-hidden
          />
        )}
      </button>
      <AnimatePresence initial={false}>
        {open && (
          <motion.div
            id={panelId}
            initial={{ height: 0, opacity: 0 }}
            animate={{ height: "auto", opacity: 1 }}
            exit={{ height: 0, opacity: 0 }}
            transition={shouldReduceMotion ? { duration: 0 } : transitions.normal}
            className="overflow-hidden"
          >
            <p className="whitespace-pre-wrap px-3 pb-3 text-xs leading-relaxed text-muted-foreground">
              {text}
            </p>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}

export interface ChatMessageProps {
  message: UIMessage;
  /** True while this message is the actively streaming assistant response. */
  isStreaming: boolean;
  /** True while any request is in flight; edit/delete/regenerate are blocked. */
  busy: boolean;
  onEditUser: (messageId: string, text: string) => void;
  onEditAssistant: (messageId: string, text: string) => void;
  onDelete: (messageId: string) => void;
  onRegenerate: (messageId: string) => void;
  onEditImage: (url: string) => void;
}

export function ChatMessage({
  message,
  isStreaming,
  busy,
  onEditUser,
  onEditAssistant,
  onDelete,
  onRegenerate,
  onEditImage,
}: ChatMessageProps) {
  const { t } = useTranslation();
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");
  const [copied, setCopied] = useState(false);

  const isUser = message.role === "user";
  const text = useMemo(() => messageText(message), [message]);
  const imageParts = message.parts.filter(
    (part): part is Extract<typeof part, { type: "file" }> =>
      part.type === "file" && part.mediaType.startsWith("image"),
  );
  const fileParts = message.parts.filter(
    (part): part is Extract<typeof part, { type: "file" }> =>
      part.type === "file" && !part.mediaType.startsWith("image"),
  );
  const reasoningSections = useMemo(
    () => collectReasoningSections(message),
    [message],
  );

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      /* clipboard unavailable */
    }
  };

  const startEdit = () => {
    setDraft(text);
    setEditing(true);
  };

  const confirmEdit = () => {
    const trimmed = draft.trim();
    if (!trimmed) return;
    setEditing(false);
    if (isUser) {
      onEditUser(message.id, trimmed);
    } else {
      onEditAssistant(message.id, trimmed);
    }
  };

  if (editing) {
    return (
      <motion.div
        initial={{ opacity: 0.6, scale: 0.99 }}
        animate={{ opacity: 1, scale: 1 }}
        transition={transitions.fast}
        className={cn("w-full", isUser && "flex justify-end")}
      >
        <div className={cn("w-full space-y-2", isUser && "max-w-[85%]")}>
          <Textarea
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            autoFocus
            className="min-h-[80px] resize-y text-sm"
          />
          <div className="flex items-center justify-end gap-2">
            <Button variant="ghost" size="sm" onClick={() => setEditing(false)}>
              {t("common.cancel")}
            </Button>
            <Button size="sm" onClick={confirmEdit} disabled={!draft.trim()}>
              {isUser ? t("playground.saveAndSend") : t("common.save")}
            </Button>
          </div>
        </div>
      </motion.div>
    );
  }

  const actions = (
    <div
      className={cn(
        "flex items-center gap-0.5 transition-opacity sm:opacity-0 sm:group-hover:opacity-100 sm:group-focus-within:opacity-100",
        isUser ? "justify-end" : "justify-start",
      )}
    >
      {text && (
        <ActionButton label={t("playground.copy")} onClick={copy}>
          {copied ? (
            <Check className="h-3.5 w-3.5 text-success" />
          ) : (
            <Copy className="h-3.5 w-3.5" />
          )}
        </ActionButton>
      )}
      {!isUser && (
        <ActionButton
          label={t("playground.regenerate")}
          onClick={() => onRegenerate(message.id)}
          disabled={busy}
        >
          <RefreshCcw className="h-3.5 w-3.5" />
        </ActionButton>
      )}
      <ActionButton
        label={t("common.edit")}
        onClick={startEdit}
        disabled={busy}
      >
        <Pencil className="h-3.5 w-3.5" />
      </ActionButton>
      <ActionButton
        label={t("common.delete")}
        onClick={() => onDelete(message.id)}
        disabled={busy}
        destructive
      >
        <Trash2 className="h-3.5 w-3.5" />
      </ActionButton>
    </div>
  );

  if (isUser) {
    return (
      <div className="group flex w-full flex-col items-end gap-1.5">
        {imageParts.length > 0 && (
          <div className="flex max-w-[85%] flex-wrap justify-end gap-2">
            {imageParts.map((part, index) => (
              <img
                key={index}
                src={part.url}
                alt={part.filename ?? t("playground.attachmentAlt")}
                className="max-h-48 rounded-xl border object-contain"
              />
            ))}
          </div>
        )}
        {fileParts.length > 0 && (
          <div className="flex max-w-[85%] flex-wrap justify-end gap-2">
            {fileParts.map((part, index) => (
              <a
                key={index}
                href={part.url}
                download={part.filename || "attachment"}
                className="flex max-w-64 items-center gap-2 rounded-xl border bg-muted/50 px-3 py-2 text-left text-xs transition-colors hover:bg-muted"
              >
                <FileText className="h-4 w-4 shrink-0 text-muted-foreground" />
                <span className="min-w-0 truncate">
                  {part.filename || t("playground.attachmentFile")}
                </span>
              </a>
            ))}
          </div>
        )}
        {text && (
          <div className="max-w-[85%] whitespace-pre-wrap rounded-2xl bg-muted px-4 py-2.5 text-sm leading-relaxed">
            {text}
          </div>
        )}
        {actions}
      </div>
    );
  }

  return (
    <div className="group flex w-full flex-col gap-2">
      {reasoningSections.map((section) => (
        <ReasoningBlock
          key={section.kind}
          label={t(
            section.kind === "content"
              ? "playground.reasoning"
              : "playground.reasoningSummary",
          )}
          text={section.parts.map((part) => part.text).join("\n\n")}
          streaming={
            isStreaming &&
            section.parts.some((part) => part.state === "streaming")
          }
        />
      ))}
      {text && (
        <div className="min-w-0 text-sm leading-relaxed">
          <Streamdown
            mode={isStreaming ? "streaming" : "static"}
            isAnimating={isStreaming}
          >
            {text}
          </Streamdown>
        </div>
      )}
      {imageParts.map((part, index) => (
        <MessageImage
          key={index}
          url={part.url}
          alt={part.filename ?? t("playground.generatedImageAlt")}
          onEditImage={onEditImage}
        />
      ))}
      {!isStreaming && actions}
    </div>
  );
}
