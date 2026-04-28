import { useMemo, useRef, useState } from 'react'
import { CalendarIcon } from 'lucide-react'
import { format, startOfDay, startOfMonth, subDays, subHours, subMonths } from 'date-fns'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Calendar } from '@/components/ui/calendar'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { cn } from '@/lib/utils'

type TimeRangePreset =
	| '1h'
	| '24h'
	| '7d'
	| '30d'
	| 'today'
	| 'yesterday'
	| 'this_month'
	| 'last_month'

interface DateRangePickerProps {
	from: Date | undefined
	to: Date | undefined
	onChange: (from: Date | undefined, to: Date | undefined) => void
	t: (key: string) => string
}

function applyPreset(preset: TimeRangePreset): { from: Date; to?: Date } {
	const now = new Date()
	switch (preset) {
		case '1h':
			return { from: subHours(now, 1) }
		case '24h':
			return { from: subHours(now, 24) }
		case '7d':
			return { from: subDays(now, 7) }
		case '30d':
			return { from: subDays(now, 30) }
		case 'today':
			return { from: startOfDay(now) }
		case 'yesterday': {
			const yesterday = subDays(now, 1)
			return { from: startOfDay(yesterday), to: startOfDay(now) }
		}
		case 'this_month':
			return { from: startOfMonth(now) }
		case 'last_month': {
			const lastMonth = subMonths(now, 1)
			return { from: startOfMonth(lastMonth), to: startOfMonth(now) }
		}
	}
}

function parseDatetimeInput(input: string, endOfDay = false): Date | undefined {
	const s = input.trim()
	if (!s) return undefined
	const dateOnly = /^\d{4}-\d{2}-\d{2}$/.test(s)
	const dateTime = /^\d{4}-\d{2}-\d{2}\s+\d{2}:\d{2}:\d{2}$/.test(s)
	if (!dateOnly && !dateTime) return undefined
	if (dateOnly) {
		const [y, m, d] = s.split('-').map(Number)
		if (endOfDay) return new Date(y, m - 1, d, 23, 59, 59, 999)
		return new Date(y, m - 1, d, 0, 0, 0, 0)
	}
	const [datePart, timePart] = s.split(/\s+/)
	const [y, m, d] = datePart.split('-').map(Number)
	const [h, mi, sec] = timePart.split(':').map(Number)
	const result = new Date(y, m - 1, d, h, mi, sec, 0)
	return isNaN(result.getTime()) ? undefined : result
}

function detectFixedPreset(
	from: Date | undefined,
	to: Date | undefined
): TimeRangePreset | null {
	if (!from) return null
	const close = (a: Date, b: Date) => Math.abs(a.getTime() - b.getTime()) < 1000
	const now = new Date()
	if (!to && close(from, startOfDay(now))) return 'today'
	if (!to && close(from, startOfMonth(now))) return 'this_month'
	if (to && close(from, startOfDay(subDays(now, 1))) && close(to, startOfDay(now))) {
		return 'yesterday'
	}
	if (
		to &&
		close(from, startOfMonth(subMonths(now, 1))) &&
		close(to, startOfMonth(now))
	) {
		return 'last_month'
	}
	return null
}

