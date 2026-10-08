import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Plus, Server } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { ConfirmDeleteDialog } from '@/components/ui/confirm-delete-dialog'
import { QueryError } from '@/components/ui/query-error'
import { toast } from 'sonner'
import type { Provider } from '@/lib/api'
import {
	useProviders,
	useModelMetadata,
	useModelPrices,
	useSettings,
	useTransformRegistry,
	deleteProviderOptimistic,
	updateProviderOptimistic,
	reorderProviders
} from '@/lib/swr'
import { AnimatedButton, PageWrapper, motion, transitions } from '@/components/ui/motion'
import { EmptyState } from '@/components/ui/empty-state'
import { PageHeader } from '@/components/ui/page-header'
import { CardsPageSkeleton } from '@/components/ui/page-skeleton'
import { ProviderCard } from './providers/ProviderCard'
import { ProviderDialog } from './providers/ProviderDialog'
import { DEFAULT_REASONING_SUFFIX_MAP } from './providers/shared'

export function ProvidersPage() {
	const { t } = useTranslation()
	const {
		data: providersData,
		error: providersError,
		isLoading,
		isValidating,
		mutate: reloadProviders
	} = useProviders({ refreshInterval: 10000 })
	const providers = providersData ?? []
	const { data: settings } = useSettings()
	const { data: transformRegistry = [], isLoading: transformRegistryLoading } =
		useTransformRegistry()
	const { data: modelMetadata = [] } = useModelMetadata()
	const { data: modelPrices = [] } = useModelPrices()
	const reasoningSuffixMap =
		settings?.reasoning_suffix_map ?? DEFAULT_REASONING_SUFFIX_MAP
	const [createOpen, setCreateOpen] = useState(false)
	const [editProvider, setEditProvider] = useState<Provider | null>(null)
	const [deleteTarget, setDeleteTarget] = useState<Provider | null>(null)
	const [draggingProviderId, setDraggingProviderId] = useState<string | null>(null)

	const applyReorder = async (orderedIds: string[]) => {
		try {
			await reorderProviders(orderedIds)
		} catch (error) {
			toast.error(error instanceof Error ? error.message : t('common.error'))
		}
	}

	const moveProvider = async (from: number, to: number) => {
		if (to < 0 || to >= providers.length || from === to) {
			return
		}
		const next = [...providers]
		const [item] = next.splice(from, 1)
		next.splice(to, 0, item)
		await applyReorder(next.map(provider => provider.id))
	}

	const handleDrop = async (targetProviderId: string) => {
		if (!draggingProviderId || draggingProviderId === targetProviderId) {
			return
		}
		const next = [...providers]
		const from = next.findIndex(provider => provider.id === draggingProviderId)
		const to = next.findIndex(provider => provider.id === targetProviderId)
		if (from < 0 || to < 0) {
			return
		}
		const [item] = next.splice(from, 1)
		next.splice(to, 0, item)
		setDraggingProviderId(null)
		await applyReorder(next.map(provider => provider.id))
	}

	const confirmDelete = async () => {
		if (!deleteTarget) return
		try {
			await deleteProviderOptimistic(deleteTarget.id, providers)
		} catch (error) {
			toast.error(error instanceof Error ? error.message : t('common.error'))
		}
	}

	const handleToggle = async (provider: Provider, enabled: boolean) => {
		try {
			await updateProviderOptimistic(provider.id, { enabled }, providers)
		} catch (error) {
			toast.error(error instanceof Error ? error.message : t('common.error'))
		}
	}

	if (isLoading) {
		return (
			<PageWrapper className='space-y-6'>
				<CardsPageSkeleton />
			</PageWrapper>
		)
	}

	const errorState = providersError ? (
		<QueryError onRetry={reloadProviders} retrying={isValidating} stale={providersData !== undefined} />
	) : null

	if (providersError && !providersData) {
		return (
			<PageWrapper className='space-y-6'>
				<motion.div
					initial={{ opacity: 0, y: -10 }}
					animate={{ opacity: 1, y: 0 }}
					transition={transitions.normal}
				>
					<PageHeader title={t('providers.title')} description={t('providers.description')} />
				</motion.div>
				{errorState}
			</PageWrapper>
		)
	}

	return (
		<PageWrapper className='space-y-6'>
			<motion.div
				initial={{ opacity: 0, y: -10 }}
				animate={{ opacity: 1, y: 0 }}
				transition={transitions.normal}
			>
				<PageHeader title={t('providers.title')} description={t('providers.description')} actions={(
					<AnimatedButton>
						<Button onClick={() => setCreateOpen(true)}>
							<Plus data-icon='inline-start' aria-hidden='true' />
							{t('providers.addProvider')}
						</Button>
					</AnimatedButton>
				)} />
			</motion.div>

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
						onDragStart={setDraggingProviderId}
						onDrop={handleDrop}
						modelMetadata={modelMetadata}
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
					if (!open) {
						setEditProvider(null)
					}
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
				description={t('providers.deleteConfirmDesc', { name: deleteTarget?.name })}
				onConfirm={confirmDelete}
			/>
		</PageWrapper>
	)
}
