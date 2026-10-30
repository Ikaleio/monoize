import { useEffect, useId, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import {
	AlertTriangle,
	ArrowDown,
	ArrowUp,
	ChevronRight,
	GripVertical,
	Layers,
	Loader2,
	Pencil,
	Radio,
	Server,
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
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
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
import {
	ChannelRuntimeStatus,
	ProviderModelRuntimeBadge
} from './ProviderRuntimeStatus'
import {
	PROVIDER_CHANNEL_OVERVIEW_ROW_HEIGHT,
	PROVIDER_TYPE_CONFIG,
	providerRoutingSummary,
	resolveQuickTestModel
} from './shared'

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
	onDragStart: (providerId: string) => void
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
	onDragStart,
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
	const titleId = useId()
	const cardRef = useRef<HTMLDivElement>(null)
	const channelListRef = useRef<VirtuosoHandle>(null)
	const [expanded, setExpanded] = useState(false)
	const [highlightChannelId, setHighlightChannelId] = useState<string | null>(null)
	const [quickTestingChannelId, setQuickTestingChannelId] = useState<string | null>(null)

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
	const attentionModelCount = useMemo(
		() =>
			(provider.model_runtime_statuses ?? []).filter(
				status =>
					status.availability_status !== 'healthy' ||
					status.pricing_status !== 'complete'
			).length || unpricedModelIdSet.size,
		[provider.model_runtime_statuses, unpricedModelIdSet]
	)
	const unavailableModelCount = useMemo(
		() =>
			(provider.model_runtime_statuses ?? []).filter(
				status => status.availability_status === 'unavailable'
			).length,
		[provider.model_runtime_statuses]
	)
	const { data: registryGroups = [] } = useDashboardGroups(provider.group_ids.length > 0)
	const groupNameById = useMemo(
		() => new Map(registryGroups.map(group => [group.id, group.name])),
		[registryGroups]
	)
	const totalWeight = useMemo(
		() => provider.channels.reduce((sum, channel) => sum + (channel.enabled && channel.weight > 0 ? channel.weight : 0), 0),
		[provider.channels]
	)

	const headerBadges = useMemo(() => {
		const channelTypeLabel = Array.from(new Set(provider.channels.map(channel => PROVIDER_TYPE_CONFIG[channel.provider_type]?.label ?? channel.provider_type)))
			.sort()
			.join(' / ')
		const outlineItem = (key: string, label: string) => ({
			key,
			collapsed: (
				<Badge variant='outline' className='max-w-[10rem] text-xs'>
					<span className='truncate'>{label}</span>
				</Badge>
			),
			full: (
				<Badge variant='outline' className='max-w-none text-xs'>
					<span className='whitespace-nowrap'>{label}</span>
				</Badge>
			)
		})
		const attentionVariant = unavailableModelCount > 0 ? 'destructive' : 'warning'
		return [
			...(channelTypeLabel ? [outlineItem('channel-types', channelTypeLabel)] : []),
			...(attentionModelCount > 0 ?
				[{
					key: 'model-attention',
					collapsed: (
						<StatusBadge variant={attentionVariant}>
							<AlertTriangle className='h-3 w-3 mr-1 shrink-0' />
							{t('providers.modelsRequireAttention', { count: attentionModelCount })}
						</StatusBadge>
					),
					full: (
						<StatusBadge variant={attentionVariant}>
							{t('providers.modelAttentionSummary', {
								count: attentionModelCount,
								unavailable: unavailableModelCount
							})}
						</StatusBadge>
					)
				}]
			: 	[]),
			...provider.group_ids.map(groupId => outlineItem(`group-${groupId}`, groupNameById.get(groupId) ?? `${groupId.slice(0, 8)}…`))
		]
	}, [attentionModelCount, groupNameById, provider.channels, provider.group_ids, t, unavailableModelCount])

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
			animate={{ opacity: 1, y: 0 }}
			transition={{ delay: index * 0.08, ...transitions.normal }}
			whileHover={canDrag ? { y: -2, transition: { duration: 0.2 } } : undefined}
		>
			<TooltipProvider delayDuration={300}>
			<Card
				className='transition-shadow hover:shadow-md'
				onDragOver={event => {
					if (canDrag) event.preventDefault()
				}}
				onDrop={event => {
					if (!canDrag) return
					event.preventDefault()
					onDrop(provider.id)
				}}
			>
				<CardHeader className={cn('py-3', expanded && 'pb-4')}>
					<div className='flex flex-col items-stretch justify-between gap-3 lg:flex-row lg:items-center'>
						<div className='flex min-w-0 items-center gap-3 lg:flex-1'>
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
											'shrink-0 rounded-sm text-muted-foreground/50 transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring',
											canDrag ? 'cursor-grab hover:text-muted-foreground' : 'hidden sm:block'
										)}
									>
										<GripVertical className='h-4 w-4' aria-hidden='true' />
									</button>
								</TooltipTrigger>
								<TooltipContent>{t('providers.dragHint')}</TooltipContent>
							</Tooltip>
							<button
								type='button'
								aria-labelledby={titleId}
								aria-expanded={expanded}
								aria-controls={expanded ? contentId : undefined}
								onClick={() => setExpanded(value => !value)}
								className='flex shrink-0 items-center gap-3 rounded-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring'
							>
								<motion.span
									animate={{ rotate: expanded ? 90 : 0 }}
									transition={{ duration: 0.15 }}
									className='shrink-0'
								>
									<ChevronRight className='h-4 w-4 text-muted-foreground' aria-hidden='true' />
								</motion.span>
								<span className='flex h-8 w-8 items-center justify-center rounded-lg bg-secondary shrink-0'>
									<Server className='h-4 w-4' aria-hidden='true' />
								</span>
							</button>
							<div className='flex items-center gap-2 min-w-0 flex-wrap'>
								<CardTitle
									id={titleId}
									onClick={() => setExpanded(value => !value)}
									className='min-w-0 cursor-pointer break-words text-base leading-normal [overflow-wrap:anywhere]'
								>
									{provider.name}
								</CardTitle>
								{!provider.enabled ? <Badge variant='secondary'>{t('providers.statusDisabled')}</Badge> : null}
								<BadgeOverflowList
									items={headerBadges}
									visibleCount={3}
									ariaLabel={t('providers.metadataBadgesAria', { name: provider.name })}
									contentClassName='max-w-[min(28rem,calc(100vw-2rem))]'
								/>
								<span className='hidden lg:inline text-xs text-muted-foreground whitespace-nowrap'>
									[{providerRoutingSummary(index, provider.max_retries, t)}]
								</span>
							</div>
						</div>
						<div className='flex w-full flex-wrap items-center gap-4 lg:w-auto lg:shrink-0 lg:flex-nowrap'>
								<Tooltip>
									<TooltipTrigger asChild>
										<div
											tabIndex={0}
											role='group'
											aria-label={t('providers.liveUsageLabel', { name: provider.name })}
											className='grid w-40 shrink-0 cursor-help grid-cols-[2.5rem_minmax(0,1fr)] gap-x-2 rounded-sm text-left font-mono text-sm font-normal leading-5 text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background'
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
								<div className='hidden md:flex items-center gap-2'>
									<Switch
										aria-label={t('providers.providerEnabledAria', { name: provider.name })}
										checked={provider.enabled}
										onCheckedChange={value => onToggle(provider, value)}
									/>
								</div>
								<div className='flex items-center gap-0.5 sm:gap-1'>
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

									<Separator orientation='vertical' className='h-7 mx-1 sm:h-6' />

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
					<div className='md:hidden mt-2 flex items-center justify-between gap-2 text-sm text-muted-foreground'>
						<span>[{providerRoutingSummary(index, provider.max_retries, t)}]</span>
						<div className='flex items-center gap-2'>
							<Switch
								aria-label={t('providers.providerEnabledAria', { name: provider.name })}
								checked={provider.enabled}
								onCheckedChange={value => onToggle(provider, value)}
							/>
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
								<div>
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
												height: Math.min(
													provider.channels.length * PROVIDER_CHANNEL_OVERVIEW_ROW_HEIGHT,
													190
												)
											}}
											data={provider.channels}
											computeItemKey={(_index, channel) => channel.id}
											itemContent={(_index, channel) => {
												const models = Object.keys(channel.models ?? {}).sort()
												const quickModel = resolveQuickTestModel(channel, provider, globalProbeModel)
												return (
													<div
														className={cn(
															'flex min-h-10 items-center gap-3 px-3 py-1.5 text-sm hover:bg-muted/50 transition-colors border-b last:border-b-0',
															highlightChannelId === channel.id && 'bg-muted/50'
														)}
													>
														<span className='flex min-w-0 items-center gap-3'>
															<ChannelRuntimeStatus
																channel={channel}
																perModelCircuitBreak={provider.per_model_circuit_break}
															/>
															<span className='min-w-0 truncate font-medium'>
																{channel.name}
															</span>
															<span className='hidden min-w-0 truncate font-mono text-xs text-muted-foreground sm:inline'>
																{channel.base_url}
															</span>
														</span>
														<span className='ml-auto flex items-center gap-3 shrink-0'>
															<span className='inline-flex items-center rounded-md border overflow-hidden h-7'>
																<Tooltip>
																	<TooltipTrigger asChild>
																		<button
																			type='button'
																			className='flex items-center gap-1.5 px-2.5 h-full text-xs font-medium hover:bg-muted/80 transition-colors disabled:opacity-50 disabled:pointer-events-none border-r cursor-pointer'
																			disabled={quickTestingChannelId === channel.id || !quickModel}
																			onClick={() => quickModel && handleQuickTest(channel, quickModel)}
																		>
																			{quickTestingChannelId === channel.id ?
																				<Loader2 className='h-3 w-3 animate-spin' aria-hidden='true' />
																			: 	<Zap className='h-3 w-3' aria-hidden='true' />}
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
																			className='flex items-center justify-center w-7 h-full hover:bg-muted/80 transition-colors cursor-pointer'
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
																			<ChevronRight className='h-3.5 w-3.5 text-muted-foreground' aria-hidden='true' />
																		</button>
																	</TooltipTrigger>
																	<TooltipContent>{t('providers.testSelectModels')}</TooltipContent>
																</Tooltip>
															</span>
															<span className='text-xs text-muted-foreground'>
																{weightText(channel)}
															</span>
															<Switch
																aria-label={t('providers.channelEnabledAria', { name: channel.name })}
																checked={channel.enabled}
																onCheckedChange={value => onToggleChannel(provider, channel, value)}
															/>
														</span>
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