export function DateRangePicker({ from, to, onChange, t }: DateRangePickerProps) {
	const [open, setOpen] = useState(false)
	const [activePreset, setActivePreset] = useState<TimeRangePreset | null>(null)
	const fromInputRef = useRef<HTMLInputElement | null>(null)
	const toInputRef = useRef<HTMLInputElement | null>(null)
	const formattedFromInput = from ? format(from, 'yyyy-MM-dd HH:mm:ss') : ''
	const formattedToInput = to ? format(to, 'yyyy-MM-dd HH:mm:ss') : ''

	const handlePreset = (preset: TimeRangePreset) => {
		const range = applyPreset(preset)
		setActivePreset(preset)
		onChange(range.from, range.to)
		setOpen(false)
	}

	const handleCalendarSelect = (range: { from?: Date; to?: Date } | undefined) => {
		if (range?.from) {
			const adjustedTo =
				range.to ?
					new Date(
						range.to.getFullYear(),
						range.to.getMonth(),
						range.to.getDate(),
						23,
						59,
						59,
						999
					)
				: undefined
			const detected = detectFixedPreset(range.from, adjustedTo)
			setActivePreset(detected)
			onChange(range.from, adjustedTo)
		} else {
			setActivePreset(null)
			onChange(undefined, undefined)
		}
	}

	const handleClear = () => {
		setActivePreset(null)
		onChange(undefined, undefined)
		setOpen(false)
	}

	const commitDateInputs = () => {
		const validFrom = parseDatetimeInput(fromInputRef.current?.value ?? '')
		const validTo = parseDatetimeInput(toInputRef.current?.value ?? '', true)
		const detected = detectFixedPreset(validFrom, validTo)
		setActivePreset(detected)
		onChange(validFrom, validTo)
	}

	const label = useMemo(() => {
		if (activePreset) {
			const presetKeys: Record<TimeRangePreset, string> = {
				'1h': 'requestLogs.timeRange1h',
				'24h': 'requestLogs.timeRange24h',
				'7d': 'requestLogs.timeRange7d',
				'30d': 'requestLogs.timeRange30d',
				today: 'requestLogs.timeRangeToday',
				yesterday: 'requestLogs.timeRangeYesterday',
				this_month: 'requestLogs.timeRangeThisMonth',
				last_month: 'requestLogs.timeRangeLastMonth'
			}
			return t(presetKeys[activePreset])
		}
		if (!from) return t('requestLogs.timeRangeAll')
		if (!to) return `${format(from, 'MM/dd HH:mm')} –`
		return `${format(from, 'MM/dd')} – ${format(to, 'MM/dd')}`
	}, [from, to, activePreset, t])

	const presets: Array<{ key: TimeRangePreset; label: string }> = [
		{ key: '1h', label: t('requestLogs.timeRange1h') },
		{ key: '24h', label: t('requestLogs.timeRange24h') },
		{ key: '7d', label: t('requestLogs.timeRange7d') },
		{ key: '30d', label: t('requestLogs.timeRange30d') },
		{ key: 'today', label: t('requestLogs.timeRangeToday') },
		{ key: 'yesterday', label: t('requestLogs.timeRangeYesterday') },
		{ key: 'this_month', label: t('requestLogs.timeRangeThisMonth') },
		{ key: 'last_month', label: t('requestLogs.timeRangeLastMonth') }
	]

	const isAllTime = !from && !to

	return (
		<Popover open={open} onOpenChange={setOpen}>
			<PopoverTrigger asChild>
				<Button
					variant='outline'
					className={cn(
						'h-9 justify-start text-left font-normal gap-2 min-w-[140px]',
						isAllTime && 'text-muted-foreground'
					)}
				>
					<CalendarIcon className='h-4 w-4 shrink-0' />
					<span className='truncate text-xs'>{label}</span>
				</Button>
			</PopoverTrigger>
			<PopoverContent className='w-auto p-0' align='start' side='bottom'>
				<div>
					<div className='[contain:inline-size] overflow-x-auto border-b [scrollbar-gutter:stable]'>
						<div className='flex gap-1 p-2 w-max min-w-full'>
							<Button
								variant='ghost'
								size='sm'
								className={cn(
									'shrink-0 text-xs h-7 px-2',
									isAllTime && 'bg-primary text-primary-foreground'
								)}
								onClick={handleClear}
							>
								{t('requestLogs.timeRangeAll')}
							</Button>
							{presets.map(p => (
								<Button
									key={p.key}
									variant='ghost'
									size='sm'
									className={cn(
										'shrink-0 text-xs h-7 px-2',
										activePreset === p.key && 'bg-primary text-primary-foreground'
									)}
									onClick={() => handlePreset(p.key)}
								>
									{p.label}
								</Button>
							))}
						</div>
					</div>
					<div className='p-2 space-y-2'>
						<div className='flex flex-col gap-1.5 px-1'>
							<Input
								key={`from-${formattedFromInput}`}
								ref={fromInputRef}
								className='h-7 text-xs font-mono w-full'
								placeholder={t('requestLogs.timeRangeFrom')}
								defaultValue={formattedFromInput}
								onBlur={commitDateInputs}
								onKeyDown={e => {
									if (e.key === 'Enter') commitDateInputs()
								}}
							/>
							<Input
								key={`to-${formattedToInput}`}
								ref={toInputRef}
								className='h-7 text-xs font-mono w-full'
								placeholder={t('requestLogs.timeRangeTo')}
								defaultValue={formattedToInput}
								onBlur={commitDateInputs}
								onKeyDown={e => {
									if (e.key === 'Enter') commitDateInputs()
								}}
							/>
						</div>
						<Calendar
							mode='range'
							selected={from ? { from, to } : undefined}
							onSelect={handleCalendarSelect}
							numberOfMonths={1}
							disabled={{ after: new Date() }}
						/>
					</div>
				</div>
			</PopoverContent>
		</Popover>
	)
}
