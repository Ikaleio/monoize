import { useCallback, useEffect, useRef, useState } from 'react'
import { ScanSearch } from 'lucide-react'
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
	formatNanoPerTokenPerMillion,
	isZeroIntegerString,
	normalizeMultiplier
} from '@/lib/exact-decimal'
import {
	asObject,
	billingValueTranslationKey,
	computeTps,
	formatCachePercentage,
	formatCost,
	formatDuration,
	formatTime,
	getDurationMs,
	getTtfbMs,
	readNanoString,
	readNumber,
	readTokenCount,
	readableAffinityTarget,
	retryAttemptRows,
	triedProvidersOf,
	type RetryAttemptRow
} from './utils'

interface LogRowCellsProps {
	affinityTargetNames: ReadonlyMap<string, string>
	log: RequestLog
	isAdmin: boolean
	showIp: boolean
	t: (key: string, options?: Record<string, unknown>) => string
	onOpenCapture: (log: RequestLog) => void
	onTooltipOpenChange: (tooltipId: string, open: boolean) => void
}

export function LogRowCells({
	affinityTargetNames,
	log,
	isAdmin,
	showIp,
	t,
	onOpenCapture,
	onTooltipOpenChange
}: LogRowCellsProps) {
	const rowTooltipIdsRef = useRef<Set<string>>(new Set())
	const tooltipPrefix = log.request_id || log.id
	const [durationTooltipOpen, setDurationTooltipOpen] = useState(false)
	const [costTooltipOpen, setCostTooltipOpen] = useState(false)

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
		const tooltipIds = rowTooltipIdsRef.current
		return () => {
			for (const tooltipId of tooltipIds) {
				onTooltipOpenChange(tooltipId, false)
			}
			tooltipIds.clear()
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
	const handleDurationTooltipOpenChange = (open: boolean) => {
		setDurationTooltipOpen(open)
		durationTooltipOpenChange(open)
	}
	const handleCostTooltipOpenChange = (open: boolean) => {
		setCostTooltipOpen(open)
		costTooltipOpenChange(open)
	}

	const isConnectivityTest =
		log.request_kind === 'active_probe_connectivity' && !log.api_key.name
	const tokenDisplay =
		isConnectivityTest ? t('requestLogs.connectivityTest') : log.api_key.name || '-'
	const durationMs = getDurationMs(log)
	const ttfbMs = getTtfbMs(log)
	const duration = formatDuration(durationMs)
	const ttfb = formatDuration(ttfbMs)
	const computedTps = computeTps(log)
	const channelDisplay = log.channel.name?.trim() || log.channel.id || null
	const providerDisplay = log.provider.name?.trim() || log.provider.id || null
	const affinityHit = log.affinity?.hit === true
	const triedProviders = triedProvidersOf(log)
	const hasTriedProviders = triedProviders.length > 0
	const attemptRows = hasTriedProviders ? retryAttemptRows(log) : []
	const affinityTargetDisplay = readableAffinityTarget(log, affinityTargetNames)
	const costDisplay = formatCost(log.billing.charge_nano_usd)
	const usageSnapshot = asObject(log.usage)
	const usageInput = asObject(usageSnapshot?.input)
	const usageOutput = asObject(usageSnapshot?.output)
	const billingSnapshot = asObject(log.billing.breakdown)
	const billingInput = asObject(billingSnapshot?.input)
	const billingOutput = asObject(billingSnapshot?.output)
	const billingTier = asObject(billingSnapshot?.tier)
	const multiplier = typeof billingSnapshot?.provider_multiplier === 'string' ?
		normalizeMultiplier(billingSnapshot.provider_multiplier)
	: null
	const tokenLineItems = Array.isArray(billingSnapshot?.token_line_items) ?
		billingSnapshot.token_line_items
			.map(asObject)
			.filter((item): item is Record<string, unknown> => item != null)
	:	[]
	const meterLineItems = Array.isArray(billingSnapshot?.meter_line_items) ?
		billingSnapshot.meter_line_items
			.map(asObject)
			.filter((item): item is Record<string, unknown> => item != null)
	:	[]
	const visibleTokenLineItems = tokenLineItems.filter((item) => {
		const charge = readNanoString(item, 'charge_nano')
		const quantity = readNumber(item.quantity)
		return charge !== '0' && quantity !== 0
	})
	const visibleMeterLineItems = meterLineItems.filter((item) => {
		const charge = readNanoString(item, 'charge_nano')
		const quantity = readNumber(item.quantity)
		return charge !== '0' && quantity !== 0
	})
	const hasMatrixLineItems =
		visibleTokenLineItems.length > 0 || visibleMeterLineItems.length > 0
	const contextTier =
		typeof billingTier?.context_tier === 'string' && billingTier.context_tier ?
			billingTier.context_tier
		:	null
	const serviceTier =
		typeof billingTier?.service_tier === 'string' && billingTier.service_tier ?
			billingTier.service_tier
		:	null
	const isEstimatedBilling = billingSnapshot?.estimated === true
	const billingExemptionReason =
		typeof billingSnapshot?.exemption_reason === 'string' ?
			billingSnapshot.exemption_reason
		:	null
	const isAdminUnpricedExemption = billingExemptionReason === 'admin_unpriced_model'

	const formatTokenCount = (value: number | null | undefined) =>
		value == null ? '-' : new Intl.NumberFormat('en-US').format(value)
	const formatRatePerMillion = (nanoPerToken: string | null) => {
		if (!nanoPerToken) return '-'
		const formatted = formatNanoPerTokenPerMillion(nanoPerToken)
		return formatted === '—' ? '-' : `${formatted}/1M`
	}
	const localizeBillingValue = (
		dimension: Parameters<typeof billingValueTranslationKey>[0],
		value: unknown
	) => {
		if (typeof value !== 'string' || !value) return null
		const translationKey = billingValueTranslationKey(dimension, value)
		return translationKey ? t(translationKey) : value
	}
	const formatUnitRate = (nanoPerUnit: string | null, unit: unknown) => {
		const rawUnit = typeof unit === 'string' && unit ? unit : null
		const unitLabel =
			localizeBillingValue('unit', rawUnit) ?? t('requestLogs.billingUnitGeneric')
		return rawUnit === 'token' ?
				formatRatePerMillion(nanoPerUnit)
			:	`${formatCost(nanoPerUnit)}/${unitLabel}`
	}
	const formatRateTimesUsage = (
		tokens: number | null,
		rateNano: string | null,
		chargeNano: string | null
	) => {
		if (tokens == null || !rateNano || !chargeNano || isZeroIntegerString(chargeNano)) {
			return null
		}
		return `${formatTokenCount(tokens)} × ${formatRatePerMillion(rateNano)} = ${formatCost(chargeNano)}`
	}
	const formatLineItemDetail = (item: Record<string, unknown>) => {
		const quantity = readNumber(item.quantity)
		const unitPrice = readNanoString(item, 'unit_price_nano')
		const charge = readNanoString(item, 'charge_nano')
		if (quantity == null || !unitPrice || !charge || isZeroIntegerString(charge)) {
			return null
		}
		return `${formatTokenCount(quantity)} × ${formatUnitRate(unitPrice, item.unit)} = ${formatCost(charge)}`
	}
	const lineItemLabel = (item: Record<string, unknown>) => {
		const usageClass = localizeBillingValue('usageClass', item.usage_class)
		const modality = localizeBillingValue('modality', item.modality)
		const cacheTtl = localizeBillingValue('cacheTtl', item.cache_ttl)
		const itemContextTier = localizeBillingValue('contextTier', item.context_tier)
		const itemServiceTier = localizeBillingValue('serviceTier', item.service_tier)
		const parts = [
			usageClass,
			modality ? `${t('requestLogs.billingModality')}: ${modality}` : null,
			cacheTtl ? `${t('requestLogs.billingCacheTtl')}: ${cacheTtl}` : null,
			itemContextTier ? `${t('requestLogs.contextTier')}: ${itemContextTier}` : null,
			itemServiceTier ? `${t('requestLogs.serviceTier')}: ${itemServiceTier}` : null
		].filter((value): value is string => value != null)
		return parts.join(' / ')
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
	const inputCachePercentage = formatCachePercentage(inputCached, inputTotal)
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

	// FL4a-3: two decimal places, `t/s` unit, no approximation prefix.
	const tpsValue = computedTps ? `${computedTps.value.toFixed(2)} t/s` : null
	const durationBadge =
		duration ?
			<Badge
				variant='secondary'
				className={cn(
					'text-[10px] h-5 px-1 font-mono rounded-full border-0 cursor-default',
					'bg-muted text-muted-foreground'
				)}
			>
				{duration}
			</Badge>
		:	null
	const ttfbBadge =
		ttfb ?
			<Badge
				variant='secondary'
				className='text-[10px] h-5 px-1 font-mono rounded-full border-info-border bg-info-soft text-info-foreground'
			>
				{ttfb}
			</Badge>
		:	null
	const hopCountBadge =
		hasTriedProviders ?
			<Badge
				variant='secondary'
				className='text-[10px] h-5 px-1 font-mono rounded-full border-warning-border bg-warning-soft text-warning-foreground'
			>
				{t('requestLogs.retryHopCount', { count: triedProviders.length })}
			</Badge>
		:	null
	const streamBadge = log.is_stream ?
		<Badge
			variant='secondary'
			className='text-[10px] h-5 px-1 font-mono rounded-full border-info-border bg-info-soft text-info-foreground'
		>
			{t('requestLogs.streamBadge')}
		</Badge>
	:	<Badge
			variant='secondary'
			className='text-[10px] h-5 px-1 font-mono rounded-full border-warning-border bg-warning-soft text-warning-foreground'
		>
			{t('requestLogs.nonStreamBadge')}
		</Badge>
	const timingTooltipContent = (
		<div className='min-w-[150px] space-y-0.5 text-xs'>
			{duration && (
				<div className='flex items-center justify-between gap-3'>
					<span className='text-muted-foreground'>{t('requestLogs.duration')}</span>
					<span className='font-mono'>{duration}</span>
				</div>
			)}
			{ttfb && (
				<div className='flex items-center justify-between gap-3'>
					<span className='text-muted-foreground'>{t('requestLogs.ttfb')}</span>
					<span className='font-mono'>{ttfb}</span>
				</div>
			)}
			{tpsValue && (
				<div className='flex items-center justify-between gap-3'>
					<span className='text-muted-foreground'>{t('requestLogs.tps')}</span>
					<span className='font-mono'>{tpsValue}</span>
				</div>
			)}
			{attemptRows.length > 0 && (
				<div className='border-t border-border/50 pt-1 mt-1'>
					<RetryAttemptList rows={attemptRows} t={t} />
				</div>
			)}
		</div>
	)

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
		log.status === 'success' ? 'bg-success'
		: log.status === 'pending' ? 'bg-info'
		: log.status === 'client_gone' ? 'bg-warning'
		: log.status === 'error' ? 'bg-destructive'
		: 'bg-zinc-400'
	const baseCharge = readNanoString(billingSnapshot, 'base_charge_nano')
	const visibleBaseCharge = baseCharge != null && !isZeroIntegerString(baseCharge) ? baseCharge : null
	const hasBreakdownContent = !!(
		hasMatrixLineItems ||
		contextTier ||
		serviceTier ||
		inputUncachedCostDetail ||
		inputCachedCostDetail ||
		inputCacheCreationCostDetail ||
		outputTextCostDetail ||
		outputReasoningCostDetail ||
		visibleBaseCharge ||
		multiplier != null ||
		isAdminUnpricedExemption ||
		!billingSnapshot
	)

	return (
		<>
			<td className='whitespace-nowrap py-1 pl-2 pr-2 align-middle font-mono text-muted-foreground'>
				<span className='inline-flex w-full flex-col leading-4'>
					<span className='h-4 whitespace-nowrap'>{formatTime(log.created_at)}</span>
					<span className='flex h-6 w-full items-center gap-1'>
						{log.request_id ? (
							<TooltipProvider delayDuration={200}>
								<Tooltip onOpenChange={requestTooltipOpenChange}>
									<TooltipTrigger asChild>
										<span className='inline-flex h-4 cursor-default items-center gap-1 whitespace-nowrap'>
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
										<div className='max-w-[480px] space-y-0.5 text-xs'>
											<div className='font-mono'>{log.request_id}</div>
											{(log.status === 'error' || log.status === 'client_gone') && (
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
														<div className='whitespace-pre-wrap break-words'>
															{t('requestLogs.errorMessage')}: {log.error.message}
														</div>
													)}
												</>
											)}
											{attemptRows.length > 0 && (
												<div className='mt-1 border-t border-border/50 pt-1'>
													<RetryAttemptList rows={attemptRows} t={t} />
												</div>
											)}
										</div>
									</TooltipContent>
								</Tooltip>
							</TooltipProvider>
						) : (
							<span className='h-4 whitespace-nowrap text-muted-foreground/50'>-</span>
						)}
						{log.has_capture === true && log.request_id ?
							<button
								type='button'
								aria-label={t('requestLogs.capture.open')}
								aria-haspopup='dialog'
								title={t('requestLogs.capture.open')}
								onClick={() => onOpenCapture(log)}
								className='ml-auto inline-flex h-6 w-6 shrink-0 items-center justify-center rounded-md border border-border/60 text-muted-foreground transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring'
							>
								<ScanSearch className='h-3.5 w-3.5' />
							</button>
						:	null}
					</span>
				</span>
			</td>

			<td className='px-2 py-1 align-middle whitespace-nowrap'>
				<span className='inline-flex flex-col items-start leading-4'>
					<TooltipProvider delayDuration={200}>
						<Tooltip onOpenChange={modelTooltipOpenChange}>
							<TooltipTrigger asChild>
								<span className='h-5 cursor-default whitespace-nowrap'>
									<ModelBadge
										model={log.model}
										multiplier={log.provider.multiplier}
										showDetails={false}
										truncateModelText={false}
										className='h-5 min-w-max px-1.5 text-[10px]'
									/>
								</span>
							</TooltipTrigger>
							<TooltipContent>
								<div className='min-w-[180px] space-y-0.5 text-xs'>
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
									log.provider.multiplier !== '1' && (
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
					{isAdmin ?
						<TooltipProvider delayDuration={200}>
							<Tooltip onOpenChange={channelTooltipOpenChange}>
								<TooltipTrigger asChild>
									<span className='inline-flex h-4 cursor-default items-center gap-1 whitespace-nowrap text-[10px] text-muted-foreground'>
										<span>{channelDisplay ?? '-'}</span>
										{affinityHit ?
											<Badge
												variant='secondary'
												className='h-4 shrink-0 rounded-full border-info-border bg-info-soft px-1 text-[10px] font-normal text-info-foreground'
											>
												{t('requestLogs.stickySession')}
											</Badge>
										: null}
									</span>
								</TooltipTrigger>
								<TooltipContent>
									<div className='max-w-[480px] space-y-1 text-xs'>
										{attemptRows.length > 0 && (
											<RetryAttemptList rows={attemptRows} t={t} />
										)}
										{providerDisplay && (
											<div>
												{t('requestLogs.modelProvider')}: {providerDisplay}
											</div>
										)}
										{channelDisplay && (
											<div className='flex items-center gap-1'>
												<span>
													{t('requestLogs.channel')}: {channelDisplay}
												</span>
												{log.affinity?.hit === true && (
													<Badge
														variant='secondary'
														className='h-4 rounded-full px-1 text-[10px] font-normal'
													>
														{t('requestLogs.affinityHit')}
													</Badge>
												)}
											</div>
										)}
										{affinityTargetDisplay && (
											<div>
												{t('requestLogs.affinityTarget')}: {affinityTargetDisplay}
											</div>
										)}
										{log.session_affinity_value && (
											<div>
												{t('requestLogs.sessionAffinity')}: {log.session_affinity_value}
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
					: <span aria-hidden='true' className='h-4' />}
				</span>
			</td>

			<td className='min-w-24 whitespace-nowrap px-2 py-1 align-middle leading-4'>
				<span className='inline-flex max-w-24 flex-col items-start'>
					{isAdmin && (
						<span className='h-4 max-w-full truncate text-xs font-medium text-foreground'>
							{log.user.username || '-'}
						</span>
					)}
					<TooltipProvider delayDuration={200}>
						<Tooltip onOpenChange={tokenTooltipOpenChange}>
							<TooltipTrigger asChild>
								<span
									className={
										isAdmin ?
											'h-4 max-w-full cursor-default truncate text-[10px] text-muted-foreground'
										: 'h-4 max-w-full cursor-default truncate text-[11px] text-muted-foreground'
									}
								>
									{tokenDisplay}
								</span>
							</TooltipTrigger>
							<TooltipContent>
								<span className='text-xs'>{tokenDisplay}</span>
							</TooltipContent>
						</Tooltip>
					</TooltipProvider>
				</span>
			</td>

			<td className='px-1 py-1 whitespace-nowrap align-middle'>
				<TooltipProvider delayDuration={200}>
					<Tooltip
						open={durationTooltipOpen}
						onOpenChange={handleDurationTooltipOpenChange}
					>
						<TooltipTrigger asChild>
							<button
								type='button'
								aria-expanded={durationTooltipOpen}
								onClick={() => handleDurationTooltipOpenChange(true)}
								className='inline-flex max-w-full items-center gap-1 overflow-x-auto overflow-y-hidden whitespace-nowrap border-0 bg-transparent p-0 align-middle [scrollbar-width:none] [&::-webkit-scrollbar]:hidden'
							>
								{durationBadge}
								{ttfbBadge}
								{hopCountBadge}
								{streamBadge}
							</button>
						</TooltipTrigger>
						<TooltipContent>{timingTooltipContent}</TooltipContent>
					</Tooltip>
				</TooltipProvider>
			</td>

			<td className='min-w-32 whitespace-nowrap px-2 py-1 text-right align-middle font-mono text-muted-foreground'>
				<TooltipProvider delayDuration={200}>
					<Tooltip onOpenChange={inputTooltipOpenChange}>
						<TooltipTrigger asChild>
							<span
								className={cn(
									'inline-flex cursor-default leading-4',
									inputCached != null && inputCachePercentage ?
										'flex-col items-end'
									: 'items-center'
								)}
							>
								<span className='h-4 whitespace-nowrap text-sm font-medium tabular-nums'>
									{formatTokenCount(inputTokensForDisplay)}
								</span>
								{inputCached != null && inputCachePercentage ?
									<span className='h-4 whitespace-nowrap text-[10px] text-muted-foreground tabular-nums'>
										{formatTokenCount(inputCached)} / {inputCachePercentage}
									</span>
								: null}
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

			<td className='min-w-24 whitespace-nowrap px-2 py-1 text-right align-middle font-mono text-muted-foreground'>
				<TooltipProvider delayDuration={200}>
					<Tooltip onOpenChange={outputTooltipOpenChange}>
						<TooltipTrigger asChild>
							<span className='cursor-default text-sm font-medium tabular-nums'>
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
						<Tooltip
							open={costTooltipOpen}
							onOpenChange={handleCostTooltipOpenChange}
						>
							<TooltipTrigger asChild>
								<button
									type='button'
									className='inline-flex items-center whitespace-nowrap border-0 bg-transparent p-0 align-bottom font-mono cursor-default'
									title={costDisplay}
									aria-expanded={costTooltipOpen}
									onClick={() => handleCostTooltipOpenChange(true)}
								>
									{costDisplay}
								</button>
							</TooltipTrigger>
							<TooltipContent className='max-w-[calc(100vw-1.5rem)] sm:max-w-xl'>
								<div className='w-[32rem] max-w-full space-y-0.5 text-xs'>
								{contextTier && (
									<div className='flex items-center justify-between gap-3'>
										<span>{t('requestLogs.contextTier')}</span>
										<span className='font-mono'>
											{localizeBillingValue('contextTier', contextTier)}
										</span>
									</div>
								)}
								{serviceTier && (
									<div className='flex items-center justify-between gap-3'>
										<span>{t('requestLogs.serviceTier')}</span>
										<span className='font-mono'>
											{localizeBillingValue('serviceTier', serviceTier)}
										</span>
									</div>
									)}
									{visibleTokenLineItems.map((item, index) => {
										const detail = formatLineItemDetail(item)
										if (!detail) return null
										return (
											<div
												key={`token-${index}`}
												className='grid gap-0.5 sm:grid-cols-[minmax(0,1fr)_auto] sm:items-center sm:gap-3'
											>
												<span className='min-w-0 break-words'>
													{lineItemLabel(item)}
												</span>
												<span className='whitespace-nowrap font-mono sm:text-right'>
													{detail}
												</span>
											</div>
										)
									})}
									{visibleMeterLineItems.map((item, index) => {
										const detail = formatLineItemDetail(item)
										if (!detail) return null
										return (
											<div
												key={`meter-${index}`}
												className='grid gap-0.5 sm:grid-cols-[minmax(0,1fr)_auto] sm:items-center sm:gap-3'
											>
												<span className='min-w-0 break-words'>
													{lineItemLabel(item)}
												</span>
												<span className='whitespace-nowrap font-mono sm:text-right'>
													{detail}
												</span>
											</div>
										)
									})}
									{!hasMatrixLineItems && inputUncachedCostDetail && (
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
									{!hasMatrixLineItems && inputCachedCostDetail && (
										<div className='flex items-center justify-between gap-3'>
											<span>
												{t('requestLogs.input')} ({t('requestLogs.cachedTokens')})
											</span>
											<span className='font-mono'>{inputCachedCostDetail}</span>
										</div>
									)}
									{!hasMatrixLineItems && inputCacheCreationCostDetail && (
										<div className='flex items-center justify-between gap-3'>
											<span>
												{t('requestLogs.input')} ({t('requestLogs.cacheCreationTokens')})
											</span>
											<span className='font-mono'>
												{inputCacheCreationCostDetail}
											</span>
										</div>
									)}
									{!hasMatrixLineItems && outputTextCostDetail && (
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
									{!hasMatrixLineItems && outputReasoningCostDetail && (
										<div className='flex items-center justify-between gap-3'>
											<span>
												{t('requestLogs.output')} ({t('requestLogs.reasoningTokens')})
											</span>
											<span className='font-mono'>
												{outputReasoningCostDetail}
											</span>
										</div>
									)}
									{visibleBaseCharge && (
										<div className='flex items-center justify-between gap-3'>
											<span>{t('requestLogs.baseCost')}</span>
											<span className='font-mono'>{formatCost(visibleBaseCharge)}</span>
										</div>
									)}
									{multiplier != null && (
										<div className='flex items-center justify-between gap-3'>
											<span>{t('requestLogs.multiplier')}</span>
											<span className='font-mono'>
												{multiplier}x
											</span>
										</div>
									)}
								{!billingSnapshot && (
									<div className='text-muted-foreground'>
										{t('requestLogs.detailsUnavailable')}
									</div>
								)}
								{isEstimatedBilling && (
									<div className='text-warning text-xs flex items-center gap-1'>
										⚡ {t('requestLogs.estimatedBilling')}
									</div>
								)}
								{isAdminUnpricedExemption && (
									<div className='text-warning text-xs flex items-center gap-1'>
										ℹ {t('requestLogs.adminUnpricedExemption')}
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

function RetryAttemptList({
	rows,
	t
}: {
	rows: RetryAttemptRow[]
	t: (key: string, options?: Record<string, unknown>) => string
}) {
	return (
		<div className='space-y-1'>
			<div className='font-medium'>{t('requestLogs.retryChain')}</div>
			{rows.map((row, index) => (
				<div key={`${row.label}-${index}`} className='break-words'>
					<span className='font-mono'>{row.label}</span>
					{row.durationMs != null && (
						<span className='font-mono text-muted-foreground'>
							{' '}
							{formatDuration(row.durationMs)}
						</span>
					)}
					{row.outcome === 'served' ?
						<span className='text-success'> {t('requestLogs.retryHopServed')}</span>
					:	<>
							{row.upstreamStatus != null && (
								<span className='text-muted-foreground'> {row.upstreamStatus}</span>
							)}
							{row.error && (
								<div className='text-muted-foreground whitespace-pre-wrap'>
									{row.error}
								</div>
							)}
						</>
					}
				</div>
			))}
		</div>
	)
}
