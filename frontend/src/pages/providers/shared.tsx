import type { ComponentType } from 'react'
import type { TFunction } from 'i18next'
import { Anthropic, Google, OpenAI, OpenRouter } from '@lobehub/icons'
import { Box, ListChecks } from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { StatusBadge, StatusDot } from '@/components/ui/status'
import type {
	AffinityFailbackMode,
	ApiTypeOverride,
	ModelPriceRecord,
	MonoizeChannel,
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
	provider_type: ProviderType
	base_url: string
	api_key: string
	weight: number
	enabled: boolean
	models: ModelRow[]
	passive_failure_count_threshold_override: number | null
	passive_cooldown_seconds_override: number | null
	passive_window_seconds_override: number | null
	passive_rate_limit_cooldown_seconds_override: number | null
	active_probe_enabled_override: boolean | null
	active_probe_interval_seconds_override: number | null
	active_probe_success_threshold_override: number | null
	active_probe_model_override: string
	affinity_enabled_override: boolean | null
	affinity_idle_ttl_seconds_override: number | null
	affinity_failback_mode_override: AffinityFailbackMode | null
	affinity_failback_delay_seconds_override: number | null
	proxy_url: string
	extra_headers: string
	session_affinity_auto: boolean | null
	websocket_supported: boolean | null
}

export type ProviderForm = {
	id?: string
	name: string
	enabled: boolean
	max_retries: number
	channel_max_retries: number
	channel_retry_interval_ms: number
	circuit_breaker_enabled: boolean
	per_model_circuit_break: boolean
	active_probe_enabled_override: boolean | null
	active_probe_interval_seconds_override: number | null
	active_probe_success_threshold_override: number | null
	active_probe_model_override: string
	request_timeout_ms_override: number | null
	extra_fields_whitelist: string[]
	strip_cross_protocol_nested_extra: boolean | null
	allow_free_when_unpriced_override: boolean | null
	allow_free_when_missing_usage_override: boolean | null
	group_ids: string[]
	priority?: number
	channels: ChannelRow[]
	transforms: TransformRuleConfig[]
	api_type_overrides: ApiTypeOverride[]
}

export const PROVIDER_TYPE_CONFIG: Record<
	ProviderType,
	{ label: string; path: string; icon: ComponentType<{ className?: string }> }
> = {
	chat_completion: { label: 'Chat Completion', path: '/v1/chat/completions', icon: OpenAI },
	responses: { label: 'Responses', path: '/v1/responses', icon: OpenAI },
	messages: { label: 'Messages', path: '/v1/messages', icon: Anthropic },
	gemini: { label: 'Gemini', path: '/v1beta/models/{model}:generateContent', icon: Google },
	openai_image: { label: 'OpenAI Image', path: '/v1/images/generations', icon: OpenAI },
	openrouter_image: { label: 'OpenRouter Image', path: '/v1/images', icon: OpenRouter },
	replicate: { label: 'Replicate', path: '/v1/replicate/predictions', icon: Box },
	system_one: { label: 'System One', path: '/v1/systemone', icon: ListChecks },
	openai_decisions: { label: 'OpenAI Decisions', path: '/v1/decisions', icon: OpenAI }
}

export const PROVIDER_CHANNEL_OVERVIEW_ROW_HEIGHT = 40
export const DEFAULT_REASONING_SUFFIX_MAP: Record<string, string> = {
	'-thinking': 'high',
	'-reasoning': 'high',
	'-nothinking': 'none'
}

const BUILTIN_REASONING_SUFFIXES = [
	'-none', '-minimum', '-low', '-medium', '-high', '-xhigh', '-max'
]
const OPENAI_COMPACT_PRICING_SUFFIX = '-openai-compact'

export function emptyChannelRow(): ChannelRow {
	return {
		id: '',
		name: '',
		provider_type: 'chat_completion',
		base_url: '',
		api_key: '',
		weight: 1,
		enabled: true,
		models: [],
		passive_failure_count_threshold_override: null,
		passive_cooldown_seconds_override: null,
		passive_window_seconds_override: null,
		passive_rate_limit_cooldown_seconds_override: null,
		active_probe_enabled_override: null,
		active_probe_interval_seconds_override: null,
		active_probe_success_threshold_override: null,
		active_probe_model_override: '',
		affinity_enabled_override: null,
		affinity_idle_ttl_seconds_override: null,
		affinity_failback_mode_override: null,
		affinity_failback_delay_seconds_override: null,
		proxy_url: '',
		extra_headers: '',
		session_affinity_auto: null,
		websocket_supported: null
	}
}

