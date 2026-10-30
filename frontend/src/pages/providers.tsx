import { useEffect, useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useSearchParams } from 'react-router-dom'
import { Plus, Server } from 'lucide-react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { ConfirmDeleteDialog } from '@/components/ui/confirm-delete-dialog'
import { QueryError } from '@/components/ui/query-error'
import type { MonoizeChannel, Provider } from '@/lib/api'
import { toastApiError } from '@/lib/api-error'
import {
	useProviders,
	useModelMetadata,
	useModelPrices,
	useSettings,
	useTransformRegistry,
	deleteProviderOptimistic,
	updateProviderOptimistic,
	reorderProviders,
	setChannelEnabledOptimistic
} from '@/lib/swr'
import { AnimatedButton, PageWrapper, motion, transitions } from '@/components/ui/motion'
import { EmptyState } from '@/components/ui/empty-state'
import { PageHeader } from '@/components/ui/page-header'
import { CardsPageSkeleton } from '@/components/ui/page-skeleton'
import { ChannelTestProvider } from './providers/ChannelTestDialog'
import { ProviderCard } from './providers/ProviderCard'
import { ProviderDialog } from './providers/ProviderDialog'
import { DEFAULT_REASONING_SUFFIX_MAP } from './providers/shared'
import { useMediaQuery } from './providers/use-media-query'

export function ProvidersPage() {
	return (
		<ChannelTestProvider>
			<ProvidersPageContent />
		</ChannelTestProvider>
	)
}

