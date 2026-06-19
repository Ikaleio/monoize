import { useEffect, useMemo, useState } from 'react'
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
import { toast } from 'sonner'
import { mutate } from 'swr'
import { ModelBadge } from '@/components/ModelBadge'
import { BadgeOverflowList } from '@/components/BadgeOverflowList'
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
import { cn } from '@/lib/utils'
import { SWR_KEYS } from '@/lib/swr'
import { Virtuoso } from 'react-virtuoso'
import type { ChannelTestResult, ModelMetadataRecord, Provider } from '@/lib/api'
import { ChannelTestDialog } from './ChannelTestDialog'
import {
	buildPricedModelIdSet,
	hasBillablePricingModelId,
	PROVIDER_CHANNEL_OVERVIEW_ROW_HEIGHT,
	PROVIDER_TYPE_CONFIG,
	statusBadge
} from './shared'

type ProviderCardProps = {
	provider: Provider
	index: number
	total: number
	onEdit: (provider: Provider) => void
	onDelete: (provider: Provider) => void
	onMove: (from: number, to: number) => void
	onToggle: (provider: Provider, enabled: boolean) => void
	onDragStart: (providerId: string) => void
	onDrop: (targetProviderId: string) => void
	modelMetadata: ModelMetadataRecord[]
	reasoningSuffixMap: Record<string, string>
}

const providerActionButtonClass = 'size-11 touch-manipulation sm:size-8'

function useFinePointer() {
	const [isFinePointer, setIsFinePointer] = useState(() =>
		typeof window === 'undefined' ? false : window.matchMedia('(pointer: fine)').matches
	)

	useEffect(() => {
		const media = window.matchMedia('(pointer: fine)')
		const syncPointerState = () => setIsFinePointer(media.matches)
		syncPointerState()
		media.addEventListener('change', syncPointerState)
		return () => media.removeEventListener('change', syncPointerState)
	}, [])

	return isFinePointer
}

