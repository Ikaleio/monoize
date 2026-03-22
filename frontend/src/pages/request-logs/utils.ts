import type { RequestLog } from '@/lib/api'

type TimingValue = number | string | null | undefined

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
