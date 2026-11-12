import {
	useCallback,
	useEffect,
	useEffectEvent,
	useLayoutEffect,
	useMemo,
	useRef,
	useState
} from 'react'
import { useSearchParams } from 'react-router-dom'
import { useTranslation } from 'react-i18next'
import useSWR from 'swr'
import {
	ChevronDown,
	Eye,
	EyeOff,
	RadioTower,
	RefreshCw,
	Search,
	WifiOff,
	X
} from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Skeleton } from '@/components/ui/skeleton'
import { QueryError } from '@/components/ui/query-error'
import {
	Select,
	SelectContent,
	SelectItem,
	SelectTrigger,
	SelectValue
} from '@/components/ui/select'
import { SWR_KEYS, useRequestLogs, useApiKeys, useProviders, useUserApiKeys } from '@/lib/swr'
import { useRequestLogSSE, type RequestLogStreamEvent } from '@/lib/sse'
import { useAuth } from '@/hooks/use-auth'
import { useIsMobile } from '@/hooks/use-mobile'
import { cn } from '@/lib/utils'
import { api } from '@/lib/api'
import type { RequestLog, RequestLogsFilter, RequestLogsResponse, User } from '@/lib/api'
import { PageWrapper, motion, transitions } from '@/components/ui/motion'
import { PageHeader } from '@/components/ui/page-header'
import { AnimatePresence } from 'framer-motion'
import { CaptureViewerDialog } from './request-logs/capture-viewer-dialog'
import { DateRangePicker } from './request-logs/date-range-picker'
import {
	RELATIVE_TIME_RANGES,
	relativeRangeStart,
	type TimeRangeValue
} from './request-logs/time-range'
import { RequestLogsTable } from './request-logs/request-logs-table'
import { asObject, formatCost } from './request-logs/utils'

const REQUEST_LOGS_PAGE_SIZE = 100
const STATUS_FILTER_VALUES = ['pending', 'success', 'client_gone', 'error'] as const
// Every URL parameter owned by the logs filter contract (LOGS-3).
const FILTER_PARAM_KEYS = [
	'q',
	'model',
	'status',
	'api_key_id',
	'username',
	'channel_id',
	'from',
	'to',
	'range'
] as const

function parseIsoParam(value: string): Date | undefined {
	if (!value) return undefined
	const date = new Date(value)
	return Number.isNaN(date.getTime()) ? undefined : date
}

