import { useEffect, useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Eye, EyeOff, RefreshCw, Search } from 'lucide-react'
import { TableVirtuoso } from 'react-virtuoso'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Badge } from '@/components/ui/badge'
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
import { useAuth } from '@/hooks/use-auth'
import { ModelBadge } from '@/components/ModelBadge'
import { cn } from '@/lib/utils'
import type { RequestLog, RequestLogsFilter } from '@/lib/api'
import { PageWrapper, motion, transitions } from '@/components/ui/motion'

const REQUEST_LOGS_PAGE_SIZE = 100

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

	const { data: apiKeys } = useApiKeys()

	const activeFilters = useMemo<RequestLogsFilter>(() => {
		const f: RequestLogsFilter = {}
		if (filters.status) f.status = filters.status
		if (filters.model) f.model = filters.model
		if (filters.api_key_id) f.api_key_id = filters.api_key_id
		if (isAdmin && filters.username) f.username = filters.username
		if (searchInput.trim()) f.search = searchInput.trim()
		return f
	}, [filters, searchInput, isAdmin])

	const filterKey = useMemo(
		() => JSON.stringify(activeFilters),
		[activeFilters]
	)

	useEffect(() => {
		setRequestOffset(0)
		setLoadedLogs([])
		setTotalCount(0)
	}, [filterKey])

	const {
		data: pageData,
		isLoading,
		isValidating,
		mutate
	} = useRequestLogs(REQUEST_LOGS_PAGE_SIZE, requestOffset, activeFilters)
	const { data: newestPageData } = useRequestLogs(
		REQUEST_LOGS_PAGE_SIZE,
		0,
		activeFilters,
		{
			refreshInterval: 2000
		}
	)

	useEffect(() => {
		if (!pageData) return

		setTotalCount(pageData.total)
		setLoadedLogs(prev => {
			if (requestOffset === 0) {
				return pageData.data
			}

			const existingIds = new Set(prev.map(log => log.id))
			const appended = pageData.data.filter(log => !existingIds.has(log.id))
			if (appended.length === 0) return prev
			return [...prev, ...appended]
		})
	}, [pageData, requestOffset])

	useEffect(() => {
		if (!newestPageData) return

		setTotalCount(newestPageData.total)
		setLoadedLogs(prev => {
			if (prev.length === 0 || requestOffset === 0) {
				return newestPageData.data
			}

			const newestIds = new Set(newestPageData.data.map(log => log.id))
			const tail = prev.filter(log => !newestIds.has(log.id))
			const merged = [...newestPageData.data, ...tail]
			return merged.slice(0, newestPageData.total)
		})
	}, [newestPageData, requestOffset])

	const isInitialLoading = isLoading && loadedLogs.length === 0
	const hasMore = loadedLogs.length < totalCount

	const formatCost = (nanoUsd: string | undefined) => {
		if (nanoUsd == null) return '-'
		const cost = Number(nanoUsd) / 1e9
		if (!Number.isFinite(cost)) return '-'
		return new Intl.NumberFormat('en-US', {
			style: 'currency',
			currency: 'USD',
			minimumSignificantDigits: 6,
			maximumSignificantDigits: 6
		}).format(cost)
	}

	const formatCostFullPrecision = (nanoUsd: string | undefined) => {
		if (nanoUsd == null) return '-'
		const cost = Number(nanoUsd) / 1e9
		if (!Number.isFinite(cost)) return '-'
		return new Intl.NumberFormat('en-US', {
			style: 'currency',
			currency: 'USD',
			minimumFractionDigits: 6,
			maximumFractionDigits: 6
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
				className='rounded-lg border bg-card p-3 space-y-2'
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
					<div className='ml-auto text-xs text-muted-foreground'>
						{showingSummary ?
							t('requestLogs.showing', {
								from: totalCount === 0 ? 0 : 1,
								to: Math.min(loadedLogs.length, totalCount),
								total: totalCount
							})
						:	<Skeleton className='h-4 w-24 inline-block' />}
					</div>
				</div>
				<div className='flex flex-wrap items-center gap-2'>
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
					<div className='ml-auto flex items-center gap-1'>
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
						data={loadedLogs}
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
								formatCostFullPrecision={formatCostFullPrecision}
								formatDuration={formatDuration}
								formatTime={formatTime}
								t={t}
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
	formatCostFullPrecision,
	formatDuration,
	formatTime,
	t
}: {
	log: RequestLog
	isAdmin: boolean
	showIp: boolean
	formatCost: (v: string | undefined) => string
	formatCostFullPrecision: (v: string | undefined) => string
	formatDuration: (v: number | null | undefined) => string | null
	formatTime: (v: string) => string
	t: (key: string) => string
}) {
	const isConnectivityTest =
		log.request_kind === 'active_probe_connectivity' && !log.api_key_name
	const duration = formatDuration(log.duration_ms)
	const ttfb = formatDuration(log.ttfb_ms)
	const channelDisplay = log.channel_name?.trim() || log.channel_id || null
	const providerDisplay = log.provider_name?.trim() || log.provider_id || null
	const costDisplay = formatCostFullPrecision(log.charge_nano_usd)
	const usageSnapshot = asObject(log.usage_breakdown_json)
	const usageInput = asObject(usageSnapshot?.input)
	const usageOutput = asObject(usageSnapshot?.output)
	const billingSnapshot = asObject(log.billing_breakdown_json)
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
		if (tokens == null || !rateNano || !chargeNano) return null
		return `${formatTokenCount(tokens)} × ${formatRatePerMillion(rateNano)} = ${formatCost(chargeNano)}`
	}

	const inputDetailRows: Array<[string, string]> = []
	const outputDetailRows: Array<[string, string]> = []

	const inputTotal =
		readTokenCount(usageInput, 'total_tokens') ?? log.prompt_tokens ?? null
	const inputUncached =
		readTokenCount(usageInput, 'uncached_tokens') ??
		Math.max((log.prompt_tokens ?? 0) - (log.cached_tokens ?? 0), 0)
	const inputText = readTokenCount(usageInput, 'text_tokens')
	const inputCached =
		readTokenCount(usageInput, 'cached_tokens') ?? log.cached_tokens ?? null
	const inputCacheCreation = readTokenCount(usageInput, 'cache_creation_tokens')
	const inputAudio = readTokenCount(usageInput, 'audio_tokens')
	const inputImage = readTokenCount(usageInput, 'image_tokens')

	inputDetailRows.push([
		t('requestLogs.totalTokens'),
		formatTokenCount(inputTotal)
	])
	inputDetailRows.push([
		t('requestLogs.uncachedTokens'),
		formatTokenCount(inputUncached)
	])
	if (inputText != null)
		inputDetailRows.push([
			t('requestLogs.textTokens'),
			formatTokenCount(inputText)
		])
	if (inputCached != null)
		inputDetailRows.push([
			t('requestLogs.cachedTokens'),
			formatTokenCount(inputCached)
		])
	if (inputCacheCreation != null)
		inputDetailRows.push([
			t('requestLogs.cacheCreationTokens'),
			formatTokenCount(inputCacheCreation)
		])
	if (inputAudio != null)
		inputDetailRows.push([
			t('requestLogs.audioTokens'),
			formatTokenCount(inputAudio)
		])
	if (inputImage != null)
		inputDetailRows.push([
			t('requestLogs.imageTokens'),
			formatTokenCount(inputImage)
		])

	const outputTotal =
		readTokenCount(usageOutput, 'total_tokens') ?? log.completion_tokens ?? null
	const outputNonReasoning =
		readTokenCount(usageOutput, 'non_reasoning_tokens') ??
		Math.max((log.completion_tokens ?? 0) - (log.reasoning_tokens ?? 0), 0)
	const outputText = readTokenCount(usageOutput, 'text_tokens')
	const outputReasoning =
		readTokenCount(usageOutput, 'reasoning_tokens') ??
		log.reasoning_tokens ??
		null
	const outputAudio = readTokenCount(usageOutput, 'audio_tokens')
	const outputImage = readTokenCount(usageOutput, 'image_tokens')

	outputDetailRows.push([
		t('requestLogs.totalTokens'),
		formatTokenCount(outputTotal)
	])
	outputDetailRows.push([
		t('requestLogs.nonReasoningTokens'),
		formatTokenCount(outputNonReasoning)
	])
	if (outputText != null)
		outputDetailRows.push([
			t('requestLogs.textTokens'),
			formatTokenCount(outputText)
		])
	if (outputReasoning != null)
		outputDetailRows.push([
			t('requestLogs.reasoningTokens'),
			formatTokenCount(outputReasoning)
		])
	if (outputAudio != null)
		outputDetailRows.push([
			t('requestLogs.audioTokens'),
			formatTokenCount(outputAudio)
		])
	if (outputImage != null)
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
	const finalCharge =
		readNanoString(billingSnapshot, 'final_charge_nano') ||
		log.charge_nano_usd ||
		null

	return (
		<>
			<td className='pl-2 pr-2 py-1 whitespace-nowrap text-muted-foreground font-mono align-middle'>
				{formatTime(log.created_at)}
			</td>

			<td className='px-2 py-1 whitespace-nowrap align-middle'>
				{log.request_id ?
					<TooltipProvider delayDuration={200}>
						<Tooltip>
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
											{log.error_http_status != null && (
												<div>
													{t('requestLogs.errorStatus')}:{' '}
													{log.error_http_status}
												</div>
											)}
											{log.error_code && (
												<div>
													{t('requestLogs.errorCode')}: {log.error_code}
												</div>
											)}
											{log.error_message && (
												<div className='break-words whitespace-pre-wrap'>
													{t('requestLogs.errorMessage')}: {log.error_message}
												</div>
											)}
										</>
									)}
									{log.tried_providers_json &&
										log.tried_providers_json.length > 0 && (
											<div className='border-t border-border/50 pt-1 mt-1'>
												<div className='font-medium mb-0.5'>
													{t('requestLogs.triedProviders')}:
												</div>
												{log.tried_providers_json.map(
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
					<Tooltip>
						<TooltipTrigger asChild>
							<span className='cursor-default'>
								<ModelBadge
									model={log.model}
									multiplier={log.provider_multiplier}
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
								{log.provider_id && (
									<div className='flex items-center justify-between gap-3'>
										<span>{t('requestLogs.modelProvider')}</span>
										<span className='font-mono'>{log.provider_id}</span>
									</div>
								)}
								{log.provider_multiplier != null &&
									log.provider_multiplier !== 1 && (
										<div className='flex items-center justify-between gap-3'>
											<span>{t('requestLogs.multiplier')}</span>
											<span className='font-mono'>
												{log.provider_multiplier}x
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
					<Tooltip>
						<TooltipTrigger asChild>
							<span className='inline-flex h-4 items-center max-w-[5rem] truncate cursor-default'>
								{isConnectivityTest ?
									t('requestLogs.connectivityTest')
								:	log.api_key_name || '-'}
							</span>
						</TooltipTrigger>
						<TooltipContent>
							<span className='text-xs'>
								{isConnectivityTest ?
									t('requestLogs.connectivityTest')
								:	log.api_key_name || '-'}
							</span>
						</TooltipContent>
					</Tooltip>
				</TooltipProvider>
			</td>

			{isAdmin && (
				<td className='px-2 py-1 whitespace-nowrap align-middle text-[11px] leading-4 text-muted-foreground'>
					<span className='inline-flex h-4 items-center max-w-[5rem] truncate'>
						{log.username || '-'}
					</span>
				</td>
			)}

			{isAdmin && (
				<td className='px-2 py-1 whitespace-nowrap align-middle text-[11px] leading-4 text-muted-foreground'>
					{providerDisplay ?
						<TooltipProvider delayDuration={200}>
							<Tooltip>
								<TooltipTrigger asChild>
									<span className='inline-flex h-4 items-center cursor-default max-w-[80px] truncate'>
										{providerDisplay}
									</span>
								</TooltipTrigger>
								<TooltipContent>
									<div className='text-xs space-y-0.5'>
										{channelDisplay && <div>Channel: {channelDisplay}</div>}
										{log.upstream_model && log.upstream_model !== log.model && (
											<div>Upstream: {log.upstream_model}</div>
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
							<Tooltip>
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
										{log.duration_ms != null &&
											log.duration_ms > 0 &&
											(log.completion_tokens ?? 0) > 0 && (
												<div className='flex items-center justify-between gap-3'>
													<span>{t('requestLogs.avgTps')}</span>
													<span className='font-mono'>
														{(
															(log.completion_tokens ?? 0) /
															(log.duration_ms / 1000)
														).toFixed(2)}{' '}
														t/s
													</span>
												</div>
											)}
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
					<Tooltip>
						<TooltipTrigger asChild>
							<span className='cursor-default'>{log.prompt_tokens ?? 0}</span>
						</TooltipTrigger>
						<TooltipContent>
							<div className='text-xs space-y-0.5 min-w-[220px]'>
								{inputDetailRows.map(([label, value]) => (
									<div
										key={label}
										className='flex items-center justify-between gap-3'
									>
										<span>{label}</span>
										<span className='font-mono'>{value}</span>
									</div>
								))}
							</div>
						</TooltipContent>
					</Tooltip>
				</TooltipProvider>
			</td>

			<td className='px-2 py-1 text-right whitespace-nowrap font-mono text-muted-foreground align-middle'>
				<TooltipProvider delayDuration={200}>
					<Tooltip>
						<TooltipTrigger asChild>
							<span className='cursor-default'>
								{log.completion_tokens ?? 0}
							</span>
						</TooltipTrigger>
						<TooltipContent>
							<div className='text-xs space-y-0.5 min-w-[220px]'>
								{outputDetailRows.map(([label, value]) => (
									<div
										key={label}
										className='flex items-center justify-between gap-3'
									>
										<span>{label}</span>
										<span className='font-mono'>{value}</span>
									</div>
								))}
							</div>
						</TooltipContent>
					</Tooltip>
				</TooltipProvider>
			</td>

			<td className='px-2 py-1 text-right whitespace-nowrap font-mono align-middle'>
				<TooltipProvider delayDuration={200}>
					<Tooltip>
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
										<span>{t('requestLogs.input')} (uncached)</span>
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
								{outputTextCostDetail && (
									<div className='flex items-center justify-between gap-3'>
										<span>
											{t('requestLogs.output')} (
											{t('requestLogs.nonReasoningTokens')})
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
								{finalCharge && (
									<div className='flex items-center justify-between gap-3'>
										<span>{t('requestLogs.finalCost')}</span>
										<span className='font-mono'>{formatCost(finalCharge)}</span>
									</div>
								)}
								{!billingSnapshot && (
									<div className='text-muted-foreground'>
										{t('requestLogs.detailsUnavailable')}
									</div>
								)}
								<div className='border-t border-muted pt-2 mt-2'>
									<div className='flex items-center justify-between gap-3'>
										<span className='text-xs text-muted-foreground'>Total</span>
										<span className='font-mono text-xs'>
											{formatCostFullPrecision(log.charge_nano_usd)}
										</span>
									</div>
								</div>
							</div>
						</TooltipContent>
					</Tooltip>
				</TooltipProvider>
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
