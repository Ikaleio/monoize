/**
 * Formats a cache-hit ratio for the user-center dropdown
 * (user-live-usage.spec.md LU-11).
 *
 * @param rate ratio in [0, 1], or null/undefined when the window had no
 *   input tokens.
 * @returns percentage text with at most 1 fractional digit (trailing `.0`
 *   removed), or an em dash for a null/undefined/non-finite rate. A null
 *   rate means "no basis", which must not render as `0%`.
 */
export function formatCacheHitRate(rate: number | null | undefined): string {
	if (rate == null || !Number.isFinite(rate)) return '—'
	const percent = Math.round(rate * 1000) / 10
	const text = percent.toFixed(1).replace(/\.0$/, '')
	return `${text}%`
}

/**
 * Fraction of the plan grant that remains, for the dropdown progress bar
 * (dashboard-ui-layout.spec.md DL3d).
 *
 * @param balanceNano canonical integer string, remaining balance in nano-USD.
 * @param grantNano canonical integer string, plan grant amount in nano-USD.
 * @returns `clamp(balance / grant, 0, 1)` computed with BigInt arithmetic,
 *   or null when either input is not a canonical integer string or the
 *   grant is not greater than zero (progress bar is omitted).
 */
export function planRemainingFraction(
	balanceNano: string | null | undefined,
	grantNano: string | null | undefined
): number | null {
	if (balanceNano == null || grantNano == null) return null
	if (!/^-?\d+$/.test(balanceNano) || !/^-?\d+$/.test(grantNano)) return null
	const grant = BigInt(grantNano)
	if (grant <= 0n) return null
	const balance = BigInt(balanceNano)
	if (balance <= 0n) return 0
	if (balance >= grant) return 1
	// Scale to 4 decimal digits before the Number conversion so the
	// fraction stays exact for the bar-width use case.
	return Number((balance * 10000n) / grant) / 10000
}
