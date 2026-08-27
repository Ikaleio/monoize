import { describe, expect, test } from 'bun:test'
import type { RequestLog } from '../src/lib/api'
import en from '../src/locales/en.json'
import ja from '../src/locales/ja.json'
import zh from '../src/locales/zh.json'
import zhTw from '../src/locales/zh-TW.json'
import {
	billingValueTranslationKey,
	compactRetryChainLabels,
	computeTps,
	formatCachePercentage,
	formatCost,
	formatRetryChain,
	hopDisplayLabel,
	readableAffinityTarget,
	retryAttemptRows,
	type BillingValueDimension
} from '../src/pages/request-logs/utils'

function requestLog(overrides: Partial<RequestLog> = {}): RequestLog {
	return {
		id: 'log-1',
		created_at: '2026-07-18T00:00:00.000Z',
		status: 'success',
		is_stream: true,
		model: 'test-model',
		provider: {},
		channel: {},
		user: { id: 'user-1' },
		api_key: {},
		tokens: {},
		timing: {},
		billing: {},
		error: {},
		...overrides
	}
}

describe('computeTps', () => {
	test('streaming rows exclude TTFB from the generation window (FL4a-2)', () => {
		const result = computeTps(
			requestLog({
				tokens: { output: 30 },
				timing: { duration_ms: 1200, ttfb_ms: 200 }
			})
		)

		expect(result).toEqual({ value: 30, tokens: 30, windowMs: 1000 })
	})

	test('non-streaming rows use total duration even when TTFB exists (FL4a-2)', () => {
		const result = computeTps(
			requestLog({
				is_stream: false,
				tokens: { output: 30 },
				timing: { duration_ms: 1200, ttfb_ms: 200 }
			})
		)

		expect(result).toEqual({ value: 25, tokens: 30, windowMs: 1200 })
	})

	test('uses duration when TTFB is absent', () => {
		const result = computeTps(
			requestLog({
				tokens: { output: 12 },
				timing: { duration_ms: 900 }
			})
		)

		expect(result).toEqual({ value: 12 / 0.9, tokens: 12, windowMs: 900 })
	})

	test('usage output total takes precedence over scalar output tokens (FL4a-1)', () => {
		const result = computeTps(
			requestLog({
				tokens: { output: 8 },
				usage: { output: { total_tokens: 40 } },
				timing: { duration_ms: 2000, ttfb_ms: 1000 }
			})
		)

		expect(result).toEqual({ value: 40, tokens: 40, windowMs: 1000 })
	})

	test('omits TPS when no positive token numerator exists (FL4a-4)', () => {
		expect(
			computeTps(
				requestLog({
					tokens: { output: 0 },
					timing: { duration_ms: 500 }
				})
			)
		).toBeNull()
	})

	test('omits TPS when no positive generation window exists (FL4a-4)', () => {
		expect(computeTps(requestLog({ tokens: { output: 30 }, timing: {} }))).toBeNull()
	})

	test('imposes no minimum window duration (FL4a-3)', () => {
		const result = computeTps(
			requestLog({
				tokens: { output: 3 },
				timing: { duration_ms: 20, ttfb_ms: 15 }
			})
		)

		expect(result).toEqual({ value: 600, tokens: 3, windowMs: 5 })
	})
})

describe('formatCost', () => {
	test('formats nano-USD with exactly six digits and exact half-up rounding', () => {
		expect(formatCost('123000')).toBe('$0.000123')
		expect(formatCost('123499')).toBe('$0.000123')
		expect(formatCost('123500')).toBe('$0.000124')
	})

	test('does not narrow large integer strings through Number', () => {
		expect(formatCost('9007199254740993000')).toBe('$9,007,199,254.740993')
	})
})

describe('formatCachePercentage', () => {
	test('rounds cached input as an integer share of total input', () => {
		expect(formatCachePercentage(16_000, 32_000)).toBe('50%')
		expect(formatCachePercentage(1, 3)).toBe('33%')
	})

	test('omits the share when cached or total input is not positive', () => {
		expect(formatCachePercentage(0, 32_000)).toBeNull()
		expect(formatCachePercentage(16_000, 0)).toBeNull()
		expect(formatCachePercentage(null, 32_000)).toBeNull()
		expect(formatCachePercentage(16_000, null)).toBeNull()
	})
})

