import { useId, useState } from 'react'
import { CalendarIcon } from 'lucide-react'
import {
	format,
	isSameDay,
	startOfDay,
	startOfMonth,
	subDays,
	subMonths,
	type Locale
} from 'date-fns'
import { enUS, ja, zhCN, zhTW } from 'date-fns/locale'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Calendar } from '@/components/ui/calendar'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { cn } from '@/lib/utils'
import { RELATIVE_TIME_RANGES, type RelativeTimeRange, type TimeRangeValue } from './time-range'

type FixedPreset = 'today' | 'yesterday' | 'this_month' | 'last_month'
type TimeRangePreset = RelativeTimeRange | FixedPreset

interface DateRangePickerProps {
	value: TimeRangeValue
	onChange: (next: TimeRangeValue) => void
	language: string
	t: (key: string) => string
}

const PRESET_LABEL_KEYS: Record<TimeRangePreset, string> = {
	'1h': 'requestLogs.timeRange1h',
	'24h': 'requestLogs.timeRange24h',
	'7d': 'requestLogs.timeRange7d',
	'30d': 'requestLogs.timeRange30d',
	today: 'requestLogs.timeRangeToday',
	yesterday: 'requestLogs.timeRangeYesterday',
	this_month: 'requestLogs.timeRangeThisMonth',
	last_month: 'requestLogs.timeRangeLastMonth'
}

const FIXED_PRESETS: FixedPreset[] = ['today', 'yesterday', 'this_month', 'last_month']

function calendarLocale(language: string): Locale {
	if (language === 'zh-TW') return zhTW
	if (language.startsWith('zh')) return zhCN
	if (language.startsWith('ja')) return ja
	return enUS
}

function applyFixedPreset(preset: FixedPreset): { from: Date; to?: Date } {
	const now = new Date()
	switch (preset) {
		case 'today':
			return { from: startOfDay(now) }
		case 'yesterday':
			return { from: startOfDay(subDays(now, 1)), to: startOfDay(now) }
		case 'this_month':
			return { from: startOfMonth(now) }
		case 'last_month':
			return { from: startOfMonth(subMonths(now, 1)), to: startOfMonth(now) }
	}
}

const DATETIME_INPUT =
	/^(\d{4})-(\d{2})-(\d{2})(?:\s+(\d{2}):(\d{2})(?::(\d{2}))?)?$/

/**
 * Parses `yyyy-MM-dd`, `yyyy-MM-dd HH:mm` or `yyyy-MM-dd HH:mm:ss` in local
 * time. Returns `null` for invalid text (including impossible calendar dates)
 * and `undefined` for empty text.
 */
function parseDatetimeInput(input: string, endOfDay = false): Date | null | undefined {
	const s = input.trim()
	if (!s) return undefined
	const match = DATETIME_INPUT.exec(s)
	if (!match) return null
	const [y, m, d] = [Number(match[1]), Number(match[2]), Number(match[3])]
	const hasTime = match[4] != null
	const h = hasTime ? Number(match[4]) : endOfDay ? 23 : 0
	const mi = hasTime ? Number(match[5]) : endOfDay ? 59 : 0
	const sec = match[6] != null ? Number(match[6]) : !hasTime && endOfDay ? 59 : 0
	const ms = !hasTime && endOfDay ? 999 : 0
	const result = new Date(y, m - 1, d, h, mi, sec, ms)
	const roundTrips =
		result.getFullYear() === y &&
		result.getMonth() === m - 1 &&
		result.getDate() === d &&
		result.getHours() === h &&
		result.getMinutes() === mi &&
		result.getSeconds() === sec
	return roundTrips ? result : null
}

function detectFixedPreset(from: Date | undefined, to: Date | undefined): FixedPreset | null {
	if (!from) return null
	const close = (a: Date, b: Date) => Math.abs(a.getTime() - b.getTime()) < 1000
	const now = new Date()
	if (!to && close(from, startOfDay(now))) return 'today'
	if (!to && close(from, startOfMonth(now))) return 'this_month'
	if (to && close(from, startOfDay(subDays(now, 1))) && close(to, startOfDay(now))) {
		return 'yesterday'
	}
	if (to && close(from, startOfMonth(subMonths(now, 1))) && close(to, startOfMonth(now))) {
		return 'last_month'
	}
	return null
}

const formatInput = (date: Date | undefined) => (date ? format(date, 'yyyy-MM-dd HH:mm:ss') : '')

function formatCustomRangeLabel(from: Date | undefined, to: Date | undefined): string | null {
	if (from && to) {
		return isSameDay(from, to) ?
				`${format(from, 'MM/dd HH:mm')}–${format(to, 'HH:mm')}`
			:	`${format(from, 'MM/dd HH:mm')} – ${format(to, 'MM/dd HH:mm')}`
	}
	if (from) return `${format(from, 'MM/dd HH:mm')} –`
	if (to) return `– ${format(to, 'MM/dd HH:mm')}`
	return null
}

