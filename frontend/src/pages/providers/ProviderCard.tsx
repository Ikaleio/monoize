import { useEffect, useId, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import {
	ArrowDown,
	ArrowUp,
	ChevronRight,
	GripVertical,
	Layers,
	ListChecks,
	Loader2,
	Pencil,
	Radio,
	Trash2,
	Zap
} from 'lucide-react'
import { AnimatePresence } from 'framer-motion'
import { Virtuoso, type VirtuosoHandle } from 'react-virtuoso'
import { toast } from 'sonner'
import { mutate } from 'swr'
import { BadgeOverflowList } from '@/components/BadgeOverflowList'
import { StackedModelList } from '@/components/StackedModelList'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader } from '@/components/ui/card'
import { motion, transitions } from '@/components/ui/motion'
import { Separator } from '@/components/ui/separator'
import { StatusBadge } from '@/components/ui/status'
import { Switch } from '@/components/ui/switch'
import {
	Tooltip,
	TooltipContent,
	TooltipProvider,
	TooltipTrigger
} from '@/components/ui/tooltip'
import { api } from '@/lib/api'
import { toastApiError } from '@/lib/api-error'
import { cn } from '@/lib/utils'
import { SWR_KEYS, useDashboardGroups } from '@/lib/swr'
import type {
	ModelMetadataRecord,
	MonoizeChannel,
	Provider
} from '@/lib/api'
import { useChannelTests } from './channel-test-context'
import { useMediaQuery } from './use-media-query'
import {
	ChannelRuntimeStatus,
	ProviderModelRuntimeBadge
} from './ProviderRuntimeStatus'
import {
	PROVIDER_CHANNEL_ROW_HEIGHT,
	PROVIDER_CHANNEL_ROW_HEIGHT_COMPACT,
	PROVIDER_TYPE_CONFIG,
	providerRoutingSummary,
	resolveQuickTestModel
} from './shared'

export type ProviderDragState = 'source' | 'before' | 'after' | null

type ProviderCardProps = {
	provider: Provider
	index: number
	total: number
	onEdit: (provider: Provider) => void
	onDelete: (provider: Provider) => void
	onMove: (from: number, to: number) => void
	onToggle: (provider: Provider, enabled: boolean) => void
	onToggleChannel: (provider: Provider, channel: MonoizeChannel, enabled: boolean) => void
	canDrag: boolean
	dragState: ProviderDragState
	onDragStart: (providerId: string) => void
	onDragOver: (providerId: string) => void
	onDrop: (providerId: string) => void
	onDragEnd: () => void
	modelMetadata: ModelMetadataRecord[]
	globalProbeModel?: string | null
	/** PL12e: a new object expands the card, scrolls to it and highlights `channelId`. */
	focusRequest: { channelId: string | null } | null
}

const providerActionButtonClass = 'size-11 touch-manipulation sm:size-8'

