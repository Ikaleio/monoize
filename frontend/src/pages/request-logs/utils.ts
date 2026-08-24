import type { RequestLog, RequestLogTriedProvider } from '@/lib/api'
import { formatNanoUsd, isSignedIntegerString } from '@/lib/exact-decimal'

type TimingValue = number | string | null | undefined

export type TpsBasis = {
	value: number
	tokens: number
	windowMs: number
}

export type BillingValueDimension =
	| 'usageClass'
	| 'unit'
	| 'modality'
	| 'cacheTtl'
	| 'contextTier'
	| 'serviceTier'

const BILLING_VALUE_TRANSLATION_KEYS: Record<
	BillingValueDimension,
	Record<string, string>
> = {
	usageClass: {
		input_uncached: 'requestLogs.billingUsageInputUncached',
		input_cached: 'requestLogs.billingUsageInputCached',
		cache_read: 'requestLogs.billingUsageCacheRead',
		cache_write_5m: 'requestLogs.billingUsageCacheWrite5m',
		cache_write_1h: 'requestLogs.billingUsageCacheWrite1h',
		output: 'requestLogs.billingUsageOutput',
		reasoning_output: 'requestLogs.billingUsageReasoningOutput',
		web_search: 'requestLogs.billingUsageWebSearch',
		file_search_tool_call: 'requestLogs.billingUsageFileSearch',
		x_search: 'requestLogs.billingUsageXSearch',
		code_execution: 'requestLogs.billingUsageCodeExecution',
		code_execution_duration: 'requestLogs.billingUsageCodeExecutionDuration',
		code_interpreter_duration: 'requestLogs.billingUsageCodeExecutionDuration'
	},
	unit: {
		token: 'requestLogs.billingUnitToken',
		call: 'requestLogs.billingUnitCall',
		request: 'requestLogs.billingUnitRequest',
		billed_minute: 'requestLogs.billingUnitBilledMinute'
	},
	modality: {
		text: 'requestLogs.billingModalityText',
		image: 'requestLogs.billingModalityImage',
		audio: 'requestLogs.billingModalityAudio',
		video: 'requestLogs.billingModalityVideo'
	},
	cacheTtl: {
		'5m': 'requestLogs.billingCacheTtl5m',
		'1h': 'requestLogs.billingCacheTtl1h'
	},
	contextTier: {
		default: 'requestLogs.billingTierDefault',
		short: 'requestLogs.billingContextShort',
		long: 'requestLogs.billingContextLong'
	},
	serviceTier: {
		default: 'requestLogs.billingTierDefault',
		standard: 'requestLogs.billingServiceStandard',
		priority: 'requestLogs.billingServicePriority',
		flex: 'requestLogs.billingServiceFlex',
		batch: 'requestLogs.billingServiceBatch'
	}
}

export type JsonObject = Record<string, unknown>

export function asObject(value: unknown): JsonObject | null {
	if (value && typeof value === 'object' && !Array.isArray(value)) {
		return value as JsonObject
	}
	return null
}

export function readNumber(value: unknown): number | null {
	if (typeof value === 'number' && Number.isFinite(value)) return value
	if (typeof value === 'string') {
		const parsed = Number(value)
		return Number.isFinite(parsed) ? parsed : null
	}
	return null
}

export function readTokenCount(obj: JsonObject | null, key: string): number | null {
	if (!obj) return null
	return readNumber(obj[key])
}

export function readNanoString(obj: JsonObject | null, key: string): string | null {
	if (!obj) return null
	const raw = obj[key]
	if (typeof raw === 'string' && raw.trim() !== '') return raw
	return null
}

function parseTimingMs(value: TimingValue): number | null {
	if (typeof value === 'number') {
		return Number.isFinite(value) && value >= 0 ? value : null
	}

	if (typeof value === 'string') {
		const trimmed = value.trim()
		if (!trimmed) return null

		const parsed = Number(trimmed)
		return Number.isFinite(parsed) && parsed >= 0 ? parsed : null
	}

	return null
}

