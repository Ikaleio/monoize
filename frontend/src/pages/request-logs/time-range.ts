import { subDays, subHours } from 'date-fns'

export const RELATIVE_TIME_RANGES = ['1h', '24h', '7d', '30d'] as const
export type RelativeTimeRange = (typeof RELATIVE_TIME_RANGES)[number]

/** `range` wins over `from`/`to`; the page recomputes `from` for a relative range. */
export interface TimeRangeValue {
	range?: RelativeTimeRange
	from?: Date
	to?: Date
}

export function relativeRangeStart(range: RelativeTimeRange): Date {
	const now = new Date()
	switch (range) {
		case '1h':
			return subHours(now, 1)
		case '24h':
			return subHours(now, 24)
		case '7d':
			return subDays(now, 7)
		case '30d':
			return subDays(now, 30)
	}
}