export function ProviderCard({
	provider,
	index,
	total,
	onEdit,
	onDelete,
	onMove,
	onToggle,
	onToggleChannel,
	canDrag,
	dragState,
	onDragStart,
	onDragOver,
	onDrop,
	onDragEnd,
	modelMetadata,
	globalProbeModel,
	focusRequest
}: ProviderCardProps) {
	const { t, i18n } = useTranslation()
	const { openTest } = useChannelTests()
	const numberFormatter = useMemo(
		() => new Intl.NumberFormat(i18n.resolvedLanguage ?? i18n.language, { maximumFractionDigits: 0 }),
		[i18n.resolvedLanguage, i18n.language]
	)
	const contentId = useId()
	const cardRef = useRef<HTMLDivElement>(null)
	const channelListRef = useRef<VirtuosoHandle>(null)
	const [expanded, setExpanded] = useState(false)
	const [highlightChannelId, setHighlightChannelId] = useState<string | null>(null)
	const [quickTestingChannelId, setQuickTestingChannelId] = useState<string | null>(null)
	const isWide = useMediaQuery('(min-width: 640px)')
	const rowHeight = isWide ? PROVIDER_CHANNEL_ROW_HEIGHT : PROVIDER_CHANNEL_ROW_HEIGHT_COMPACT

	useEffect(() => {
		if (!focusRequest) return
		setExpanded(true)
		setHighlightChannelId(focusRequest.channelId)
		const frame = requestAnimationFrame(() => {
			cardRef.current?.scrollIntoView({ block: 'center', behavior: 'smooth' })
			const channelIndex = provider.channels.findIndex(channel => channel.id === focusRequest.channelId)
			if (channelIndex >= 0) channelListRef.current?.scrollToIndex({ index: channelIndex, align: 'center' })
		})
		const timer = window.setTimeout(() => setHighlightChannelId(null), 2000)
		return () => {
			cancelAnimationFrame(frame)
			window.clearTimeout(timer)
		}
		// Only a new request object triggers the deep-link focus.
		// eslint-disable-next-line react-hooks/exhaustive-deps
	}, [focusRequest])

	const modelEntries = useMemo(
		() => Array.from(new Set(provider.channels.flatMap(channel => Object.keys(channel.models ?? {})))).sort(),
		[provider.channels]
	)
	const modelMetadataById = useMemo(
		() => new Map(modelMetadata.map(item => [item.model_id, item])),
		[modelMetadata]
	)
	const unpricedModelIdSet = useMemo(
		() => new Set(provider.unpriced_model_ids ?? []),
		[provider.unpriced_model_ids]
	)
	const modelRuntimeStatusById = useMemo(
		() => new Map((provider.model_runtime_statuses ?? []).map(status => [status.model, status])),
		[provider.model_runtime_statuses]
	)
	const statusCounts = useMemo(() => {
		const statuses = provider.model_runtime_statuses ?? []
		const unpriced = new Set(provider.unpriced_model_ids ?? [])
		for (const status of statuses) {
			if (status.pricing_status !== 'complete') unpriced.add(status.model)
		}
		return {
			unavailable: statuses.filter(status => status.availability_status === 'unavailable').length,
			degraded: statuses.filter(status => status.availability_status === 'degraded').length,
			unpriced: unpriced.size
		}
	}, [provider.model_runtime_statuses, provider.unpriced_model_ids])
	const { data: registryGroups = [] } = useDashboardGroups(provider.group_ids.length > 0)
	const groupNameById = useMemo(
		() => new Map(registryGroups.map(group => [group.id, group.name])),
		[registryGroups]
	)
	const totalWeight = useMemo(
		() => provider.channels.reduce((sum, channel) => sum + (channel.enabled && channel.weight > 0 ? channel.weight : 0), 0),
		[provider.channels]
	)

	const metadataBadges = useMemo(() => {
		const channelTypeLabel = Array.from(new Set(provider.channels.map(channel => PROVIDER_TYPE_CONFIG[channel.provider_type]?.label ?? channel.provider_type)))
			.sort()
			.join(' / ')
		const labels = [
			...(channelTypeLabel ? [{ key: 'channel-types', label: channelTypeLabel }] : []),
			...provider.group_ids.map(groupId => ({ key: `group-${groupId}`, label: groupNameById.get(groupId) ?? groupId }))
		]
		return labels.map(({ key, label }) => {
			const badge = <Badge variant='outline' className='max-w-none text-xs'><span className='whitespace-nowrap'>{label}</span></Badge>
			return { key, collapsed: badge, full: badge }
		})
	}, [groupNameById, provider.channels, provider.group_ids])

	const handleQuickTest = async (channel: MonoizeChannel, model: string) => {
		setQuickTestingChannelId(channel.id)
		try {
			const result = await api.testChannel(provider.id, channel.id, undefined, false)
			if (result.success) {
				toast.success(t('providers.quickTestPassed', { channel: channel.name, ms: result.latency_ms }), { description: result.model ?? model })
			} else {
				toast.error(t('providers.quickTestFailed', { channel: channel.name }), { description: result.error ?? result.model ?? model })
			}
		} catch (error) {
			toastApiError(error, t, t('providers.quickTestFailed', { channel: channel.name }))
		} finally {
			setQuickTestingChannelId(null)
			mutate(SWR_KEYS.PROVIDERS)
		}
	}

	const weightText = (channel: MonoizeChannel) => {
		const models = t('providers.modelCount', { count: Object.keys(channel.models ?? {}).length })
		const share = channel.enabled && channel.weight > 0 && totalWeight > 0
			? Math.round((channel.weight / totalWeight) * 100)
			: null
		const weight = share == null
			? t('providers.weightValue', { weight: channel.weight })
			: t('providers.weightShare', { weight: channel.weight, percent: share })
		return `${models} · ${weight}`
	}

	return (
		<motion.div
			ref={cardRef}
			initial={{ opacity: 0, y: 20 }}
			animate={{ opacity: dragState === 'source' ? 0.5 : 1, y: 0 }}
			transition={{ delay: index * 0.08, ...transitions.normal }}
			className='relative'
			onDragOver={event => {
				if (!canDrag) return
				event.preventDefault()
				onDragOver(provider.id)
			}}
			onDrop={event => {
				if (!canDrag) return
				event.preventDefault()
				onDrop(provider.id)
			}}
		>
			{dragState === 'before' || dragState === 'after' ?
				<div
					aria-hidden='true'
					className={cn('pointer-events-none absolute inset-x-0 z-10 h-0.5 rounded-full bg-primary', dragState === 'before' ? '-top-2' : '-bottom-2')}
				/>
			: 	null}
			<TooltipProvider delayDuration={300}>
			<Card className='transition-shadow hover:shadow-md'>
				<CardHeader className={cn('py-3', expanded && 'pb-4')}>
					<div className='flex flex-col items-stretch gap-3 lg:flex-row lg:items-center'>
						<div className='flex min-w-0 items-start gap-1 lg:flex-1'>
							<Tooltip>
								<TooltipTrigger asChild>
									<button
										type='button'
										draggable={canDrag}
										onDragStart={event => {
											event.dataTransfer.effectAllowed = 'move'
											event.dataTransfer.setData('text/plain', provider.id)
											onDragStart(provider.id)
										}}
										onDragEnd={onDragEnd}
										aria-label={t('providers.dragHandle', { name: provider.name })}
										className={cn(
											'hidden size-8 shrink-0 items-center justify-center rounded-md text-muted-foreground/60 transition-colors hover:text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring sm:flex',
											canDrag ? 'cursor-grab active:cursor-grabbing' : 'cursor-default'
										)}
									>
										<GripVertical className='size-4' aria-hidden='true' />
									</button>
								</TooltipTrigger>
								<TooltipContent>{t('providers.dragHint')}</TooltipContent>
							</Tooltip>
							<div className='min-w-0 flex-1'>
								<div className='flex min-h-8 flex-wrap items-center gap-x-2 gap-y-1'>
									<button
										type='button'
										aria-expanded={expanded}
										aria-controls={expanded ? contentId : undefined}
										onClick={() => setExpanded(value => !value)}
										className='-ml-1 flex min-w-0 items-center gap-2 rounded-sm px-1 py-1 text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring'
									>
										<motion.span
											animate={{ rotate: expanded ? 90 : 0 }}
											transition={{ duration: 0.15 }}
											className='shrink-0'
										>
											<ChevronRight className='size-4 text-muted-foreground' aria-hidden='true' />
										</motion.span>
										<span className='min-w-0 break-words text-base font-semibold leading-normal [overflow-wrap:anywhere]'>
											{provider.name}
										</span>
									</button>
									{!provider.enabled ? <Badge variant='secondary' className='shrink-0'>{t('providers.statusDisabled')}</Badge> : null}
									<div className={cn('flex min-w-0 flex-wrap items-center gap-1.5', !provider.enabled && 'opacity-60')}>
										{statusCounts.unavailable > 0 ?
											<StatusBadge variant='destructive' className='shrink-0'>{t('providers.modelsUnavailable', { count: statusCounts.unavailable })}</StatusBadge>
										: 	null}
										{statusCounts.degraded > 0 ?
											<StatusBadge variant='warning' className='shrink-0'>{t('providers.modelsDegraded', { count: statusCounts.degraded })}</StatusBadge>
										: 	null}
										{statusCounts.unpriced > 0 ?
											<StatusBadge variant='warning' className='shrink-0'>{t('providers.modelsUnpriced', { count: statusCounts.unpriced })}</StatusBadge>
										: 	null}
										<BadgeOverflowList
											items={metadataBadges}
											visibleCount={2}
											ariaLabel={t('providers.metadataBadgesAria', { name: provider.name })}
											contentClassName='max-w-[min(28rem,calc(100vw-2rem))]'
										/>
									</div>
								</div>
								<p className='pl-7 text-xs text-muted-foreground'>
									{providerRoutingSummary(index, provider.max_retries, t)}
								</p>
							</div>
						</div>
						<div className='flex w-full flex-wrap items-center gap-4 lg:w-auto lg:shrink-0 lg:flex-nowrap'>
								<Tooltip>
									<TooltipTrigger asChild>
										<div
											tabIndex={0}
											role='group'
											aria-label={t('providers.liveUsageLabel', { name: provider.name })}
											className={cn(
												'grid w-40 shrink-0 cursor-help grid-cols-[2.5rem_minmax(0,1fr)] gap-x-2 rounded-sm text-left font-mono text-sm font-normal leading-5 text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background',
												!provider.enabled && 'opacity-60'
											)}
										>
											<span>{t('providers.liveRpm')}</span>
											<span className='tabular-nums [overflow-wrap:anywhere]'>
												{provider.live_usage == null ? '—' : numberFormatter.format(provider.live_usage.rpm)}
											</span>
											<span>{t('providers.liveTpm')}</span>
											<span className='tabular-nums [overflow-wrap:anywhere]'>
												{provider.live_usage == null ? '—' : numberFormatter.format(provider.live_usage.tpm)}
											</span>
										</div>
									</TooltipTrigger>
									<TooltipContent align='start' className='max-w-[min(24rem,calc(100vw-2rem))]'>
										{t('providers.liveUsageHelp')}
									</TooltipContent>
								</Tooltip>
								<Switch
									aria-label={t('providers.providerEnabledAria', { name: provider.name })}
									checked={provider.enabled}
									onCheckedChange={value => onToggle(provider, value)}
								/>
								<div className='ml-auto flex items-center gap-0.5 sm:gap-1 lg:ml-0'>
									<Tooltip>
										<TooltipTrigger asChild>
											<Button
												variant='ghost'
												size='icon'
												className={providerActionButtonClass}
												aria-label={t('providers.moveUp')}
												onClick={() => onMove(index, index - 1)}
												disabled={index === 0}
											>
												<ArrowUp />
											</Button>
										</TooltipTrigger>
										<TooltipContent>{t('providers.moveUp')}</TooltipContent>
									</Tooltip>
									<Tooltip>
										<TooltipTrigger asChild>
											<Button
												variant='ghost'
												size='icon'
												className={providerActionButtonClass}
												aria-label={t('providers.moveDown')}
												onClick={() => onMove(index, index + 1)}
												disabled={index === total - 1}
											>
												<ArrowDown />
											</Button>
										</TooltipTrigger>
										<TooltipContent>{t('providers.moveDown')}</TooltipContent>
									</Tooltip>

									<Separator orientation='vertical' className='mx-1 h-7 sm:h-6' />

									<Tooltip>
										<TooltipTrigger asChild>
											<Button
												variant='ghost'
												size='icon'
												className={providerActionButtonClass}
												aria-label={t('common.edit')}
												onClick={() => onEdit(provider)}
											>
												<Pencil />
											</Button>
										</TooltipTrigger>
										<TooltipContent>{t('common.edit')}</TooltipContent>
									</Tooltip>
									<Tooltip>
										<TooltipTrigger asChild>
											<Button
												variant='ghost'
												size='icon'
												className={providerActionButtonClass}
												aria-label={t('common.delete')}
												onClick={() => onDelete(provider)}
											>
												<Trash2 className='text-error-foreground' />
											</Button>
										</TooltipTrigger>
										<TooltipContent>{t('common.delete')}</TooltipContent>
									</Tooltip>
								</div>
						</div>
					</div>
				</CardHeader>
				<AnimatePresence initial={false}>
					{expanded && (
						<motion.div
							id={contentId}
							initial={{ height: 0, opacity: 0 }}
							animate={{ height: 'auto', opacity: 1 }}
							exit={{ height: 0, opacity: 0 }}
							transition={{ duration: 0.2, ease: 'easeInOut' }}
							style={{ overflow: 'hidden' }}
						>
							<CardContent className='space-y-5 pt-2'>
								<div className={cn(!provider.enabled && 'opacity-60')}>
									<div className='mb-3 flex items-center gap-2'>
										<Layers className='size-4 text-muted-foreground' aria-hidden='true' />
										<h4 className='text-sm font-medium'>
											{t('providers.modelsSection')}
										</h4>
										<Badge variant='secondary' className='text-xs'>
											{modelEntries.length}
										</Badge>
									</div>
									<StackedModelList className='mt-1'>
										{modelEntries.map(model => (
											<div key={model} className='min-w-0 max-w-full shrink-0'>
												<ProviderModelRuntimeBadge
													model={model}
													metadata={modelMetadataById.get(model)}
													status={modelRuntimeStatusById.get(model)}
													highlightUnpriced={unpricedModelIdSet.has(model)}
												/>
											</div>
										))}
									</StackedModelList>
								</div>

								<div>
									<div className='mb-3 flex items-center gap-2'>
										<Radio className='size-4 text-muted-foreground' aria-hidden='true' />
										<h4 className='text-sm font-medium'>
											{t('providers.channelsSection')}
										</h4>
										<Badge variant='secondary' className='text-xs'>
											{provider.channels.length}
										</Badge>
									</div>
									<div className='overflow-hidden rounded-lg border'>
										<Virtuoso
											ref={channelListRef}
											style={{
												height: Math.min(provider.channels.length * rowHeight, isWide ? 190 : 300)
											}}
											data={provider.channels}
											computeItemKey={(_index, channel) => channel.id}
											itemContent={(_index, channel) => {
												const models = Object.keys(channel.models ?? {}).sort()
												const quickModel = resolveQuickTestModel(channel, provider, globalProbeModel)
												return (
													<div
														style={{ minHeight: rowHeight }}
														className={cn(
															'flex flex-col justify-center gap-2 border-b px-3 py-2 text-sm transition-colors last:border-b-0 hover:bg-muted/50 sm:grid sm:grid-cols-[6.5rem_minmax(0,11rem)_minmax(0,1fr)_minmax(0,11rem)_auto_auto] sm:items-center sm:gap-3 sm:py-1.5',
															highlightChannelId === channel.id && 'bg-primary/10 ring-1 ring-inset ring-primary'
														)}
													>
														<div className='flex min-w-0 items-center gap-2 sm:contents'>
															<span className='flex shrink-0 items-center'>
																<ChannelRuntimeStatus
																	channel={channel}
																	perModelCircuitBreak={provider.per_model_circuit_break}
																/>
															</span>
															<span className='min-w-0 flex-1 truncate font-medium'>{channel.name}</span>
															{/* Line 1 on narrow screens; pinned to grid column 5 at sm so the weight text keeps line 2's width. */}
															<Switch
																className='shrink-0 sm:col-start-5 sm:row-start-1'
																aria-label={t('providers.channelEnabledAria', { name: channel.name })}
																checked={channel.enabled}
																onCheckedChange={value => onToggleChannel(provider, channel, value)}
															/>
														</div>
														<span className='hidden min-w-0 truncate font-mono text-xs text-muted-foreground sm:block'>
															{channel.base_url}
														</span>
														<div className='flex items-center gap-3 sm:contents'>
															<span className='min-w-0 flex-1 text-xs text-muted-foreground sm:truncate'>
																{weightText(channel)}
															</span>
															<span className='inline-flex h-11 shrink-0 items-center overflow-hidden rounded-md border sm:h-7'>
																<Tooltip>
																	<TooltipTrigger asChild>
																		<button
																			type='button'
																			className='flex h-full cursor-pointer items-center gap-1.5 border-r px-2.5 text-xs font-medium transition-colors hover:bg-muted/80 disabled:pointer-events-none disabled:opacity-50'
																			disabled={quickTestingChannelId === channel.id || !quickModel}
																			onClick={() => quickModel && handleQuickTest(channel, quickModel)}
																		>
																			{quickTestingChannelId === channel.id ?
																				<Loader2 className='size-3 animate-spin' aria-hidden='true' />
																			: 	<Zap className='size-3' aria-hidden='true' />}
																			{t('providers.quickTest')}
																		</button>
																	</TooltipTrigger>
																	<TooltipContent>
																		{quickModel ? t('providers.quickTestModel', { model: quickModel }) : t('providers.quickTestNoModel')}
																	</TooltipContent>
																</Tooltip>
																<Tooltip>
																	<TooltipTrigger asChild>
																		<button
																			type='button'
																			className='flex h-full w-11 cursor-pointer items-center justify-center transition-colors hover:bg-muted/80 sm:w-7'
																			aria-label={t('providers.testSelectModels')}
																			onClick={() => openTest({
																				providerId: provider.id,
																				providerName: provider.name,
																				channelId: channel.id,
																				channelName: channel.name,
																				providerType: channel.provider_type,
																				models
																			})}
																		>
																			<ListChecks className='size-3.5 text-muted-foreground' aria-hidden='true' />
																		</button>
																	</TooltipTrigger>
																	<TooltipContent>{t('providers.testSelectModels')}</TooltipContent>
																</Tooltip>
															</span>
														</div>
													</div>
												)
											}}
										/>
									</div>
								</div>
							</CardContent>
						</motion.div>
					)}
				</AnimatePresence>
			</Card>
			</TooltipProvider>
		</motion.div>
	)
}
