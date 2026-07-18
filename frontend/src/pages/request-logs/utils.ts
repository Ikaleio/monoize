import type { RequestLog } from '@/lib/api'

type TimingValue = number | string | null | undefined

export type ComputedTps =
	| {
			state: 'display'
			value: number
			tokens: number
			denominatorMs: number
	  }
	| {
			state: 'unavailable'
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
	if (typeof raw === 'number' && Number.isFinite(raw)) return String(raw)
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

function tpsFromBasis(tokens: number | null, denominatorMs: number | null): ComputedTps {
	if (tokens == null || tokens <= 0 || denominatorMs == null || denominatorMs <= 0) {
		return { state: 'unavailable' }
	}
	return {
		state: 'display',
		value: tokens / (denominatorMs / 1000),
		tokens,
		denominatorMs
	}
}

function legacyOutputTokens(log: RequestLog): number | null {
	const usageOutput = asObject(asObject(log.usage)?.output)
	const outputTotal = readTokenCount(usageOutput, 'total_tokens') ?? log.tokens.output ?? null
	if (outputTotal == null) return null
	const reasoning = readTokenCount(usageOutput, 'reasoning_tokens') ?? log.tokens.reasoning ?? null
	return reasoning == null ? outputTotal : Math.max(outputTotal - reasoning, 0)
}

export function computeTps(log: RequestLog): ComputedTps {
	const outputTokens = legacyOutputTokens(log)
	const visibleTokens = readNumber(log.timing.visible_output_tokens)
	const visibleGenerationMs = parseTimingMs(log.timing.visible_generation_ms)
	const durationMs = getDurationMs(log)
	const ttfbMs = getTtfbMs(log)
	const fallbackDenominatorMs =
		durationMs == null ? null
		: ttfbMs != null && durationMs > ttfbMs ? durationMs - ttfbMs
		: durationMs
	const denominatorMs =
		visibleGenerationMs != null && visibleGenerationMs > 0 ?
			visibleGenerationMs
		: fallbackDenominatorMs
	const tokens = outputTokens != null && outputTokens > 0 ? outputTokens : visibleTokens
	return tpsFromBasis(tokens, denominatorMs)
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
	const cost = Number(nanoUsd) / 1e9
	if (!Number.isFinite(cost)) return '-'
	return new Intl.NumberFormat('en-US', {
		style: 'currency',
		currency: 'USD',
		minimumFractionDigits: 6,
		maximumFractionDigits: 9
	}).format(cost)
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