describe('readableAffinityTarget', () => {
	test('resolves an internal binding id through the admin Provider catalog', () => {
		const log = requestLog({
			affinity: {
				target: '8rrbb6hw/mono_ch_7bf5f8e819794fd3b2911e8c35f38556'
			}
		})
		const knownTargets = new Map([
			[
				'8rrbb6hw/mono_ch_7bf5f8e819794fd3b2911e8c35f38556',
				'codex/input-im'
			]
		])

		expect(readableAffinityTarget(log, knownTargets)).toBe('codex/input-im')
	})

	test('uses request-time names when the terminal route is the binding target', () => {
		const log = requestLog({
			provider: { id: 'provider-id', name: 'codex' },
			channel: { id: 'channel-id', name: 'input-im' },
			affinity: { target: 'provider-id/channel-id' }
		})

		expect(readableAffinityTarget(log, new Map())).toBe('codex/input-im')
	})

	test('never exposes an unresolved internal binding id', () => {
		const log = requestLog({
			affinity: { target: 'provider-id/channel-id' }
		})

		expect(readableAffinityTarget(log, new Map())).toBeNull()
	})
})

describe('billing breakdown translations', () => {
	const canonicalValues: Array<[BillingValueDimension, string]> = [
		['usageClass', 'input_uncached'],
		['usageClass', 'cache_read'],
		['usageClass', 'cache_write_5m'],
		['usageClass', 'cache_write_1h'],
		['usageClass', 'output'],
		['usageClass', 'reasoning_output'],
		['usageClass', 'web_search'],
		['usageClass', 'file_search_tool_call'],
		['usageClass', 'x_search'],
		['usageClass', 'code_execution'],
		['usageClass', 'code_execution_duration'],
		['usageClass', 'code_interpreter_duration'],
		['unit', 'token'],
		['unit', 'call'],
		['unit', 'request'],
		['unit', 'billed_minute'],
		['modality', 'text'],
		['modality', 'image'],
		['modality', 'audio'],
		['modality', 'video'],
		['cacheTtl', '5m'],
		['cacheTtl', '1h'],
		['contextTier', 'default'],
		['contextTier', 'short'],
		['contextTier', 'long'],
		['serviceTier', 'default'],
		['serviceTier', 'standard'],
		['serviceTier', 'priority'],
		['serviceTier', 'flex'],
		['serviceTier', 'batch']
	]
	const staticKeys = [
		'billingUnitGeneric',
		'billingModality',
		'billingCacheTtl',
		'tps'
	] as const
	const locales = [en, zh, zhTw, ja]

	test('every canonical value resolves in every shipped locale', () => {
		for (const [dimension, value] of canonicalValues) {
			const key = billingValueTranslationKey(dimension, value)
			expect(key).not.toBeNull()
			const requestLogsKey = key?.replace('requestLogs.', '')
			for (const locale of locales) {
				expect(locale.requestLogs[requestLogsKey as keyof typeof locale.requestLogs]).toBeTruthy()
			}
		}
		for (const locale of locales) {
			for (const key of staticKeys) expect(locale.requestLogs[key]).toBeTruthy()
		}
	})

	test('preserves unknown custom profile values by returning no translation key', () => {
		expect(billingValueTranslationKey('usageClass', 'custom_gpu_second')).toBeNull()
	})
})

describe('request log controls translations', () => {
	const locales = [en, zh, zhTw, ja]

	test('ships automatic-update and batch-delete labels in every locale', () => {
		for (const locale of locales) {
			expect(locale.requestLogs.automaticUpdates).toBeTruthy()
			expect(locale.requestLogs.enableAutomaticUpdates).toBeTruthy()
			expect(locale.requestLogs.disableAutomaticUpdates).toBeTruthy()
			expect(locale.apiKeys.batchDelete).toBeTruthy()
		}
	})
})

