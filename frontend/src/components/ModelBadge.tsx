import {
  OpenAI,
  Anthropic,
  Google,
  Meta,
  Mistral,
  Perplexity,
  Groq,
  Cohere,
  DeepSeek,
  Qwen,
  Minimax,
  Zhipu,
  Spark,
  Moonshot,
  ByteDance,
  Alibaba,
  Tencent,
  Baidu,
  Stepfun,
  Wenxin,
  Yi,
  HuggingFace,
  Github,
  XAI,
  Vllm,
  Ollama,
  ZeroOne,
} from "@lobehub/icons";
import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";
import { Box } from "lucide-react";

const PROVIDER_ICONS: Record<string, React.ComponentType<{ className?: string }>> = {
  openai: OpenAI,
  anthropic: Anthropic,
  google: Google,
  meta: Meta,
  mistral: Mistral,
  perplexity: Perplexity,
  groq: Groq,
  cohere: Cohere,
  deepseek: DeepSeek,
  qwen: Qwen,
  minimax: Minimax,
  zhipu: Zhipu,
  spark: Spark,
  moonshot: Moonshot,
  bytedance: ByteDance,
  alibaba: Alibaba,
  tencent: Tencent,
  baidu: Baidu,
  stepfun: Stepfun,
  wenxin: Wenxin,
  yi: Yi,
  huggingface: HuggingFace,
  github: Github,
  xai: XAI,
  grok: XAI,
  vllm: Vllm,
  ollama: Ollama,
  "01": ZeroOne,
  zeroone: ZeroOne,
};

const normalizeProvider = (value: string) => value.toLowerCase().replace(/[\s_-]/g, "");

export interface ModelBadgeProps {
  model: string;
  provider?: string | null;
  multiplier?: number | string;
  redirect?: string | null;
  detailTarget?: string;
  showDetails?: boolean;
  highlightUnpriced?: boolean;
  className?: string;
}

export function ModelBadge({
  model,
  provider,
  multiplier = 1,
  redirect,
  detailTarget,
  showDetails = true,
  highlightUnpriced = false,
  className,
}: ModelBadgeProps) {
  const normalizedProvider = provider ? normalizeProvider(provider) : undefined;
  const resolvedTarget = (detailTarget ?? redirect ?? model).trim();
  const numericMultiplier = Number(multiplier);
  const hasCustomMultiplier = Number.isFinite(numericMultiplier)
    ? numericMultiplier !== 1
    : String(multiplier).trim() !== "1";
  const hasRedirectTarget = resolvedTarget.length > 0 && resolvedTarget !== model;
  const shouldRenderDetails = showDetails && (hasCustomMultiplier || hasRedirectTarget);

  // Resolve Icon
  let Icon: React.ComponentType<{ className?: string }> = Box;
  
  if (normalizedProvider && PROVIDER_ICONS[normalizedProvider]) {
    Icon = PROVIDER_ICONS[normalizedProvider];
  } else {
    const lowerModel = model.toLowerCase();
    if (lowerModel.includes("gpt") || lowerModel.includes("davinci") || lowerModel.includes("o1-")) Icon = OpenAI;
    else if (lowerModel.includes("claude")) Icon = Anthropic;
    else if (lowerModel.includes("gemini")) Icon = Google;
    else if (lowerModel.includes("llama")) Icon = Meta;
    else if (lowerModel.includes("mistral") || lowerModel.includes("mixtral")) Icon = Mistral;
    else if (lowerModel.includes("deepseek")) Icon = DeepSeek;
    else if (lowerModel.includes("qwen")) Icon = Qwen;
    else if (lowerModel.includes("grok")) Icon = XAI;
    else if (lowerModel.includes("command")) Icon = Cohere;
  }

  return (
    <Badge
      variant="secondary"
      className={cn(
        "font-mono text-xs border transition-all gap-1.5 py-1 px-2 h-7",
        highlightUnpriced
          ? "bg-amber-500/20 text-amber-900 border-amber-500/40 hover:bg-amber-500/25 dark:bg-amber-500/20 dark:text-amber-200 dark:border-amber-400/40"
          : "bg-sidebar-accent/40 hover:bg-sidebar-accent text-foreground border-transparent hover:border-sidebar-border",
        className
      )}
    >
      <Icon className="h-3.5 w-3.5 flex-shrink-0" />
      <span className="truncate max-w-[220px]" title={model}>{model}</span>
      {shouldRenderDetails && (
        <span className="opacity-60 text-[11px]">
          [
          {hasCustomMultiplier && <span className="opacity-80">{multiplier}x</span>}
          {hasCustomMultiplier && hasRedirectTarget && <span className="mx-1">,</span>}
          {hasRedirectTarget && <span className="opacity-80">{resolvedTarget}</span>}
          ]
        </span>
      )}
    </Badge>
  );
}