export function DateRangePicker({ value, onChange, language, t }: DateRangePickerProps) {
	const { range, from, to } = value
	const [open, setOpen] = useState(false)
	const [fromText, setFromText] = useState('')
	const [toText, setToText] = useState('')
	const [inputError, setInputError] = useState<'from' | 'to' | 'order' | null>(null)
	const fromId = useId()
	const toId = useId()
	const errorId = useId()

	const activePreset: TimeRangePreset | null = range ?? detectFixedPreset(from, to)
	const isAllTime = !range && !from && !to

	const syncDrafts = (nextFrom: Date | undefined, nextTo: Date | undefined) => {
		setFromText(formatInput(nextFrom))
		setToText(formatInput(nextTo))
		setInputError(null)
	}

	const handleOpenChange = (next: boolean) => {
		if (next) syncDrafts(from, to)
		setOpen(next)
	}

	const handleRelative = (preset: RelativeTimeRange) => {
		onChange({ range: preset })
		setOpen(false)
	}

	const handleFixed = (preset: FixedPreset) => {
		onChange(applyFixedPreset(preset))
		setOpen(false)
	}

	const handleCalendarSelect = (selected: { from?: Date; to?: Date } | undefined) => {
		if (!selected?.from) {
			syncDrafts(undefined, undefined)
			onChange({})
			return
		}
		const adjustedTo =
			selected.to ?
				new Date(
					selected.to.getFullYear(),
					selected.to.getMonth(),
					selected.to.getDate(),
					23,
					59,
					59,
					999
				)
			:	undefined
		syncDrafts(selected.from, adjustedTo)
		onChange({ from: selected.from, to: adjustedTo })
	}

	const handleClear = () => {
		onChange({})
		setOpen(false)
	}

	// LOGS-4: manual input commits only on Enter or Apply. Invalid text keeps
	// the previous filter and shows an inline error instead of resetting it.
	const commitDateInputs = () => {
		const nextFrom = parseDatetimeInput(fromText)
		const nextTo = parseDatetimeInput(toText, true)
		if (nextFrom === null) {
			setInputError('from')
			return
		}
		if (nextTo === null) {
			setInputError('to')
			return
		}
		if (nextFrom && nextTo && nextFrom >= nextTo) {
			setInputError('order')
			return
		}
		setInputError(null)
		onChange({ from: nextFrom, to: nextTo })
	}

	const label =
		activePreset ? t(PRESET_LABEL_KEYS[activePreset])
		: (formatCustomRangeLabel(from, to) ?? t('requestLogs.timeRangeAll'))

	const presetButtonClass = (active: boolean) =>
		cn('shrink-0 text-xs h-7 px-2', active && 'bg-foreground text-background hover:bg-foreground/90 hover:text-background')

	const errorMessage =
		inputError === 'order' ? t('requestLogs.timeRangeOrderError')
		: inputError ? t('requestLogs.timeRangeFormatError')
		: null

	return (
		<Popover open={open} onOpenChange={handleOpenChange}>
			<PopoverTrigger asChild>
				<Button
					variant='outline'
					className={cn(
						'h-9 justify-start text-left font-normal gap-2 min-w-[140px]',
						isAllTime && 'text-muted-foreground'
					)}
				>
					<CalendarIcon aria-hidden='true' />
					<span className='truncate text-xs'>{label}</span>
				</Button>
			</PopoverTrigger>
			<PopoverContent className='w-auto min-w-[16rem] p-0' align='start' side='bottom'>
				<div>
					<div className='[contain:inline-size] overflow-x-auto border-b [scrollbar-gutter:stable]'>
						<div className='flex gap-1 p-2 w-max min-w-full'>
							<Button
								variant='ghost'
								size='sm'
								aria-pressed={isAllTime}
								className={presetButtonClass(isAllTime)}
								onClick={handleClear}
							>
								{t('requestLogs.timeRangeAll')}
							</Button>
							{RELATIVE_TIME_RANGES.map(preset => (
								<Button
									key={preset}
									variant='ghost'
									size='sm'
									aria-pressed={activePreset === preset}
									className={presetButtonClass(activePreset === preset)}
									onClick={() => handleRelative(preset)}
								>
									{t(PRESET_LABEL_KEYS[preset])}
								</Button>
							))}
							{FIXED_PRESETS.map(preset => (
								<Button
									key={preset}
									variant='ghost'
									size='sm'
									aria-pressed={activePreset === preset}
									className={presetButtonClass(activePreset === preset)}
									onClick={() => handleFixed(preset)}
								>
									{t(PRESET_LABEL_KEYS[preset])}
								</Button>
							))}
						</div>
					</div>
					<div className='p-2 space-y-2'>
						<form
							className='flex flex-col gap-1.5 px-1'
							onSubmit={e => {
								e.preventDefault()
								commitDateInputs()
							}}
						>
							<label htmlFor={fromId} className='sr-only'>
								{t('requestLogs.timeRangeFrom')}
							</label>
							<Input
								id={fromId}
								className='h-7 text-xs md:text-xs font-mono w-full'
								placeholder={t('requestLogs.timeRangeFrom')}
								value={fromText}
								aria-invalid={inputError === 'from' || inputError === 'order'}
								aria-describedby={errorMessage ? errorId : undefined}
								onChange={e => {
									setFromText(e.target.value)
									setInputError(null)
								}}
							/>
							<label htmlFor={toId} className='sr-only'>
								{t('requestLogs.timeRangeTo')}
							</label>
							<Input
								id={toId}
								className='h-7 text-xs md:text-xs font-mono w-full'
								placeholder={t('requestLogs.timeRangeTo')}
								value={toText}
								aria-invalid={inputError === 'to' || inputError === 'order'}
								aria-describedby={errorMessage ? errorId : undefined}
								onChange={e => {
									setToText(e.target.value)
									setInputError(null)
								}}
							/>
							{errorMessage && (
								<p id={errorId} role='alert' className='text-xs text-destructive'>
									{errorMessage}
								</p>
							)}
							<Button type='submit' size='sm' variant='secondary' className='h-7 self-end text-xs'>
								{t('requestLogs.timeRangeApply')}
							</Button>
						</form>
						<Calendar
							mode='range'
							selected={from ? { from, to } : undefined}
							onSelect={handleCalendarSelect}
							numberOfMonths={1}
							disabled={{ after: new Date() }}
							locale={calendarLocale(language)}
						/>
					</div>
				</div>
			</PopoverContent>
		</Popover>
	)
}
