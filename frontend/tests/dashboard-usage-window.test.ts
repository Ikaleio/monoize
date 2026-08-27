import { describe, expect, test } from 'bun:test'
import type { DashboardAnalytics, RequestLog } from '../src/lib/api'
import en from '../src/locales/en.json'
import ja from '../src/locales/ja.json'
import zh from '../src/locales/zh.json'
import zhTw from '../src/locales/zh-TW.json'
import {
	bucketLabelForWindow,
	bucketStartDates,
	DEFAULT_USAGE_WINDOW,
	USAGE_WINDOW_QUERY,
	USAGE_WINDOWS,
	usageWindowStartIso,
	usesTodayMarker
} from '../src/lib/usage-window'
import {
	aggregateRecentUsage,
	buildCumulativeTokenSeries
} from '../src/pages/dashboard/utils'

describe('usage window mapping (DH-6a)', () => {
	test('exposes exactly four windows in order with 1h default', () => {
		expect(USAGE_WINDOWS).toEqual(['1h', '24h', '7d', '30d'])
		expect(DEFAULT_USAGE_WINDOW).toBe('1h')
	})

	test('window → {buckets, range_hours} matches the spec table', () => {
		expect(USAGE_WINDOW_QUERY['1h']).toEqual({ rangeHours: 1, buckets: 12 })
		expect(USAGE_WINDOW_QUERY['24h']).toEqual({ rangeHours: 24, buckets: 24 })
		expect(USAGE_WINDOW_QUERY['7d']).toEqual({ rangeHours: 168, buckets: 7 })
		expect(USAGE_WINDOW_QUERY['30d']).toEqual({ rangeHours: 720, buckets: 30 })
	})

	test('usageWindowStartIso subtracts range_hours from now (DH-7a time_from)', () => {
		const now = new Date('2026-08-27T10:00:00.000Z')
		expect(usageWindowStartIso('1h', now)).toBe('2026-08-27T09:00:00.000Z')
		expect(usageWindowStartIso('24h', now)).toBe('2026-08-26T10:00:00.000Z')
		expect(usageWindowStartIso('7d', now)).toBe('2026-08-20T10:00:00.000Z')
		expect(usageWindowStartIso('30d', now)).toBe('2026-07-28T10:00:00.000Z')
	})

	test('7d/30d mark Today; 1h/24h mark Now (DH-6d)', () => {
		expect(usesTodayMarker('1h')).toBe(false)
		expect(usesTodayMarker('24h')).toBe(false)
		expect(usesTodayMarker('7d')).toBe(true)
		expect(usesTodayMarker('30d')).toBe(true)
	})
})

describe('bucketStartDates (DH-6c)', () => {
	test('splits the range into equal-width bucket starts', () => {
		const starts = bucketStartDates(
			'2026-08-27T09:00:00.000Z',
			'2026-08-27T10:00:00.000Z',
			12
		)
		expect(starts).not.toBeNull()
		expect(starts).toHaveLength(12)
		expect(starts![0].toISOString()).toBe('2026-08-27T09:00:00.000Z')
		expect(starts![1].toISOString()).toBe('2026-08-27T09:05:00.000Z')
		expect(starts![11].toISOString()).toBe('2026-08-27T09:55:00.000Z')
	})

	test('returns null for unparsable or degenerate ranges', () => {
		expect(bucketStartDates('not-a-date', '2026-08-27T10:00:00.000Z', 12)).toBeNull()
		expect(
			bucketStartDates('2026-08-27T10:00:00.000Z', '2026-08-27T10:00:00.000Z', 12)
		).toBeNull()
		expect(
			bucketStartDates('2026-08-27T09:00:00.000Z', '2026-08-27T10:00:00.000Z', 0)
		).toBeNull()
	})
})

describe('bucketLabelForWindow (DH-6c)', () => {
	// Local-time constructor keeps assertions independent of the host TZ.
	const localDate = new Date(2026, 0, 27, 9, 5, 0)

	test('1h and 24h use zero-padded HH:mm clock time', () => {
		expect(bucketLabelForWindow('1h', localDate)).toBe('09:05')
		expect(bucketLabelForWindow('24h', localDate)).toBe('09:05')
	})

	test('7d and 30d use short month + day', () => {
		expect(bucketLabelForWindow('7d', localDate)).toBe('Jan 27')
		expect(bucketLabelForWindow('30d', localDate)).toBe('Jan 27')
	})
})

function analyticsFixture(
	overrides: Partial<DashboardAnalytics> = {}
): DashboardAnalytics {
	return {
		buckets: [],
		time_from: '2026-08-27T09:00:00.000Z',
		time_to: '2026-08-27T10:00:00.000Z',
		total_cost_nano_usd: '0',
		total_calls: 0,
		today_cost_nano_usd: '0',
		today_calls: 0,
		...overrides
	}
}

function bucket(tokensByModel: Record<string, number>, label = '08-27 09:00') {
	return {
		label,
		cost_by_model: {},
		calls_by_model: {},
		tokens_by_model: tokensByModel,
		calls_by_provider: {}
	}
}

