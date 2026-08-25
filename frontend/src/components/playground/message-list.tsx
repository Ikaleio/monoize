import { useCallback, useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import type { UIMessage } from "ai";
import { AnimatePresence, useReducedMotion } from "framer-motion";
import { ImageIcon, RefreshCcw, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { motion, springs } from "@/components/ui/motion";
import { ChatMessage } from "./chat-message";
import type { ImageJobState } from "./use-image-generation";

/** Auto-scroll re-engages when the viewport is within this distance of the bottom (PG-L4). */
const AUTOSCROLL_THRESHOLD_PX = 80;

function PendingDots() {
  return (
    <div className="flex items-center gap-1 py-1" aria-hidden>
      {[0, 1, 2].map((index) => (
        <motion.span
          key={index}
          className="h-1.5 w-1.5 rounded-full bg-muted-foreground"
          animate={{ opacity: [0.25, 1, 0.25] }}
          transition={{ duration: 1.2, repeat: Infinity, delay: index * 0.18 }}
        />
      ))}
    </div>
  );
}

function ImageJobRow({
  job,
  onRetry,
  onDismiss,
}: {
  job: ImageJobState;
  onRetry: () => void;
  onDismiss: () => void;
}) {
  const { t } = useTranslation();

  if (job.status === "pending") {
    return (
      <div className="flex w-fit items-center gap-3 rounded-xl border bg-muted/40 px-4 py-3">
        <motion.span
          animate={{ opacity: [0.4, 1, 0.4] }}
          transition={{ duration: 1.4, repeat: Infinity }}
          className="inline-flex"
        >
          <ImageIcon className="h-4 w-4 text-muted-foreground" />
        </motion.span>
        <span className="text-sm text-muted-foreground">
          {t("playground.generatingImage")}
        </span>
      </div>
    );
  }

  return (
    <div className="flex w-fit max-w-full flex-wrap items-center gap-2 rounded-xl border border-destructive/40 bg-destructive/5 px-4 py-3">
      <span className="min-w-0 break-all text-sm text-destructive">
        {t("playground.imageError")}: {job.error}
      </span>
      <div className="flex items-center gap-1">
        <Button variant="outline" size="sm" onClick={onRetry} className="h-7 gap-1.5">
          <RefreshCcw className="h-3 w-3" />
          {t("playground.retry")}
        </Button>
        <Button
          variant="ghost"
          size="icon"
          onClick={onDismiss}
          aria-label={t("playground.dismiss")}
          className="size-7 text-muted-foreground hover:text-foreground"
        >
          <X className="h-3.5 w-3.5" />
        </Button>
      </div>
    </div>
  );
}

export interface MessageListProps {
  messages: UIMessage[];
  status: "submitted" | "streaming" | "ready" | "error";
  imageJob: ImageJobState | null;
  busy: boolean;
  onEditUser: (messageId: string, text: string) => void;
  onEditAssistant: (messageId: string, text: string) => void;
  onDelete: (messageId: string) => void;
  onRegenerate: (messageId: string) => void;
  onEditImage: (url: string) => void;
  onRetryImage: () => void;
  onDismissImage: () => void;
}

export function MessageList({
  messages,
  status,
  imageJob,
  busy,
  onEditUser,
  onEditAssistant,
  onDelete,
  onRegenerate,
  onEditImage,
  onRetryImage,
  onDismissImage,
}: MessageListProps) {
  const shouldReduceMotion = useReducedMotion();
  const scrollRef = useRef<HTMLDivElement>(null);
  const pinnedToBottom = useRef(true);

  const handleScroll = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    pinnedToBottom.current =
      el.scrollHeight - el.scrollTop - el.clientHeight <= AUTOSCROLL_THRESHOLD_PX;
  }, []);

  const lastMessage = messages[messages.length - 1];
  const streamingSize =
    lastMessage?.role === "assistant" ? JSON.stringify(lastMessage.parts).length : 0;

  useEffect(() => {
    const el = scrollRef.current;
    if (el && pinnedToBottom.current) {
      el.scrollTop = el.scrollHeight;
    }
  }, [messages.length, streamingSize, status, imageJob]);

  const waitingForFirstToken =
    status === "submitted" ||
    (status === "streaming" &&
      (lastMessage?.role !== "assistant" ||
        lastMessage.parts.every(
          (part) => part.type !== "text" || part.text.length === 0,
        )));

  const enter = shouldReduceMotion
    ? { initial: { opacity: 0 }, animate: { opacity: 1 }, exit: { opacity: 0 } }
    : {
        initial: { opacity: 0, y: 12, scale: 0.98 },
        animate: { opacity: 1, y: 0, scale: 1 },
        exit: { opacity: 0, scale: 0.96 },
      };

  return (
    <div
      ref={scrollRef}
      onScroll={handleScroll}
      className="min-h-0 flex-1 overflow-y-auto overscroll-contain"
    >
      <div className="mx-auto flex w-full max-w-3xl flex-col gap-6 px-1 py-6">
        <AnimatePresence mode="popLayout" initial={false}>
          {messages.map((message, index) => (
            <motion.div
              key={message.id}
              layout={!shouldReduceMotion}
              {...enter}
              transition={springs.smooth}
              className="w-full"
            >
              <ChatMessage
                message={message}
                isStreaming={
                  status === "streaming" &&
                  index === messages.length - 1 &&
                  message.role === "assistant"
                }
                busy={busy}
                onEditUser={onEditUser}
                onEditAssistant={onEditAssistant}
                onDelete={onDelete}
                onRegenerate={onRegenerate}
                onEditImage={onEditImage}
              />
            </motion.div>
          ))}
          {waitingForFirstToken && (
            <motion.div key="pending-dots" {...enter} transition={springs.smooth}>
              <PendingDots />
            </motion.div>
          )}
          {imageJob && (
            <motion.div key="image-job" {...enter} transition={springs.smooth}>
              <ImageJobRow
                job={imageJob}
                onRetry={onRetryImage}
                onDismiss={onDismissImage}
              />
            </motion.div>
          )}
        </AnimatePresence>
      </div>
    </div>
  );
}
