const I128_MAX = 170141183460469231731687303715884105727n
const RUST_DECIMAL_MAX = 79228162514264337593543950335n

function groupInteger(digits: string): string {
	return digits.replace(/\B(?=(\d{3})+(?!\d))/g, ',')
}

function formatScaledInteger(value: bigint, fractionalDigits: number): string {
	const negative = value < 0n
	const absolute = negative ? -value : value
	const scale = 10n ** BigInt(fractionalDigits)
	const whole = absolute / scale
	const fraction = fractionalDigits > 0 ? `.${(absolute % scale).toString().padStart(fractionalDigits, '0')}` : ''
	return `${negative ? '-' : ''}${groupInteger(whole.toString())}${fraction}`
}

export function isCanonicalIntegerString(value: string): boolean {
	return /^(?:0|[1-9]\d*)$/.test(value) && BigInt(value) <= I128_MAX
}

export function isSignedIntegerString(value: string): boolean {
	return /^-?(?:0|[1-9]\d*)$/.test(value)
}

export function usdPerMillionToNanoPerToken(value: string): string | null {
	const trimmed = value.trim()
	if (!/^\d+(?:\.\d*)?$/.test(trimmed)) return null
	const [wholeRaw, fractionRaw = ''] = trimmed.split('.')
	const whole = BigInt(wholeRaw)
	const thousandths = BigInt(fractionRaw.slice(0, 3).padEnd(3, '0'))
	const nano = whole * 1000n + thousandths
	return nano <= I128_MAX ? nano.toString() : null
}

export function nanoPerTokenToUsdPerMillion(value?: string | null): string | null {
	if (value == null || !isCanonicalIntegerString(value)) return null
	const nano = BigInt(value)
	const whole = nano / 1000n
	const fraction = (nano % 1000n).toString().padStart(3, '0').replace(/0+$/, '')
	return fraction ? `${whole}.${fraction}` : whole.toString()
}

export function formatNanoPerTokenPerMillion(value?: string | null): string {
	const decimal = nanoPerTokenToUsdPerMillion(value)
	return decimal == null ? '—' : `$${groupInteger(decimal.split('.')[0])}${decimal.includes('.') ? `.${decimal.split('.')[1]}` : ''}`
}

// CP-INV-3/MP-C12: multipliers accept any non-negative decimal with at most 9
// fractional digits; "0" is a valid explicit free configuration.
export function normalizeMultiplier(value: string): string | null {
	const trimmed = value.trim()
	if (!/^\d+(?:\.\d{0,9})?$/.test(trimmed)) return null
	const [wholeRaw, fractionRaw = ''] = trimmed.split('.')
	const whole = wholeRaw.replace(/^0+(?=\d)/, '')
	const fraction = fractionRaw.replace(/0+$/, '')
	if (BigInt(`${whole || '0'}${fraction}`) > RUST_DECIMAL_MAX) return null
	return fraction ? `${whole || '0'}.${fraction}` : whole || '0'
}

export function formatNanoUsd(value: string | bigint | null | undefined, fractionalDigits = 6): string {
	if (!Number.isInteger(fractionalDigits) || fractionalDigits < 0 || fractionalDigits > 9) return '$0.000000'
	const raw = typeof value === 'bigint' ? value.toString() : value
	if (raw == null || !isSignedIntegerString(raw)) {
		return `$${formatScaledInteger(0n, fractionalDigits)}`
	}
	const nano = BigInt(raw)
	const negative = nano < 0n
	const absolute = negative ? -nano : nano
	const divisor = 10n ** BigInt(9 - fractionalDigits)
	let rounded = absolute / divisor
	if ((absolute % divisor) * 2n >= divisor) rounded += 1n
	return `${negative ? '-' : ''}$${formatScaledInteger(rounded, fractionalDigits)}`
}

export function formatUsdDecimal(value: string | null | undefined, fractionalDigits = 2): string {
	const raw = value?.trim() ?? ''
	const match = /^(-?)(\d+)(?:\.(\d*))?$/.exec(raw)
	if (!match || fractionalDigits < 0 || fractionalDigits > 9) {
		return `$${formatScaledInteger(0n, fractionalDigits)}`
	}
	const negative = match[1] === '-'
	const whole = BigInt(match[2])
	const fraction = match[3] ?? ''
	const kept = fraction.slice(0, fractionalDigits).padEnd(fractionalDigits, '0')
	let scaled = whole * 10n ** BigInt(fractionalDigits) + BigInt(kept || '0')
	const nextDigit = fraction[fractionalDigits]
	if (nextDigit != null && nextDigit >= '5') scaled += 1n
	return `${negative && scaled !== 0n ? '-' : ''}$${formatScaledInteger(scaled, fractionalDigits)}`
}

export function nanoUsdToChartNumber(value: string | undefined): number {
	if (value == null || !isSignedIntegerString(value)) return 0
	return Number(BigInt(value)) / 1e9
}

export function isZeroIntegerString(value: string | null | undefined): boolean {
	return value != null && isSignedIntegerString(value) && BigInt(value) === 0n
}
