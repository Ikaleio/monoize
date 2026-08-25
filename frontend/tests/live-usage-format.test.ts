import { describe, expect, test } from 'bun:test'
import { formatCacheHitRate, planRemainingFraction } from '../src/lib/live-usage'
import en from '../src/locales/en.json'
import ja from '../src/locales/ja.json'
import zh from '../src/locales/zh.json'
import zhTw from '../src/locales/zh-TW.json'

describe('user-center menu locale keys (dashboard-ui-layout.spec.md DL3d/DL3e)', () => {
	const keys = [
		'balance',
		'plan',
		'grant',
		'nextReset',
		'remainingOfGrant',
		'liveUsage',
		'rpm',
		'tpm',
		'cacheHit',
		'liveUsageError'
	] as const

	test('every userMenu key resolves in every shipped locale', () => {
		for (const locale of [en, zh, zhTw, ja]) {
			for (const key of keys) {
				expect(locale.userMenu[key]).toBeTruthy()
			}
		}
	})
})

describe('cache hit rate formatting (user-live-usage.spec.md LU-11)', () => {
	test('null and undefined render as em dash, never 0%', () => {
		expect(formatCacheHitRate(null)).toBe('—')
		expect(formatCacheHitRate(undefined)).toBe('—')
		expect(formatCacheHitRate(Number.NaN)).toBe('—')
	})

	test('ratios render as percent with at most 1 fractional digit', () => {
		expect(formatCacheHitRate(0)).toBe('0%')
		expect(formatCacheHitRate(0.25)).toBe('25%')
		expect(formatCacheHitRate(0.423)).toBe('42.3%')
		expect(formatCacheHitRate(0.9999)).toBe('100%')
		expect(formatCacheHitRate(1)).toBe('100%')
	})

	test('rounding keeps one fractional digit and strips trailing .0', () => {
		expect(formatCacheHitRate(0.12345)).toBe('12.3%')
		expect(formatCacheHitRate(0.1299)).toBe('13%')
	})
})

describe('plan remaining fraction (dashboard-ui-layout.spec.md DL3d)', () => {
	test('clamps to [0, 1] with BigInt arithmetic', () => {
		expect(planRemainingFraction('500000000', '1000000000')).toBe(0.5)
		expect(planRemainingFraction('0', '1000000000')).toBe(0)
		expect(planRemainingFraction('-5', '1000000000')).toBe(0)
		expect(planRemainingFraction('2000000000', '1000000000')).toBe(1)
	})

	test('returns null when grant is missing, non-positive, or malformed', () => {
		expect(planRemainingFraction('100', '0')).toBeNull()
		expect(planRemainingFraction('100', null)).toBeNull()
		expect(planRemainingFraction(null, '100')).toBeNull()
		expect(planRemainingFraction('abc', '100')).toBeNull()
		expect(planRemainingFraction('100', '1.5')).toBeNull()
	})

	test('handles values beyond the f64 integer domain exactly', () => {
		expect(planRemainingFraction('92233720368547758080', '184467440737095516160')).toBe(0.5)
	})
})
