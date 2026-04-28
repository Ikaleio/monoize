import { useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import useSWR from 'swr'
import { Search } from 'lucide-react'
import { toast } from 'sonner'
import { ModelBadge } from '@/components/ModelBadge'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import {
	Dialog,
	DialogContent,
	DialogDescription,
	DialogFooter,
	DialogHeader,
	DialogTitle
} from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { api } from '@/lib/api'
import { cn } from '@/lib/utils'
import type { ModelMetadataRecord } from '@/lib/api'
import {
	buildPricedModelIdSet,
	hasBillablePricingModelId
} from './shared'

type ModelPickerDialogProps = {
	open: boolean
	onOpenChange: (open: boolean) => void
	providerId?: string
	channelInfo?: { base_url: string; api_key: string }
	providerName: string
	existingModels: string[]
	modelMetadata: ModelMetadataRecord[]
	reasoningSuffixMap: Record<string, string>
	onConfirm: (checkedModels: string[]) => void
}

type FetchModelsKey =
	| readonly ['provider-models', string]
	| readonly ['channel-models', string, string]

export function ModelPickerDialog({
	open,
	onOpenChange,
	...props
}: ModelPickerDialogProps) {
	return (
		<Dialog open={open} onOpenChange={onOpenChange}>
			{open ? <ModelPickerDialogContent onOpenChange={onOpenChange} {...props} /> : null}
		</Dialog>
	)
}

function ModelPickerDialogContent({
	onOpenChange,
	providerId,
	channelInfo,
	providerName,
	existingModels,
	modelMetadata,
	reasoningSuffixMap,
	onConfirm
}: Omit<ModelPickerDialogProps, 'open'>) {
	const { t } = useTranslation()
	const [checked, setChecked] = useState<Set<string>>(
		() => new Set(existingModels)
	)
	const [search, setSearch] = useState('')
	const [tab, setTab] = useState<'new' | 'existing'>('new')

	const fetchKey =
		providerId ?
			(['provider-models', providerId] as FetchModelsKey)
		: channelInfo ?
			([
				'channel-models',
				channelInfo.base_url,
				channelInfo.api_key
			] as FetchModelsKey)
		: null

	const { data: fetchedModels = [], isLoading: loading } = useSWR(
		fetchKey,
		async (key: FetchModelsKey) => {
			if (key[0] === 'provider-models') {
				return (await api.fetchProviderModels(key[1])).models
			}
			return (await api.fetchChannelModels(key[1], key[2])).models
		},
		{
			revalidateOnFocus: false,
			onError: error => {
				toast.error(
					error instanceof Error ?
						error.message
					: t('providers.fetchModelsError')
				)
			}
		}
	)

	const existingSet = useMemo(() => new Set(existingModels), [existingModels])

	const newModels = useMemo(
		() => fetchedModels.filter(model => !existingSet.has(model)),
		[fetchedModels, existingSet]
	)

	const displayModels = tab === 'new' ? newModels : existingModels

	const filtered = useMemo(() => {
		if (!search.trim()) return displayModels
		const q = search.toLowerCase()
		return displayModels.filter(model => model.toLowerCase().includes(q))
	}, [displayModels, search])

	const modelProviderMap = useMemo(() => {
		const map = new Map<string, string | undefined>()
		for (const item of modelMetadata) {
			map.set(item.model_id, item.models_dev_provider)
		}
		return map
	}, [modelMetadata])

	const pricedModelIdSet = useMemo(
		() => buildPricedModelIdSet(modelMetadata),
		[modelMetadata]
	)

	const toggleModel = (model: string) => {
		setChecked(prev => {
			const next = new Set(prev)
			if (next.has(model)) next.delete(model)
			else next.add(model)
			return next
		})
	}

	const hasChanges = useMemo(() => {
		if (checked.size !== existingSet.size) return true
		for (const model of checked) {
			if (!existingSet.has(model)) return true
		}
		return false
	}, [checked, existingSet])

	const handleConfirm = () => {
		onConfirm([...checked])
		onOpenChange(false)
	}

	return (
		<DialogContent className='max-h-[85vh] flex flex-col overflow-hidden max-w-4xl'>
			<DialogHeader>
				<div className='flex items-center justify-between pr-8'>
					<DialogTitle>{t('providers.selectModels')}</DialogTitle>
					<div className='flex items-center gap-1 text-sm text-muted-foreground'>
						<button
							type='button'
							className={cn(
								'px-2 py-1 rounded transition-colors',
								tab === 'new' ?
									'font-bold text-foreground'
								:	'hover:text-foreground cursor-pointer'
							)}
							onClick={() => setTab('new')}
						>
							{t('providers.newModels')} ({newModels.length})
						</button>
						<span>/</span>
						<button
							type='button'
							className={cn(
								'px-2 py-1 rounded transition-colors',
								tab === 'existing' ?
									'font-bold text-foreground'
								:	'hover:text-foreground cursor-pointer'
							)}
							onClick={() => setTab('existing')}
						>
							{t('providers.existingModels')} ({existingModels.length})
						</button>
					</div>
				</div>
				<DialogDescription>{providerName}</DialogDescription>
			</DialogHeader>

			<div className='flex flex-col gap-4'>
				<div className='relative'>
					<Search className='absolute left-3 top-1/2 -translate-y-1/2 h-4 w-4 text-muted-foreground' />
					<Input
						className='pl-10'
						placeholder={t('providers.searchModels')}
						value={search}
						onChange={event => setSearch(event.target.value)}
					/>
				</div>

				<div className='border rounded-lg h-[clamp(220px,45vh,420px)] overflow-y-auto p-3'>
					{loading ?
						<div className='text-sm text-muted-foreground py-8 text-center'>
							{t('providers.fetchingModels')}
						</div>
					: filtered.length === 0 ?
							<div className='text-sm text-muted-foreground py-8 text-center'>
								{t('providers.noNewModels')}
							</div>
					:	<div className='flex flex-wrap gap-2'>
							{filtered.map(model => {
								const provider = modelProviderMap.get(model)
								return (
									<label
										key={model}
										className='inline-flex items-center gap-2 cursor-pointer'
									>
										<Checkbox
											checked={checked.has(model)}
											onCheckedChange={() => toggleModel(model)}
										/>
										<ModelBadge
											model={model}
											provider={provider}
											highlightUnpriced={
												!hasBillablePricingModelId(
													pricedModelIdSet,
													model,
													null,
													reasoningSuffixMap
												)
											}
										/>
									</label>
								)
							})}
						</div>
					}
				</div>
			</div>

			<DialogFooter>
				<Button variant='outline' onClick={() => onOpenChange(false)}>
					{t('common.cancel')}
				</Button>
				<Button onClick={handleConfirm} disabled={!hasChanges}>
					{t('providers.confirmAdd')}
				</Button>
			</DialogFooter>
		</DialogContent>
	)
}
