import { useState, useRef, useCallback, useEffect } from "react";
import { useTranslation } from "react-i18next";
import {
  Plus,
  Send,
  Square,
  Trash2,
  MessageSquare,
  Bot,
  User,
  Settings2,
  ChevronDown,
  ChevronUp,
  KeyRound,
} from "lucide-react";
import { Card, CardContent } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { Separator } from "@/components/ui/separator";
import { toast } from "sonner";
import { PageWrapper, motion, transitions } from "@/components/ui/motion";

// ── Types ──────────────────────────────────────────────────────────

type MessageRole = "system" | "user" | "assistant";

interface PlaygroundMessage {
  id: string;
  role: MessageRole;
  content: string;
}

// ── Persistence helpers ────────────────────────────────────────────

const LS_KEY_API_KEY = "playground_api_key";
const LS_KEY_MODEL = "playground_model";
const LS_KEY_TEMPERATURE = "playground_temperature";
const LS_KEY_MAX_TOKENS = "playground_max_tokens";

function loadString(key: string, fallback: string): string {
  try {
    return localStorage.getItem(key) ?? fallback;
  } catch {
    return fallback;
  }
}

function saveString(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    /* quota exceeded – ignore */
  }
}

// ── Unique ID generator ────────────────────────────────────────────

let _seq = 0;
function uid(): string {
  return `msg-${Date.now()}-${++_seq}`;
}

// ── Role icon helper ───────────────────────────────────────────────

function RoleIcon({ role }: { role: MessageRole }) {
  if (role === "system")
    return <Settings2 className="h-4 w-4 text-amber-500" />;
  if (role === "assistant") return <Bot className="h-4 w-4 text-primary" />;
  return <User className="h-4 w-4 text-emerald-500" />;
}

function roleBadgeClass(role: MessageRole): string {
  if (role === "system")
    return "bg-amber-500/15 text-amber-700 dark:text-amber-400 border-0";
  if (role === "assistant")
    return "bg-primary/15 text-primary border-0";
  return "bg-emerald-600/15 text-emerald-700 dark:text-emerald-400 border-0";
}

// ── SSE streaming helper ───────────────────────────────────────────

