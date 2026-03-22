import type { ComponentType } from 'react'
import { Anthropic, Google, OpenAI, XAI } from '@lobehub/icons'
import { Badge } from '@/components/ui/badge'
import type {
	ApiTypeOverride,
	ModelMetadataRecord,
	Provider,
	ProviderType,
	TransformRuleConfig
} from '@/lib/api'

export type ModelRow = {
	model: string
	redirect: string
	multiplier: string
}

export type ChannelRow = {
	id: string
	name: string
	base_url: string
	api_key: string
	weight: string
	enabled: boolean
	passive_failure_threshold_override: string
	passive_cooldown_seconds_override: string
	passive_window_seconds_override: string
	passive_min_samples_override: string
	passive_failure_rate_threshold_override: string
	passive_rate_limit_cooldown_seconds_override: string
}

export type ProviderForm = {
	id?: string
	name: string
	provider_type: ProviderType
	enabled: boolean
	max_retries: number
	active_probe_enabled_override: boolean | null
	active_probe_interval_seconds_override: number | null
	active_probe_success_threshold_override: number | null
	active_probe_model_override: string | null
	request_timeout_ms_override: string
	priority?: number
	models: ModelRow[]
	channels: ChannelRow[]
	transforms: TransformRuleConfig[]
	api_type_overrides: ApiTypeOverride[]
}

export const PROVIDER_TYPE_CONFIG: Record<
	ProviderForm['provider_type'],
	{
		label: string
		path: string
		icon: ComponentType<{ className?: string }>
	}
> = {
	chat_completion: {
		label: 'Chat Completion',
		path: '/v1/chat/completions',
		icon: OpenAI
	},
	responses: { label: 'Responses', path: '/v1/responses', icon: OpenAI },
	messages: { label: 'Messages', path: '/v1/messages', icon: Anthropic },
	gemini: {
		label: 'Gemini',
		path: '/v1beta/models/{model}:generateContent',
		icon: Google
	},
	grok: { label: 'Responses (Grok)', path: '/v1/responses', icon: XAI }
}

export const PROVIDER_CHANNEL_OVERVIEW_ROW_HEIGHT = 40
export const PROVIDER_EDIT_CHANNEL_ROW_HEIGHT = 56
export const DEFAULT_REASONING_SUFFIX_MAP: Record<string, string> = {
	'-thinking': 'high',
	'-reasoning': 'high',
	'-nothinking': 'none'
}

const BUILTIN_REASONING_SUFFIXES = [
	'-none',
	'-minimum',
	'-low',
	'-medium',
	'-high',
	'-xhigh',
	'-max'
]

export function emptyForm(): ProviderForm {
	return {
		id: '',
		name: '',
		provider_type: 'chat_completion',
		enabled: true,
		max_retries: -1,
		active_probe_enabled_override: null,
		active_probe_interval_seconds_override: null,
		active_probe_success_threshold_override: null,
		active_probe_model_override: null,
		request_timeout_ms_override: '',
		priority: undefined,
		models: [],
		channels: [],
		transforms: [],
		api_type_overrides: []
	}
}

export function emptyModelRow(): ModelRow {
	return {
		model: '',
		redirect: '',
		multiplier: '1'
	}
}

export function emptyChannelRow(): ChannelRow {
	return {
		id: '',
		name: '',
		base_url: '',
		api_key: '',
		weight: '1',
		enabled: true,
		passive_failure_threshold_override: '',
		passive_cooldown_seconds_override: '',
		passive_window_seconds_override: '',
		passive_min_samples_override: '',
		passive_failure_rate_threshold_override: '',
		passive_rate_limit_cooldown_seconds_override: ''
	}
}