function ProvidersPageContent() {
	const { t } = useTranslation()
	const [searchParams, setSearchParams] = useSearchParams()
	const {
		data: providersData,
		error: providersError,
		isLoading,
		isValidating,
		mutate: reloadProviders
	} = useProviders({ refreshInterval: 10000 })
	const providers = useMemo(() => providersData ?? [], [providersData])
	const { data: settings } = useSettings()
	const { data: transformRegistry = [], isLoading: transformRegistryLoading } =
		useTransformRegistry()
	const { data: modelMetadata = [] } = useModelMetadata()
	const { data: modelPrices = [] } = useModelPrices()
	const reasoningSuffixMap =
		settings?.reasoning_suffix_map ?? DEFAULT_REASONING_SUFFIX_MAP
	const canDrag = useMediaQuery('(pointer: fine)')
	const [createOpen, setCreateOpen] = useState(false)
	const [editProvider, setEditProvider] = useState<Provider | null>(null)
	const [deleteTarget, setDeleteTarget] = useState<Provider | null>(null)
	const [draggingProviderId, setDraggingProviderId] = useState<string | null>(null)
	const [focusRequest, setFocusRequest] = useState<{ providerId: string; request: { channelId: string | null } } | null>(null)
	const hasGroups = providers.some(provider => provider.group_ids.length > 0)

	// PL12e: apply `?provider=&channel=` once (state adjusted during render), then drop the params.
	const deepLinkProvider = searchParams.get('provider')
	const deepLinkChannel = searchParams.get('channel')
	const deepLinkKey = deepLinkProvider && providersData !== undefined ? `${deepLinkProvider}\n${deepLinkChannel ?? ''}` : null
	const [appliedDeepLink, setAppliedDeepLink] = useState<string | null>(null)
	if (deepLinkProvider && providersData !== undefined && deepLinkKey !== appliedDeepLink) {
		setAppliedDeepLink(deepLinkKey)
		setFocusRequest(providersData.some(provider => provider.id === deepLinkProvider)
			? { providerId: deepLinkProvider, request: { channelId: deepLinkChannel } }
			: null)
	}
	useEffect(() => {
		if (deepLinkKey === null) return
		setSearchParams(current => {
			const next = new URLSearchParams(current)
			next.delete('provider')
			next.delete('channel')
			return next
		}, { replace: true })
	}, [deepLinkKey, setSearchParams])

	const applyOrder = async (next: Provider[]) => {
		try {
			await reorderProviders(next)
		} catch (error) {
			toastApiError(error, t, t('providers.reorderFailed'))
		}
	}

	const moveProvider = (from: number, to: number) => {
		if (to < 0 || to >= providers.length || from === to) return
		const next = [...providers]
		const [item] = next.splice(from, 1)
		next.splice(to, 0, item)
		void applyOrder(next)
	}

	const endDrag = () => setDraggingProviderId(null)

	const handleDrop = (targetProviderId: string) => {
		const from = providers.findIndex(provider => provider.id === draggingProviderId)
		const to = providers.findIndex(provider => provider.id === targetProviderId)
		endDrag()
		if (from >= 0 && to >= 0) moveProvider(from, to)
	}

	const confirmDelete = async () => {
		if (!deleteTarget) return
		const target = deleteTarget
		try {
			await deleteProviderOptimistic(target.id, providers)
			toast.success(t('providers.deleteSuccess', { name: target.name }))
		} catch (error) {
			toastApiError(error, t, t('providers.deleteFailed'))
		}
	}

	// PL12c: immediate toggle with an undo action instead of a confirmation.
	const handleToggle = async (provider: Provider, enabled: boolean) => {
		try {
			await updateProviderOptimistic(provider.id, { enabled }, providers)
		} catch (error) {
			toastApiError(error, t, t('providers.toggleFailed', { name: provider.name }))
			return
		}
		toast.success(
			enabled ? t('providers.enabledToast', { name: provider.name }) : t('providers.disabledToast', { name: provider.name }),
			{
				action: {
					label: t('providers.undo'),
					onClick: () => {
						updateProviderOptimistic(provider.id, { enabled: !enabled }, providers).catch(error =>
							toastApiError(error, t, t('providers.toggleFailed', { name: provider.name }))
						)
					}
				}
			}
		)
	}

	const handleToggleChannel = async (provider: Provider, channel: MonoizeChannel, enabled: boolean) => {
		try {
			await setChannelEnabledOptimistic(provider.id, channel.id, enabled)
		} catch (error) {
			toastApiError(error, t, t('providers.channelToggleFailed', { name: channel.name }))
		}
	}

	if (isLoading) {
		return (
			<PageWrapper className='space-y-6'>
				<CardsPageSkeleton />
			</PageWrapper>
		)
	}

	const header = (
		<motion.div
			initial={{ opacity: 0, y: -10 }}
			animate={{ opacity: 1, y: 0 }}
			transition={transitions.normal}
		>
			<PageHeader
				title={t('providers.title')}
				description={hasGroups ? `${t('providers.description')} ${t('providers.descriptionGroups')}` : t('providers.description')}
				actions={providersError && !providersData ? undefined : (
					<AnimatedButton>
						<Button onClick={() => setCreateOpen(true)}>
							<Plus data-icon='inline-start' aria-hidden='true' />
							{t('providers.addProvider')}
						</Button>
					</AnimatedButton>
				)}
			/>
		</motion.div>
	)

	const errorState = providersError ? (
		<QueryError onRetry={reloadProviders} retrying={isValidating} stale={providersData !== undefined} />
	) : null

	if (providersError && !providersData) {
		return (
			<PageWrapper className='space-y-6'>
				{header}
				{errorState}
			</PageWrapper>
		)
	}

	return (
		<PageWrapper className='space-y-6'>
			{header}

			{errorState}

			<div className='space-y-4'>
				{providersData !== undefined && providers.length === 0 && (
					<motion.div
						initial={{ opacity: 0, scale: 0.95 }}
						animate={{ opacity: 1, scale: 1 }}
						transition={transitions.normal}
					>
						<EmptyState
							variant='card'
							icon={<Server className='size-10' aria-hidden='true' />}
							title={t('providers.noProviders')}
							description={t('providers.emptyStateDesc')}
							action={<Button onClick={() => setCreateOpen(true)}><Plus data-icon='inline-start' aria-hidden='true' />{t('providers.addProvider')}</Button>}
						/>
					</motion.div>
				)}

				{providers.map((provider, index) => (
					<ProviderCard
						key={provider.id}
						provider={provider}
						index={index}
						total={providers.length}
						onEdit={setEditProvider}
						onDelete={setDeleteTarget}
						onMove={moveProvider}
						onToggle={handleToggle}
						onToggleChannel={handleToggleChannel}
						canDrag={canDrag}
						onDragStart={setDraggingProviderId}
						onDrop={handleDrop}
						onDragEnd={endDrag}
						modelMetadata={modelMetadata}
						globalProbeModel={settings?.monoize_active_probe_model}
						focusRequest={focusRequest?.providerId === provider.id ? focusRequest.request : null}
					/>
				))}
			</div>

			<ProviderDialog
				open={createOpen}
				onOpenChange={setCreateOpen}
				mode='create'
				current={null}
				providers={providers}
				transformRegistry={transformRegistry}
				transformRegistryLoading={transformRegistryLoading}
				modelMetadata={modelMetadata}
				modelPrices={modelPrices}
				reasoningSuffixMap={reasoningSuffixMap}
				settings={settings}
			/>

			<ProviderDialog
				open={!!editProvider}
				onOpenChange={open => {
					if (!open) setEditProvider(null)
				}}
				mode='edit'
				current={editProvider}
				providers={providers}
				transformRegistry={transformRegistry}
				transformRegistryLoading={transformRegistryLoading}
				modelMetadata={modelMetadata}
				modelPrices={modelPrices}
				reasoningSuffixMap={reasoningSuffixMap}
				settings={settings}
			/>

			<ConfirmDeleteDialog
				open={deleteTarget !== null}
				onOpenChange={open => {
					if (!open) setDeleteTarget(null)
				}}
				title={t('providers.deleteConfirm')}
				description={
					deleteTarget?.live_usage && deleteTarget.live_usage.rpm > 0
						? `${t('providers.deleteConfirmDesc', { name: deleteTarget.name })} ${t('providers.deleteConfirmTraffic', { count: deleteTarget.live_usage.rpm })}`
						: t('providers.deleteConfirmDesc', { name: deleteTarget?.name })
				}
				onConfirm={confirmDelete}
			/>
		</PageWrapper>
	)
}
