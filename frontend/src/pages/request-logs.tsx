import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { ChevronDown, Eye, EyeOff, RefreshCw, Search } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Skeleton } from '@/components/ui/skeleton'
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
import { cn } from '@/lib/utils'
import type { RequestLog, RequestLogsFilter, RequestLogsResponse } from '@/lib/api'
import { PageWrapper, motion, transitions } from '@/components/ui/motion'
import { AnimatePresence } from 'framer-motion'
import { DateRangePicker } from './request-logs/date-range-picker'
import { RequestLogsTable } from './request-logs/request-logs-table'
import { asObject, formatCost } from './request-logs/utils'

const REQUEST_LOGS_PAGE_SIZE = 100

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
	const pendingPageDataRef = useRef<RequestLogsResponse | null>(null)
	const pendingNewestDataRef = useRef<RequestLogsResponse | null>(null)
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

	const filterKey = useMemo(() => JSON.stringify(activeFilters), [activeFilters])

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

	const matchesActiveFilters = useCallback(
		(log: RequestLog) => {
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
			if (activeFilters.api_key_id && log.api_key.id !== activeFilters.api_key_id) {
				return false
			}

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
				const matchesSearch = searchFields.some(
					value => typeof value === 'string' && value.toLowerCase().includes(q)
				)
				if (!matchesSearch) return false
			}

			return true
		},
		[activeFilters]
	)

	const prependSSELogs = useCallback(
		(logs: RequestLog[]) => {
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
							const duplicateIndex = next.findIndex(
								item => item.request_id === log.request_id
							)
							if (duplicateIndex >= 0) next[duplicateIndex] = log
							continue
						}
						handledRequestIds.add(log.request_id)
						const existingIndex = next.findIndex(
							item => item.request_id === log.request_id
						)
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
		},
		[matchesActiveFilters]
	)

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

	const showingSummary = !!pageData || loadedLogs.length > 0

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
						: <Skeleton className='h-4 w-24 inline-block' />}
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
													{sseConnected ?
														'Real-time updates active'
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
										onClick={() => {
											void mutate()
										}}
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
										aria-label={showIp ? t('requestLogs.hideIp') : t('requestLogs.showIp')}
										aria-pressed={showIp}
									>
										{showIp ?
											<Eye className='h-4 w-4' />
										: <EyeOff className='h-4 w-4' />}
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
				<RequestLogsTable
					isAdmin={isAdmin}
					isInitialLoading={isInitialLoading}
					logs={sortedLogs}
					onLoadMore={handleLoadMore}
					onTooltipOpenChange={onTooltipOpenChange}
					showIp={showIp}
					t={t}
				/>
			</motion.div>
		</PageWrapper>
	)
}