function tpsFromBasis(tokens: number | null, windowMs: number | null): TpsBasis | null {
	if (tokens == null || tokens <= 0 || windowMs == null || windowMs <= 0) {
		return null
	}
	return {
		value: tokens / (windowMs / 1000),
		tokens,
		windowMs
	}
}

/** FL4a-1: the total output token count for the TPS numerator. */
function totalOutputTokens(log: RequestLog): number | null {
	const usageOutput = asObject(asObject(log.usage)?.output)
	return readTokenCount(usageOutput, 'total_tokens') ?? log.tokens.output ?? null
}

/**
 * FL4a-1..FL4a-3: single TPS metric following the new-api RecordRelaySample
 * model. Streaming rows with a known TTFB use `duration - ttfb` as the
 * generation window; every other row uses the total duration.
 * Returns null when the numerator or the window is absent or non-positive.
 */
export function computeTps(log: RequestLog): TpsBasis | null {
	const durationMs = getDurationMs(log)
	const ttfbMs = getTtfbMs(log)
	const windowMs =
		log.is_stream && durationMs != null && ttfbMs != null && durationMs > ttfbMs ?
			durationMs - ttfbMs
		:	durationMs
	return tpsFromBasis(totalOutputTokens(log), windowMs)
}

export function billingValueTranslationKey(
	dimension: BillingValueDimension,
	value: string
): string | null {
	return BILLING_VALUE_TRANSLATION_KEYS[dimension][value] ?? null
}

export function getDurationMs(log: RequestLog): number | null {
	return parseTimingMs(log.timing.duration_ms)
}

export function getTtfbMs(log: RequestLog): number | null {
	return parseTimingMs(log.timing.ttfb_ms)
}

export function formatCost(nanoUsd: string | null | undefined): string {
	if (nanoUsd == null) return '-'
	if (!isSignedIntegerString(nanoUsd)) return '-'
	return formatNanoUsd(nanoUsd, 6)
}

export function formatCachePercentage(
	cachedTokens: number | null | undefined,
	totalTokens: number | null | undefined
): string | null {
	if (
		cachedTokens == null ||
		!Number.isFinite(cachedTokens) ||
		cachedTokens <= 0 ||
		totalTokens == null ||
		!Number.isFinite(totalTokens) ||
		totalTokens <= 0
	) {
		return null
	}

	return `${Math.round((cachedTokens / totalTokens) * 100)}%`
}

export function formatDuration(ms: number | null | undefined): string | null {
	if (ms == null) return null
	if (ms < 1000) return `${ms}ms`
	return `${(ms / 1000).toFixed(2)}s`
}

export function formatTime(dateString: string): string {
	const date = new Date(dateString)
	const y = date.getFullYear()
	const mo = String(date.getMonth() + 1).padStart(2, '0')
	const d = String(date.getDate()).padStart(2, '0')
	const h = String(date.getHours()).padStart(2, '0')
	const mi = String(date.getMinutes()).padStart(2, '0')
	const s = String(date.getSeconds()).padStart(2, '0')
	return `${y}-${mo}-${d} ${h}:${mi}:${s}`
}

const RETRY_CHAIN_SEPARATOR = ' → '

function nonempty(value: string | null | undefined): string | null {
	const trimmed = value?.trim()
	return trimmed ? trimmed : null
}

function readableRouteName(
	providerName: string | null | undefined,
	channelName: string | null | undefined
): string | null {
	const names = [nonempty(providerName), nonempty(channelName)].filter(
		(value): value is string => value != null
	)
	return names.length > 0 ? names.join('/') : null
}

export function triedProvidersOf(log: RequestLog): RequestLogTriedProvider[] {
	const raw = log.tried_providers as unknown
	if (Array.isArray(raw)) {
		return raw
	}
	if (typeof raw === 'string' && raw.trim()) {
		try {
			const parsed = JSON.parse(raw) as unknown
			return Array.isArray(parsed) ? (parsed as RequestLogTriedProvider[]) : []
		} catch {
			return []
		}
	}
	return []
}

export type RetryHopIdentity = {
	provider_id?: string | null
	channel_id?: string | null
	provider_name?: string | null
	channel_name?: string | null
}

