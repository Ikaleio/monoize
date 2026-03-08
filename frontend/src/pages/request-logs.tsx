import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { CalendarIcon, ChevronDown, Eye, EyeOff, RefreshCw, Search } from 'lucide-react'
import { TableVirtuoso } from 'react-virtuoso'
import { format, startOfDay, startOfMonth, subDays, subHours, subMonths } from 'date-fns'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Badge } from '@/components/ui/badge'
import { Skeleton } from '@/components/ui/skeleton'
import { Calendar } from '@/components/ui/calendar'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import {
	Select,
	SelectContent,
	SelectItem,
	SelectTrigger,
	SelectValue
} from '@/components/ui/select'
import {
	Tooltip,
	TooltipContent,
	TooltipProvider,
	TooltipTrigger
} from '@/components/ui/tooltip'
import { useRequestLogs, useApiKeys } from '@/lib/swr'
import { useRequestLogSSE } from '@/lib/sse'
import { useAuth } from '@/hooks/use-auth'
import { ModelBadge } from '@/components/ModelBadge'
import { cn } from '@/lib/utils'
import type { RequestLog, RequestLogsFilter } from '@/lib/api'
import { PageWrapper, motion, transitions } from '@/components/ui/motion'
import { AnimatePresence } from 'framer-motion'

const REQUEST_LOGS_PAGE_SIZE = 100

type TimeRangePreset = '1h' | '24h' | '7d' | '30d' | 'today' | 'yesterday' | 'this_month' | 'last_month'


type TimingValue = number | string | null | undefined
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

/** Parse datetime string: accepts `yyyy-MM-dd` or `yyyy-MM-dd HH:mm:ss`. */
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

