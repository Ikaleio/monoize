import { createElement, type CSSProperties } from 'react'
import Ai302 from '@lobehub/icons/es/Ai302/components/Mono'
import AiHubMix from '@lobehub/icons/es/AiHubMix/components/Mono'
import AionLabs from '@lobehub/icons/es/AionLabs/components/Mono'
import Alibaba from '@lobehub/icons/es/Alibaba/components/Mono'
import AntGroup from '@lobehub/icons/es/AntGroup/components/Mono'
import Anthropic from '@lobehub/icons/es/Anthropic/components/Mono'
import Arcee from '@lobehub/icons/es/Arcee/components/Mono'
import Aws from '@lobehub/icons/es/Aws/components/Mono'
import Azure from '@lobehub/icons/es/Azure/components/Mono'
import BAAI from '@lobehub/icons/es/BAAI/components/Mono'
import Baichuan from '@lobehub/icons/es/Baichuan/components/Mono'
import Baidu from '@lobehub/icons/es/Baidu/components/Mono'
import Baseten from '@lobehub/icons/es/Baseten/components/Mono'
import Bedrock from '@lobehub/icons/es/Bedrock/components/Mono'
import ByteDance from '@lobehub/icons/es/ByteDance/components/Mono'
import Cerebras from '@lobehub/icons/es/Cerebras/components/Mono'
import ChatGLM from '@lobehub/icons/es/ChatGLM/components/Mono'
import Cloudflare from '@lobehub/icons/es/Cloudflare/components/Mono'
import Cohere from '@lobehub/icons/es/Cohere/components/Mono'
import DeepCogito from '@lobehub/icons/es/DeepCogito/components/Mono'
import DeepInfra from '@lobehub/icons/es/DeepInfra/components/Mono'
import DeepSeek from '@lobehub/icons/es/DeepSeek/components/Mono'
import Dolphin from '@lobehub/icons/es/Dolphin/components/Mono'
import Doubao from '@lobehub/icons/es/Doubao/components/Mono'
import Fireworks from '@lobehub/icons/es/Fireworks/components/Mono'
import Flux from '@lobehub/icons/es/Flux/components/Mono'
import Gemma from '@lobehub/icons/es/Gemma/components/Mono'
import Github from '@lobehub/icons/es/Github/components/Mono'
import Google from '@lobehub/icons/es/Google/components/Mono'
import Groq from '@lobehub/icons/es/Groq/components/Mono'
import HuggingFace from '@lobehub/icons/es/HuggingFace/components/Mono'
import Hunyuan from '@lobehub/icons/es/Hunyuan/components/Mono'
import Hyperbolic from '@lobehub/icons/es/Hyperbolic/components/Mono'
import IBM from '@lobehub/icons/es/IBM/components/Mono'
import Inception from '@lobehub/icons/es/Inception/components/Mono'
import Kimi from '@lobehub/icons/es/Kimi/components/Mono'
import KwaiKAT from '@lobehub/icons/es/KwaiKAT/components/Mono'
import Lambda from '@lobehub/icons/es/Lambda/components/Mono'
import Liquid from '@lobehub/icons/es/Liquid/components/Mono'
import LongCat from '@lobehub/icons/es/LongCat/components/Mono'
import Meta from '@lobehub/icons/es/Meta/components/Mono'
import Microsoft from '@lobehub/icons/es/Microsoft/components/Mono'
import Minimax from '@lobehub/icons/es/Minimax/components/Mono'
import Mistral from '@lobehub/icons/es/Mistral/components/Mono'
import Moonshot from '@lobehub/icons/es/Moonshot/components/Mono'
import Nebius from '@lobehub/icons/es/Nebius/components/Mono'
import NousResearch from '@lobehub/icons/es/NousResearch/components/Mono'
import Nova from '@lobehub/icons/es/Nova/components/Mono'
import Novita from '@lobehub/icons/es/Novita/components/Mono'
import Nvidia from '@lobehub/icons/es/Nvidia/components/Mono'
import Ollama from '@lobehub/icons/es/Ollama/components/Mono'
import OpenAI from '@lobehub/icons/es/OpenAI/components/Mono'
import OpenRouter from '@lobehub/icons/es/OpenRouter/components/Mono'
import Perplexity from '@lobehub/icons/es/Perplexity/components/Mono'
import Poe from '@lobehub/icons/es/Poe/components/Mono'
import Qwen from '@lobehub/icons/es/Qwen/components/Mono'
import Replicate from '@lobehub/icons/es/Replicate/components/Mono'
import SambaNova from '@lobehub/icons/es/SambaNova/components/Mono'
import SiliconCloud from '@lobehub/icons/es/SiliconCloud/components/Mono'
import Snowflake from '@lobehub/icons/es/Snowflake/components/Mono'
import Spark from '@lobehub/icons/es/Spark/components/Mono'
import Stepfun from '@lobehub/icons/es/Stepfun/components/Mono'
import Tencent from '@lobehub/icons/es/Tencent/components/Mono'
import Together from '@lobehub/icons/es/Together/components/Mono'
import Upstage from '@lobehub/icons/es/Upstage/components/Mono'
import Vercel from '@lobehub/icons/es/Vercel/components/Mono'
import VertexAI from '@lobehub/icons/es/VertexAI/components/Mono'
import Vllm from '@lobehub/icons/es/Vllm/components/Mono'
import Volcengine from '@lobehub/icons/es/Volcengine/components/Mono'
import Wenxin from '@lobehub/icons/es/Wenxin/components/Mono'
import XAI from '@lobehub/icons/es/XAI/components/Mono'
import XiaomiMiMo from '@lobehub/icons/es/XiaomiMiMo/components/Mono'
import Yi from '@lobehub/icons/es/Yi/components/Mono'
import ZenMux from '@lobehub/icons/es/ZenMux/components/Mono'
import ZeroOne from '@lobehub/icons/es/ZeroOne/components/Mono'
import Zhipu from '@lobehub/icons/es/Zhipu/components/Mono'
import { Box } from 'lucide-react'

