import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useChat } from "@ai-sdk/react";
import type { FileUIPart, UIMessage } from "ai";
import { AnimatePresence, useReducedMotion } from "framer-motion";
import { KeyRound, RefreshCcw, SquarePen, X } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { LayoutGroup, motion, springs } from "@/components/ui/motion";
import {
  createApiKeyOptimistic,
  useApiKeys,
  useCurrentUser,
  useDashboardGroups,
  useMarketplaceModels,
} from "@/lib/swr";
import { resolvePlaygroundKey } from "@/components/playground/auth";
import {
  MonoizeChatTransport,
  type ChatRequestConfig,
} from "@/components/playground/chat-transport";
import { Composer, type ComposerMode } from "@/components/playground/composer";
import { MessageList } from "@/components/playground/message-list";
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
  const [creatingKey, setCreatingKey] = useState(false);

  useEffect(() => purgeLegacyPlaygroundKeys(), []);

  const { data: apiKeys, isLoading: keysLoading } = useApiKeys();
  const { data: groups, isLoading: groupsLoading } = useDashboardGroups();
  const { data: models, isLoading: modelsLoading } = useMarketplaceModels();
  const { data: user } = useCurrentUser();

  const userAllowedGroups = useMemo(
    () => user?.allowed_groups ?? [],
    [user?.allowed_groups],
  );
  const resolution = useMemo(
    () =>
      resolvePlaygroundKey(apiKeys, prefs.apiKeyId, prefs.group, userAllowedGroups),
    [apiKeys, prefs.apiKeyId, prefs.group, userAllowedGroups],
  );

  // Refs let the memoized transport observe the latest selector state at call
  // time (PG-CHAT2 step 1) without recreating the useChat instance.
  const configRef = useRef<ChatRequestConfig>({
    model: "",
    apiKey: null,
    systemPrompt: "",
    temperature: "",
    maxTokens: "",
  });
  configRef.current = {
    model: prefs.chatModel,
    apiKey: resolution.key?.key ?? null,
    systemPrompt: prefs.systemPrompt,
    temperature: prefs.temperature,
    maxTokens: prefs.maxTokens,
  };
  const tRef = useRef(t);
  tRef.current = t;

  const transport = useMemo(
    () =>
      new MonoizeChatTransport(
        () => configRef.current,
        (reason) =>
          tRef.current(
            reason === "model" ? "playground.errorNoModel" : "playground.errorNoKey",
          ),
      ),
    [],
  );

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
  const modelForMode = mode === "image" ? prefs.imageModel : prefs.chatModel;
  const canSend =
    resolution.key !== null &&
    modelForMode.trim().length > 0 &&
    (status === "ready" || status === "error") &&
    !imageBusy &&
    (trimmedText.length > 0 || (mode === "chat" && attachments.length > 0));

  const blockedHint =
    resolution.reason === "no-group-key"
      ? t("playground.groupKeyBlocked", { group: prefs.group })
      : null;

  const handleAddFiles = useCallback(async (files: FileList) => {
    const imageFiles = Array.from(files).filter((file) =>
      file.type.startsWith("image/"),
    );
    const staged = await Promise.all(
      imageFiles.map(async (file) => ({
        id: playgroundMessageId(),
        file,
        url: await readAsDataUrl(file),
      })),
    );
    if (staged.length > 0) {
      setAttachments((prev) => [...prev, ...staged]);
    }
  }, []);

  const handleRemoveAttachment = useCallback((id: string) => {
    setAttachments((prev) => prev.filter((attachment) => attachment.id !== id));
  }, []);

  const handleSend = useCallback(() => {
    if (!canSend || !resolution.key) return;
    if (mode === "image") {
      images.generate({
        prompt: trimmedText,
        model: prefs.imageModel.trim(),
        apiKey: resolution.key.key,
        attachment: attachments[0] ?? null,
      });
    } else {
      const files: FileUIPart[] = attachments.map((attachment) => ({
        type: "file",
        mediaType: attachment.file.type || "image/png",
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
    resolution.key,
    mode,
    images,
    trimmedText,
    prefs.imageModel,
    attachments,
    sendMessage,
  ]);

  const handleStop = useCallback(() => {
    if (imageBusy) images.abort();
    if (chatBusy) void stop();
  }, [imageBusy, chatBusy, images, stop]);

  const handleEditUser = useCallback(
    (messageId: string, newText: string) => {
      void sendMessage({ text: newText, messageId });
    },
    [sendMessage],
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
      void regenerate({ messageId });
    },
    [regenerate],
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
    images.clear();
    setMessages([]);
    setAttachments([]);
    clearError();
  }, [stop, images, setMessages, clearError]);

  const handleCreateKey = useCallback(async () => {
    setCreatingKey(true);
    try {
      await createApiKeyOptimistic({ name: "Playground" }, apiKeys ?? []);
    } catch (error) {
      toast.error((error as Error).message || t("common.error"));
    } finally {
      setCreatingKey(false);
    }
  }, [apiKeys, t]);

  const layoutTransition = shouldReduceMotion ? { duration: 0 } : springs.smooth;

  return (
    <div className="flex h-[calc(100dvh-5.5rem)] flex-col lg:h-[calc(100dvh-3rem)]">
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
              {resolution.reason === "no-keys" && !keysLoading && (
                <div className="mx-auto mt-6 flex max-w-md flex-col items-center gap-3 rounded-2xl border border-dashed px-6 py-5">
                  <KeyRound className="h-5 w-5 text-muted-foreground" />
                  <p className="text-sm text-muted-foreground">
                    {t("playground.noKeysHint")}
                  </p>
                  <Button size="sm" onClick={handleCreateKey} disabled={creatingKey}>
                    {creatingKey
                      ? t("playground.creatingKey")
                      : t("playground.createKey")}
                  </Button>
                </div>
              )}
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
            onModeChange={setMode}
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
            groups={groups ?? []}
            userAllowedGroups={userAllowedGroups}
            groupsLoading={groupsLoading && !groups}
            models={models ?? []}
            modelsLoading={modelsLoading && !models}
            apiKeys={apiKeys ?? []}
            keysLoading={keysLoading && !apiKeys}
            resolvedKeyId={resolution.key?.id ?? null}
          />
        </motion.div>

        {conversationEmpty && <div className="flex-[1.4]" />}
      </LayoutGroup>
    </div>
  );
}
