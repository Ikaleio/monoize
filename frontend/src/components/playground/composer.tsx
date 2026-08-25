import { useEffect, useMemo, useRef } from "react";
import { useTranslation } from "react-i18next";
import { AnimatePresence, useReducedMotion } from "framer-motion";
import {
  ArrowUp,
  FileText,
  ImageIcon,
  MessageSquare,
  Paperclip,
  Square,
  X,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { motion, springs } from "@/components/ui/motion";
import { cn } from "@/lib/utils";
import type { ApiKey, Group, ModelMetadataRecord } from "@/lib/api";
import type { PlaygroundPrefs } from "./prefs";
import type { ComposerAttachment } from "./use-image-generation";
import { ApiKeyDropdown } from "./api-key-dropdown";
import { GroupSelector } from "./group-selector";
import { ModelCombobox } from "./model-combobox";
import { SettingsPopover } from "./settings-popover";

export type ComposerMode = "chat" | "image";

const MAX_TEXTAREA_HEIGHT_PX = 200;

function ModeToggle({
  mode,
  onModeChange,
  disabled,
}: {
  mode: ComposerMode;
  onModeChange: (mode: ComposerMode) => void;
  disabled: boolean;
}) {
  const { t } = useTranslation();
  const shouldReduceMotion = useReducedMotion();
  const options = [
    { value: "chat" as const, icon: MessageSquare, label: t("playground.modeChat") },
    { value: "image" as const, icon: ImageIcon, label: t("playground.modeImage") },
  ];

  return (
    <div className="relative flex h-8 shrink-0 items-center rounded-full bg-muted p-0.5">
      {options.map((option) => {
        const Icon = option.icon;
        const isActive = mode === option.value;
        return (
          <button
            key={option.value}
            type="button"
            disabled={disabled}
            onClick={() => onModeChange(option.value)}
            aria-label={option.label}
            aria-pressed={isActive}
            title={option.label}
            className={cn(
              "relative z-10 flex h-7 w-9 items-center justify-center rounded-full transition-colors",
              isActive
                ? "text-foreground"
                : "text-muted-foreground hover:text-foreground",
            )}
          >
            {isActive && (
              <motion.span
                layoutId="playground-mode-indicator"
                className="absolute inset-0 rounded-full bg-background shadow-sm"
                transition={shouldReduceMotion ? { duration: 0 } : springs.snappy}
              />
            )}
            <Icon className="relative z-10 h-3.5 w-3.5" />
          </button>
        );
      })}
    </div>
  );
}

export interface ComposerProps {
  mode: ComposerMode;
  onModeChange: (mode: ComposerMode) => void;
  text: string;
  onTextChange: (text: string) => void;
  attachments: ComposerAttachment[];
  onAddFiles: (files: FileList) => void;
  onRemoveAttachment: (id: string) => void;
  onSend: () => void;
  onStop: () => void;
  canSend: boolean;
  /** True while a chat stream or image request is in flight (stop affordance). */
  isBusy: boolean;
  blockedHint: string | null;
  prefs: PlaygroundPrefs;
  setPref: (name: keyof PlaygroundPrefs, value: string) => void;
  groups: Group[];
  groupsLoading: boolean;
  models: ModelMetadataRecord[];
  modelsLoading: boolean;
  apiKeys: ApiKey[];
  keysLoading: boolean;
  resolvedKeyId: string | null;
  isDraggingFiles: boolean;
}

export function Composer({
  mode,
  onModeChange,
  text,
  onTextChange,
  attachments,
  onAddFiles,
  onRemoveAttachment,
  onSend,
  onStop,
  canSend,
  isBusy,
  blockedHint,
  prefs,
  setPref,
  groups,
  groupsLoading,
  models,
  modelsLoading,
  apiKeys,
  keysLoading,
  resolvedKeyId,
  isDraggingFiles,
}: ComposerProps) {
  const { t } = useTranslation();
  const shouldReduceMotion = useReducedMotion();
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);

  const finePointer = useMemo(
    () =>
      typeof window !== "undefined" &&
      window.matchMedia("(pointer: fine)").matches,
    [],
  );

  useEffect(() => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, MAX_TEXTAREA_HEIGHT_PX)}px`;
  }, [text]);

  const handleKeyDown = (event: React.KeyboardEvent<HTMLTextAreaElement>) => {
    // PG-CMP2: Enter submits only on fine-pointer devices; Shift+Enter always newline.
    if (event.key === "Enter" && !event.shiftKey && finePointer && !event.nativeEvent.isComposing) {
      event.preventDefault();
      if (canSend) onSend();
    }
  };

  return (
    <div className="mx-auto w-full max-w-3xl">
      <div
        className={cn(
          "rounded-2xl border bg-card shadow-sm transition-colors focus-within:border-ring/60",
          isDraggingFiles && "border-ring bg-accent/30 ring-2 ring-ring/20",
        )}
      >
        <AnimatePresence initial={false}>
          {attachments.length > 0 && (
            <motion.div
              initial={{ height: 0, opacity: 0 }}
              animate={{ height: "auto", opacity: 1 }}
              exit={{ height: 0, opacity: 0 }}
              transition={shouldReduceMotion ? { duration: 0 } : springs.smooth}
              className="overflow-hidden"
            >
              <div className="flex flex-wrap gap-2 px-3 pt-3">
                {attachments.map((attachment) => (
                  <div
                    key={attachment.id}
                    className={cn(
                      "relative h-16 overflow-hidden rounded-lg border bg-muted/40",
                      attachment.file.type.startsWith("image/") ? "w-16" : "w-28",
                    )}
                    title={attachment.file.name}
                  >
                    {attachment.file.type.startsWith("image/") ? (
                      <img
                        src={attachment.url}
                        alt={attachment.file.name}
                        className="h-full w-full object-cover"
                      />
                    ) : (
                      <div className="flex h-full min-w-0 items-center gap-2 px-2 pr-5">
                        <FileText className="h-5 w-5 shrink-0 text-muted-foreground" />
                        <span className="min-w-0 truncate text-xs">
                          {attachment.file.name}
                        </span>
                      </div>
                    )}
                    <button
                      type="button"
                      onClick={() => onRemoveAttachment(attachment.id)}
                      aria-label={t("playground.removeAttachment")}
                      className="absolute right-0.5 top-0.5 rounded-full bg-background/90 p-0.5 text-muted-foreground transition-colors hover:text-foreground"
                    >
                      <X className="h-3 w-3" />
                    </button>
                  </div>
                ))}
              </div>
            </motion.div>
          )}
        </AnimatePresence>

        <textarea
          ref={textareaRef}
          value={text}
          onChange={(e) => onTextChange(e.target.value)}
          onKeyDown={handleKeyDown}
          rows={1}
          placeholder={
            mode === "image"
              ? t("playground.imagePlaceholder")
              : t("playground.chatPlaceholder")
          }
          aria-label={t("playground.composerLabel")}
          className="max-h-[200px] w-full resize-none bg-transparent px-4 pb-1 pt-3.5 text-sm leading-relaxed outline-none placeholder:text-muted-foreground"
        />

        <div className="flex flex-wrap items-center gap-1 px-2 pb-2">
          <GroupSelector
            value={prefs.group}
            onChange={(group) => setPref("group", group)}
            groups={groups}
            isLoading={groupsLoading}
          />
          <ModelCombobox
            value={prefs.chatModel}
            onChange={(model) => setPref("chatModel", model)}
            records={models}
            kind="chat"
            isLoading={modelsLoading}
          />
          <AnimatePresence initial={false}>
            {mode === "image" && (
              <motion.div
                initial={
                  shouldReduceMotion ? { opacity: 0 } : { opacity: 0, x: -8, scale: 0.96 }
                }
                animate={
                  shouldReduceMotion ? { opacity: 1 } : { opacity: 1, x: 0, scale: 1 }
                }
                exit={
                  shouldReduceMotion ? { opacity: 0 } : { opacity: 0, x: -8, scale: 0.96 }
                }
                transition={springs.smooth}
              >
                <ModelCombobox
                  value={prefs.imageModel}
                  onChange={(model) => setPref("imageModel", model)}
                  records={models}
                  kind="image"
                  isLoading={modelsLoading}
                />
              </motion.div>
            )}
          </AnimatePresence>
          <ApiKeyDropdown
            value={prefs.apiKeyId}
            onChange={(apiKeyId) => setPref("apiKeyId", apiKeyId)}
            apiKeys={apiKeys}
            isLoading={keysLoading}
            resolvedKeyId={resolvedKeyId}
          />

          <div className="ml-auto flex items-center gap-1">
            <input
              ref={fileInputRef}
              type="file"
              accept={mode === "image" ? "image/*" : undefined}
              multiple
              className="hidden"
              onChange={(e) => {
                if (e.target.files?.length) onAddFiles(e.target.files);
                e.target.value = "";
              }}
            />
            <Button
              variant="ghost"
              size="icon"
              aria-label={t("playground.attach")}
              onClick={() => fileInputRef.current?.click()}
              className="size-11 shrink-0 touch-manipulation text-muted-foreground hover:text-foreground sm:size-8"
            >
              <Paperclip className="h-4 w-4" />
            </Button>
            <ModeToggle mode={mode} onModeChange={onModeChange} disabled={isBusy} />
            <SettingsPopover prefs={prefs} setPref={setPref} />
            <Button
              size="icon"
              aria-label={isBusy ? t("playground.stop") : t("playground.send")}
              onClick={isBusy ? onStop : onSend}
              disabled={!isBusy && !canSend}
              className="size-11 shrink-0 touch-manipulation sm:size-8"
            >
              <AnimatePresence mode="popLayout" initial={false}>
                <motion.span
                  key={isBusy ? "stop" : "send"}
                  initial={
                    shouldReduceMotion ? { opacity: 0 } : { opacity: 0, scale: 0.5 }
                  }
                  animate={
                    shouldReduceMotion ? { opacity: 1 } : { opacity: 1, scale: 1 }
                  }
                  exit={shouldReduceMotion ? { opacity: 0 } : { opacity: 0, scale: 0.5 }}
                  transition={shouldReduceMotion ? { duration: 0 } : springs.snappy}
                  className="inline-flex"
                >
                  {isBusy ? (
                    <Square className="h-3.5 w-3.5 fill-current" />
                  ) : (
                    <ArrowUp className="h-4 w-4" />
                  )}
                </motion.span>
              </AnimatePresence>
            </Button>
          </div>
        </div>
      </div>

      <AnimatePresence initial={false}>
        {blockedHint && (
          <motion.p
            initial={{ opacity: 0, height: 0 }}
            animate={{ opacity: 1, height: "auto" }}
            exit={{ opacity: 0, height: 0 }}
            transition={shouldReduceMotion ? { duration: 0 } : springs.smooth}
            className="overflow-hidden px-2 pt-1.5 text-xs text-warning-foreground"
          >
            {blockedHint}
          </motion.p>
        )}
      </AnimatePresence>
    </div>
  );
}