type IconComponent = React.ComponentType<{
	className?: string
	style?: CSSProperties
}>

// Per-icon optical centering for @lobehub/icons SVGs whose path bounding boxes
// sit >3% off the 24x24 viewBox center.
const OPTICAL_STYLES: ReadonlyMap<IconComponent, CSSProperties> = new Map([
	[Kimi, { transform: 'translate(-5.8%, 8.3%)' }],
	[Arcee, { transform: 'translate(0.6%, 7.3%)' }],
	[Vercel, { transform: 'translate(0%, 4.5%)' }],
	[Aws, { transform: 'translate(0%, -4.9%)' }],
	[AntGroup, { transform: 'translate(2.1%, 4.2%)' }],
	[Volcengine, { transform: 'translate(0.4%, 4.2%)' }],
	[Fireworks, { transform: 'translate(0%, 4.2%)' }],
	[Baichuan, { transform: 'translate(4.2%, 0%)' }],
	[Nvidia, { transform: 'translate(-4.2%, 0%)' }],
	[Poe, { transform: 'translate(0%, -3.8%)' }],
	[Yi, { transform: 'translate(3.7%, 0%)' }],
	[Vllm, { transform: 'translate(3.4%, 2.1%)' }]
])

const PROVIDER_ICONS: Record<string, IconComponent> = {
	openai: OpenAI,
	anthropic: Anthropic,
	google: Google,
	meta: Meta,
	llama: Meta,
	mistral: Mistral,
	perplexity: Perplexity,
	groq: Groq,
	cohere: Cohere,
	deepseek: DeepSeek,
	qwen: Qwen,
	minimax: Minimax,
	zhipu: Zhipu,
	zhipuai: Zhipu,
	spark: Spark,
	moonshot: Moonshot,
	moonshotai: Moonshot,
	kimi: Kimi,
	bytedance: ByteDance,
	doubao: Doubao,
	volcengine: Volcengine,
	alibaba: Alibaba,
	alibabacn: Alibaba,
	tencent: Tencent,
	hunyuan: Hunyuan,
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
	chatglm: ChatGLM,
	nvidia: Nvidia,
	xiaomi: XiaomiMiMo,
	xiaomimimo: XiaomiMiMo,
	upstage: Upstage,
	inception: Inception,
	ibm: IBM,
	watsonx: IBM,
	microsoft: Microsoft,
	azure: Azure,
	azurecognitiveservices: Azure,
	aws: Aws,
	bedrock: Bedrock,
	amazonbedrock: Bedrock,
	vertexai: VertexAI,
	googlevertex: VertexAI,
	googlevertexanthropic: VertexAI,
	openrouter: OpenRouter,
	vercel: Vercel,
	together: Together,
	togetherai: Together,
	fireworks: Fireworks,
	fireworksai: Fireworks,
	cloudflare: Cloudflare,
	cloudflareworkersai: Cloudflare,
	siliconflow: SiliconCloud,
	siliconflowcn: SiliconCloud,
	siliconcloud: SiliconCloud,
	novita: Novita,
	novitaai: Novita,
	'302ai': Ai302,
	ai302: Ai302,
	aihubmix: AiHubMix,
	zenmux: ZenMux,
	poe: Poe,
	replicate: Replicate,
	baseten: Baseten,
	cerebras: Cerebras,
	deepinfra: DeepInfra,
	hyperbolic: Hyperbolic,
	lambda: Lambda,
	nebius: Nebius,
	sambanova: SambaNova,
	snowflake: Snowflake,
	antgroup: AntGroup,
	bailing: AntGroup,
	baichuan: Baichuan,
	kwaikat: KwaiKAT,
	nousresearch: NousResearch,
	aionlabs: AionLabs,
	baai: BAAI,
	liquid: Liquid,
	deepcogito: DeepCogito,
	arcee: Arcee,
	bfl: Flux,
	flux: Flux,
	longcat: LongCat
}