function affinityTargetKey(
	providerId: string | null | undefined,
	channelId: string | null | undefined
): string | null {
	const provider = nonempty(providerId)
	const channel = nonempty(channelId)
	return provider && channel ? `${provider}/${channel}` : null
}

export function readableAffinityTarget(
	log: RequestLog,
	knownTargets: ReadonlyMap<string, string>
): string | null {
	const target = nonempty(log.affinity.target)
	if (!target) return null

	const knownName = nonempty(knownTargets.get(target))
	if (knownName) return knownName

	const terminalKey = affinityTargetKey(log.provider.id, log.channel.id)
	if (terminalKey === target) {
		const terminalName = readableRouteName(log.provider.name, log.channel.name)
		if (terminalName) return terminalName
	}

	for (const attempt of triedProvidersOf(log)) {
		if (affinityTargetKey(attempt.provider_id, attempt.channel_id) !== target) continue
		const attemptName = readableRouteName(attempt.provider_name, attempt.channel_name)
		if (attemptName) return attemptName
	}

	return null
}

export function hopDisplayLabel(hop: RetryHopIdentity): string {
	return (
		nonempty(hop.channel_name) ||
		nonempty(hop.provider_name) ||
		nonempty(hop.channel_id) ||
		nonempty(hop.provider_id) ||
		''
	)
}

function hopIdentityKey(providerId: string | null | undefined, channelId: string | null | undefined): string {
	return `${providerId ?? ''}\0${channelId ?? ''}`
}

export function compactRetryChainLabels(log: RequestLog): string[] | null {
	const hops: string[] = []
	const seen = new Set<string>()
	const push = (key: string, label: string) => {
		if (!label || seen.has(key)) return
		seen.add(key)
		hops.push(label)
	}
	for (const tried of triedProvidersOf(log)) {
		push(hopIdentityKey(tried.provider_id, tried.channel_id), hopDisplayLabel(tried))
	}
	if (log.provider.id || log.channel.id) {
		push(
			hopIdentityKey(log.provider.id, log.channel.id),
			hopDisplayLabel({
				provider_id: log.provider.id,
				channel_id: log.channel.id,
				provider_name: log.provider.name,
				channel_name: log.channel.name
			})
		)
	}
	return hops.length >= 2 ? hops : null
}

export function formatRetryChain(labels: string[]): string {
	return labels.join(RETRY_CHAIN_SEPARATOR)
}

export type RetryAttemptRow = {
	label: string
	error: string | null
	upstreamStatus: number | null
	durationMs: number | null
	outcome: 'failed' | 'served'
}

export function retryAttemptRows(log: RequestLog): RetryAttemptRow[] {
	const tried = triedProvidersOf(log)
	const rows: RetryAttemptRow[] = tried.map((entry: RequestLogTriedProvider) => ({
		label:
			hopDisplayLabel(entry) || `${entry.provider_id}/${entry.channel_id}`,
		error: entry.error || null,
		upstreamStatus: entry.upstream_status ?? null,
		durationMs: readNumber(entry.duration_ms),
		outcome: 'failed'
	}))
	const terminalLabel = hopDisplayLabel({
		provider_id: log.provider.id,
		channel_id: log.channel.id,
		provider_name: log.provider.name,
		channel_name: log.channel.name
	})
	if (!terminalLabel || (!log.provider.id && !log.channel.id)) {
		return rows
	}
	const terminalKey = hopIdentityKey(log.provider.id, log.channel.id)
	const lastTried = tried.at(-1)
	const lastTriedKey = lastTried
		? hopIdentityKey(lastTried.provider_id, lastTried.channel_id)
		: null
	if (log.status === 'error' && lastTriedKey === terminalKey) {
		return rows
	}
	if (
		log.status === 'success' ||
		log.status === 'client_gone' ||
		lastTriedKey !== terminalKey
	) {
		rows.push({
			label: terminalLabel,
			error: log.status === 'error' ? log.error.message ?? null : null,
			upstreamStatus: log.status === 'error' ? log.error.http_status ?? null : null,
			durationMs: null,
			outcome: log.status === 'error' ? 'failed' : 'served'
		})
	}
	return rows
}