export function emptyForm(): ProviderForm {
	return {
		id: '',
		name: '',
		enabled: true,
		max_retries: -1,
		channel_max_retries: 0,
		channel_retry_interval_ms: 0,
		circuit_breaker_enabled: true,
		per_model_circuit_break: false,
		active_probe_enabled_override: null,
		active_probe_interval_seconds_override: null,
		active_probe_success_threshold_override: null,
		active_probe_model_override: '',
		request_timeout_ms_override: null,
		extra_fields_whitelist: [],
		strip_cross_protocol_nested_extra: null,
		allow_free_when_unpriced_override: null,
		allow_free_when_missing_usage_override: null,
		group_ids: [],
		priority: undefined,
		channels: [emptyChannelRow()],
		transforms: [],
		api_type_overrides: []
	}
}

export function fromProvider(provider: Provider): ProviderForm {
	return {
		...emptyForm(),
		id: provider.id,
		name: provider.name,
		enabled: provider.enabled,
		max_retries: provider.max_retries,
		channel_max_retries: provider.channel_max_retries ?? 0,
		channel_retry_interval_ms: provider.channel_retry_interval_ms ?? 0,
		circuit_breaker_enabled: provider.circuit_breaker_enabled ?? true,
		per_model_circuit_break: provider.per_model_circuit_break ?? false,
		active_probe_enabled_override: provider.active_probe_enabled_override ?? null,
		active_probe_interval_seconds_override: provider.active_probe_interval_seconds_override ?? null,
		active_probe_success_threshold_override: provider.active_probe_success_threshold_override ?? null,
		active_probe_model_override: provider.active_probe_model_override ?? '',
		request_timeout_ms_override: provider.request_timeout_ms_override ?? null,
		extra_fields_whitelist: provider.extra_fields_whitelist ?? [],
		strip_cross_protocol_nested_extra: provider.strip_cross_protocol_nested_extra ?? null,
		allow_free_when_unpriced_override: provider.allow_free_when_unpriced_override ?? null,
		allow_free_when_missing_usage_override: provider.allow_free_when_missing_usage_override ?? null,
		group_ids: [...(provider.group_ids ?? [])],
		priority: provider.priority,
		channels: provider.channels.map(channel => ({
			id: channel.id,
			name: channel.name,
			provider_type: channel.provider_type,
			base_url: channel.base_url,
			api_key: '',
			weight: channel.weight,
			enabled: channel.enabled,
			models: Object.entries(channel.models ?? {})
				.sort(([left], [right]) => left.localeCompare(right))
				.map(([model, entry]) => ({
					model,
					redirect: entry.redirect ?? '',
					multiplier: String(entry.multiplier)
				})),
			passive_failure_count_threshold_override: channel.passive_failure_count_threshold_override ?? null,
			passive_cooldown_seconds_override: channel.passive_cooldown_seconds_override ?? null,
			passive_window_seconds_override: channel.passive_window_seconds_override ?? null,
			passive_rate_limit_cooldown_seconds_override: channel.passive_rate_limit_cooldown_seconds_override ?? null,
			active_probe_enabled_override: channel.active_probe_enabled_override ?? null,
			active_probe_interval_seconds_override: channel.active_probe_interval_seconds_override ?? null,
			active_probe_success_threshold_override: channel.active_probe_success_threshold_override ?? null,
			active_probe_model_override: channel.active_probe_model_override ?? '',
			affinity_enabled_override: channel.affinity_enabled_override ?? null,
			affinity_idle_ttl_seconds_override: channel.affinity_idle_ttl_seconds_override ?? null,
			affinity_failback_mode_override: channel.affinity_failback_mode_override ?? null,
			affinity_failback_delay_seconds_override: channel.affinity_failback_delay_seconds_override ?? null,
			proxy_url: channel.proxy_url ?? '',
			extra_headers: channel.extra_headers && Object.keys(channel.extra_headers).length > 0 ? JSON.stringify(channel.extra_headers, null, 2) : '',
			session_affinity_auto: channel.session_affinity_auto ?? null,
			websocket_supported: channel.websocket_supported ?? null
		})),
		transforms: (provider.transforms ?? []).map(rule => ({ ...rule, config: { ...rule.config } })),
		api_type_overrides: (provider.api_type_overrides ?? []).map(rule => ({ ...rule }))
	}
}

