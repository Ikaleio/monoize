import { createElement } from 'react'
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

function resolveModelIcon(
	model: string,
	provider?: string | null
): React.ComponentType<{ className?: string }> {
	const normalizedProvider = provider ? normalizeProvider(provider) : undefined
	const lowerModel = model.toLowerCase()
	if (normalizedProvider && PROVIDER_ICONS[normalizedProvider]) {
		return PROVIDER_ICONS[normalizedProvider]
	}
	if (
		lowerModel.includes('gpt') ||
		lowerModel.includes('davinci') ||
		lowerModel.includes('o1-') ||
		lowerModel.includes('o3-') ||
		lowerModel.includes('o4-')
	)
		return OpenAI
	if (lowerModel.includes('claude')) return Anthropic
	if (lowerModel.includes('gemini')) return Google
	if (lowerModel.includes('llama')) return Meta
	if (lowerModel.includes('mistral') || lowerModel.includes('mixtral'))
		return Mistral
	if (lowerModel.includes('deepseek')) return DeepSeek
	if (lowerModel.includes('qwen')) return Qwen
	if (lowerModel.includes('grok')) return XAI
	if (lowerModel.includes('command')) return Cohere
	if (lowerModel.includes('glm') || lowerModel.includes('chatglm'))
		return ChatGLM
	return Box
}

export interface ModelIconProps {
	model: string
	provider?: string | null
	className?: string
}

/**
 * Provider/model brand icon shared by ModelBadge and the playground model
 * selectors. `createElement` renders the resolved (statically defined) icon
 * without binding a component identifier during render, which would trip
 * react-hooks/static-components.
 */
export function ModelIcon({ model, provider, className }: ModelIconProps) {
	return createElement(resolveModelIcon(model, provider), { className })
}