/** Check if a range matches a fixed-time preset (tolerance: 1s). */
function detectFixedPreset(from: Date | undefined, to: Date | undefined): TimeRangePreset | null {
	if (!from) return null
	const close = (a: Date, b: Date) => Math.abs(a.getTime() - b.getTime()) < 1000
	const now = new Date()
	// today: from=startOfDay(now), no to
	if (!to && close(from, startOfDay(now))) return 'today'
	// this_month: from=startOfMonth(now), no to
	if (!to && close(from, startOfMonth(now))) return 'this_month'
	// yesterday: from=startOfDay(yesterday), to=startOfDay(now)
	if (to && close(from, startOfDay(subDays(now, 1))) && close(to, startOfDay(now))) return 'yesterday'
	// last_month: from=startOfMonth(lastMonth), to=startOfMonth(now)
	if (to && close(from, startOfMonth(subMonths(now, 1))) && close(to, startOfMonth(now))) return 'last_month'
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

function getDurationMs(log: RequestLog): number | null {
	return parseTimingMs(log.timing.duration_ms)
}

function getTtfbMs(log: RequestLog): number | null {
	return parseTimingMs(log.timing.ttfb_ms)
}

function DateRangePicker({
	from,
	to,
	onChange,
	t
}: {
	from: Date | undefined
	to: Date | undefined
	onChange: (from: Date | undefined, to: Date | undefined) => void
	t: (key: string) => string
}) {
	const [open, setOpen] = useState(false)
	const [activePreset, setActivePreset] = useState<TimeRangePreset | null>(null)
	const [fromInput, setFromInput] = useState('')
	const [toInput, setToInput] = useState('')
	useEffect(() => {
		setFromInput(from ? format(from, 'yyyy-MM-dd HH:mm:ss') : '')
		setToInput(to ? format(to, 'yyyy-MM-dd HH:mm:ss') : '')
	}, [from, to])

	const handlePreset = (preset: TimeRangePreset) => {
		const range = applyPreset(preset)
		setActivePreset(preset)
		onChange(range.from, range.to)
		setOpen(false)
	}

	const handleCalendarSelect = (range: { from?: Date; to?: Date } | undefined) => {
		if (range?.from) {
			const adjustedTo = range.to ? new Date(range.to.getFullYear(), range.to.getMonth(), range.to.getDate(), 23, 59, 59, 999) : undefined
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
		const validFrom = parseDatetimeInput(fromInput)
		const validTo = parseDatetimeInput(toInput, true)
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
								className='h-7 text-xs font-mono w-full'
								placeholder={t('requestLogs.timeRangeFrom')}
								value={fromInput}
								onChange={e => setFromInput(e.target.value)}
								onBlur={commitDateInputs}
								onKeyDown={e => { if (e.key === 'Enter') commitDateInputs() }}
							/>
							<Input
								className='h-7 text-xs font-mono w-full'
								placeholder={t('requestLogs.timeRangeTo')}
								value={toInput}
								onChange={e => setToInput(e.target.value)}
								onBlur={commitDateInputs}
								onKeyDown={e => { if (e.key === 'Enter') commitDateInputs() }}
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

type JsonObject = Record<string, unknown>

function asObject(value: unknown): JsonObject | null {
	if (value && typeof value === 'object' && !Array.isArray(value)) {
		return value as JsonObject
	}
	return null
}

function readNumber(value: unknown): number | null {
	if (typeof value === 'number' && Number.isFinite(value)) return value
	if (typeof value === 'string') {
		const parsed = Number(value)
		return Number.isFinite(parsed) ? parsed : null
	}
	return null
}

function readTokenCount(obj: JsonObject | null, key: string): number | null {
	if (!obj) return null
	return readNumber(obj[key])
}

function readNanoString(obj: JsonObject | null, key: string): string | null {
	if (!obj) return null
	const raw = obj[key]
	if (typeof raw === 'string' && raw.trim() !== '') return raw
	if (typeof raw === 'number' && Number.isFinite(raw)) return String(raw)
	return null
}

export function RequestLogsPage() {
	const { t } = useTranslation()
	const { user } = useAuth()
	const isAdmin = user?.role === 'super_admin' || user?.role === 'admin'

	const [searchInput, setSearchInput] = useState('')
	const [modelInput, setModelInput] = useState('')
	const [usernameInput, setUsernameInput] = useState(user?.username ?? '')
	const [filters, setFilters] = useState<RequestLogsFilter>(() => ({
		username: user?.username
	}))
	const [showIp, setShowIp] = useState(false)
	const [requestOffset, setRequestOffset] = useState(0)
	const [loadedLogs, setLoadedLogs] = useState<RequestLog[]>([])
	const [totalCount, setTotalCount] = useState(0)
	const [totalCharge, setTotalCharge] = useState<string>('0')
	const [timeFrom, setTimeFrom] = useState<Date | undefined>(undefined)
	const [timeTo, setTimeTo] = useState<Date | undefined>(undefined)
	const [filtersExpanded, setFiltersExpanded] = useState(true)
	const openTooltipIdsRef = useRef<Set<string>>(new Set())
	const pendingPageDataRef = useRef<import('@/lib/api').RequestLogsResponse | null>(null)
	const pendingNewestDataRef = useRef<import('@/lib/api').RequestLogsResponse | null>(null)
	const pendingSSERef = useRef<RequestLog[]>([])
	const [flushSignal, setFlushSignal] = useState(0)

	const onTooltipOpenChange = useCallback((tooltipId: string, open: boolean) => {
		if (open) {
			openTooltipIdsRef.current.add(tooltipId)
			return
		}

		openTooltipIdsRef.current.delete(tooltipId)
		if (openTooltipIdsRef.current.size === 0) {
			setFlushSignal(c => c + 1)
		}
	}, [])

	const { data: apiKeys } = useApiKeys()

	const activeFilters = useMemo<RequestLogsFilter>(() => {
		const f: RequestLogsFilter = {}
		if (filters.status) f.status = filters.status
		if (filters.model) f.model = filters.model
		if (filters.api_key_id) f.api_key_id = filters.api_key_id
		if (isAdmin && filters.username) f.username = filters.username
		if (searchInput.trim()) f.search = searchInput.trim()
		if (timeFrom) f.time_from = timeFrom.toISOString()
		if (timeTo) f.time_to = timeTo.toISOString()
		return f
	}, [filters, searchInput, isAdmin, timeFrom, timeTo])

	const filterKey = useMemo(
		() => JSON.stringify(activeFilters),
		[activeFilters]
	)

	useEffect(() => {
		setRequestOffset(0)
		setLoadedLogs([])
		setTotalCount(0)
		setTotalCharge('0')
	}, [filterKey])

	const {
		data: pageData,
		isLoading,
		isValidating,
		mutate
	} = useRequestLogs(REQUEST_LOGS_PAGE_SIZE, requestOffset, activeFilters)

	const matchesActiveFilters = useCallback((log: RequestLog) => {
		if (activeFilters.model) {
			const requestedModels = activeFilters.model
				.split(',')
				.map(part => part.trim().toLowerCase())
				.filter(Boolean)
			if (
				requestedModels.length > 0 &&
				!requestedModels.some(part => log.model.toLowerCase().includes(part))
			) {
				return false
			}
		}
		if (activeFilters.status && log.status !== activeFilters.status) return false
		if (activeFilters.api_key_id && log.api_key.id !== activeFilters.api_key_id) return false

		const userObj = asObject(log.user)
		const userName =
			(typeof userObj?.username === 'string' ? userObj.username : undefined) ||
			(typeof userObj?.name === 'string' ? userObj.name : undefined)
		if (activeFilters.username && userName !== activeFilters.username) return false

		if (activeFilters.time_from || activeFilters.time_to) {
			const createdAtMs = Date.parse(log.created_at)
			if (!Number.isFinite(createdAtMs)) return false
			if (activeFilters.time_from) {
				const fromMs = Date.parse(activeFilters.time_from)
				if (Number.isFinite(fromMs) && createdAtMs < fromMs) return false
			}
			if (activeFilters.time_to) {
				const toMs = Date.parse(activeFilters.time_to)
				if (Number.isFinite(toMs) && createdAtMs > toMs) return false
			}
		}

		if (activeFilters.search) {
			const q = activeFilters.search.toLowerCase()
			const providerObj = asObject(log.provider)
			const channelObj = asObject(log.channel)
			const apiKeyObj = asObject(log.api_key)
			const searchFields = [
				log.id,
				log.request_id,
				log.model,
				log.upstream_model,
				log.request_ip,
				log.status,
				typeof providerObj?.id === 'string' ? providerObj.id : undefined,
				typeof providerObj?.name === 'string' ? providerObj.name : undefined,
				typeof channelObj?.id === 'string' ? channelObj.id : undefined,
				typeof channelObj?.name === 'string' ? channelObj.name : undefined,
				typeof apiKeyObj?.id === 'string' ? apiKeyObj.id : undefined,
				typeof apiKeyObj?.name === 'string' ? apiKeyObj.name : undefined,
				userName
			]
			const matchesSearch = searchFields.some(value =>
				typeof value === 'string' && value.toLowerCase().includes(q)
			)
			if (!matchesSearch) return false
		}

		return true
	}, [activeFilters])

	const prependSSELogs = useCallback((logs: RequestLog[]) => {
		if (logs.length === 0) return

		setLoadedLogs(prev => {
			const next = [...prev]
			const existingIds = new Set(prev.map(log => log.id))
			const incomingIds = new Set<string>()
			const handledRequestIds = new Set<string>()

			for (const log of logs) {
				if (!matchesActiveFilters(log)) continue
				if (incomingIds.has(log.id)) continue
				incomingIds.add(log.id)

				if (log.request_id) {
					if (handledRequestIds.has(log.request_id)) {
						const duplicateIndex = next.findIndex(item => item.request_id === log.request_id)
						if (duplicateIndex >= 0) next[duplicateIndex] = log
						continue
					}
					handledRequestIds.add(log.request_id)
					const existingIndex = next.findIndex(item => item.request_id === log.request_id)
					if (existingIndex >= 0) {
						next.splice(existingIndex, 1)
						next.unshift(log)
						continue
					}
				}

				if (existingIds.has(log.id)) continue
				next.unshift(log)
			}

			return next
		})
	}, [matchesActiveFilters])

	const { connected: sseConnected, event: sseEvent } = useRequestLogSSE(true)

	const { data: newestPageData, mutate: mutateNewest } = useRequestLogs(
		REQUEST_LOGS_PAGE_SIZE,
		0,
		activeFilters,
		{
			refreshInterval: sseConnected ? 0 : 3000,
			isPaused: () => openTooltipIdsRef.current.size > 0
		}
	)

	useEffect(() => {
		if (!sseEvent) return

		if (sseEvent.type === 'resync') {
			void mutate()
			void mutateNewest()
			return
		}

		if (openTooltipIdsRef.current.size > 0) {
			pendingSSERef.current = [...pendingSSERef.current, ...sseEvent.logs]
			return
		}

		prependSSELogs(sseEvent.logs)
	}, [sseEvent, mutate, mutateNewest, prependSSELogs])

	const prevConnectedRef = useRef(false)
	useEffect(() => {
		if (sseConnected && !prevConnectedRef.current) {
			void mutate()
			void mutateNewest()
		}
		prevConnectedRef.current = sseConnected
	}, [sseConnected, mutate, mutateNewest])


	useEffect(() => {
		if (!pageData) return

		if (openTooltipIdsRef.current.size > 0) {
			pendingPageDataRef.current = pageData
			return
		}

		setTotalCount(pageData.total)
		setTotalCharge(pageData.total_charge_nano_usd)
		setLoadedLogs(prev => {
			if (requestOffset === 0) {
				return pageData.data
			}

			const existingIds = new Set(prev.map(log => log.id))
			const appended = pageData.data.filter(log => !existingIds.has(log.id))
			if (appended.length === 0) return prev
			return [...prev, ...appended]
		})
		pendingPageDataRef.current = null
	}, [pageData, requestOffset])

	useEffect(() => {
		if (!newestPageData) return

		if (openTooltipIdsRef.current.size > 0) {
			pendingNewestDataRef.current = newestPageData
			return
		}

		setTotalCount(newestPageData.total)
		setTotalCharge(newestPageData.total_charge_nano_usd)
		setLoadedLogs(prev => {
			if (prev.length === 0 || requestOffset === 0) {
				return newestPageData.data
			}

			const newestIds = new Set(newestPageData.data.map(log => log.id))
			const tail = prev.filter(log => !newestIds.has(log.id))
			const merged = [...newestPageData.data, ...tail]
			return merged.slice(0, newestPageData.total)
		})
		pendingNewestDataRef.current = null
	}, [newestPageData, requestOffset])

	// Flush buffered data when all tooltips close
	// eslint-disable-next-line react-hooks/exhaustive-deps
	useEffect(() => {
		if (openTooltipIdsRef.current.size > 0) return

		const bufferedPage = pendingPageDataRef.current
		const bufferedNewest = pendingNewestDataRef.current

		if (bufferedNewest) {
			pendingNewestDataRef.current = null
			setTotalCount(bufferedNewest.total)
			setTotalCharge(bufferedNewest.total_charge_nano_usd)
			setLoadedLogs(prev => {
				if (prev.length === 0 || requestOffset === 0) {
					return bufferedNewest.data
				}
				const newestIds = new Set(bufferedNewest.data.map(log => log.id))
				const tail = prev.filter(log => !newestIds.has(log.id))
				const merged = [...bufferedNewest.data, ...tail]
				return merged.slice(0, bufferedNewest.total)
			})
		}

		if (bufferedPage) {
			pendingPageDataRef.current = null
			setTotalCount(bufferedPage.total)
			setTotalCharge(bufferedPage.total_charge_nano_usd)
			setLoadedLogs(prev => {
				if (requestOffset === 0) {
					return bufferedPage.data
				}
				const existingIds = new Set(prev.map(log => log.id))
				const appended = bufferedPage.data.filter(log => !existingIds.has(log.id))
				if (appended.length === 0) return prev
				return [...prev, ...appended]
			})
		}

		if (pendingSSERef.current.length > 0) {
			const bufferedSSE = pendingSSERef.current
			pendingSSERef.current = []
			prependSSELogs(bufferedSSE)
		}
	}, [flushSignal, requestOffset])

	const isInitialLoading = isLoading && loadedLogs.length === 0
	const hasMore = loadedLogs.length < totalCount

	// Guarantee chronological order (newest first) regardless of SSE insertion order
	const sortedLogs = useMemo(() => {
		const sorted = [...loadedLogs]
		sorted.sort((a, b) => {
			const ta = Date.parse(a.created_at)
			const tb = Date.parse(b.created_at)
			if (ta !== tb) return tb - ta
			return b.id.localeCompare(a.id)
		})
		return sorted
	}, [loadedLogs])

	const formatCost = (nanoUsd: string | undefined) => {
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


	const formatDuration = (ms: number | null | undefined) => {
		if (ms == null) return null
		if (ms < 1000) return `${ms}ms`
		return `${(ms / 1000).toFixed(2)}s`
	}

	const formatTime = (dateString: string) => {
		const date = new Date(dateString)
		const y = date.getFullYear()
		const mo = String(date.getMonth() + 1).padStart(2, '0')
		const d = String(date.getDate()).padStart(2, '0')
		const h = String(date.getHours()).padStart(2, '0')
		const mi = String(date.getMinutes()).padStart(2, '0')
		const s = String(date.getSeconds()).padStart(2, '0')
		return `${y}-${mo}-${d} ${h}:${mi}:${s}`
	}

	const handleLoadMore = () => {
		if (!hasMore || isLoading || isValidating) return
		setRequestOffset(loadedLogs.length)
	}

	const handleStatusChange = (value: string) => {
		setFilters(prev => ({
			...prev,
			status: value === 'all' ? undefined : value
		}))
	}

	const handleModelCommit = () => {
		const trimmed = modelInput.trim()
		setFilters(prev => ({
			...prev,
			model: trimmed || undefined
		}))
	}

	const handleUsernameCommit = () => {
		const trimmed = usernameInput.trim()
		setFilters(prev => ({
			...prev,
			username: trimmed || undefined
		}))
	}

	const handleTokenChange = (value: string) => {
		setFilters(prev => ({
			...prev,
			api_key_id: value === 'all' ? undefined : value
		}))
	}

	const handleTimeRangeChange = (from: Date | undefined, to: Date | undefined) => {
		setTimeFrom(from)
		setTimeTo(to)
	}

	const showingSummary = pageData || loadedLogs.length > 0

	return (
		<PageWrapper className='flex h-full min-h-0 flex-col gap-4 overflow-hidden'>
			<motion.div
				initial={{ opacity: 0, y: -10 }}
				animate={{ opacity: 1, y: 0 }}
				transition={transitions.normal}
			>
				<div className='flex items-center justify-between'>
					<div>
						<h1 className='text-3xl font-bold tracking-tight'>
							{t('requestLogs.title')}
						</h1>
						<p className='text-muted-foreground text-sm'>
							{t('requestLogs.description')}
						</p>
					</div>
				</div>
			</motion.div>

			<motion.div
				initial={{ opacity: 0, y: 10 }}
				animate={{ opacity: 1, y: 0 }}
				transition={{ delay: 0.05, ...transitions.normal }}
				className='rounded-lg border bg-card px-3 py-1.5 space-y-1.5'
			>
				<div className='flex items-center gap-2'>
					<div className='relative flex-1 min-w-[200px] max-w-sm'>
						<Search className='absolute left-3 top-1/2 -translate-y-1/2 h-4 w-4 text-muted-foreground' />
						<Input
							className='pl-10 h-9'
							placeholder={t('requestLogs.searchPlaceholder')}
							value={searchInput}
							onChange={e => setSearchInput(e.target.value)}
						/>
					</div>
					<Button
						type='button'
						variant='ghost'
						size='sm'
						className='h-9 px-2 text-muted-foreground'
						onClick={() => setFiltersExpanded(prev => !prev)}
						aria-label={t('requestLogs.toggleFilters')}
					>
						<motion.span
							animate={{ rotate: filtersExpanded ? 180 : 0 }}
							transition={transitions.fast}
							className='inline-flex'
						>
							<ChevronDown className='h-4 w-4' />
						</motion.span>
					</Button>
					<div className='ml-auto flex items-center gap-3 text-xs text-muted-foreground'>
						{showingSummary && (
							<span className='font-medium text-foreground'>
							{t('requestLogs.totalCost')}: {formatCost(totalCharge)}
							</span>
						)}
						{showingSummary ?
							t('requestLogs.showing', {
								from: totalCount === 0 ? 0 : 1,
								to: Math.min(loadedLogs.length, totalCount),
								total: totalCount
							})
						:	<Skeleton className='h-4 w-24 inline-block' />}
					</div>
				</div>
				<AnimatePresence initial={false}>
					{filtersExpanded && (
						<motion.div
							key='filters'
							initial={{ height: 0, opacity: 0 }}
							animate={{ height: 'auto', opacity: 1 }}
							exit={{ height: 0, opacity: 0 }}
							transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
							className='overflow-hidden'
						>
							<div className='flex flex-wrap items-center gap-2 pt-0.5'>
								{isAdmin && (
									<Input
										className='w-[140px] h-9'
										placeholder={t('requestLogs.filterUsername')}
										value={usernameInput}
										onChange={e => setUsernameInput(e.target.value)}
										onBlur={handleUsernameCommit}
										onKeyDown={e => {
											if (e.key === 'Enter') handleUsernameCommit()
										}}
									/>
								)}
								<Input
									className='w-[200px] h-9'
									placeholder={t('requestLogs.filterModelPlaceholder')}
									value={modelInput}
									onChange={e => setModelInput(e.target.value)}
									onBlur={handleModelCommit}
									onKeyDown={e => {
										if (e.key === 'Enter') handleModelCommit()
									}}
								/>
								<Select
									value={filters.api_key_id || 'all'}
									onValueChange={handleTokenChange}
								>
									<SelectTrigger className='w-[140px] h-9'>
										<SelectValue placeholder={t('requestLogs.filterToken')} />
									</SelectTrigger>
									<SelectContent>
										<SelectItem value='all'>{t('requestLogs.allTokens')}</SelectItem>
										{apiKeys?.map(key => (
											<SelectItem key={key.id} value={key.id}>
												{key.name}
											</SelectItem>
										))}
									</SelectContent>
								</Select>
								<Select
									value={filters.status || 'all'}
									onValueChange={handleStatusChange}
								>
									<SelectTrigger className='w-[120px] h-9'>
										<SelectValue />
									</SelectTrigger>
									<SelectContent>
										<SelectItem value='all'>
											{t('requestLogs.allStatuses')}
										</SelectItem>
										<SelectItem value='pending'>
											{t('requestLogs.pending')}
										</SelectItem>
										<SelectItem value='success'>
											{t('requestLogs.success')}
										</SelectItem>
										<SelectItem value='error'>{t('requestLogs.error')}</SelectItem>
									</SelectContent>
								</Select>
							<DateRangePicker
								from={timeFrom}
								to={timeTo}
								onChange={handleTimeRangeChange}
								t={t}
							/>
							<div className='ml-auto flex items-center gap-1'>
								<TooltipProvider>
									<Tooltip>
										<TooltipTrigger asChild>
											<span
												className={cn(
													'inline-block h-2 w-2 rounded-full transition-colors duration-300',
													sseConnected ? 'bg-emerald-500' : 'bg-amber-500 animate-pulse'
												)}
											/>
										</TooltipTrigger>
										<TooltipContent side='bottom'>
											<p className='text-xs'>
												{sseConnected
													? 'Real-time updates active'
													: 'Real-time updates disconnected, polling...'}
											</p>
										</TooltipContent>
									</Tooltip>
								</TooltipProvider>
								<Button
									type='button'
									variant='outline'
									size='icon'
										className='h-9 w-9'
										onClick={() => mutate()}
										disabled={isValidating}
										title={t('requestLogs.refresh')}
										aria-label={t('requestLogs.refresh')}
									>
										<RefreshCw
											className={cn('h-4 w-4', isValidating && 'animate-spin')}
										/>
								</Button>
								<Button
									type='button'
									variant='outline'
									size='icon'
										className='h-9 w-9'
										onClick={() => setShowIp(prev => !prev)}
										title={showIp ? t('requestLogs.hideIp') : t('requestLogs.showIp')}
										aria-label={
											showIp ? t('requestLogs.hideIp') : t('requestLogs.showIp')
										}
										aria-pressed={showIp}
									>
										{showIp ?
											<Eye className='h-4 w-4' />
										:	<EyeOff className='h-4 w-4' />}
									</Button>
								</div>
							</div>
						</motion.div>
					)}
				</AnimatePresence>
			</motion.div>

			<motion.div
				initial={{ opacity: 0, y: 20 }}
				animate={{ opacity: 1, y: 0 }}
				transition={{ delay: 0.1, ...transitions.normal }}
				className='rounded-lg border bg-card flex-1 min-h-0 overflow-auto'
			>
				{isInitialLoading ?
					<div className='p-4 space-y-1.5'>
						{Array.from({ length: 24 }).map((_, i) => (
							<Skeleton key={i} className='h-9 w-full' />
						))}
					</div>
				: loadedLogs.length === 0 ?
					<div className='h-full flex items-center justify-center text-muted-foreground text-sm px-4'>
						{t('requestLogs.noLogs')}
					</div>
				:	<TableVirtuoso
						style={{ height: '100%', overflowX: 'auto' }}
					data={sortedLogs}
						overscan={480}
						endReached={handleLoadMore}
						components={{
							Table: props => (
								<table
									{...props}
									className='w-full table-auto text-xs'
									style={{ minWidth: '60rem' }}
								/>
							),
							TableHead: props => (
								<thead {...props} className='[&_tr]:border-b' />
							),
							TableBody: props => (
								<tbody {...props} className='[&_tr:last-child]:border-0' />
							),
							TableRow: props => (
								<tr
									{...props}
									className='border-b transition-colors hover:bg-muted/30 align-middle'
								/>
							)
						}}
						fixedHeaderContent={() => (
							<tr className='border-b bg-muted/30'>
								<th className='w-[10rem] text-left font-medium text-muted-foreground pl-2 pr-2 py-1.5 whitespace-nowrap'>
									{t('requestLogs.time')}
								</th>
								<th className='w-[5rem] text-left font-medium text-muted-foreground px-2 py-1.5 whitespace-nowrap'>
									{t('requestLogs.requestId')}
								</th>
								<th className='min-w-[13.5rem] text-left font-medium text-muted-foreground px-2 py-1.5 whitespace-nowrap'>
									{t('requestLogs.model')}
								</th>
								<th className='w-[5rem] text-left font-medium text-muted-foreground px-2 py-1.5 whitespace-nowrap'>
									{t('requestLogs.tokenName')}
								</th>
								{isAdmin && (
									<th className='w-[4rem] text-left font-medium text-muted-foreground px-2 py-1.5 whitespace-nowrap'>
										{t('requestLogs.username')}
									</th>
								)}
								{isAdmin && (
									<th className='w-[5.5rem] text-left font-medium text-muted-foreground px-2 py-1.5 whitespace-nowrap'>
										{t('requestLogs.channel')}
									</th>
								)}
								<th className='w-[8rem] text-left font-medium text-muted-foreground px-2 py-1.5 whitespace-nowrap'>
									{t('requestLogs.duration')} / {t('requestLogs.ttfb')}
								</th>
								<th className='w-[3.25rem] text-right font-medium text-muted-foreground px-2 py-1.5 whitespace-nowrap'>
									{t('requestLogs.input')}
								</th>
								<th className='w-[3.25rem] text-right font-medium text-muted-foreground px-2 py-1.5 whitespace-nowrap'>
									{t('requestLogs.output')}
								</th>
								<th className='min-w-[8.5rem] text-right font-medium text-muted-foreground px-2 py-1.5 whitespace-nowrap'>
									{t('requestLogs.cost')}
								</th>
								<th className='text-left font-medium text-muted-foreground pl-2 pr-2 py-1.5 whitespace-nowrap'>
									{t('requestLogs.requestIp')}
								</th>
							</tr>
						)}
					itemContent={(_index, log) => (
						<LogRowCells
							log={log}
							isAdmin={isAdmin}
							showIp={showIp}
							formatCost={formatCost}
							formatDuration={formatDuration}
							formatTime={formatTime}
							t={t}
							onTooltipOpenChange={onTooltipOpenChange}
						/>
					)}
					/>
				}
			</motion.div>
		</PageWrapper>
	)
}

function LogRowCells({
	log,
	isAdmin,
	showIp,
	formatCost,
	formatDuration,
	formatTime,
	t,
	onTooltipOpenChange
}: {
	log: RequestLog
	isAdmin: boolean
	showIp: boolean
	formatCost: (v: string | undefined) => string
	formatDuration: (v: number | null | undefined) => string | null
	formatTime: (v: string) => string
	t: (key: string) => string
	onTooltipOpenChange: (tooltipId: string, open: boolean) => void
}) {
	const rowTooltipIdsRef = useRef<Set<string>>(new Set())
	const tooltipPrefix = log.request_id || log.id

	const bindTooltipOpenChange = useCallback(
		(suffix: string) => {
			const tooltipId = `${tooltipPrefix}:${suffix}`
			return (open: boolean) => {
				if (open) rowTooltipIdsRef.current.add(tooltipId)
				else rowTooltipIdsRef.current.delete(tooltipId)
				onTooltipOpenChange(tooltipId, open)
			}
		},
		[onTooltipOpenChange, tooltipPrefix]
	)

	useEffect(() => {
		return () => {
			for (const tooltipId of rowTooltipIdsRef.current) {
				onTooltipOpenChange(tooltipId, false)
			}
			rowTooltipIdsRef.current.clear()
		}
	}, [onTooltipOpenChange])

	const requestTooltipOpenChange = bindTooltipOpenChange('request')
	const modelTooltipOpenChange = bindTooltipOpenChange('model')
	const tokenTooltipOpenChange = bindTooltipOpenChange('token')
	const channelTooltipOpenChange = bindTooltipOpenChange('channel')
	const durationTooltipOpenChange = bindTooltipOpenChange('duration')
	const inputTooltipOpenChange = bindTooltipOpenChange('input')
	const outputTooltipOpenChange = bindTooltipOpenChange('output')
	const costTooltipOpenChange = bindTooltipOpenChange('cost')

	const isConnectivityTest =
		log.request_kind === 'active_probe_connectivity' && !log.api_key.name
	const durationMs = getDurationMs(log)
	const ttfbMs = getTtfbMs(log)
	const duration = formatDuration(durationMs)
	const ttfb = formatDuration(ttfbMs)
	const channelDisplay = log.channel.name?.trim() || log.channel.id || null
	const providerDisplay = log.provider.name?.trim() || log.provider.id || null
	const costDisplay = formatCost(log.billing.charge_nano_usd)
	const usageSnapshot = asObject(log.usage)
	const usageInput = asObject(usageSnapshot?.input)
	const usageOutput = asObject(usageSnapshot?.output)
	const billingSnapshot = asObject(log.billing.breakdown)
	const billingInput = asObject(billingSnapshot?.input)
	const billingOutput = asObject(billingSnapshot?.output)
	const multiplier = readNumber(billingSnapshot?.provider_multiplier)

	const formatTokenCount = (value: number | null | undefined) =>
		value == null ? '-' : new Intl.NumberFormat('en-US').format(value)
	const formatRatePerMillion = (nanoPerToken: string | null) => {
		if (!nanoPerToken) return '-'
		const parsed = Number(nanoPerToken)
		if (!Number.isFinite(parsed)) return '-'
		return `$${(parsed / 1000).toLocaleString('en-US', {
			minimumFractionDigits: 2,
			maximumFractionDigits: 6
		})}/1M`
	}
	const formatRateTimesUsage = (
		tokens: number | null,
		rateNano: string | null,
		chargeNano: string | null
	) => {
		if (tokens == null || !rateNano || !chargeNano || Number(chargeNano) === 0) return null
		return `${formatTokenCount(tokens)} × ${formatRatePerMillion(rateNano)} = ${formatCost(chargeNano)}`
	}

	const inputDetailRows: Array<[string, string]> = []
	const outputDetailRows: Array<[string, string]> = []

	const inputTotal =
		readTokenCount(usageInput, 'total_tokens') ?? log.tokens.input ?? null
	const inputUsageUnavailable = inputTotal == null
	const inputUncached =
		readTokenCount(usageInput, 'uncached_tokens') ??
		Math.max((log.tokens.input ?? 0) - (log.tokens.cache_read ?? 0), 0)
	const inputText = readTokenCount(usageInput, 'text_tokens')
	const inputCached =
		readTokenCount(usageInput, 'cached_tokens') ?? log.tokens.cache_read ?? null
	const inputCacheCreation = readTokenCount(usageInput, 'cache_creation_tokens')
	const inputAudio = readTokenCount(usageInput, 'audio_tokens')
	const inputImage = readTokenCount(usageInput, 'image_tokens')

	const hasInputBreakdown = !!(inputCached || inputCacheCreation || inputText || inputAudio || inputImage)

	if (inputTotal)
		inputDetailRows.push([
			t('requestLogs.totalTokens'),
			formatTokenCount(inputTotal)
		])
	if (hasInputBreakdown && inputUncached)
		inputDetailRows.push([
			t('requestLogs.uncachedTokens'),
			formatTokenCount(inputUncached)
		])
	if (inputText)
		inputDetailRows.push([
			t('requestLogs.textTokens'),
			formatTokenCount(inputText)
		])
	if (inputCached)
		inputDetailRows.push([
			t('requestLogs.cachedTokens'),
			formatTokenCount(inputCached)
		])
	if (inputCacheCreation)
		inputDetailRows.push([
			t('requestLogs.cacheCreationTokens'),
			formatTokenCount(inputCacheCreation)
		])
	if (inputAudio)
		inputDetailRows.push([
			t('requestLogs.audioTokens'),
			formatTokenCount(inputAudio)
		])
	if (inputImage)
		inputDetailRows.push([
			t('requestLogs.imageTokens'),
			formatTokenCount(inputImage)
		])

	const outputTotal =
		readTokenCount(usageOutput, 'total_tokens') ?? log.tokens.output ?? null
	const outputUsageUnavailable = outputTotal == null
	const outputNonReasoning =
		readTokenCount(usageOutput, 'non_reasoning_tokens') ??
		Math.max((log.tokens.output ?? 0) - (log.tokens.reasoning ?? 0), 0)
	const outputText = readTokenCount(usageOutput, 'text_tokens')
	const outputReasoning =
		readTokenCount(usageOutput, 'reasoning_tokens') ??
		log.tokens.reasoning ??
		null
	const inputTokensForDisplay = inputTotal ?? null
	const outputTokensForDisplay = outputTotal ?? null
	const outputAudio = readTokenCount(usageOutput, 'audio_tokens')
	const outputImage = readTokenCount(usageOutput, 'image_tokens')

	const hasOutputBreakdown = !!(outputReasoning || outputText || outputAudio || outputImage)

	if (outputTotal)
		outputDetailRows.push([
			t('requestLogs.totalTokens'),
			formatTokenCount(outputTotal)
		])
	if (hasOutputBreakdown && outputNonReasoning)
		outputDetailRows.push([
			t('requestLogs.nonReasoningTokens'),
			formatTokenCount(outputNonReasoning)
		])
	if (outputText)
		outputDetailRows.push([
			t('requestLogs.textTokens'),
			formatTokenCount(outputText)
		])
	if (outputReasoning)
		outputDetailRows.push([
			t('requestLogs.reasoningTokens'),
			formatTokenCount(outputReasoning)
		])
	if (outputAudio)
		outputDetailRows.push([
			t('requestLogs.audioTokens'),
			formatTokenCount(outputAudio)
		])
	if (outputImage)
		outputDetailRows.push([
			t('requestLogs.imageTokens'),
			formatTokenCount(outputImage)
		])

	const inputUncachedCostDetail = formatRateTimesUsage(
		readTokenCount(billingInput, 'billed_uncached_tokens'),
		readNanoString(billingInput, 'unit_price_nano'),
		readNanoString(billingInput, 'uncached_charge_nano')
	)
	const inputCachedCostDetail = formatRateTimesUsage(
		readTokenCount(billingInput, 'billed_cached_tokens'),
		readNanoString(billingInput, 'cached_unit_price_nano'),
		readNanoString(billingInput, 'cached_charge_nano')
	)
	const inputCacheCreationCostDetail = formatRateTimesUsage(
		readTokenCount(billingInput, 'billed_cache_creation_tokens'),
		readNanoString(billingInput, 'cache_creation_unit_price_nano'),
		readNanoString(billingInput, 'cache_creation_charge_nano')
	)
	const outputTextCostDetail = formatRateTimesUsage(
		readTokenCount(billingOutput, 'billed_non_reasoning_tokens'),
		readNanoString(billingOutput, 'unit_price_nano'),
		readNanoString(billingOutput, 'non_reasoning_charge_nano')
	)
	const outputReasoningCostDetail = formatRateTimesUsage(
		readTokenCount(billingOutput, 'billed_reasoning_tokens'),
		readNanoString(billingOutput, 'reasoning_unit_price_nano'),
		readNanoString(billingOutput, 'reasoning_charge_nano')
	)
	const statusIndicatorClass =
		log.status === 'success' ? 'bg-emerald-500'
		: log.status === 'pending' ? 'bg-sky-500'
		: log.status === 'error' ? 'bg-red-500'
		: 'bg-zinc-400'
	const baseCharge = readNanoString(billingSnapshot, 'base_charge_nano')
	const hasBreakdownContent = !!(inputUncachedCostDetail || inputCachedCostDetail || inputCacheCreationCostDetail || outputTextCostDetail || outputReasoningCostDetail || baseCharge || multiplier != null || !billingSnapshot)

	return (
		<>
			<td className='pl-2 pr-2 py-1 whitespace-nowrap text-muted-foreground font-mono align-middle'>
				{formatTime(log.created_at)}
			</td>

			<td className='px-2 py-1 whitespace-nowrap align-middle'>
				{log.request_id ?
					<TooltipProvider delayDuration={200}>
						<Tooltip onOpenChange={requestTooltipOpenChange}>
							<TooltipTrigger asChild>
								<span className='inline-flex items-center gap-1 font-mono text-muted-foreground cursor-default'>
									<span>{log.request_id.substring(0, 8)}</span>
									<span
										className={cn(
											'h-1.5 w-1.5 rounded-full',
											statusIndicatorClass
										)}
									/>
								</span>
							</TooltipTrigger>
							<TooltipContent>
								<div className='text-xs space-y-0.5 max-w-[480px]'>
									<div className='font-mono'>{log.request_id}</div>
									{log.status === 'error' && (
										<>
											{log.error.http_status != null && (
												<div>
													{t('requestLogs.errorStatus')}:{' '}
														{log.error.http_status}
												</div>
											)}
											{log.error.code && (
												<div>
														{t('requestLogs.errorCode')}: {log.error.code}
												</div>
											)}
											{log.error.message && (
												<div className='break-words whitespace-pre-wrap'>
														{t('requestLogs.errorMessage')}: {log.error.message}
												</div>
											)}
										</>
									)}
									{log.tried_providers &&
										log.tried_providers.length > 0 && (
											<div className='border-t border-border/50 pt-1 mt-1'>
												<div className='font-medium mb-0.5'>
													{t('requestLogs.triedProviders')}:
												</div>
												{log.tried_providers.map(
													(
														tp: {
															provider_id: string
															channel_id: string
															error: string
														},
														i: number
													) => (
														<div
															key={i}
															className='text-muted-foreground break-words'
														>
															{tp.provider_id}/{tp.channel_id}: {tp.error}
														</div>
													)
												)}
											</div>
										)}
								</div>
							</TooltipContent>
						</Tooltip>
					</TooltipProvider>
				:	<span className='text-muted-foreground/50'>-</span>}
			</td>

			<td className='px-2 py-1 align-middle whitespace-nowrap'>
				<TooltipProvider delayDuration={200}>
					<Tooltip onOpenChange={modelTooltipOpenChange}>
						<TooltipTrigger asChild>
							<span className='cursor-default'>
								<ModelBadge
									model={log.model}
										multiplier={log.provider.multiplier}
									showDetails={false}
									truncateModelText={false}
									className='text-[10px] h-5 px-1.5 min-w-max'
								/>
							</span>
						</TooltipTrigger>
						<TooltipContent>
							<div className='text-xs space-y-0.5 min-w-[180px]'>
								<div className='flex items-center justify-between gap-3'>
									<span>{t('requestLogs.model')}</span>
									<span className='font-mono'>{log.model}</span>
								</div>
								{log.upstream_model && log.upstream_model !== log.model && (
									<div className='flex items-center justify-between gap-3'>
										<span>{t('requestLogs.upstreamModel')}</span>
										<span className='font-mono'>{log.upstream_model}</span>
									</div>
								)}
								{log.provider.id && (
									<div className='flex items-center justify-between gap-3'>
										<span>{t('requestLogs.modelProvider')}</span>
											<span className='font-mono'>{log.provider.id}</span>
									</div>
								)}
								{log.provider.multiplier != null &&
											log.provider.multiplier !== 1 && (
										<div className='flex items-center justify-between gap-3'>
											<span>{t('requestLogs.multiplier')}</span>
											<span className='font-mono'>
														{log.provider.multiplier}x
											</span>
										</div>
									)}
								{log.reasoning_effort && (
									<div className='flex items-center justify-between gap-3'>
										<span>{t('requestLogs.reasoningEffort')}</span>
										<span className='font-mono'>{log.reasoning_effort}</span>
									</div>
								)}
							</div>
						</TooltipContent>
					</Tooltip>
				</TooltipProvider>
			</td>

			<td className='px-2 py-1 whitespace-nowrap align-middle text-[11px] leading-4 text-muted-foreground'>
				<TooltipProvider delayDuration={200}>
					<Tooltip onOpenChange={tokenTooltipOpenChange}>
						<TooltipTrigger asChild>
							<span className='inline-flex h-4 items-center max-w-[5rem] truncate cursor-default'>
								{isConnectivityTest ?
									t('requestLogs.connectivityTest')
									:	log.api_key.name || '-'}
							</span>
						</TooltipTrigger>
						<TooltipContent>
							<span className='text-xs'>
								{isConnectivityTest ?
									t('requestLogs.connectivityTest')
									:	log.api_key.name || '-'}
							</span>
						</TooltipContent>
					</Tooltip>
				</TooltipProvider>
			</td>

			{isAdmin && (
				<td className='px-2 py-1 whitespace-nowrap align-middle text-[11px] leading-4 text-muted-foreground'>
					<span className='inline-flex h-4 items-center max-w-[5rem] truncate'>
						{log.user.username || '-'}
					</span>
				</td>
			)}

			{isAdmin && (
				<td className='px-2 py-1 whitespace-nowrap align-middle text-[11px] leading-4 text-muted-foreground'>
					{providerDisplay ?
						<TooltipProvider delayDuration={200}>
							<Tooltip onOpenChange={channelTooltipOpenChange}>
								<TooltipTrigger asChild>
									<span className='inline-flex h-4 items-center cursor-default max-w-[80px] truncate'>
										{providerDisplay}
									</span>
								</TooltipTrigger>
								<TooltipContent>
									<div className='text-xs space-y-0.5'>
{channelDisplay && <div>{t('requestLogs.channel')}: {channelDisplay}</div>}
									{log.upstream_model && log.upstream_model !== log.model && (
										<div>{t('requestLogs.upstreamModel')}: {log.upstream_model}</div>
										)}
									</div>
								</TooltipContent>
							</Tooltip>
						</TooltipProvider>
					:	<span className='inline-flex h-4 items-center text-muted-foreground/50'>
							-
						</span>
					}
				</td>
			)}

			<td className='px-2 py-1 whitespace-nowrap align-middle'>
				<div className='flex items-center gap-px'>
					{duration && (
						<TooltipProvider delayDuration={200}>
							<Tooltip onOpenChange={durationTooltipOpenChange}>
								<TooltipTrigger asChild>
									<Badge
										variant='secondary'
										className={cn(
											'text-[10px] h-5 px-1 font-mono rounded-full border-0 cursor-default',
											'bg-muted text-muted-foreground'
										)}
									>
										{duration}
									</Badge>
								</TooltipTrigger>
								<TooltipContent>
									<div className='text-xs space-y-0.5 min-w-[140px]'>
										<div className='flex items-center justify-between gap-3'>
											<span>{t('requestLogs.duration')}</span>
											<span className='font-mono'>{duration}</span>
										</div>
										{durationMs != null &&
											durationMs > 0 &&
											outputTotal != null &&
											outputTotal > 0 && (() => {
												// When ttfb ≈ duration (no visible streaming output,
												// e.g. pure reasoning then tool_call), use total time
												const generationMs = (ttfbMs != null && durationMs > ttfbMs)
													? durationMs - ttfbMs
													: durationMs
												const tpsValue = outputTotal / (generationMs / 1000);
												return (
													<div className='flex items-center justify-between gap-3'>
														<span>{t('requestLogs.avgTps')}</span>
														<span className='font-mono'>
															{tpsValue.toFixed(2)} t/s
														</span>
													</div>
												);
											})()}
									</div>
								</TooltipContent>
							</Tooltip>
						</TooltipProvider>
					)}
					{ttfb && (
						<Badge
							variant='secondary'
							className='text-[10px] h-5 px-1 font-mono rounded-full border-0 bg-sky-500/15 text-sky-700 dark:text-sky-400'
						>
							{ttfb}
						</Badge>
					)}
					{log.is_stream ?
						<Badge
							variant='secondary'
							className='text-[10px] h-5 px-1 font-mono rounded-full border-0 bg-indigo-500/15 text-indigo-700 dark:text-indigo-400'
						>
							{t('requestLogs.streamBadge')}
						</Badge>
					:	<Badge
							variant='secondary'
							className='text-[10px] h-5 px-1 font-mono rounded-full border-0 bg-amber-500/15 text-amber-700 dark:text-amber-400'
						>
							{t('requestLogs.nonStreamBadge')}
						</Badge>
					}
				</div>
			</td>

			<td className='px-2 py-1 text-right whitespace-nowrap font-mono text-muted-foreground align-middle'>
				<TooltipProvider delayDuration={200}>
					<Tooltip onOpenChange={inputTooltipOpenChange}>
						<TooltipTrigger asChild>
							<span className='cursor-default'>{formatTokenCount(inputTokensForDisplay)}</span>
						</TooltipTrigger>
						<TooltipContent>
							<div className='text-xs space-y-0.5 min-w-[220px]'>
								{inputUsageUnavailable ? (
									<div className='text-muted-foreground'>
										{t('requestLogs.usageUnavailable')}
									</div>
								) : (
									inputDetailRows.map(([label, value]) => (
										<div
											key={label}
											className='flex items-center justify-between gap-3'
										>
											<span>{label}</span>
											<span className='font-mono'>{value}</span>
										</div>
									))
								)}
							</div>
						</TooltipContent>
					</Tooltip>
				</TooltipProvider>
			</td>

			<td className='px-2 py-1 text-right whitespace-nowrap font-mono text-muted-foreground align-middle'>
				<TooltipProvider delayDuration={200}>
					<Tooltip onOpenChange={outputTooltipOpenChange}>
						<TooltipTrigger asChild>
							<span className='cursor-default'>
								{formatTokenCount(outputTokensForDisplay)}
							</span>
						</TooltipTrigger>
						<TooltipContent>
							<div className='text-xs space-y-0.5 min-w-[220px]'>
								{outputUsageUnavailable ? (
									<div className='text-muted-foreground'>
										{t('requestLogs.usageUnavailable')}
									</div>
								) : (
									outputDetailRows.map(([label, value]) => (
										<div
											key={label}
											className='flex items-center justify-between gap-3'
										>
											<span>{label}</span>
											<span className='font-mono'>{value}</span>
										</div>
									))
								)}
							</div>
						</TooltipContent>
					</Tooltip>
				</TooltipProvider>
			</td>

			<td className='px-2 py-1 text-right whitespace-nowrap font-mono align-middle'>
				{hasBreakdownContent ? (
					<TooltipProvider delayDuration={200}>
						<Tooltip onOpenChange={costTooltipOpenChange}>
							<TooltipTrigger asChild>
								<span
									className='inline-flex items-center whitespace-nowrap align-bottom cursor-default'
									title={costDisplay}
								>
									{costDisplay}
								</span>
							</TooltipTrigger>
							<TooltipContent>
								<div className='text-xs space-y-0.5 min-w-[300px]'>
									{inputUncachedCostDetail && (
										<div className='flex items-center justify-between gap-3'>
											<span>{t('requestLogs.input')}{inputCachedCostDetail ? ` (${t('requestLogs.uncachedTokens')})` : ''}</span>
											<span className='font-mono'>{inputUncachedCostDetail}</span>
										</div>
									)}
									{inputCachedCostDetail && (
										<div className='flex items-center justify-between gap-3'>
											<span>
												{t('requestLogs.input')} ({t('requestLogs.cachedTokens')})
											</span>
											<span className='font-mono'>{inputCachedCostDetail}</span>
										</div>
									)}
									{inputCacheCreationCostDetail && (
										<div className='flex items-center justify-between gap-3'>
											<span>
												{t('requestLogs.input')} ({t('requestLogs.cacheCreationTokens')})
											</span>
											<span className='font-mono'>{inputCacheCreationCostDetail}</span>
										</div>
									)}
									{outputTextCostDetail && (
										<div className='flex items-center justify-between gap-3'>
											<span>
												{t('requestLogs.output')}{outputReasoningCostDetail ? ` (${t('requestLogs.nonReasoningTokens')})` : ''}
											</span>
											<span className='font-mono'>{outputTextCostDetail}</span>
										</div>
									)}
									{outputReasoningCostDetail && (
										<div className='flex items-center justify-between gap-3'>
											<span>
												{t('requestLogs.output')} (
												{t('requestLogs.reasoningTokens')})
											</span>
											<span className='font-mono'>
												{outputReasoningCostDetail}
											</span>
										</div>
									)}
									{baseCharge && (
										<div className='flex items-center justify-between gap-3'>
											<span>{t('requestLogs.baseCost')}</span>
											<span className='font-mono'>{formatCost(baseCharge)}</span>
										</div>
									)}
									{multiplier != null && (
										<div className='flex items-center justify-between gap-3'>
											<span>{t('requestLogs.multiplier')}</span>
											<span className='font-mono'>{multiplier.toFixed(6)}x</span>
										</div>
									)}
									{!billingSnapshot && (
										<div className='text-muted-foreground'>
											{t('requestLogs.detailsUnavailable')}
										</div>
									)}
									<div className='border-t border-muted pt-2 mt-2'>
										<div className='flex items-center justify-between gap-3'>
											<span className='text-xs text-muted-foreground'>{t('requestLogs.totalCost')}</span>
											<span className='font-mono text-xs'>
										{formatCost(log.billing.charge_nano_usd)}
											</span>
										</div>
									</div>
								</div>
							</TooltipContent>
						</Tooltip>
					</TooltipProvider>
				) : (
					<span
						className='inline-flex items-center whitespace-nowrap align-bottom'
						title={costDisplay}
					>
						{costDisplay}
					</span>
				)}
			</td>

			<td className='pl-2 pr-2 py-1 whitespace-nowrap font-mono text-muted-foreground text-[11px] align-middle'>
				<span
					className={cn(
						'inline-block align-bottom transition-[filter] duration-150',
						!showIp && 'blur-[3px]'
					)}
					title={log.request_ip || '-'}
				>
					{log.request_ip || '-'}
				</span>
			</td>
		</>
	)
}