const MODEL_BRAND_RULES: ReadonlyArray<readonly [RegExp, IconComponent]> = [
	// Derivative/specialized families before base families (llama, qwen, mistral, gemma)
	[/deepseek/, DeepSeek],
	[/nemotron|nvidia/, Nvidia],
	[/hermes/, NousResearch],
	[/dolphin/, Dolphin],
	[/(?:^|[-/_.:])bge[-_]/, BAAI],
	[/cogito/, DeepCogito],
	[/aion/, AionLabs],
	// Major model families
	[
		/gpt|chatgpt|davinci|text-embedding|whisper|dall-?e|sora|(?:^|[-/_.:])o[134](?:$|[-/_.:])/,
		OpenAI
	],
	[/claude/, Anthropic],
	[/gemini|imagen|(?:^|[-/_.:])veo(?:$|[-/_.:])/, Google],
	[/gemma/, Gemma],
	[/grok/, XAI],
	[/qwen|(?:^|[-/_.:])q[wv]q(?:$|[-/_.:])/, Qwen],
	[/kimi/, Kimi],
	[/moonshot/, Moonshot],
	[/doubao|dola-seed|(?:^|[-/_.:])seed-[0-9]/, Doubao],
	[/minimax/, Minimax],
	[/glm|chatglm/, ChatGLM],
	[/mistral|mixtral|ministral|devstral|codestral|pixtral|magistral|voxtral/, Mistral],
	[/llama/, Meta],
	[/command|cohere/, Cohere],
	[/mimo/, XiaomiMiMo],
	[/(?:^|[-/_.:])nova(?:$|[-/_.:])/, Nova],
	[/amazon--titan|amazon\.titan/, Aws],
	[/(?:^|[-/_.:])step-/, Stepfun],
	[/ernie|wenxin/, Wenxin],
	[/(?:^|[-/_.:])phi[-_]?[1-9]/, Microsoft],
	[/hunyuan|(?:^|[-/_.:])hy-/, Hunyuan],
	[/(?:^|[-/_.:])sonar(?:$|[-/_.:])/, Perplexity],
	[/(?:^|[-/_.:])(ling|ring)-/, AntGroup],
	[/(?:^|[-/_.:])solar[-_]/, Upstage],
	[/(?:^|[-/_.:])kat-/, KwaiKAT],
	[/mercury/, Inception],
	[/granite/, IBM],
	[/(?:^|[-/_.:])yi-/, Yi],
	[/baichuan/, Baichuan],
	[/longcat/, LongCat],
	[/(?:^|[-/_.:])lfm\d?[-_]/, Liquid],
	[/(?:^|[-/_.:])flux(?:$|[-/_.:])/, Flux],
	[/trinity/, Arcee]
]

const normalizeProvider = (value: string) =>
	value.toLowerCase().replace(/[\s_-]/g, '')

function resolveModelIcon(
	model: string,
	provider?: string | null
): IconComponent {
	const lowerModel = model.toLowerCase()
	for (const [pattern, icon] of MODEL_BRAND_RULES) {
		if (pattern.test(lowerModel)) {
			return icon
		}
	}
	const normalizedProvider = provider ? normalizeProvider(provider) : undefined
	if (normalizedProvider && PROVIDER_ICONS[normalizedProvider]) {
		return PROVIDER_ICONS[normalizedProvider]
	}
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
	const icon = resolveModelIcon(model, provider)
	return createElement(icon, {
		className,
		style: OPTICAL_STYLES.get(icon)
	})
}
