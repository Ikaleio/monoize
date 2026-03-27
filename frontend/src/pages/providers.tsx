import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import useSWR from 'swr'
import { Plus, Server } from 'lucide-react'
import { Card, CardContent } from '@/components/ui/card'
import { Button } from '@/components/ui/button'
import {
	AlertDialog,
	AlertDialogAction,
	AlertDialogCancel,
	AlertDialogContent,
	AlertDialogDescription,
	AlertDialogFooter,
	AlertDialogHeader,
	AlertDialogTitle
} from '@/components/ui/alert-dialog'
import { Skeleton } from '@/components/ui/skeleton'
import { toast } from 'sonner'
import { api } from '@/lib/api'
import type { Provider } from '@/lib/api'
import {
	useProviders,
	useSettings,
	useTransformRegistry,
	deleteProviderOptimistic,
	updateProviderOptimistic,
	reorderProviders
} from '@/lib/swr'
import { PageWrapper, motion, transitions } from '@/components/ui/motion'
import { ProviderCard } from './providers/ProviderCard'
import { ProviderDialog } from './providers/ProviderDialog'
import { DEFAULT_REASONING_SUFFIX_MAP } from './providers/shared'

export function ProvidersPage() {
	const { t } = useTranslation()
	const { data: providers = [], isLoading } = useProviders()
	const { data: settings } = useSettings()
	const { data: transformRegistry = [] } = useTransformRegistry()
	const { data: modelMetadata = [] } = useSWR('model-metadata', () =>
		api.listModelMetadata()
	)
	const reasoningSuffixMap =
		settings?.reasoning_suffix_map ?? DEFAULT_REASONING_SUFFIX_MAP
	const [createOpen, setCreateOpen] = useState(false)
	const [editProvider, setEditProvider] = useState<Provider | null>(null)
	const [deleteTarget, setDeleteTarget] = useState<Provider | null>(null)
	const [draggingProviderId, setDraggingProviderId] = useState<string | null>(null)

	const applyReorder = async (orderedIds: string[]) => {
		try {
			await reorderProviders(orderedIds)
			toast.success(t('providers.reorderSuccess'))
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

	const handleDelete = async (provider: Provider) => {
		setDeleteTarget(provider)
	}

	const confirmDelete = async () => {
		if (!deleteTarget) return
		try {
			await deleteProviderOptimistic(deleteTarget.id, providers)
			toast.success(t('providers.deleteSuccess'))
		} catch (error) {
			toast.error(error instanceof Error ? error.message : t('common.error'))
		} finally {
			setDeleteTarget(null)
		}
	}

	const handleToggle = async (provider: Provider, enabled: boolean) => {
		try {
			await updateProviderOptimistic(provider.id, { enabled }, providers)
			toast.success(t('providers.updateSuccess'))
		} catch (error) {
			toast.error(error instanceof Error ? error.message : t('common.error'))
		}
	}

	if (isLoading) {
		return (
			<div className='space-y-6'>
				<div>
					<Skeleton className='h-9 w-48' />
					<Skeleton className='mt-2 h-4 w-80' />
				</div>
				<div className='space-y-4'>
					{[...Array(3)].map((_, index) => (
						<Skeleton key={index} className='h-48 w-full' />
					))}
				</div>
			</div>
		)
	}

	return (
		<PageWrapper className='space-y-6'>
			<motion.div
				initial={{ opacity: 0, y: -10 }}
				animate={{ opacity: 1, y: 0 }}
				transition={transitions.normal}
				className='flex items-center justify-between'
			>
				<div>
					<h1 className='text-3xl font-bold tracking-tight'>
						{t('providers.title')}
					</h1>
					<p className='text-muted-foreground'>{t('providers.description')}</p>
				</div>
				<motion.div whileHover={{ scale: 1.02 }} whileTap={{ scale: 0.98 }}>
					<Button onClick={() => setCreateOpen(true)}>
						<Plus className='h-4 w-4 mr-2' />
						{t('providers.addProvider')}
					</Button>
				</motion.div>
			</motion.div>

			<div className='space-y-4'>
				{providers.length === 0 && (
					<motion.div
						initial={{ opacity: 0, scale: 0.95 }}
						animate={{ opacity: 1, scale: 1 }}
						transition={transitions.normal}
					>
						<Card>
							<CardContent className='py-16 flex flex-col items-center justify-center text-center'>
								<motion.div
									initial={{ scale: 0 }}
									animate={{ scale: 1 }}
									transition={{
										type: 'spring',
										stiffness: 300,
										damping: 20,
										delay: 0.1
									}}
									className='flex h-16 w-16 items-center justify-center rounded-full bg-muted mb-4'
								>
									<Server className='h-8 w-8 text-muted-foreground' />
								</motion.div>
								<h3 className='text-lg font-medium mb-1'>
									{t('providers.noProviders')}
								</h3>
								<p className='text-sm text-muted-foreground mb-4'>
									{t('providers.emptyStateDesc')}
								</p>
								<Button variant='outline' onClick={() => setCreateOpen(true)}>
									<Plus className='h-4 w-4 mr-2' />
									{t('providers.addProvider')}
								</Button>
							</CardContent>
						</Card>
					</motion.div>
				)}

				{providers.map((provider, index) => (
					<ProviderCard
						key={provider.id}
						provider={provider}
						index={index}
						total={providers.length}
						onEdit={setEditProvider}
						onDelete={handleDelete}
						onMove={moveProvider}
						onToggle={handleToggle}
						onDragStart={setDraggingProviderId}
						onDrop={handleDrop}
						modelMetadata={modelMetadata}
						reasoningSuffixMap={reasoningSuffixMap}
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
				modelMetadata={modelMetadata}
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
				modelMetadata={modelMetadata}
				reasoningSuffixMap={reasoningSuffixMap}
				settings={settings}
			/>

			<AlertDialog open={!!deleteTarget} onOpenChange={open => { if (!open) setDeleteTarget(null) }}>
				<AlertDialogContent>
					<AlertDialogHeader>
						<AlertDialogTitle>{t('providers.deleteConfirm')}</AlertDialogTitle>
						<AlertDialogDescription>
							{t('providers.deleteConfirmDesc', { id: deleteTarget?.name })}
						</AlertDialogDescription>
					</AlertDialogHeader>
					<AlertDialogFooter>
						<AlertDialogCancel>{t('common.cancel')}</AlertDialogCancel>
						<AlertDialogAction
							className='bg-destructive text-destructive-foreground hover:bg-destructive/90'
							onClick={confirmDelete}
						>
							{t('common.delete')}
						</AlertDialogAction>
					</AlertDialogFooter>
				</AlertDialogContent>
			</AlertDialog>
		</PageWrapper>
	)
}
