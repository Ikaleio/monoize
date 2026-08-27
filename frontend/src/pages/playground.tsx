import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useChat } from "@ai-sdk/react";
import type { FileUIPart, UIMessage } from "ai";
import { AnimatePresence, useReducedMotion } from "framer-motion";
import { RefreshCcw, SquarePen, X } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { LayoutGroup, motion, springs } from "@/components/ui/motion";
import {
  useApiKeys,
  useCurrentUser,
  useDashboardGroups,
  useMarketplaceModels,
} from "@/lib/swr";
import {
  MonoizeChatTransport,
  type ChatRequestAuth,
} from "@/components/playground/chat-transport";
import { Composer, type ComposerMode } from "@/components/playground/composer";
import { MessageList } from "@/components/playground/message-list";
import { filePartsForEditedUserMessage } from "@/components/playground/message-operations";
import {
  resolvePlaygroundKey,
  type ResolvedPlaygroundKey,
} from "@/components/playground/auth";
import {
  purgeLegacyPlaygroundKeys,
  usePlaygroundPrefs,
} from "@/components/playground/prefs";
import {
  playgroundMessageId,
  usePlaygroundImages,
  type ComposerAttachment,
} from "@/components/playground/use-image-generation";

function readAsDataUrl(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(reader.result as string);
    reader.onerror = () => reject(reader.error ?? new Error("read failed"));
    reader.readAsDataURL(file);
  });
}