async function streamChatCompletion(
  apiKey: string,
  model: string,
  messages: { role: string; content: string }[],
  params: { temperature?: number; max_tokens?: number },
  onToken: (token: string) => void,
  signal: AbortSignal,
): Promise<void> {
  const body: Record<string, unknown> = {
    model,
    messages,
    stream: true,
  };
  if (params.temperature !== undefined) body.temperature = params.temperature;
  if (params.max_tokens !== undefined) body.max_tokens = params.max_tokens;

  const res = await fetch("/api/v1/chat/completions", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${apiKey}`,
    },
    body: JSON.stringify(body),
    signal,
  });

  if (!res.ok) {
    let msg = `HTTP ${res.status}`;
    try {
      const j = await res.json();
      msg = j.error?.message || j.error?.code || msg;
    } catch {
      /* ignore parse failures */
    }
    throw new Error(msg);
  }

  const reader = res.body?.getReader();
  if (!reader) throw new Error("No response body");

  const decoder = new TextDecoder();
  let buffer = "";

  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    buffer += decoder.decode(value, { stream: true });

    const lines = buffer.split("\n");
    // Keep the last partial line in the buffer
    buffer = lines.pop() ?? "";

    for (const line of lines) {
      const trimmed = line.trim();
      if (!trimmed || trimmed.startsWith(":")) continue;
      if (trimmed === "data: [DONE]") return;
      if (trimmed.startsWith("data: ")) {
        try {
          const payload = JSON.parse(trimmed.slice(6));
          const delta = payload.choices?.[0]?.delta?.content;
          if (typeof delta === "string") {
            onToken(delta);
          }
        } catch {
          /* skip malformed JSON chunks */
        }
      }
    }
  }
}

// ── Component: MessageRow ──────────────────────────────────────────

function MessageRow({
  message,
  index,
  isStreaming,
  onChange,
  onDelete,
}: {
  message: PlaygroundMessage;
  index: number;
  isStreaming: boolean;
  onChange: (id: string, patch: Partial<PlaygroundMessage>) => void;
  onDelete: (id: string) => void;
}) {
  const { t } = useTranslation();

  return (
    <motion.div
      initial={{ opacity: 0, y: 12 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ delay: index * 0.03, ...transitions.fast }}
      className="group"
    >
      <div className="flex gap-3">
        {/* Role selector */}
        <div className="pt-1">
          <Select
            value={message.role}
            onValueChange={(v) =>
              onChange(message.id, { role: v as MessageRole })
            }
            disabled={isStreaming}
          >
            <SelectTrigger
              className={`h-7 w-[110px] text-xs gap-1.5 border-0 px-2.5 font-medium ${roleBadgeClass(message.role)}`}
            >
              <RoleIcon role={message.role} />
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="system">{t("playground.system")}</SelectItem>
              <SelectItem value="user">{t("playground.user")}</SelectItem>
              <SelectItem value="assistant">
                {t("playground.assistant")}
              </SelectItem>
            </SelectContent>
          </Select>
        </div>

        {/* Content textarea */}
        <div className="flex-1 min-w-0">
          <Textarea
            value={message.content}
            onChange={(e) =>
              onChange(message.id, { content: e.target.value })
            }
            placeholder={
              message.role === "system"
                ? t("playground.systemPlaceholder")
                : message.role === "user"
                  ? t("playground.userPlaceholder")
                  : t("playground.assistantPlaceholder")
            }
            className="min-h-[60px] resize-y font-mono text-sm"
            disabled={isStreaming}
          />
        </div>

        {/* Delete button */}
        <div className="pt-1">
          <TooltipProvider delayDuration={300}>
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  variant="ghost"
                  size="icon"
                  className="h-8 w-8 opacity-0 group-hover:opacity-100 transition-opacity text-muted-foreground hover:text-destructive"
                  onClick={() => onDelete(message.id)}
                  disabled={isStreaming}
                >
                  <Trash2 className="h-4 w-4" />
                </Button>
              </TooltipTrigger>
              <TooltipContent>{t("common.delete")}</TooltipContent>
            </Tooltip>
          </TooltipProvider>
        </div>
      </div>
    </motion.div>
  );
}

// ── Component: StreamingOutput ─────────────────────────────────────

function StreamingOutput({ content }: { content: string }) {
  const { t } = useTranslation();
  const bottomRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [content]);

  return (
    <motion.div
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      transition={transitions.fast}
    >
      <Card className="border-primary/20 bg-primary/[0.02]">
        <CardContent className="pt-4">
          <div className="flex items-center gap-2 mb-2">
            <Bot className="h-4 w-4 text-primary" />
            <span className="text-xs font-medium text-primary">
              {t("playground.assistant")}
            </span>
            <div className="flex items-center gap-1 ml-auto">
              <span className="relative flex h-2 w-2">
                <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-primary opacity-75" />
                <span className="relative inline-flex rounded-full h-2 w-2 bg-primary" />
              </span>
              <span className="text-xs text-muted-foreground">
                {t("playground.streaming")}
              </span>
            </div>
          </div>
          <div className="font-mono text-sm whitespace-pre-wrap break-words leading-relaxed min-h-[40px]">
            {content || (
              <span className="text-muted-foreground italic">
                {t("playground.waitingForResponse")}
              </span>
            )}
          </div>
          <div ref={bottomRef} />
        </CardContent>
      </Card>
    </motion.div>
  );
}

// ── Page Component ─────────────────────────────────────────────────

export function PlaygroundPage() {
  const { t } = useTranslation();

  // ── State ──────────────────────────────────────────────────────
  const [apiKey, setApiKey] = useState(() =>
    loadString(LS_KEY_API_KEY, ""),
  );
  const [model, setModel] = useState(() =>
    loadString(LS_KEY_MODEL, ""),
  );
  const [temperature, setTemperature] = useState(() =>
    loadString(LS_KEY_TEMPERATURE, "1"),
  );
  const [maxTokens, setMaxTokens] = useState(() =>
    loadString(LS_KEY_MAX_TOKENS, ""),
  );
  const [showParams, setShowParams] = useState(false);

  const [messages, setMessages] = useState<PlaygroundMessage[]>([
    { id: uid(), role: "system", content: "" },
    { id: uid(), role: "user", content: "" },
  ]);

  const [streaming, setStreaming] = useState(false);
  const [streamContent, setStreamContent] = useState("");
  const abortRef = useRef<AbortController | null>(null);
  const messagesEndRef = useRef<HTMLDivElement>(null);

  // ── Persist settings on change ─────────────────────────────────
  useEffect(() => saveString(LS_KEY_API_KEY, apiKey), [apiKey]);
  useEffect(() => saveString(LS_KEY_MODEL, model), [model]);
  useEffect(() => saveString(LS_KEY_TEMPERATURE, temperature), [temperature]);
  useEffect(() => saveString(LS_KEY_MAX_TOKENS, maxTokens), [maxTokens]);

  // ── Handlers ───────────────────────────────────────────────────
  const updateMessage = useCallback(
    (id: string, patch: Partial<PlaygroundMessage>) => {
      setMessages((prev) =>
        prev.map((m) => (m.id === id ? { ...m, ...patch } : m)),
      );
    },
    [],
  );

  const deleteMessage = useCallback((id: string) => {
    setMessages((prev) => prev.filter((m) => m.id !== id));
  }, []);

  const addMessage = useCallback(() => {
    setMessages((prev) => [...prev, { id: uid(), role: "user", content: "" }]);
    // Scroll to bottom after next render
    setTimeout(
      () => messagesEndRef.current?.scrollIntoView({ behavior: "smooth" }),
      50,
    );
  }, []);

  const handleSend = useCallback(async () => {
    if (!apiKey.trim()) {
      toast.error(t("playground.apiKeyRequired"));
      return;
    }
    if (!model.trim()) {
      toast.error(t("playground.modelRequired"));
      return;
    }

    const nonEmpty = messages
      .filter((m) => m.content.trim())
      .map((m) => ({ role: m.role, content: m.content }));

    if (nonEmpty.length === 0) {
      toast.error(t("playground.noMessages"));
      return;
    }

    const params: { temperature?: number; max_tokens?: number } = {};
    const tempNum = parseFloat(temperature);
    if (temperature.trim() && Number.isFinite(tempNum)) {
      params.temperature = tempNum;
    }
    const maxNum = parseInt(maxTokens, 10);
    if (maxTokens.trim() && Number.isFinite(maxNum) && maxNum > 0) {
      params.max_tokens = maxNum;
    }

    const controller = new AbortController();
    abortRef.current = controller;
    setStreaming(true);
    setStreamContent("");

    let accumulated = "";

    try {
      await streamChatCompletion(
        apiKey.trim(),
        model.trim(),
        nonEmpty,
        params,
        (token) => {
          accumulated += token;
          setStreamContent(accumulated);
        },
        controller.signal,
      );
    } catch (err) {
      if ((err as Error).name !== "AbortError") {
        const errorMsg = (err as Error).message || t("common.error");
        setMessages((prev) => [
          ...prev,
          {
            id: uid(),
            role: "assistant" as MessageRole,
            content: `[Error] ${errorMsg}`,
          },
          { id: uid(), role: "user" as MessageRole, content: "" },
        ]);
      }
    } finally {
      setStreaming(false);
      abortRef.current = null;

      if (accumulated.trim()) {
        setMessages((prev) => [
          ...prev,
          { id: uid(), role: "assistant" as MessageRole, content: accumulated },
          { id: uid(), role: "user" as MessageRole, content: "" },
        ]);
      }

      setStreamContent("");
      setTimeout(
        () => messagesEndRef.current?.scrollIntoView({ behavior: "smooth" }),
        50,
      );
    }
  }, [apiKey, model, messages, temperature, maxTokens, t]);

  const handleStop = useCallback(() => {
    abortRef.current?.abort();
  }, []);

  // ── Loading state (none needed – pure frontend) ────────────────

  return (
    <PageWrapper className="space-y-6">
      {/* Header */}
      <motion.div
        initial={{ opacity: 0, y: -10 }}
        animate={{ opacity: 1, y: 0 }}
        transition={transitions.normal}
      >
        <h1 className="text-3xl font-bold tracking-tight">
          {t("playground.title")}
        </h1>
        <p className="text-muted-foreground">{t("playground.description")}</p>
      </motion.div>

      {/* Controls */}
      <motion.div
        initial={{ opacity: 0, y: 10 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ delay: 0.05, ...transitions.normal }}
      >
        <Card>
          <CardContent className="pt-6 space-y-4">
            {/* Row 1: Model + API Key */}
            <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
              <div className="space-y-2">
                <Label>{t("playground.model")}</Label>
                <Input
                  placeholder={t("playground.modelPlaceholder")}
                  value={model}
                  onChange={(e) => setModel(e.target.value)}
                  disabled={streaming}
                />
              </div>
              <div className="space-y-2">
                <Label className="flex items-center gap-1.5">
                  <KeyRound className="h-3.5 w-3.5" />
                  {t("playground.apiKey")}
                </Label>
                <Input
                  type="password"
                  placeholder="sk-..."
                  value={apiKey}
                  onChange={(e) => setApiKey(e.target.value)}
                  disabled={streaming}
                />
              </div>
            </div>

            {/* Expandable params */}
            <div>
              <button
                type="button"
                className="flex items-center gap-1.5 text-sm text-muted-foreground hover:text-foreground transition-colors"
                onClick={() => setShowParams((p) => !p)}
              >
                <Settings2 className="h-3.5 w-3.5" />
                {t("playground.parameters")}
                {showParams ? (
                  <ChevronUp className="h-3.5 w-3.5" />
                ) : (
                  <ChevronDown className="h-3.5 w-3.5" />
                )}
              </button>

              {showParams && (
                <motion.div
                  initial={{ opacity: 0, height: 0 }}
                  animate={{ opacity: 1, height: "auto" }}
                  transition={transitions.fast}
                  className="grid grid-cols-1 md:grid-cols-2 gap-4 mt-3"
                >
                  <div className="space-y-2">
                    <Label>{t("playground.temperature")}</Label>
                    <Input
                      type="number"
                      min="0"
                      max="2"
                      step="0.1"
                      value={temperature}
                      onChange={(e) => setTemperature(e.target.value)}
                      disabled={streaming}
                    />
                  </div>
                  <div className="space-y-2">
                    <Label>{t("playground.maxTokens")}</Label>
                    <Input
                      type="number"
                      min="1"
                      placeholder={t("playground.maxTokensPlaceholder")}
                      value={maxTokens}
                      onChange={(e) => setMaxTokens(e.target.value)}
                      disabled={streaming}
                    />
                  </div>
                </motion.div>
              )}
            </div>
          </CardContent>
        </Card>
      </motion.div>

      {/* Messages */}
      <motion.div
        initial={{ opacity: 0, y: 10 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ delay: 0.1, ...transitions.normal }}
      >
        <Card>
          <CardContent className="pt-6 space-y-4">
            <div className="flex items-center justify-between">
              <div className="flex items-center gap-2">
                <MessageSquare className="h-4 w-4 text-muted-foreground" />
                <h3 className="text-sm font-medium">
                  {t("playground.messages")}
                </h3>
                <Badge variant="secondary" className="text-xs">
                  {messages.length}
                </Badge>
              </div>
              <Button
                variant="outline"
                size="sm"
                onClick={addMessage}
                disabled={streaming}
              >
                <Plus className="h-4 w-4 mr-2" />
                {t("playground.addMessage")}
              </Button>
            </div>

            <Separator />

            <div className="space-y-3">
              {messages.map((msg, idx) => (
                <MessageRow
                  key={msg.id}
                  message={msg}
                  index={idx}
                  isStreaming={streaming}
                  onChange={updateMessage}
                  onDelete={deleteMessage}
                />
              ))}
              <div ref={messagesEndRef} />
            </div>

            {/* Streaming output */}
            {streaming && <StreamingOutput content={streamContent} />}

            {/* Action buttons */}
            <Separator />
            <div className="flex items-center justify-between">
              <Button
                variant="outline"
                size="sm"
                onClick={addMessage}
                disabled={streaming}
              >
                <Plus className="h-4 w-4 mr-2" />
                {t("playground.addMessage")}
              </Button>

              <div className="flex items-center gap-2">
                {streaming ? (
                  <motion.div
                    initial={{ scale: 0.9 }}
                    animate={{ scale: 1 }}
                    transition={transitions.spring}
                  >
                    <Button variant="destructive" onClick={handleStop}>
                      <Square className="h-4 w-4 mr-2" />
                      {t("playground.stop")}
                    </Button>
                  </motion.div>
                ) : (
                  <motion.div
                    whileHover={{ scale: 1.02 }}
                    whileTap={{ scale: 0.98 }}
                  >
                    <Button onClick={handleSend}>
                      <Send className="h-4 w-4 mr-2" />
                      {t("playground.send")}
                    </Button>
                  </motion.div>
                )}
              </div>
            </div>
          </CardContent>
        </Card>
      </motion.div>
    </PageWrapper>
  );
}