describe('retry chain', () => {
	test('prefers channel name, then provider name, then ids', () => {
		expect(
			hopDisplayLabel({
				provider_id: 'p1',
				channel_id: 'c1',
				provider_name: 'Ciii',
				channel_name: 'ciii_1'
			})
		).toBe('ciii_1')
		expect(
			hopDisplayLabel({
				provider_id: 'p1',
				channel_id: 'c1',
				provider_name: 'Ciii'
			})
		).toBe('Ciii')
		expect(hopDisplayLabel({ provider_id: 'p1', channel_id: 'c1' })).toBe('c1')
	})

	test('shows ciii then beikun for the production error payload', () => {
		const labels = compactRetryChainLabels(
			requestLog({
				status: 'error',
				provider: { id: '2iy0koao', name: 'beikun' },
				channel: { id: 'mono_ch_e63df88ce165421aad3299f4e2a2816a', name: 'beikun' },
				tried_providers: [
					{
						attempt_number: 1,
						provider_id: 'mlprglfg',
						channel_id: 'mono_ch_2c91e8fcd4b34cefa0264792b107ebe8',
						provider_name: 'Ciii',
						channel_name: 'ciii_1',
						error: 'upstream status 502',
						upstream_status: 502,
						duration_ms: 98000
					},
					{
						attempt_number: 2,
						provider_id: '2iy0koao',
						channel_id: 'mono_ch_e63df88ce165421aad3299f4e2a2816a',
						provider_name: 'beikun',
						channel_name: 'beikun',
						error: 'upstream status 429',
						upstream_status: 429,
						duration_ms: 31000
					}
				]
			})
		)
		expect(labels).toEqual(['ciii_1', 'beikun'])
		expect(formatRetryChain(labels ?? [])).toBe('ciii_1 → beikun')
	})

	test('builds a compact unique hop chain including the terminal channel', () => {
		const labels = compactRetryChainLabels(
			requestLog({
				status: 'success',
				provider: { id: 'input', name: 'Input' },
				channel: { id: 'input-1', name: 'Input1' },
				tried_providers: [
					{
						provider_id: 'ciii',
						channel_id: 'ciii-2',
						provider_name: 'Ciii',
						channel_name: 'ciii_2',
						error: '429'
					},
					{
						provider_id: 'ciii',
						channel_id: 'ciii-2',
						provider_name: 'Ciii',
						channel_name: 'ciii_2',
						error: '429 again'
					}
				]
			})
		)
		expect(labels).toEqual(['ciii_2', 'Input1'])
		expect(formatRetryChain(labels ?? [])).toBe('ciii_2 → Input1')
	})

	test('does not render a chain for a single hop', () => {
		expect(
			compactRetryChainLabels(
				requestLog({
					status: 'success',
					provider: { id: 'cf', name: 'CloudFlare' },
					channel: { id: 'cf-1', name: 'CloudFlare' },
					tried_providers: []
				})
			)
		).toBeNull()
	})

	test('appends a served terminal hop after failed attempts on success', () => {
		const rows = retryAttemptRows(
			requestLog({
				status: 'success',
				provider: { id: 'input', name: 'Input' },
				channel: { id: 'input-1', name: 'Input1' },
				tried_providers: [
					{
						provider_id: 'ciii',
						channel_id: 'ciii-1',
						provider_name: 'Ciii',
						channel_name: 'ciii_1',
						error: 'upstream status 429',
						upstream_status: 429
					}
				]
			})
		)
		expect(rows).toEqual([
			{
				label: 'ciii_1',
				error: 'upstream status 429',
				upstreamStatus: 429,
				durationMs: null,
				outcome: 'failed'
			},
			{
				label: 'Input1',
				error: null,
				upstreamStatus: null,
				durationMs: null,
				outcome: 'served'
			}
		])
	})

	test('does not duplicate the terminal hop on a failed last attempt', () => {
		const rows = retryAttemptRows(
			requestLog({
				status: 'error',
				provider: { id: 'input', name: 'Input' },
				channel: { id: 'input-1', name: 'Input1' },
				error: { message: 'upstream status 502', http_status: 502 },
				tried_providers: [
					{
						provider_id: 'input',
						channel_id: 'input-1',
						provider_name: 'Input',
						channel_name: 'Input1',
						error: 'upstream status 502',
						upstream_status: 502,
						duration_ms: 1214
					}
				]
			})
		)
		expect(rows).toEqual([
			{
				label: 'Input1',
				error: 'upstream status 502',
				upstreamStatus: 502,
				durationMs: 1214,
				outcome: 'failed'
			}
		])
	})

	test('ships retry-chain labels in every locale', () => {
		for (const locale of [en, zh, zhTw, ja]) {
			expect(locale.requestLogs.retryChain).toBeTruthy()
			expect(locale.requestLogs.retryHopServed).toBeTruthy()
			expect(locale.requestLogs.retryHopCount).toBeTruthy()
			expect(locale.requestLogs.stickySession).toBeTruthy()
		}
	})
})