export function PlaygroundPage() {
  const { t } = useTranslation();
  const shouldReduceMotion = useReducedMotion();
  const [prefs, setPref] = usePlaygroundPrefs();
  const [mode, setMode] = useState<ComposerMode>("chat");
  const [text, setText] = useState("");
  const [attachments, setAttachments] = useState<ComposerAttachment[]>([]);
  const [isDraggingFiles, setIsDraggingFiles] = useState(false);
  const dragDepthRef = useRef(0);

  useEffect(() => purgeLegacyPlaygroundKeys(), []);

  const { data: apiKeys, isLoading: keysLoading } = useApiKeys();
  const { data: groups, isLoading: groupsLoading } = useDashboardGroups();
  const { data: models, isLoading: modelsLoading } = useMarketplaceModels();
  const { data: user } = useCurrentUser();

  const modelForMode = mode === "image" ? prefs.imageModel : prefs.chatModel;
  const resolution = useMemo(
    () =>
      resolvePlaygroundKey(
        apiKeys,
        prefs.apiKeyId,
        prefs.group,
        user?.group_id ?? "",
        modelForMode.trim(),
      ),
    [apiKeys, prefs.apiKeyId, prefs.group, user?.group_id, modelForMode],
  );
  const chatResolution = useMemo(
    () =>
      mode === "chat"
        ? resolution
        : resolvePlaygroundKey(
            apiKeys,
            prefs.apiKeyId,
            prefs.group,
            user?.group_id ?? "",
            prefs.chatModel.trim(),
          ),
    [
      mode,
      resolution,
      apiKeys,
      prefs.apiKeyId,
      prefs.group,
      prefs.chatModel,
      user?.group_id,
    ],
  );
  const selectableGroups = useMemo(() => {
    if (!groups || !user) return [];
    const planGroups =
      user.billing_plan?.enabled && user.billing_plan.group_ids.length > 0
        ? new Set(user.billing_plan.group_ids)
        : null;
    return groups.filter((group) => {
      const userMaySelect =
        user.role === "admin" ||
        user.role === "super_admin" ||
        group.user_selectable ||
        group.id === user.group_id;
      return userMaySelect && (!planGroups || planGroups.has(group.id));
    });
  }, [groups, user]);

  useEffect(() => {
    if (!groups || !user || !prefs.group) return;
    if (!selectableGroups.some((group) => group.id === prefs.group)) {
      setPref("group", "");
    }
  }, [groups, user, selectableGroups, prefs.group, setPref]);

  useEffect(() => {
    if (!apiKeys || !prefs.apiKeyId) return;
    if (resolution.reason === "key-unavailable") {
      setPref("apiKeyId", "");
    }
  }, [apiKeys, prefs.apiKeyId, resolution.reason, setPref]);

  const selectedGroupName = useMemo(
    () => groups?.find((group) => group.id === prefs.group)?.name,
    [groups, prefs.group],
  );
  const resolutionError = (
    current: ResolvedPlaygroundKey,
    model: string,
  ): string | null => {
    if (current.reason === "key-unavailable") {
      return t("playground.apiKeyUnavailable");
    }
    if (current.reason === "no-model-key") {
      return t("playground.modelKeyBlocked", { model });
    }
    if (current.reason === "no-group-key") {
      return t("playground.groupKeyBlocked", {
        group: selectedGroupName ?? prefs.group,
      });
    }
    return null;
  };
  const chatAuth = useMemo<ChatRequestAuth>(
    () => {
      if (chatResolution.reason === "internal") {
        return { mode: "internal", group: prefs.group };
      }
      if (chatResolution.reason === "ok" && chatResolution.key) {
        return { mode: "api-key", apiKey: chatResolution.key.key };
      }
      if (chatResolution.reason === "no-model-key") {
        return {
          mode: "invalid",
          message: t("playground.modelKeyBlocked", { model: prefs.chatModel }),
        };
      }
      if (chatResolution.reason === "no-group-key") {
        return {
          mode: "invalid",
          message: t("playground.groupKeyBlocked", {
            group: selectedGroupName ?? prefs.group,
          }),
        };
      }
      return { mode: "invalid", message: t("playground.apiKeyUnavailable") };
    },
    [chatResolution, prefs.group, prefs.chatModel, selectedGroupName, t],
  );

  const [transport] = useState(
    () =>
      new MonoizeChatTransport(
        {
          model: prefs.chatModel,
          auth: chatAuth,
          systemPrompt: prefs.systemPrompt,
          temperature: prefs.temperature,
          maxTokens: prefs.maxTokens,
        },
        t("playground.errorNoModel"),
      ),
  );
  useEffect(() => {
    transport.updateConfig(
      {
        model: prefs.chatModel,
        auth: chatAuth,
        systemPrompt: prefs.systemPrompt,
        temperature: prefs.temperature,
        maxTokens: prefs.maxTokens,
      },
      t("playground.errorNoModel"),
    );
  }, [
    transport,
    t,
    prefs.chatModel,
    chatAuth,
    prefs.systemPrompt,
    prefs.temperature,
    prefs.maxTokens,
  ]);

  const {
    messages,
    setMessages,
    sendMessage,
    regenerate,
    stop,
    status,
    error,
    clearError,
  } = useChat({ transport });

  const appendMessage = useCallback(
    (message: UIMessage) => setMessages((prev) => [...prev, message]),
    [setMessages],
  );
  const images = usePlaygroundImages(appendMessage);

  const chatBusy = status === "submitted" || status === "streaming";
  const imageBusy = images.job?.status === "pending";
  const busy = chatBusy || imageBusy;
  const conversationEmpty = messages.length === 0 && !images.job;

  const trimmedText = text.trim();
  const canSend =
    (resolution.reason === "internal" || resolution.reason === "ok") &&
    modelForMode.trim().length > 0 &&
    (status === "ready" || status === "error") &&
    !imageBusy &&
    (trimmedText.length > 0 || (mode === "chat" && attachments.length > 0));
  const blockedHint =
    resolution.reason === "key-unavailable" && !apiKeys
      ? null
      : resolutionError(resolution, modelForMode);

  const handleAddFiles = useCallback(
    async (files: FileList | File[]) => {
      const incoming = Array.from(files);
      const accepted =
        mode === "image"
          ? incoming.filter((file) => file.type.startsWith("image/"))
          : incoming;
      if (accepted.length !== incoming.length) {
        toast.error(t("playground.imageFilesOnly"));
      }
      const staged = await Promise.all(
        accepted.map(async (file) => ({
          id: playgroundMessageId(),
          file,
          url: await readAsDataUrl(file),
        })),
      );
      if (staged.length > 0) {
        setAttachments((prev) => [...prev, ...staged]);
      }
    },
    [mode, t],
  );

  const handleModeChange = useCallback(
    (nextMode: ComposerMode) => {
      if (nextMode === "image") {
        setAttachments((current) => {
          const imagesOnly = current.filter((attachment) =>
            attachment.file.type.startsWith("image/"),
          );
          if (imagesOnly.length !== current.length) {
            toast.error(t("playground.imageFilesOnly"));
          }
          return imagesOnly;
        });
      }
      setMode(nextMode);
    },
    [t],
  );

  const handleDragEnter = useCallback((event: React.DragEvent<HTMLDivElement>) => {
    if (!event.dataTransfer.types.includes("Files")) return;
    event.preventDefault();
    dragDepthRef.current += 1;
    setIsDraggingFiles(true);
  }, []);

  const handleDragOver = useCallback((event: React.DragEvent<HTMLDivElement>) => {
    if (!event.dataTransfer.types.includes("Files")) return;
    event.preventDefault();
    event.dataTransfer.dropEffect = "copy";
  }, []);

  const handleDragLeave = useCallback((event: React.DragEvent<HTMLDivElement>) => {
    if (dragDepthRef.current === 0) return;
    event.preventDefault();
    dragDepthRef.current = Math.max(0, dragDepthRef.current - 1);
    if (dragDepthRef.current === 0) setIsDraggingFiles(false);
  }, []);

  const handleDrop = useCallback(
    (event: React.DragEvent<HTMLDivElement>) => {
      if (!event.dataTransfer.types.includes("Files")) return;
      event.preventDefault();
      dragDepthRef.current = 0;
      setIsDraggingFiles(false);
      if (event.dataTransfer.files.length > 0) {
        void handleAddFiles(event.dataTransfer.files);
      }
    },
    [handleAddFiles],
  );

  const handlePaste = useCallback(
    (event: React.ClipboardEvent<HTMLDivElement>) => {
      const files = Array.from(event.clipboardData.items)
        .filter((item) => item.kind === "file")
        .map((item) => item.getAsFile())
        .filter((file): file is File => file !== null);
      if (files.length === 0) return;
      event.preventDefault();
      void handleAddFiles(files);
    },
    [handleAddFiles],
  );

  const handleRemoveAttachment = useCallback((id: string) => {
    setAttachments((prev) => prev.filter((attachment) => attachment.id !== id));
  }, []);

  const handleSend = useCallback(() => {
    if (!canSend) return;
    if (mode === "image") {
      images.generate({
        prompt: trimmedText,
        model: prefs.imageModel.trim(),
        size: prefs.imageSize,
        group: prefs.group,
        apiKey: resolution.key?.key ?? null,
        attachment: attachments[0] ?? null,
      });
    } else {
      const files: FileUIPart[] = attachments.map((attachment) => ({
        type: "file",
        mediaType: attachment.file.type || "application/octet-stream",
        filename: attachment.file.name,
        url: attachment.url,
      }));
      void sendMessage(
        files.length > 0 ? { text: trimmedText, files } : { text: trimmedText },
      );
    }
    setText("");
    setAttachments([]);
  }, [
    canSend,
    mode,
    images,
    trimmedText,
    prefs.imageModel,
    prefs.imageSize,
    prefs.group,
    resolution.key,
    attachments,
    sendMessage,
  ]);

  const handleStop = useCallback(() => {
    if (imageBusy) images.abort();
    if (chatBusy) void stop();
  }, [imageBusy, chatBusy, images, stop]);

  const handleEditUser = useCallback(
    (messageId: string, newText: string) => {
      const files = filePartsForEditedUserMessage(messages, messageId);
      void sendMessage({ text: newText, files, messageId });
    },
    [messages, sendMessage],
  );

  const handleEditAssistant = useCallback(
    (messageId: string, newText: string) => {
      setMessages((prev) =>
        prev.map((message) => {
          if (message.id !== messageId) return message;
          // PG-MSG4: all text parts collapse into one edited text part while
          // non-text parts (files, reasoning) keep their relative order.
          const parts: UIMessage["parts"] = [];
          let replaced = false;
          for (const part of message.parts) {
            if (part.type === "text") {
              if (!replaced) {
                parts.push({ type: "text", text: newText });
                replaced = true;
              }
            } else {
              parts.push(part);
            }
          }
          if (!replaced) parts.push({ type: "text", text: newText });
          return { ...message, parts };
        }),
      );
    },
    [setMessages],
  );

  const handleDelete = useCallback(
    (messageId: string) => {
      setMessages((prev) => prev.filter((message) => message.id !== messageId));
    },
    [setMessages],
  );

  const handleRegenerate = useCallback(
    (messageId: string) => {
      if (images.regenerate(messageId)) {
        setMessages((prev) => {
          const messageIndex = prev.findIndex((message) => message.id === messageId);
          return messageIndex < 0 ? prev : prev.slice(0, messageIndex);
        });
        return;
      }
      void regenerate({ messageId });
    },
    [images, regenerate, setMessages],
  );

  const handleEditImage = useCallback(
    async (url: string) => {
      try {
        const response = await fetch(url);
        const blob = await response.blob();
        const file = new File([blob], "playground-image.png", {
          type: blob.type || "image/png",
        });
        const dataUrl = url.startsWith("data:") ? url : await readAsDataUrl(file);
        setAttachments([{ id: playgroundMessageId(), file, url: dataUrl }]);
        setMode("image");
      } catch {
        toast.error(t("playground.stageImageFailed"));
      }
    },
    [t],
  );

  const handleNewChat = useCallback(() => {
    void stop();
    images.reset();
    setMessages([]);
    setAttachments([]);
    clearError();
  }, [stop, images, setMessages, clearError]);

  const layoutTransition = shouldReduceMotion ? { duration: 0 } : springs.smooth;

  return (
    <div
      className="flex h-[calc(100dvh-5.5rem)] flex-col lg:h-[calc(100dvh-3rem)]"
      onDragEnter={handleDragEnter}
      onDragOver={handleDragOver}
      onDragLeave={handleDragLeave}
      onDrop={handleDrop}
      onPasteCapture={handlePaste}
    >
      <LayoutGroup>
        {conversationEmpty ? (
          <>
            <div className="flex-1" />
            <motion.div
              layout
              transition={layoutTransition}
              className="mx-auto w-full max-w-3xl px-1 pb-8 text-center"
            >
              <h1 className="font-display text-3xl font-semibold tracking-tight sm:text-4xl">
                {t("playground.greeting")}
              </h1>
              <p className="mt-2 text-sm text-muted-foreground">
                {t("playground.greetingHint")}
              </p>
            </motion.div>
          </>
        ) : (
          <>
            <div className="flex shrink-0 items-center justify-end pb-1">
              <Button
                variant="ghost"
                size="sm"
                onClick={handleNewChat}
                className="h-8 gap-1.5 text-muted-foreground hover:text-foreground"
              >
                <SquarePen className="h-3.5 w-3.5" />
                {t("playground.newChat")}
              </Button>
            </div>
            <MessageList
              messages={messages}
              status={status}
              imageJob={images.job}
              busy={busy}
              onEditUser={handleEditUser}
              onEditAssistant={handleEditAssistant}
              onDelete={handleDelete}
              onRegenerate={handleRegenerate}
              onEditImage={(url) => void handleEditImage(url)}
              onRetryImage={images.retry}
              onDismissImage={images.clear}
            />
          </>
        )}

        <AnimatePresence initial={false}>
          {error && (
            <motion.div
              key="chat-error"
              initial={
                shouldReduceMotion ? { opacity: 0 } : { opacity: 0, y: 8 }
              }
              animate={shouldReduceMotion ? { opacity: 1 } : { opacity: 1, y: 0 }}
              exit={{ opacity: 0 }}
              transition={layoutTransition}
              className="mx-auto mb-2 flex w-full max-w-3xl shrink-0 items-center gap-2 rounded-xl border border-destructive/40 bg-destructive/5 px-3 py-2"
            >
              <span className="min-w-0 flex-1 break-words text-sm text-destructive">
                {error.message}
              </span>
              <Button
                variant="outline"
                size="sm"
                onClick={() => void regenerate()}
                className="h-7 shrink-0 gap-1.5"
              >
                <RefreshCcw className="h-3 w-3" />
                {t("playground.retry")}
              </Button>
              <Button
                variant="ghost"
                size="icon"
                onClick={clearError}
                aria-label={t("playground.dismiss")}
                className="size-7 shrink-0 text-muted-foreground hover:text-foreground"
              >
                <X className="h-3.5 w-3.5" />
              </Button>
            </motion.div>
          )}
        </AnimatePresence>

        <motion.div
          layout
          transition={layoutTransition}
          className="shrink-0 pb-1"
        >
          <Composer
            mode={mode}
            onModeChange={handleModeChange}
            text={text}
            onTextChange={setText}
            attachments={attachments}
            onAddFiles={(files) => void handleAddFiles(files)}
            onRemoveAttachment={handleRemoveAttachment}
            onSend={handleSend}
            onStop={handleStop}
            canSend={canSend}
            isBusy={busy}
            blockedHint={blockedHint}
            prefs={prefs}
            setPref={setPref}
            groups={selectableGroups}
            groupsLoading={(groupsLoading && !groups) || !user}
            models={models ?? []}
            modelsLoading={modelsLoading && !models}
            apiKeys={apiKeys ?? []}
            keysLoading={keysLoading && !apiKeys}
            resolvedKeyId={resolution.key?.id ?? null}
            isDraggingFiles={isDraggingFiles}
          />
        </motion.div>

        {conversationEmpty && <div className="flex-[1.4]" />}
      </LayoutGroup>
    </div>
  );
}
