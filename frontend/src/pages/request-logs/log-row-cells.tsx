import { useCallback, useEffect, useRef } from 'react'
import { Badge } from '@/components/ui/badge'
import {
	Tooltip,
	TooltipContent,
	TooltipProvider,
	TooltipTrigger
} from '@/components/ui/tooltip'
import { ModelBadge } from '@/components/ModelBadge'
import { cn } from '@/lib/utils'
import type { RequestLog } from '@/lib/api'
import {
	asObject,
	formatCost,
	formatDuration,
	formatTime,
	getDurationMs,
	getTtfbMs,
	readNanoString,
	readNumber,
	readTokenCount
} from './utils'

interface LogRowCellsProps {
	log: RequestLog
	isAdmin: boolean
	showIp: boolean
	t: (key: string) => string
	onTooltipOpenChange: (tooltipId: string, open: boolean) => void
}

export function LogRowCells({
	log,
	isAdmin,
	showIp,
	t,
	onTooltipOpenChange
}: LogRowCellsProps) {
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
	const isEstimatedBilling = billingSnapshot?.estimated === true

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
		if (tokens == null || !rateNano || !chargeNano || Number(chargeNano) === 0) {
			return null
		}
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

	const hasInputBreakdown = !!(
		inputCached ||
		inputCacheCreation ||
		inputText ||
		inputAudio ||
		inputImage
	)

	if (inputTotal) {
		inputDetailRows.push([
			t('requestLogs.totalTokens'),
			formatTokenCount(inputTotal)
		])
	}
	if (hasInputBreakdown && inputUncached) {
		inputDetailRows.push([
			t('requestLogs.uncachedTokens'),
			formatTokenCount(inputUncached)
		])
	}
	if (inputText) {
		inputDetailRows.push([t('requestLogs.textTokens'), formatTokenCount(inputText)])
	}
	if (inputCached) {
		inputDetailRows.push([
			t('requestLogs.cachedTokens'),
			formatTokenCount(inputCached)
		])
	}
	if (inputCacheCreation) {
		inputDetailRows.push([
			t('requestLogs.cacheCreationTokens'),
			formatTokenCount(inputCacheCreation)
		])
	}
	if (inputAudio) {
		inputDetailRows.push([t('requestLogs.audioTokens'), formatTokenCount(inputAudio)])
	}
	if (inputImage) {
		inputDetailRows.push([t('requestLogs.imageTokens'), formatTokenCount(inputImage)])
	}

	const outputTotal =
		readTokenCount(usageOutput, 'total_tokens') ?? log.tokens.output ?? null
	const outputUsageUnavailable = outputTotal == null
	const outputNonReasoning =
		readTokenCount(usageOutput, 'non_reasoning_tokens') ??
		Math.max((log.tokens.output ?? 0) - (log.tokens.reasoning ?? 0), 0)
	const outputText = readTokenCount(usageOutput, 'text_tokens')
	const outputReasoning =
		readTokenCount(usageOutput, 'reasoning_tokens') ?? log.tokens.reasoning ?? null
	const inputTokensForDisplay = inputTotal ?? null
	const outputTokensForDisplay = outputTotal ?? null
	const outputAudio = readTokenCount(usageOutput, 'audio_tokens')
	const outputImage = readTokenCount(usageOutput, 'image_tokens')

	const hasOutputBreakdown = !!(
		outputReasoning ||
		outputText ||
		outputAudio ||
		outputImage
	)

	if (outputTotal) {
		outputDetailRows.push([
			t('requestLogs.totalTokens'),
			formatTokenCount(outputTotal)
		])
	}
	if (hasOutputBreakdown && outputNonReasoning) {
		outputDetailRows.push([
			t('requestLogs.nonReasoningTokens'),
			formatTokenCount(outputNonReasoning)
		])
	}
	if (outputText) {
		outputDetailRows.push([t('requestLogs.textTokens'), formatTokenCount(outputText)])
	}
	if (outputReasoning) {
		outputDetailRows.push([
			t('requestLogs.reasoningTokens'),
			formatTokenCount(outputReasoning)
		])
	}
	if (outputAudio) {
		outputDetailRows.push([t('requestLogs.audioTokens'), formatTokenCount(outputAudio)])
	}
	if (outputImage) {
		outputDetailRows.push([t('requestLogs.imageTokens'), formatTokenCount(outputImage)])
	}

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
	const hasBreakdownContent = !!(
		inputUncachedCostDetail ||
		inputCachedCostDetail ||
		inputCacheCreationCostDetail ||
		outputTextCostDetail ||
		outputReasoningCostDetail ||
		baseCharge ||
		multiplier != null ||
		!billingSnapshot
	)

	return (
		<>
			<td className='pl-2 pr-2 py-1 whitespace-nowrap text-muted-foreground font-mono align-middle'>
				{formatTime(log.created_at)}
			</td>

			<td className='px-2 py-1 whitespace-nowrap align-middle'>
				{log.request_id ? (
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
													{t('requestLogs.errorStatus')}: {log.error.http_status}
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
									{log.tried_providers && log.tried_providers.length > 0 && (
										<div className='border-t border-border/50 pt-1 mt-1'>
											<div className='font-medium mb-0.5'>
												{t('requestLogs.triedProviders')}:
											</div>
											{log.tried_providers.map((tp, i) => (
												<div
													key={i}
													className='text-muted-foreground break-words'
												>
													{tp.provider_id}/{tp.channel_id}: {tp.error}
												</div>
											))}
										</div>
									)}
								</div>
							</TooltipContent>
						</Tooltip>
					</TooltipProvider>
				) : (
					<span className='text-muted-foreground/50'>-</span>
				)}
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
								: log.api_key.name || '-'}
							</span>
						</TooltipTrigger>
						<TooltipContent>
							<span className='text-xs'>
								{isConnectivityTest ?
									t('requestLogs.connectivityTest')
								: log.api_key.name || '-'}
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
					{providerDisplay ? (
						<TooltipProvider delayDuration={200}>
							<Tooltip onOpenChange={channelTooltipOpenChange}>
								<TooltipTrigger asChild>
									<span className='inline-flex h-4 items-center cursor-default max-w-[80px] truncate'>
										{providerDisplay}
									</span>
								</TooltipTrigger>
								<TooltipContent>
									<div className='text-xs space-y-0.5'>
										{channelDisplay && (
											<div>
												{t('requestLogs.channel')}: {channelDisplay}
											</div>
										)}
										{log.upstream_model && log.upstream_model !== log.model && (
											<div>
												{t('requestLogs.upstreamModel')}: {log.upstream_model}
											</div>
										)}
									</div>
								</TooltipContent>
							</Tooltip>
						</TooltipProvider>
					) : (
						<span className='inline-flex h-4 items-center text-muted-foreground/50'>
							-
						</span>
					)}
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
											outputTotal > 0 &&
											(() => {
												const generationMs =
													ttfbMs != null && durationMs > ttfbMs ?
														durationMs - ttfbMs
													: durationMs
												const tpsValue = outputTotal / (generationMs / 1000)
												return (
													<div className='flex items-center justify-between gap-3'>
														<span>{t('requestLogs.avgTps')}</span>
														<span className='font-mono'>
															{tpsValue.toFixed(2)} t/s
														</span>
													</div>
												)
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
					{log.is_stream ? (
						<Badge
							variant='secondary'
							className='text-[10px] h-5 px-1 font-mono rounded-full border-0 bg-indigo-500/15 text-indigo-700 dark:text-indigo-400'
						>
							{t('requestLogs.streamBadge')}
						</Badge>
					) : (
						<Badge
							variant='secondary'
							className='text-[10px] h-5 px-1 font-mono rounded-full border-0 bg-amber-500/15 text-amber-700 dark:text-amber-400'
						>
							{t('requestLogs.nonStreamBadge')}
						</Badge>
					)}
				</div>
			</td>

			<td className='px-2 py-1 text-right whitespace-nowrap font-mono text-muted-foreground align-middle'>
				<TooltipProvider delayDuration={200}>
					<Tooltip onOpenChange={inputTooltipOpenChange}>
						<TooltipTrigger asChild>
							<span className='cursor-default'>
								{formatTokenCount(inputTokensForDisplay)}
							</span>
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
											<span>
												{t('requestLogs.input')}
												{inputCachedCostDetail ?
													` (${t('requestLogs.uncachedTokens')})`
												: ''}
											</span>
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
											<span className='font-mono'>
												{inputCacheCreationCostDetail}
											</span>
										</div>
									)}
									{outputTextCostDetail && (
										<div className='flex items-center justify-between gap-3'>
											<span>
												{t('requestLogs.output')}
												{outputReasoningCostDetail ?
													` (${t('requestLogs.nonReasoningTokens')})`
												: ''}
											</span>
											<span className='font-mono'>{outputTextCostDetail}</span>
										</div>
									)}
									{outputReasoningCostDetail && (
										<div className='flex items-center justify-between gap-3'>
											<span>
												{t('requestLogs.output')} ({t('requestLogs.reasoningTokens')})
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
											<span className='font-mono'>
												{multiplier.toFixed(6)}x
											</span>
										</div>
									)}
								{!billingSnapshot && (
									<div className='text-muted-foreground'>
										{t('requestLogs.detailsUnavailable')}
									</div>
								)}
								{isEstimatedBilling && (
									<div className='text-amber-500 text-xs flex items-center gap-1'>
										⚡ {t('requestLogs.estimatedBilling')}
									</div>
								)}
									<div className='border-t border-muted pt-2 mt-2'>
										<div className='flex items-center justify-between gap-3'>
											<span className='text-xs text-muted-foreground'>
												{t('requestLogs.totalCost')}
											</span>
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