export function fromProvider(provider: Provider): ProviderForm {
	return {
		id: provider.id,
		name: provider.name,
		provider_type: provider.provider_type,
		enabled: provider.enabled,
		max_retries: provider.max_retries,
		active_probe_enabled_override:
			provider.active_probe_enabled_override ?? null,
		active_probe_interval_seconds_override:
			provider.active_probe_interval_seconds_override ?? null,
		active_probe_success_threshold_override:
			provider.active_probe_success_threshold_override ?? null,
		active_probe_model_override: provider.active_probe_model_override ?? null,
		request_timeout_ms_override:
			provider.request_timeout_ms_override != null ?
				String(provider.request_timeout_ms_override)
			:	'',
		priority: provider.priority,
		models: Object.entries(provider.models).map(([model, entry]) => ({
			model,
			redirect: entry.redirect ?? '',
			multiplier: String(entry.multiplier)
		})),
		channels: provider.channels.map(channel => ({
			id: channel.id,
			name: channel.name,
			base_url: channel.base_url,
			api_key: '',
			weight: String(channel.weight),
			enabled: channel.enabled,
			passive_failure_threshold_override:
				channel.passive_failure_threshold_override != null ?
					String(channel.passive_failure_threshold_override)
				:	'',
			passive_cooldown_seconds_override:
				channel.passive_cooldown_seconds_override != null ?
					String(channel.passive_cooldown_seconds_override)
				:	'',
			passive_window_seconds_override:
				channel.passive_window_seconds_override != null ?
					String(channel.passive_window_seconds_override)
				:	'',
			passive_min_samples_override:
				channel.passive_min_samples_override != null ?
					String(channel.passive_min_samples_override)
				:	'',
			passive_failure_rate_threshold_override:
				channel.passive_failure_rate_threshold_override != null ?
					String(channel.passive_failure_rate_threshold_override)
				:	'',
			passive_rate_limit_cooldown_seconds_override:
				channel.passive_rate_limit_cooldown_seconds_override != null ?
					String(channel.passive_rate_limit_cooldown_seconds_override)
				:	''
		})),
		transforms: provider.transforms ?? [],
		api_type_overrides: provider.api_type_overrides ?? []
	}
}

export function hasTrailingV1(baseUrl: string): boolean {
	return /\/v1\/?$/i.test(baseUrl.trim())
}

export function removeTrailingV1(baseUrl: string): string {
	return baseUrl.trim().replace(/\/v1\/?$/i, '')
}

export function buildPricedModelIdSet(
	modelMetadata: ModelMetadataRecord[]
): Set<string> {
	const set = new Set<string>()
	for (const item of modelMetadata) {
		if (
			item.input_cost_per_token_nano != null &&
			item.output_cost_per_token_nano != null
		) {
			set.add(item.model_id)
		}
	}
	return set
}

function resolvePricingModelId(model: string, redirect?: string | null): string {
	const redirectTarget = redirect?.trim()
	return redirectTarget ? redirectTarget : model.trim()
}

export function normalizePricingModelId(
	model: string,
	reasoningSuffixMap: Record<string, string>
): string {
	const trimmed = model.trim()
	if (!trimmed) return ''
	const suffixes = Array.from(
		new Set([
			...Object.keys(reasoningSuffixMap),
			...BUILTIN_REASONING_SUFFIXES
		])
	).sort((a, b) => b.length - a.length || a.localeCompare(b))
	for (const suffix of suffixes) {
		if (trimmed.endsWith(suffix)) {
			const base = trimmed.slice(0, -suffix.length)
			if (base) {
				return base
			}
		}
	}
	return trimmed
}

function resolveNormalizedPricingModelId(
	model: string,
	redirect: string | null | undefined,
	reasoningSuffixMap: Record<string, string>
): string {
	return normalizePricingModelId(
		resolvePricingModelId(model, redirect),
		reasoningSuffixMap
	)
}

export function hasBillablePricingModelId(
	pricedModelIdSet: Set<string>,
	model: string,
	redirect: string | null | undefined,
	reasoningSuffixMap: Record<string, string>
): boolean {
	const normalizedLogicalModelId = normalizePricingModelId(
		model,
		reasoningSuffixMap
	)
	const normalizedPricingModelId = resolveNormalizedPricingModelId(
		model,
		redirect,
		reasoningSuffixMap
	)
	return (
		pricedModelIdSet.has(normalizedPricingModelId) ||
		(normalizedPricingModelId !== normalizedLogicalModelId &&
			pricedModelIdSet.has(normalizedLogicalModelId))
	)
}

export function statusBadge(status?: string) {
	if (status === 'healthy') {
		return (
			<Badge className='bg-emerald-600/15 text-emerald-700 hover:bg-emerald-600/15 dark:bg-emerald-500/15 dark:text-emerald-400 border-0'>
				<span className='mr-1.5 h-1.5 w-1.5 rounded-full bg-emerald-500 inline-block animate-pulse' />
				Healthy
			</Badge>
		)
	}
	if (status === 'probing') {
		return (
			<Badge className='bg-amber-500/15 text-amber-700 hover:bg-amber-500/15 dark:bg-amber-500/15 dark:text-amber-400 border-0'>
				<span className='mr-1.5 h-1.5 w-1.5 rounded-full bg-amber-500 inline-block animate-pulse' />
				Probing
			</Badge>
		)
	}
	return (
		<Badge className='bg-red-500/15 text-red-700 hover:bg-red-500/15 dark:bg-red-500/15 dark:text-red-400 border-0'>
			<span className='mr-1.5 h-1.5 w-1.5 rounded-full bg-red-500 inline-block animate-pulse' />
			Unhealthy
		</Badge>
	)
}