describe('buildCumulativeTokenSeries', () => {
	test('1h window produces distinct 5-minute clock labels from the time range', () => {
		const analytics = analyticsFixture({
			buckets: Array.from({ length: 12 }, () => bucket({ 'model-a': 10 }))
		})
		const series = buildCumulativeTokenSeries(analytics, '1h')
		expect(series.rows).toHaveLength(12)
		const labels = series.rows.map(row => row.label)
		expect(new Set(labels).size).toBe(12)
		for (const label of labels) {
			expect(String(label)).toMatch(/^\d{2}:\d{2}$/)
		}
	})

	test('7d window produces short-date labels', () => {
		const analytics = analyticsFixture({
			time_from: '2026-08-20T10:00:00.000Z',
			time_to: '2026-08-27T10:00:00.000Z',
			buckets: Array.from({ length: 7 }, () => bucket({ 'model-a': 1 }))
		})
		const series = buildCumulativeTokenSeries(analytics, '7d')
		expect(series.rows).toHaveLength(7)
		for (const row of series.rows) {
			expect(String(row.label)).toMatch(/^[A-Z][a-z]{2} \d{1,2}$/)
		}
	})

	test('falls back to the backend label when the range does not parse', () => {
		const analytics = analyticsFixture({
			time_from: 'bogus',
			buckets: [bucket({ 'model-a': 5 }, '01-27 09:00')]
		})
		const series = buildCumulativeTokenSeries(analytics, '1h')
		expect(series.rows[0].label).toBe('Jan 27')
	})

	test('accumulates per-model values and omits zero-token models', () => {
		const analytics = analyticsFixture({
			buckets: [
				bucket({ 'model-a': 10, 'model-b': 5, 'model-zero': 0 }),
				bucket({ 'model-a': 20 }),
				bucket({ 'model-b': 15 })
			]
		})
		const series = buildCumulativeTokenSeries(analytics, '24h')
		expect(series.models).toEqual(['model-a', 'model-b'])
		expect(series.rows.map(row => row['model-a'])).toEqual([10, 30, 30])
		expect(series.rows.map(row => row['model-b'])).toEqual([5, 5, 20])
		expect(series.bucketTotals).toEqual([15, 20, 15])
		expect(series.cumulativeTotals).toEqual([15, 35, 50])
		expect(series.bucketByModel[1]).toEqual({ 'model-a': 20, 'model-b': 0 })
	})

	test('undefined analytics produces an empty series', () => {
		const series = buildCumulativeTokenSeries(undefined, '1h')
		expect(series.models).toEqual([])
		expect(series.rows).toEqual([])
	})
})

function requestLog(overrides: Partial<RequestLog> = {}): RequestLog {
	return {
		id: 'log-1',
		created_at: '2026-08-27T09:30:00.000Z',
		status: 'success',
		is_stream: false,
		model: 'model-a',
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

describe('aggregateRecentUsage (DH-7a)', () => {
	test('sums token fields, computes cache hit rate, and sorts by tokens desc', () => {
		const rows = aggregateRecentUsage([
			requestLog({
				model: 'model-a',
				tokens: { input: 100, output: 50, cache_read: 25 },
				billing: { charge_nano_usd: '1000000000' }
			}),
			requestLog({
				id: 'log-2',
				model: 'model-b',
				tokens: { input: 1000, output: 500 },
				billing: { charge_nano_usd: '2000000000' }
			})
		])
		expect(rows.map(row => row.model)).toEqual(['model-b', 'model-a'])
		expect(rows[1].tokens).toBe(175)
		expect(rows[1].cacheHitRate).toBeCloseTo(0.25)
		expect(rows[1].chargeNano).toBe(1000000000n)
	})

	test('omits models with zero tokens and zero charge', () => {
		const rows = aggregateRecentUsage([
			requestLog({ model: 'model-empty', tokens: {}, billing: {} }),
			requestLog({ id: 'log-2', model: 'model-a', tokens: { input: 1 } })
		])
		expect(rows.map(row => row.model)).toEqual(['model-a'])
	})
})

describe('dashboard usage locale keys (DH-16)', () => {
	type LocaleUsage = { dashboard: { usage: Record<string, unknown> } }
	const locales: LocaleUsage[] = [en, zh, zhTw, ja]

	test('all locales define the time-window and period keys', () => {
		for (const locale of locales) {
			const usage = locale.dashboard.usage
			expect(typeof usage.timeRange).toBe('string')
			expect(typeof usage.periodBreakdown).toBe('string')
			expect(typeof usage.periodTotal).toBe('string')
			expect(typeof usage.today).toBe('string')
			expect(typeof usage.now).toBe('string')
		}
	})

	test('no locale keeps the removed Group By keys', () => {
		for (const locale of locales) {
			const usage = locale.dashboard.usage
			expect(usage.groupBy).toBeUndefined()
			expect(usage.groupByModel).toBeUndefined()
			expect(usage.dailyBreakdown).toBeUndefined()
			expect(usage.dailyTotal).toBeUndefined()
		}
	})
})