export function hasTrailingV1(baseUrl: string): boolean {
	return /\/v1\/?$/i.test(baseUrl.trim())
}

export function removeTrailingV1(baseUrl: string): string {
	return baseUrl.trim().replace(/\/v1\/?$/i, '')
}

/** PL2: "第 N 位 · 尝试全部 Channel / 最多 N 次" for a list position (0-based). */
export function providerRoutingSummary(position: number, maxRetries: number, t: TFunction): string {
	const retries = maxRetries < 0
		? t('providers.retryAllChannels')
		: t('providers.retryAtMost', { count: maxRetries + 1 })
	return `${t('providers.position', { position: position + 1 })} · ${retries}`
}

/** Server-side probe-model resolution (channel-management §3.8) mirrored for the quick-test hint. */
export function resolveQuickTestModel(
	channel: MonoizeChannel,
	provider: Provider,
	globalProbeModel: string | null | undefined
): string | undefined {
	return channel.active_probe_model_override
		|| provider.active_probe_model_override
		|| globalProbeModel
		|| Object.keys(channel.models ?? {}).sort()[0]
}

// MP-R3: a model counts as priced when its enabled `model_prices` row is
// complete for its billing mode.
export function buildPricedModelIdSet(modelPrices: ModelPriceRecord[]): Set<string> {
	return new Set(
		modelPrices
			.filter(record => {
				if (!record.enabled) return false
				if (record.billing_mode === 'per_token') return record.input_usd_per_1m != null
				if (record.billing_mode === 'per_request') return record.per_request_usd != null
				return record.billing_expr != null
			})
			.map(record => record.model_id)
	)
}

export function normalizePricingModelId(model: string, reasoningSuffixMap: Record<string, string>): string {
	const trimmed = model.trim()
	if (!trimmed) return ''
	const suffixes = Array.from(new Set([...Object.keys(reasoningSuffixMap), ...BUILTIN_REASONING_SUFFIXES, OPENAI_COMPACT_PRICING_SUFFIX]))
		.sort((a, b) => b.length - a.length || a.localeCompare(b))
	for (const suffix of suffixes) {
		if (trimmed.endsWith(suffix) && trimmed.length > suffix.length) {
			return trimmed.slice(0, -suffix.length)
		}
	}
	return trimmed
}

export function hasBillablePricingModelId(
	pricedModelIdSet: Set<string>,
	model: string,
	redirect: string | null | undefined,
	reasoningSuffixMap: Record<string, string>
): boolean {
	const logical = normalizePricingModelId(model, reasoningSuffixMap)
	const pricing = normalizePricingModelId(redirect?.trim() || model, reasoningSuffixMap)
	return pricedModelIdSet.has(pricing) || (pricing !== logical && pricedModelIdSet.has(logical))
}

export type HealthStatus = 'healthy' | 'probing' | 'unhealthy'
export type EffectiveChannelStatus = 'disabled' | 'no_traffic' | HealthStatus | 'unsaved'

/** PL18b: one status per Channel; disabled and zero weight hide health. */
export function effectiveChannelStatus(
	enabled: boolean,
	weight: number,
	health: HealthStatus | undefined
): EffectiveChannelStatus {
	if (!enabled) return 'disabled'
	if (weight <= 0) return 'no_traffic'
	return health ?? 'unsaved'
}

export function healthBadge(status: HealthStatus, t: TFunction) {
	if (status === 'healthy') {
		return <StatusBadge variant='success'><StatusDot variant='success' className='mr-1.5 h-1.5 w-1.5 animate-pulse' />{t('providers.statusHealthy')}</StatusBadge>
	}
	if (status === 'probing') {
		return <StatusBadge variant='warning'><StatusDot variant='warning' className='mr-1.5 h-1.5 w-1.5 animate-pulse' />{t('providers.statusProbing')}</StatusBadge>
	}
	return <StatusBadge variant='destructive'><StatusDot variant='destructive' className='mr-1.5 h-1.5 w-1.5 animate-pulse' />{t('providers.statusUnhealthy')}</StatusBadge>
}

export function effectiveStatusBadge(status: EffectiveChannelStatus, t: TFunction) {
	if (status === 'disabled') return <Badge variant='secondary'>{t('providers.statusDisabled')}</Badge>
	if (status === 'no_traffic') return <Badge variant='secondary'>{t('providers.statusNoTraffic')}</Badge>
	if (status === 'unsaved') return <Badge variant='secondary'>{t('providers.editor.unsaved')}</Badge>
	return healthBadge(status, t)
}