export function ProviderCard({
	provider,
	index,
	total,
	onEdit,
	onDelete,
	onMove,
	onToggle,
	onDragStart,
	onDrop,
	modelMetadata,
	reasoningSuffixMap
}: ProviderCardProps) {
	const { t } = useTranslation()
	const [expanded, setExpanded] = useState(false)
	const [testDialogOpen, setTestDialogOpen] = useState(false)
	const [testDialogChannel, setTestDialogChannel] = useState<{
		id: string
		name: string
		models: string[]
	} | null>(null)
	const [quickTestingChannelId, setQuickTestingChannelId] = useState<string | null>(
		null
	)
	const canDragCard = useFinePointer()
	const modelEntries = useMemo(
		() => Object.entries(provider.models).sort(([a], [b]) => a.localeCompare(b)),
		[provider.models]
	)
	const modelMetadataById = useMemo(() => {
		const map = new Map<string, ModelMetadataRecord>()
		for (const item of modelMetadata) {
			map.set(item.model_id, item)
		}
		return map
	}, [modelMetadata])
	const pricedModelIdSet = useMemo(
		() => buildPricedModelIdSet(modelMetadata),
		[modelMetadata]
	)

	const unpricedCount = provider.unpriced_model_count ?? 0

	const channelTypeLabelEntries = useMemo(() => {
		const types = Array.from(new Set(provider.channels.map(channel => channel.provider_type)))
		return types
			.map(type => ({
				key: type,
				label: PROVIDER_TYPE_CONFIG[type]?.label ?? type
			}))
			.sort((a, b) => a.label.localeCompare(b.label))
	}, [provider.channels])

	const headerBadgeItems = useMemo(() => {
		const channelTypeLabel = channelTypeLabelEntries
			.map(entry => entry.label)
			.sort()
			.join(' / ')
		const items = []

		if (channelTypeLabel) {
			items.push({
				key: 'channel-types',
				collapsed: (
					<Badge variant='outline' className='max-w-[10rem] text-xs'>
						<span className='truncate'>{channelTypeLabel}</span>
					</Badge>
				),
				full: (
					<Badge variant='outline' className='max-w-none text-xs'>
						<span className='whitespace-nowrap'>{channelTypeLabel}</span>
					</Badge>
				)
			})
		}

		items.push({
			key: 'enabled-state',
			collapsed: (
				<StatusBadge variant={provider.enabled ? 'success' : 'info'}>
					{provider.enabled ? t('common.enabled') : t('common.disabled')}
				</StatusBadge>
			)
		})

		if (unpricedCount > 0) {
			items.push({
				key: 'unpriced',
				collapsed: (
					<StatusBadge variant='warning'>
						<AlertTriangle className='h-3 w-3 mr-1 shrink-0' />
						{t('providers.unpricedModels', { count: unpricedCount })}
					</StatusBadge>
				)
			})
		}

		for (const group of provider.groups) {
			items.push({
				key: `group-${group}`,
				collapsed: (
					<Badge variant='outline' className='max-w-[10rem] font-mono text-xs'>
						<span className='truncate'>{group}</span>
					</Badge>
				),
				full: (
					<Badge variant='outline' className='max-w-none font-mono text-xs'>
						<span className='whitespace-nowrap'>{group}</span>
					</Badge>
				)
			})
		}

		return items
	}, [channelTypeLabelEntries, provider.enabled, provider.groups, t, unpricedCount])

	const handleQuickTest = async (channelId: string) => {
		setQuickTestingChannelId(channelId)
		try {
			const result: ChannelTestResult = await api.testChannel(provider.id, channelId)
			if (result.success) {
				toast.success(
					`${t('providers.testPassed')} — ${t('providers.testLatency', { ms: result.latency_ms })}`,
					{ description: result.model }
				)
			} else {
				toast.error(t('providers.testFailed'), {
					description: result.error ?? result.model
				})
			}
		} catch (err) {
			toast.error(err instanceof Error ? err.message : t('common.error'))
		} finally {
			setQuickTestingChannelId(null)
			mutate(SWR_KEYS.PROVIDERS)
		}
	}

	return (
		<motion.div
			initial={{ opacity: 0, y: 20 }}
			animate={{ opacity: 1, y: 0 }}
			transition={{ delay: index * 0.08, ...transitions.normal }}
			whileHover={canDragCard ? { y: -2, transition: { duration: 0.2 } } : undefined}
		>
			<Card
				className='transition-shadow hover:shadow-md'
				draggable={canDragCard}
				onDragStart={event => {
					if (!canDragCard) {
						event.preventDefault()
						return
					}
					onDragStart(provider.id)
				}}
				onDragOver={event => {
					if (canDragCard) {
						event.preventDefault()
					}
				}}
				onDrop={() => {
					if (canDragCard) {
						onDrop(provider.id)
					}
				}}
			>
				<CardHeader
					className={cn('cursor-pointer select-none py-3', expanded && 'pb-4')}
					onClick={() => setExpanded(value => !value)}
					>
						<div className='flex items-center justify-between gap-3'>
							<div className='flex items-center gap-3 min-w-0'>
								<GripVertical
									className={cn(
										'h-4 w-4 text-muted-foreground/50 transition-colors shrink-0',
										canDragCard ? 'cursor-grab hover:text-muted-foreground' : 'hidden sm:block'
									)}
									onClick={event => event.stopPropagation()}
								/>
							<motion.div
								animate={{ rotate: expanded ? 90 : 0 }}
								transition={{ duration: 0.15 }}
								className='shrink-0'
							>
								<ChevronRight className='h-4 w-4 text-muted-foreground' />
							</motion.div>
							<motion.div
								whileHover={{ rotate: 10 }}
								transition={{ type: 'spring', stiffness: 300 }}
								className='flex h-8 w-8 items-center justify-center rounded-lg bg-secondary shrink-0'
							>
								<Server className='h-4 w-4' />
							</motion.div>
							<div className='flex items-center gap-2 min-w-0 flex-wrap'>
								<CardTitle className='text-base leading-normal -translate-y-px'>
									{provider.name}
								</CardTitle>
								<BadgeOverflowList
									items={headerBadgeItems}
									visibleCount={3}
									ariaLabel={`${provider.name}: ${t('common.status')}`}
									contentClassName='max-w-[min(28rem,calc(100vw-2rem))]'
								/>
								<span className='hidden lg:inline text-xs text-muted-foreground whitespace-nowrap'>
									[{t('providers.priority')}: {provider.priority} ·{' '}
									{t('providers.maxRetriesLabel')}: {provider.max_retries}]
								</span>
							</div>
						</div>
						<div
							className='flex items-center gap-4'
							onClick={event => event.stopPropagation()}
						>
							<div className='hidden md:flex items-center gap-2'>
								<Switch
									checked={provider.enabled}
									onCheckedChange={value => onToggle(provider, value)}
								/>
							</div>
							<TooltipProvider delayDuration={300}>
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
												className={cn(providerActionButtonClass, 'text-destructive hover:text-destructive')}
												aria-label={t('common.delete')}
												onClick={() => onDelete(provider)}
											>
												<Trash2 />
											</Button>
										</TooltipTrigger>
										<TooltipContent>{t('common.delete')}</TooltipContent>
									</Tooltip>
								</div>
							</TooltipProvider>
						</div>
					</div>
					<div className='md:hidden mt-2 flex items-center justify-between gap-2 text-sm text-muted-foreground'>
						<span>
							[{t('providers.priority')}: {provider.priority} ·{' '}
							{t('providers.maxRetriesLabel')}: {provider.max_retries}]
						</span>
						<div
							className='flex items-center gap-2'
							onClick={event => event.stopPropagation()}
						>
							<Switch
								checked={provider.enabled}
								onCheckedChange={value => onToggle(provider, value)}
							/>
						</div>
					</div>
				</CardHeader>
				<AnimatePresence initial={false}>
					{expanded && (
						<motion.div
							initial={{ height: 0, opacity: 0 }}
							animate={{ height: 'auto', opacity: 1 }}
							exit={{ height: 0, opacity: 0 }}
							transition={{ duration: 0.2, ease: 'easeInOut' }}
							style={{ overflow: 'hidden' }}
						>
							<CardContent className='space-y-5 pt-2'>
								<div>
									<div className='flex items-center gap-2 mb-3'>
										<Layers className='h-4 w-4 text-muted-foreground' />
										<h4 className='text-sm font-medium'>
											{t('providers.modelsSection')}
										</h4>
										<Badge variant='secondary' className='text-xs'>
											{modelEntries.length}
										</Badge>
									</div>
									<div className='mt-1 rounded-lg border overflow-hidden px-3 py-2'>
										<BadgeOverflowList
											items={modelEntries.map(([model, modelEntry]) => {
												const meta = modelMetadataById.get(model)
												const highlightUnpriced = !hasBillablePricingModelId(
													pricedModelIdSet,
													model,
													modelEntry.redirect,
													reasoningSuffixMap
												)

												return {
													key: model,
													collapsed: (
														<ModelBadge
															model={model}
															provider={meta?.models_dev_provider}
															multiplier={modelEntry.multiplier}
															redirect={modelEntry.redirect}
															highlightUnpriced={highlightUnpriced}
														/>
													),
													full: (
														<ModelBadge
															model={model}
															provider={meta?.models_dev_provider}
															multiplier={modelEntry.multiplier}
															redirect={modelEntry.redirect}
															highlightUnpriced={highlightUnpriced}
															truncateModelText={false}
															className='max-w-none'
														/>
													)
												}
											})}
											visibleCount={3}
											popoverOnSingle
											ariaLabel={`${t('providers.modelsSection')}: ${modelEntries.length}`}
											contentClassName='max-w-[min(44rem,calc(100vw-2rem))]'
										/>
									</div>
								</div>

								<div>
									<div className='flex items-center gap-2 mb-3'>
										<Radio className='h-4 w-4 text-muted-foreground' />
										<h4 className='text-sm font-medium'>
											{t('providers.channelsSection')}
										</h4>
										<Badge variant='secondary' className='text-xs'>
											{provider.channels.length}
										</Badge>
									</div>
									<div className='rounded-lg border overflow-hidden'>
										<Virtuoso
											style={{
												height: Math.min(
													provider.channels.length * PROVIDER_CHANNEL_OVERVIEW_ROW_HEIGHT,
													190
												)
											}}
											data={provider.channels}
											itemContent={(_idx, channel) => (
												<div className='flex min-h-10 items-center gap-3 px-3 py-1.5 text-sm hover:bg-muted/50 transition-colors border-b last:border-b-0'>
													{statusBadge(channel._health_status)}
													<span className='font-medium truncate min-w-0'>
														{channel.name}
													</span>
													<span className='font-mono text-xs text-muted-foreground truncate min-w-0 hidden sm:inline'>
														{channel.base_url}
													</span>
													<span className='ml-auto flex items-center gap-3 shrink-0'>
														<span className='inline-flex items-center rounded-md border overflow-hidden h-7'>
															<button
																type='button'
																className='flex items-center gap-1.5 px-2.5 h-full text-xs font-medium hover:bg-muted/80 transition-colors disabled:opacity-50 disabled:pointer-events-none border-r cursor-pointer'
																disabled={quickTestingChannelId === channel.id}
																onClick={() => handleQuickTest(channel.id)}
															>
																{quickTestingChannelId === channel.id ?
																	<Loader2 className='h-3 w-3 animate-spin' />
																:	<Zap className='h-3 w-3' />
																}
																{t('providers.quickTest')}
															</button>
															<button
																type='button'
																className='flex items-center justify-center w-7 h-full hover:bg-muted/80 transition-colors cursor-pointer'
																onClick={() => {
																	setTestDialogChannel({
																		id: channel.id,
																		name: channel.name,
																		models: [...channel.supported_models].sort()
																	})
																	setTestDialogOpen(true)
																}}
															>
																<ChevronRight className='h-3.5 w-3.5 text-muted-foreground' />
															</button>
														</span>
														<span className='text-xs text-muted-foreground'>
															{channel.supported_models.length}M ·{' '}
															W:{channel.weight}
														</span>
														<StatusBadge variant={channel.enabled ? 'success' : 'info'}>
															{channel.enabled ?
																t('common.enabled')
															: 	t('common.disabled')}
														</StatusBadge>
													</span>
												</div>
											)}
										/>
									</div>
								</div>
							</CardContent>
						</motion.div>
					)}
				</AnimatePresence>
			</Card>

			{testDialogChannel && (
				<ChannelTestDialog
					open={testDialogOpen}
					onOpenChange={open => {
						setTestDialogOpen(open)
						if (!open) setTestDialogChannel(null)
					}}
					providerId={provider.id}
					channelId={testDialogChannel.id}
					channelName={testDialogChannel.name}
					providerName={provider.name}
					models={testDialogChannel.models}
				/>
			)}
		</motion.div>
	)
}