export function RequestLogsPage() {
	const { t, i18n } = useTranslation()
	const { user } = useAuth()
	const isMobile = useIsMobile()
	const [searchParams, setSearchParams] = useSearchParams()
	const isAdmin = user?.role === 'super_admin' || user?.role === 'admin'

	// LOGS-3: the URL is the single source of truth for every applied filter.
	const urlQ = searchParams.get('q')?.trim() ?? ''
	const urlModel = searchParams.get('model')?.trim() ?? ''
	const rawStatus = searchParams.get('status') ?? ''
	const urlStatus = STATUS_FILTER_VALUES.find(value => value === rawStatus) ?? ''
	const urlApiKeyId = searchParams.get('api_key_id')?.trim() ?? ''
	const urlUsername = isAdmin ? (searchParams.get('username')?.trim() ?? '') : ''
	const urlChannelId = isAdmin ? (searchParams.get('channel_id')?.trim() ?? '') : ''
	const rawRange = searchParams.get('range')
	const urlRange = RELATIVE_TIME_RANGES.find(value => value === rawRange)
	const fromParam = urlRange ? '' : (searchParams.get('from') ?? '')
	const toParam = urlRange ? '' : (searchParams.get('to') ?? '')

	const updateParams = useCallback(
		(patch: Partial<Record<(typeof FILTER_PARAM_KEYS)[number], string | undefined>>) => {
			setSearchParams(
				prev => {
					const next = new URLSearchParams(prev)
					for (const [key, value] of Object.entries(patch)) {
						if (value) next.set(key, value)
						else next.delete(key)
					}
					return next
				},
				{ replace: true }
			)
		},
		[setSearchParams]
	)

	// A relative range is recomputed from "now" whenever it is (re)selected or
	// loaded, so shared links never pin an old window.
	const [rangeAnchor, setRangeAnchor] = useState(() => ({
		range: urlRange,
		from: urlRange ? relativeRangeStart(urlRange) : undefined
	}))
	if (rangeAnchor.range !== urlRange) {
		setRangeAnchor({ range: urlRange, from: urlRange ? relativeRangeStart(urlRange) : undefined })
	}
	const timeFrom = useMemo(
		() => (urlRange ? rangeAnchor.from : parseIsoParam(fromParam)),
		[urlRange, rangeAnchor.from, fromParam]
	)
	const timeTo = useMemo(() => (urlRange ? undefined : parseIsoParam(toParam)), [urlRange, toParam])
	const timeRangeValue = useMemo<TimeRangeValue>(
		() => ({ range: urlRange, from: timeFrom, to: timeTo }),
		[urlRange, timeFrom, timeTo]
	)

	// Text drafts follow the URL when it changes from outside (deep links,
	// "clear filters"), but local typing is never overwritten.
	const [searchInput, setSearchInput] = useState(urlQ)
	const [syncedQ, setSyncedQ] = useState(urlQ)
	if (syncedQ !== urlQ) {
		setSyncedQ(urlQ)
		if (searchInput.trim() !== urlQ) setSearchInput(urlQ)
	}
	// FL7c: only the debounced value reaches the URL and the fetch filters.
	useEffect(() => {
		const trimmed = searchInput.trim()
		if (trimmed === urlQ) return
		const handle = window.setTimeout(() => updateParams({ q: trimmed || undefined }), 300)
		return () => window.clearTimeout(handle)
	}, [searchInput, urlQ, updateParams])

	const [modelInput, setModelInput] = useState(urlModel)
	const [syncedModel, setSyncedModel] = useState(urlModel)
	if (syncedModel !== urlModel) {
		setSyncedModel(urlModel)
		setModelInput(urlModel)
	}
	const [usernameInput, setUsernameInput] = useState(urlUsername)
	const [syncedUsername, setSyncedUsername] = useState(urlUsername)
	if (syncedUsername !== urlUsername) {
		setSyncedUsername(urlUsername)
		setUsernameInput(urlUsername)
	}

	const [showIp, setShowIp] = useState(false)
	const [requestOffset, setRequestOffset] = useState(0)
	const [loadedLogs, setLoadedLogs] = useState<RequestLog[]>([])
	const [totalCount, setTotalCount] = useState(0)
	// This REST aggregate covers every filtered row and must never be derived from loadedLogs.
	const [totalCharge, setTotalCharge] = useState<string>('0')
	const [filtersExpanded, setFiltersExpanded] = useState(() => !isMobile)
	const [automaticUpdatesEnabled, setAutomaticUpdatesEnabled] = useState(true)
	const openTooltipIdsRef = useRef<Set<string>>(new Set())
	const pendingPageDataRef = useRef<RequestLogsResponse | null>(null)
	const pendingNewestDataRef = useRef<RequestLogsResponse | null>(null)
	const pendingSSERef = useRef<RequestLog[]>([])
	// Points at the latest buffer flush; event callbacks call it so they stay stable.
	const flushBuffersRef = useRef<() => void>(() => {})
	// LOGS-7: while the table is scrolled away from the top, live rows are
	// buffered so the rows being read do not move.
	const atTopRef = useRef(true)
	const [captureOpen, setCaptureOpen] = useState(false)
	const [captureTarget, setCaptureTarget] = useState<RequestLog | null>(null)
	const captureReadyRef = useRef<Set<string>>(new Set())
	const streamEventHandlerRef = useRef<(event: RequestLogStreamEvent) => void>(() => {})
	const receiveStreamEvent = useCallback((event: RequestLogStreamEvent) => {
		streamEventHandlerRef.current(event)
	}, [])

	const handleOpenCapture = useCallback((log: RequestLog) => {
		if (!log.request_id || !log.user?.id) return
		setCaptureTarget(log)
		setCaptureOpen(true)
	}, [])

	const onTooltipOpenChange = useCallback((tooltipId: string, open: boolean) => {
		if (open) {
			openTooltipIdsRef.current.add(tooltipId)
			return
		}

		openTooltipIdsRef.current.delete(tooltipId)
		if (openTooltipIdsRef.current.size === 0) {
			flushBuffersRef.current()
		}
	}, [])

	const handleAtTopChange = useCallback((next: boolean) => {
		atTopRef.current = next
		if (next) flushBuffersRef.current()
	}, [])

	const activeFilters = useMemo<RequestLogsFilter>(() => {
		const f: RequestLogsFilter = {}
		if (urlStatus) f.status = urlStatus
		if (urlModel) f.model = urlModel
		if (urlApiKeyId) f.api_key_id = urlApiKeyId
		if (urlChannelId) f.channel_id = urlChannelId
		if (urlUsername) f.username = urlUsername
		if (urlQ) f.search = urlQ
		if (timeFrom) f.time_from = timeFrom.toISOString()
		if (timeTo) f.time_to = timeTo.toISOString()
		return f
	}, [urlStatus, urlModel, urlApiKeyId, urlChannelId, urlUsername, urlQ, timeFrom, timeTo])

	const filterKey = useMemo(() => JSON.stringify(activeFilters), [activeFilters])

	useEffect(() => {
		// eslint-disable-next-line react-hooks/set-state-in-effect -- reset local pagination when the serialized filter input changes
		setRequestOffset(0)
		setLoadedLogs([])
		setTotalCount(0)
		setTotalCharge('0')
		pendingPageDataRef.current = null
		pendingNewestDataRef.current = null
		captureReadyRef.current.clear()
		atTopRef.current = true
	}, [filterKey])

	// LOGS-1 / FL38a: tooltip pause applies to periodic and focus revalidation
	// only. SWR checks `isPaused` synchronously when a new key starts its first
	// fetch, so this layout effect (declared before the SWR hooks, therefore run
	// before their key-change effects) lifts the pause for that one fetch.
	const bypassTooltipPauseRef = useRef(false)
	const swrRequestKey = `${filterKey}@${requestOffset}`
	useLayoutEffect(() => {
		bypassTooltipPauseRef.current = true
	}, [swrRequestKey])
	useEffect(() => {
		bypassTooltipPauseRef.current = false
	}, [swrRequestKey])
	const isTooltipPaused = useCallback(
		() => !bypassTooltipPauseRef.current && openTooltipIdsRef.current.size > 0,
		[]
	)

	// LOGS-10: admins pick from the filtered user's keys; regular users from their own.
	const { data: ownApiKeys } = useApiKeys()
	const { data: users } = useSWR<User[]>(
		isAdmin && urlUsername ? SWR_KEYS.USERS : null,
		() => api.listUsers()
	)
	const filteredUserId = users?.find(item => item.username === urlUsername)?.id ?? null
	const { data: filteredUserApiKeys } = useUserApiKeys(isAdmin ? filteredUserId : null)
	const tokenOptions = isAdmin ? filteredUserApiKeys : ownApiKeys
	const tokenSelectNeedsUser = isAdmin && !urlUsername

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
			if (activeFilters.api_key_id && log.api_key?.id !== activeFilters.api_key_id) {
				return false
			}
			if (activeFilters.channel_id && log.channel?.id !== activeFilters.channel_id) {
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

	/**
	 * Re-prepend SSE-only pending items that the server response doesn't know about.
	 * Pending items are never persisted to the DB (RL1a-1), so SWR responses will
	 * never contain them. Without this, every SWR revalidation silently drops them.
	 */
	const preservePendingItems = useCallback(
		(prev: RequestLog[], serverData: RequestLog[]): RequestLog[] => {
			const serverRequestIds = new Set(serverData.map(log => log.request_id).filter(Boolean))
			const serverIds = new Set(serverData.map(log => log.id))
			const pendingItems = prev.filter(
				log =>
					log.status === 'pending' &&
					!serverIds.has(log.id) &&
					(!log.request_id || !serverRequestIds.has(log.request_id))
			)
			if (pendingItems.length === 0) return serverData
			return [...pendingItems, ...serverData]
		},
		[]
	)

	const applyCaptureReadiness = useCallback(
		(logs: RequestLog[]): RequestLog[] =>
			logs.map(log =>
				!log.has_capture && captureReadyRef.current.has(JSON.stringify([log.user?.id, log.request_id]))
					? { ...log, has_capture: true }
					: log
			),
		[]
	)

	const prependSSELogs = useCallback(
		(logs: RequestLog[]) => {
			if (logs.length === 0) return

			setLoadedLogs(prev => {
				const next = [...prev]
				const existingIds = new Set(prev.map(log => log.id))
				const incomingIds = new Set<string>()
				const handledRequestIds = new Set<string>()

				for (let log of applyCaptureReadiness(logs)) {
					if (incomingIds.has(log.id)) continue
					incomingIds.add(log.id)
					const matchesFilters = matchesActiveFilters(log)

					if (log.request_id) {
						if (handledRequestIds.has(log.request_id)) {
							const duplicateIndex = next.findIndex(
								item => item.request_id === log.request_id
							)
							if (duplicateIndex >= 0) {
								if (
									next[duplicateIndex].has_capture &&
									next[duplicateIndex].user?.id === log.user?.id
								) {
									log = { ...log, has_capture: true }
								}
								if (matchesFilters) next[duplicateIndex] = log
								else next.splice(duplicateIndex, 1)
							}
							continue
						}
						handledRequestIds.add(log.request_id)
						const existingIndex = next.findIndex(
							item => item.request_id === log.request_id
						)
						// A buffered `pending` event can be flushed after the server
						// snapshot that already holds the terminal row (FL37e); it
						// must never downgrade that row back to pending.
						if (
							existingIndex >= 0 &&
							log.status === 'pending' &&
							next[existingIndex].status !== 'pending'
						) {
							continue
						}
						if (existingIndex >= 0) {
							if (
								next[existingIndex].has_capture &&
								next[existingIndex].user?.id === log.user?.id
							) {
								log = { ...log, has_capture: true }
							}
							next.splice(existingIndex, 1)
							if (matchesFilters) next.unshift(log)
							continue
						}
					}

					if (!matchesFilters) continue
					if (existingIds.has(log.id)) continue
					next.unshift(log)
				}

				return next
			})
		},
		[matchesActiveFilters, applyCaptureReadiness]
	)

	const receiveSSELogs = useCallback(
		(logs: RequestLog[]) => {
			if (openTooltipIdsRef.current.size > 0 || !atTopRef.current) {
				pendingSSERef.current = [...pendingSSERef.current, ...logs]
				return
			}
			prependSSELogs(logs)
		},
		[prependSSELogs]
	)

	// Server responses are either applied or buffered per FL38 (tooltip open) and
	// FL37e (scrolled). Both hooks share a cache key at offset 0, where the two
	// merges coincide, so applying one response through both paths is idempotent.
	const applyPageData = (data: RequestLogsResponse) => {
		setTotalCount(data.total)
		setTotalCharge(data.total_charge_nano_usd)
		setLoadedLogs(prev => {
			const rows = applyCaptureReadiness(data.data)
			if (data.offset === 0) return preservePendingItems(prev, rows)
			const existingIds = new Set(prev.map(log => log.id))
			const appended = rows.filter(log => !existingIds.has(log.id))
			return appended.length === 0 ? prev : [...prev, ...appended]
		})
	}

	const applyNewestData = (data: RequestLogsResponse) => {
		setTotalCount(data.total)
		setTotalCharge(data.total_charge_nano_usd)
		setLoadedLogs(prev => {
			const rows = applyCaptureReadiness(data.data)
			if (prev.length === 0 || requestOffset === 0) {
				return preservePendingItems(prev, rows)
			}
			const newestIds = new Set(rows.map(log => log.id))
			const tail = prev.filter(log => !newestIds.has(log.id))
			return [...rows, ...tail].slice(0, data.total)
		})
	}

	const handlePageResponse = (data: RequestLogsResponse) => {
		// Appended pages (offset > 0) land below the reading position and only
		// wait for tooltips; a first-page replacement also waits for the top.
		const holdForScroll = data.offset === 0 && !atTopRef.current
		if (openTooltipIdsRef.current.size > 0 || holdForScroll) {
			pendingPageDataRef.current = data
			return
		}
		pendingPageDataRef.current = null
		applyPageData(data)
	}

	const handleNewestResponse = (data: RequestLogsResponse) => {
		if (openTooltipIdsRef.current.size > 0 || !atTopRef.current) {
			pendingNewestDataRef.current = data
			return
		}
		pendingNewestDataRef.current = null
		applyNewestData(data)
	}

	const {
		data: pageData,
		error: logsError,
		isLoading,
		isValidating,
		mutate
	} = useRequestLogs(REQUEST_LOGS_PAGE_SIZE, requestOffset, activeFilters, {
		refreshInterval: 0,
		revalidateOnFocus: automaticUpdatesEnabled,
		revalidateOnReconnect: automaticUpdatesEnabled,
		isPaused: isTooltipPaused
	})
	// SWR serves a cached page synchronously on mount and on key change without
	// calling `onSuccess`, so pages are applied by data identity. Otherwise the
	// first live SSE row would replace the cached page after a revisit.
	const onPageData = useEffectEvent(handlePageResponse)
	useEffect(() => {
		if (pageData) onPageData(pageData)
	}, [pageData])
	const pageSettled = pageData != null || logsError != null
	const { data: providers } = useProviders(undefined, isAdmin && pageSettled)
	const affinityTargetNames = useMemo(() => {
		const names = new Map<string, string>()
		for (const provider of providers ?? []) {
			for (const channel of provider.channels ?? []) {
				const readableNames = [provider.name.trim(), channel.name.trim()].filter(Boolean)
				if (readableNames.length > 0) {
					names.set(`${provider.id}/${channel.id}`, readableNames.join('/'))
				}
			}
		}
		return names
	}, [providers])

	const { connected: sseConnected } = useRequestLogSSE(
		automaticUpdatesEnabled && pageSettled,
		receiveStreamEvent
	)

	const { error: newestError, mutate: mutateNewest } = useRequestLogs(
		REQUEST_LOGS_PAGE_SIZE,
		0,
		activeFilters,
		{
			refreshInterval: automaticUpdatesEnabled && pageSettled && !sseConnected ? 3000 : 0,
			revalidateOnFocus: automaticUpdatesEnabled && pageSettled,
			revalidateOnReconnect: automaticUpdatesEnabled && pageSettled,
			isPaused: isTooltipPaused,
			onSuccess: handleNewestResponse
		}
	)

	const toggleAutomaticUpdates = useCallback(() => {
		if (automaticUpdatesEnabled) {
			setAutomaticUpdatesEnabled(false)
			return
		}

		setAutomaticUpdatesEnabled(true)
		void mutate()
		void mutateNewest()
	}, [automaticUpdatesEnabled, mutate, mutateNewest])

	useEffect(() => {
		streamEventHandlerRef.current = sseEvent => {
			if (!automaticUpdatesEnabled) return

			if (sseEvent.type === 'resync') {
				void mutate()
				void mutateNewest()
				return
			}

			if (sseEvent.type === 'capture_ready') {
				const key = JSON.stringify([sseEvent.user_id, sseEvent.request_id])
				if (!captureReadyRef.current.has(key) && captureReadyRef.current.size >= 4096) {
					captureReadyRef.current.clear()
					void mutate()
					void mutateNewest()
				}
				captureReadyRef.current.add(key)
				flushBuffersRef.current()
				return
			}

			receiveSSELogs(sseEvent.logs)
		}
	}, [automaticUpdatesEnabled, mutate, mutateNewest, receiveSSELogs])

	const prevConnectedRef = useRef(false)
	useEffect(() => {
		if (!automaticUpdatesEnabled) {
			prevConnectedRef.current = false
			return
		}

		if (sseConnected && !prevConnectedRef.current) {
			void mutate()
			void mutateNewest()
		}
		prevConnectedRef.current = sseConnected
	}, [automaticUpdatesEnabled, sseConnected, mutate, mutateNewest])

	// Applies buffered data once no tooltip is open; live rows above the reading
	// position additionally wait until the table is back at the top (FL37e).
	const flushBuffers = () => {
		if (openTooltipIdsRef.current.size > 0) return
		const atTopNow = atTopRef.current

		const bufferedNewest = atTopNow ? pendingNewestDataRef.current : null
		if (bufferedNewest) {
			pendingNewestDataRef.current = null
			applyNewestData(bufferedNewest)
		}

		const bufferedPage = pendingPageDataRef.current
		if (bufferedPage && (bufferedPage.offset !== 0 || atTopNow)) {
			pendingPageDataRef.current = null
			applyPageData(bufferedPage)
		}

		if (atTopNow && pendingSSERef.current.length > 0) {
			const bufferedSSE = pendingSSERef.current
			pendingSSERef.current = []
			prependSSELogs(bufferedSSE)
		}
		if (atTopNow && captureReadyRef.current.size > 0) {
			setLoadedLogs(applyCaptureReadiness)
		}
	}
	useEffect(() => {
		flushBuffersRef.current = flushBuffers
	})

	const visibleLogs = useMemo(() => {
		if (loadedLogs.length > 0) return loadedLogs
		if (requestOffset === 0) return pageData?.data ?? []
		return []
	}, [loadedLogs, pageData, requestOffset])
	const isInitialLoading = isLoading && visibleLogs.length === 0
	const visibleTotal = pageData?.total ?? totalCount
	const hasMore = visibleLogs.length < visibleTotal
	const listError = logsError ?? newestError
	const retryList = () => {
		captureReadyRef.current.clear()
		return Promise.all([mutate(), mutateNewest()])
	}

	const sortedLogs = useMemo(() => {
		const sorted = [...visibleLogs]
		sorted.sort((a, b) => {
			const ta = Date.parse(a.created_at)
			const tb = Date.parse(b.created_at)
			if (ta !== tb) return tb - ta
			return b.id.localeCompare(a.id)
		})
		return sorted
	}, [visibleLogs])

	const handleLoadMore = () => {
		if (!hasMore || isLoading || isValidating) return
		setRequestOffset(visibleLogs.length)
	}

	const handleModelCommit = () => {
		const trimmed = modelInput.trim()
		if (trimmed !== urlModel) updateParams({ model: trimmed || undefined })
	}

	const handleUsernameCommit = () => {
		const trimmed = usernameInput.trim()
		// A selected key belongs to the previous user, so it is cleared with it.
		if (trimmed !== urlUsername) {
			updateParams({ username: trimmed || undefined, api_key_id: undefined })
		}
	}

	const handleTimeRangeChange = (next: TimeRangeValue) => {
		if (next.range) {
			if (next.range === urlRange) {
				setRangeAnchor({ range: next.range, from: relativeRangeStart(next.range) })
			}
			updateParams({ range: next.range, from: undefined, to: undefined })
			return
		}
		updateParams({
			range: undefined,
			from: next.from?.toISOString(),
			to: next.to?.toISOString()
		})
	}

	const panelFilterCount = [
		urlModel,
		urlStatus,
		urlApiKeyId,
		urlUsername,
		urlChannelId,
		urlRange || timeFrom || timeTo
	].filter(Boolean).length
	const hasAnyFilter = panelFilterCount > 0 || urlQ !== '' || searchInput.trim() !== ''

	const clearFilters = () => {
		setSearchInput('')
		setModelInput('')
		setUsernameInput('')
		setSearchParams(
			prev => {
				const next = new URLSearchParams(prev)
				for (const key of FILTER_PARAM_KEYS) next.delete(key)
				return next
			},
			{ replace: true }
		)
	}

	const channelFilterLabel = useMemo(() => {
		if (!urlChannelId) return null
		for (const provider of providers ?? []) {
			const channel = provider.channels?.find(item => item.id === urlChannelId)
			if (channel) return `${provider.name}/${channel.name}`
		}
		const fromLogs = sortedLogs.find(log => log.channel?.id === urlChannelId)
		const names = [fromLogs?.provider?.name, fromLogs?.channel?.name].filter(Boolean)
		return names.length > 0 ? names.join('/') : t('requestLogs.unknownChannel')
	}, [urlChannelId, providers, sortedLogs, t])

	const selectedTokenMissing =
		urlApiKeyId !== '' && !tokenOptions?.some(key => key.id === urlApiKeyId)
	const selectedTokenFallbackName =
		sortedLogs.find(log => log.api_key?.id === urlApiKeyId)?.api_key?.name ||
		t('requestLogs.selectedToken')

	const showingSummary = !!pageData || visibleLogs.length > 0

	return (
		<PageWrapper instant className='flex min-h-0 flex-1 flex-col gap-4'>
			<motion.div initial={false}>
				<PageHeader title={t('requestLogs.title')} description={t('requestLogs.description')} />
			</motion.div>

			<motion.div
				initial={false}
				className='rounded-lg border bg-card px-3 py-1.5 space-y-1.5'
			>
				<div className='flex flex-wrap items-center gap-2'>
					<div className='relative w-full sm:w-80'>
						<Search className='absolute left-3 top-1/2 -translate-y-1/2 h-4 w-4 text-muted-foreground' />
						<Input
							className='pl-10 h-9'
							placeholder={t('requestLogs.searchPlaceholder')}
							aria-label={t('requestLogs.searchPlaceholder')}
							value={searchInput}
							onChange={e => setSearchInput(e.target.value)}
						/>
					</div>
					<div className='flex items-center gap-1'>
						<Button
							type='button'
							variant='outline'
							size='icon'
							className={cn(
								'size-9',
								automaticUpdatesEnabled && 'border-primary/50 bg-primary/10 text-primary'
							)}
							onClick={toggleAutomaticUpdates}
							aria-pressed={automaticUpdatesEnabled}
							aria-label={t(
								automaticUpdatesEnabled ?
									'requestLogs.disableAutomaticUpdates'
								: 'requestLogs.enableAutomaticUpdates'
							)}
							title={t(
								automaticUpdatesEnabled ?
									'requestLogs.disableAutomaticUpdates'
								: 'requestLogs.enableAutomaticUpdates'
							)}
						>
							{automaticUpdatesEnabled ?
								<RadioTower
									data-icon='inline-start'
									className={cn(!sseConnected && 'animate-pulse')}
								/>
							: <WifiOff data-icon='inline-start' />}
						</Button>
						<Button
							type='button'
							variant='outline'
							size='icon'
							className='size-9'
							onClick={() => {
								void retryList()
							}}
							disabled={isValidating}
							title={t('requestLogs.refresh')}
							aria-label={t('requestLogs.refresh')}
						>
							<RefreshCw
								data-icon='inline-start'
								className={cn(isValidating && 'animate-spin')}
							/>
						</Button>
						<Button
							type='button'
							variant='outline'
							size='icon'
							className='size-9'
							onClick={() => setShowIp(prev => !prev)}
							title={showIp ? t('requestLogs.hideIp') : t('requestLogs.showIp')}
							aria-label={showIp ? t('requestLogs.hideIp') : t('requestLogs.showIp')}
							aria-pressed={showIp}
						>
							{showIp ?
								<Eye data-icon='inline-start' />
							: <EyeOff data-icon='inline-start' />}
						</Button>
					</div>
					<Button
						type='button'
						variant='ghost'
						size='sm'
						className='h-9 gap-1 px-2 text-muted-foreground'
						onClick={() => setFiltersExpanded(prev => !prev)}
						aria-expanded={filtersExpanded}
					>
						<span className='text-xs'>
							{panelFilterCount > 0 ?
								t('requestLogs.filtersWithCount', { count: panelFilterCount })
							:	t('requestLogs.filters')}
						</span>
						<motion.span
							animate={{ rotate: filtersExpanded ? 180 : 0 }}
							transition={transitions.fast}
							className='inline-flex'
						>
							<ChevronDown className='h-4 w-4' />
						</motion.span>
					</Button>
					<div className='ml-auto flex items-center gap-3 text-xs text-muted-foreground'>
						{hasAnyFilter && (
							<Button
								type='button'
								variant='ghost'
								size='sm'
								className='h-7 px-2 text-xs'
								onClick={clearFilters}
							>
								{t('requestLogs.clearFilters')}
							</Button>
						)}
						{showingSummary && (
							<span className='font-medium text-foreground'>
								{t('requestLogs.totalCost')}: {formatCost(pageData?.total_charge_nano_usd ?? totalCharge)}
							</span>
						)}
						{showingSummary ?
							t('requestLogs.showing', {
								from: visibleTotal === 0 ? 0 : 1,
								to: Math.min(visibleLogs.length, visibleTotal),
								total: visibleTotal
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
										className='w-[140px]'
										placeholder={t('requestLogs.filterUsernamePlaceholder')}
										aria-label={t('requestLogs.filterUsernamePlaceholder')}
										value={usernameInput}
										onChange={e => setUsernameInput(e.target.value)}
										onBlur={handleUsernameCommit}
										onKeyDown={e => {
											if (e.key === 'Enter') handleUsernameCommit()
										}}
									/>
								)}
								<Input
									className='w-[200px]'
									placeholder={t('requestLogs.filterModelPlaceholder')}
									aria-label={t('requestLogs.filterModelPlaceholder')}
									value={modelInput}
									onChange={e => setModelInput(e.target.value)}
									onBlur={handleModelCommit}
									onKeyDown={e => {
										if (e.key === 'Enter') handleModelCommit()
									}}
								/>
								<Select
									value={tokenSelectNeedsUser && !urlApiKeyId ? '' : urlApiKeyId || 'all'}
									onValueChange={value =>
										updateParams({ api_key_id: value === 'all' ? undefined : value })
									}
									disabled={tokenSelectNeedsUser}
								>
									<SelectTrigger
										className='w-[140px]'
										aria-label={t('requestLogs.filterToken')}
										title={tokenSelectNeedsUser ? t('requestLogs.tokenFilterNeedsUser') : undefined}
									>
										<SelectValue
											placeholder={
												tokenSelectNeedsUser ?
													t('requestLogs.tokenFilterNeedsUser')
												:	t('requestLogs.filterToken')
											}
										/>
									</SelectTrigger>
									<SelectContent>
										<SelectItem value='all'>{t('requestLogs.allTokens')}</SelectItem>
										{selectedTokenMissing && (
											<SelectItem value={urlApiKeyId}>{selectedTokenFallbackName}</SelectItem>
										)}
										{tokenOptions?.map(key => (
											<SelectItem key={key.id} value={key.id}>
												{key.name}
											</SelectItem>
										))}
									</SelectContent>
								</Select>
								<Select
									value={urlStatus || 'all'}
									onValueChange={value =>
										updateParams({ status: value === 'all' ? undefined : value })
									}
								>
									<SelectTrigger className='w-[120px]' aria-label={t('requestLogs.filterStatus')}>
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
										<SelectItem value='client_gone'>
											{t('requestLogs.clientGone')}
										</SelectItem>
										<SelectItem value='error'>{t('requestLogs.error')}</SelectItem>
									</SelectContent>
								</Select>
								<DateRangePicker
									value={timeRangeValue}
									onChange={handleTimeRangeChange}
									language={i18n.language}
									t={t}
								/>
								{channelFilterLabel && (
									<span className='inline-flex h-9 items-center gap-1 rounded-md border border-border bg-background px-3 text-sm'>
										<span className='text-muted-foreground'>{t('requestLogs.channel')}:</span>
										<span className='max-w-48 truncate'>{channelFilterLabel}</span>
										<button
											type='button'
											className='ml-0.5 inline-flex size-5 items-center justify-center rounded-sm text-muted-foreground hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring'
											aria-label={t('requestLogs.clearChannelFilter')}
											onClick={() => updateParams({ channel_id: undefined })}
										>
											<X className='size-3.5' aria-hidden='true' />
										</button>
									</span>
								)}
							</div>
						</motion.div>
					)}
				</AnimatePresence>
			</motion.div>

			{listError != null && visibleLogs.length > 0 && (
				<QueryError stale onRetry={retryList} retrying={isValidating} />
			)}

			<motion.div
				initial={false}
				className='relative min-h-[20rem] flex-1 overflow-hidden rounded-lg border bg-card'
			>
				<RequestLogsTable
					affinityTargetNames={affinityTargetNames}
					emptyTitle={hasAnyFilter ? t('requestLogs.noMatchingLogs') : t('requestLogs.noLogs')}
					error={listError}
					isAdmin={isAdmin}
					isInitialLoading={isInitialLoading}
					logs={sortedLogs}
					onAtTopChange={handleAtTopChange}
					onLoadMore={handleLoadMore}
					onOpenCapture={handleOpenCapture}
					onRetry={retryList}
					onTooltipOpenChange={onTooltipOpenChange}
					retrying={isValidating}
					showIp={showIp}
					t={t}
				/>
			</motion.div>

			<CaptureViewerDialog
				open={captureOpen}
				onOpenChange={setCaptureOpen}
				log={captureTarget}
				isAdmin={isAdmin}
				knownTargets={affinityTargetNames}
			/>
		</PageWrapper>
	)
}
