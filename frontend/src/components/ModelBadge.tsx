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
	ChatGLM,
	Yi,
	HuggingFace,
	Github,
	XAI,
	Vllm,
	Ollama,
	ZeroOne
} from '@lobehub/icons'
import { Badge } from '@/components/ui/badge'
import { cn } from '@/lib/utils'
import { Box } from 'lucide-react'

const PROVIDER_ICONS: Record<
	string,
	React.ComponentType<{ className?: string }>
> = {
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
	'01': ZeroOne,
	zeroone: ZeroOne,
	glm: ChatGLM,
	chatglm: ChatGLM
}

const normalizeProvider = (value: string) =>
	value.toLowerCase().replace(/[\s_-]/g, '')

export interface ModelBadgeProps {
	model: string
	provider?: string | null
	multiplier?: number | string
	redirect?: string | null
	detailTarget?: string
	showDetails?: boolean
	highlightUnpriced?: boolean
	truncateModelText?: boolean
	className?: string
}

export function ModelBadge({
	model,
	provider,
	multiplier = 1,
	redirect,
	detailTarget,
	showDetails = true,
	highlightUnpriced = false,
	truncateModelText = true,
	className
}: ModelBadgeProps) {
	const normalizedProvider = provider ? normalizeProvider(provider) : undefined
	const lowerModel = model.toLowerCase()
	const resolvedTarget = (detailTarget ?? redirect ?? model).trim()
	const numericMultiplier = Number(multiplier)
	const hasCustomMultiplier =
		Number.isFinite(numericMultiplier) ?
			numericMultiplier !== 1
		:	String(multiplier).trim() !== '1'
	const hasRedirectTarget =
		resolvedTarget.length > 0 && resolvedTarget !== model
	const shouldRenderDetails =
		showDetails && (hasCustomMultiplier || hasRedirectTarget)

	// Resolve Icon
	let Icon: React.ComponentType<{ className?: string }> = Box

	if (normalizedProvider && PROVIDER_ICONS[normalizedProvider]) {
		Icon = PROVIDER_ICONS[normalizedProvider]
	} else {
		if (
			lowerModel.includes('gpt') ||
			lowerModel.includes('davinci') ||
			lowerModel.includes('o1-') ||
			lowerModel.includes('o3-') ||
			lowerModel.includes('o4-')
		)
			Icon = OpenAI
		else if (lowerModel.includes('claude')) Icon = Anthropic
		else if (lowerModel.includes('gemini')) Icon = Google
		else if (lowerModel.includes('llama')) Icon = Meta
		else if (lowerModel.includes('mistral') || lowerModel.includes('mixtral'))
			Icon = Mistral
		else if (lowerModel.includes('deepseek')) Icon = DeepSeek
		else if (lowerModel.includes('qwen')) Icon = Qwen
		else if (lowerModel.includes('grok')) Icon = XAI
		else if (lowerModel.includes('command')) Icon = Cohere
		else if (lowerModel.includes('glm') || lowerModel.includes('chatglm'))
			Icon = ChatGLM
	}

	return (
		<Badge
			variant='secondary'
			className={cn(
				'h-7 max-w-full gap-1.5 overflow-hidden border px-2 py-1 font-mono text-xs whitespace-nowrap transition-all',
				highlightUnpriced ?
					'border-warning-border bg-warning-soft text-warning-foreground hover:bg-warning-soft/80'
				:	'bg-sidebar-accent/40 hover:bg-sidebar-accent text-foreground border-transparent hover:border-sidebar-border',
				className
			)}
		>
			<Icon className='h-3.5 w-3.5 shrink-0' />
			<span
				className={cn(
					'min-w-0',
					truncateModelText ? 'max-w-[220px] truncate' : 'whitespace-nowrap'
				)}
				title={model}
			>
				{model}
			</span>
			{shouldRenderDetails && (
				<span
					className={cn(
						'min-w-0 text-[11px] opacity-60',
						truncateModelText ? 'max-w-[160px] truncate' : 'whitespace-nowrap'
					)}
					title={`[${[
						hasCustomMultiplier ? `${multiplier}x` : null,
						hasRedirectTarget ? resolvedTarget : null
					]
						.filter(Boolean)
						.join(', ')}]`}
				>
					[
					{hasCustomMultiplier && (
						<span className='opacity-80'>{multiplier}x</span>
					)}
					{hasCustomMultiplier && hasRedirectTarget && (
						<span className='mx-1'>,</span>
					)}
					{hasRedirectTarget && (
						<span className='opacity-80'>{resolvedTarget}</span>
					)}
					]
				</span>
			)}
		</Badge>
	)
}
