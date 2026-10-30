import { useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link } from 'react-router-dom'
import useSWR from 'swr'
import { AlertTriangle, Search } from 'lucide-react'
import { ModelBadge } from '@/components/ModelBadge'
import { StackedModelList } from '@/components/StackedModelList'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
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
import { SegmentedControl } from '@/components/ui/segmented-control'
import { api } from '@/lib/api'
import { apiErrorMessage } from '@/lib/api-error'
import type { FetchChannelModelsInput, ModelMetadataRecord, ModelPriceRecord } from '@/lib/api'
import {
	buildPricedModelIdSet,
	hasBillablePricingModelId
} from './shared'

type ModelPickerDialogProps = {
	open: boolean
	onOpenChange: (open: boolean) => void
	channelInfo?: FetchChannelModelsInput
	providerName: string
	existingModels: string[]
	modelMetadata: ModelMetadataRecord[]
	modelPrices: ModelPriceRecord[]
	reasoningSuffixMap: Record<string, string>
	onConfirm: (checkedModels: string[]) => void
}

type FetchModelsKey = readonly [
	'channel-models',
	FetchChannelModelsInput['provider_type'],
	string,
	string,
	string,
	string
]

const PICKER_TABS = ['new', 'existing'] as const
type PickerTab = (typeof PICKER_TABS)[number]

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
	channelInfo,
	providerName,
	existingModels,
	modelMetadata,
	modelPrices,
	reasoningSuffixMap,
	onConfirm
}: Omit<ModelPickerDialogProps, 'open'>) {
	const { t } = useTranslation()
	const [checked, setChecked] = useState<Set<string>>(() => new Set(existingModels))
	const [search, setSearch] = useState('')
	const [tab, setTab] = useState<PickerTab>('new')

	const fetchKey =
		channelInfo ?
			([
				'channel-models',
				channelInfo.provider_type,
				channelInfo.base_url,
				channelInfo.api_key ?? '',
				channelInfo.provider_id ?? '',
				channelInfo.channel_id ?? ''
			] as FetchModelsKey)
		: null

	const { data: fetched, error, isLoading: loading, isValidating, mutate: retry } = useSWR(
		fetchKey,
		(key: FetchModelsKey) => api.fetchChannelModels({
			provider_type: key[1],
			base_url: key[2],
			api_key: key[3] || undefined,
			provider_id: key[4] || undefined,
			channel_id: key[5] || undefined
		}),
		{ revalidateOnFocus: false, revalidateOnReconnect: false, shouldRetryOnError: false }
	)

	const existingSet = useMemo(() => new Set(existingModels), [existingModels])
	const sortedExisting = useMemo(() => [...existingModels].sort(), [existingModels])
	const newModels = useMemo(
		() => (fetched?.models ?? []).filter(model => !existingSet.has(model)).sort(),
		[fetched, existingSet]
	)
	const displayModels = tab === 'new' ? newModels : sortedExisting

	const filtered = useMemo(() => {
		const query = search.trim().toLowerCase()
		if (!query) return displayModels
		return displayModels.filter(model => model.toLowerCase().includes(query))
	}, [displayModels, search])

	const modelProviderMap = useMemo(
		() => new Map(modelMetadata.map(item => [item.model_id, item.models_dev_provider])),
		[modelMetadata]
	)
	const pricedModelIdSet = useMemo(() => buildPricedModelIdSet(modelPrices), [modelPrices])
	const isUnpriced = (model: string) => !hasBillablePricingModelId(pricedModelIdSet, model, null, reasoningSuffixMap)
	const anyUnpriced = filtered.some(isUnpriced)

	const toggleModel = (model: string) => {
		setChecked(previous => {
			const next = new Set(previous)
			if (next.has(model)) next.delete(model)
			else next.add(model)
			return next
		})
	}

	const setFiltered = (value: boolean) => {
		setChecked(previous => {
			const next = new Set(previous)
			for (const model of filtered) {
				if (value) next.add(model)
				else next.delete(model)
			}
			return next
		})
	}

	const added = [...checked].filter(model => !existingSet.has(model)).length
	const removed = existingModels.filter(model => !checked.has(model)).length
	const hasChanges = added > 0 || removed > 0

	const emptyText =
		search.trim() && displayModels.length > 0 ? t('providers.pickerNoMatch')
		: tab === 'new' ? t('providers.pickerNoNewModels')
		: t('providers.pickerNoExistingModels')

	return (
		<DialogContent className='flex max-h-[min(85dvh,52rem)] max-w-4xl flex-col gap-4 overflow-hidden'>
			<DialogHeader className='shrink-0'>
				<div className='flex flex-wrap items-center justify-between gap-3 pr-8'>
					<DialogTitle>{t('providers.selectModels')}</DialogTitle>
					<SegmentedControl
						value={tab}
						options={PICKER_TABS}
						onChange={setTab}
						ariaLabel={t('providers.selectModels')}
						label={option =>
							option === 'new' ?
								`${t('providers.newModels')} (${newModels.length})`
							: 	`${t('providers.existingModels')} (${existingModels.length})`
						}
					/>
				</div>
				<DialogDescription>
					{providerName}
					{fetched?.compact_scheme === 'openai_compact_sibling'
						? ` — ${t('providers.compactSchemeSibling')}`
						: fetched?.compact_scheme === 'same_model'
							? ` — ${t('providers.compactSchemeSame')}`
							: ''}
				</DialogDescription>
			</DialogHeader>

			<div className='flex shrink-0 flex-col gap-2 sm:flex-row sm:items-center'>
				<div className='relative min-w-0 flex-1'>
					<Search className='absolute left-3 top-1/2 size-4 -translate-y-1/2 text-muted-foreground' aria-hidden='true' />
					<Input
						className='pl-10'
						placeholder={t('providers.searchModels')}
						aria-label={t('providers.searchModels')}
						value={search}
						onChange={event => setSearch(event.target.value)}
					/>
				</div>
				<div className='flex gap-2'>
					<Button type='button' variant='outline' size='sm' disabled={filtered.length === 0} onClick={() => setFiltered(true)}>
						{t('providers.pickerSelectFiltered')}
					</Button>
					<Button type='button' variant='outline' size='sm' disabled={filtered.length === 0} onClick={() => setFiltered(false)}>
						{t('providers.pickerClearFiltered')}
					</Button>
				</div>
			</div>

			{tab === 'existing' ?
				<p className='shrink-0 text-xs text-muted-foreground'>{t('providers.pickerExistingHint')}</p>
			: 	null}

			<div className='flex min-h-0 flex-1 flex-col'>
				{tab === 'new' && error ?
					<Alert variant='destructive'>
						<AlertTriangle className='size-4' aria-hidden='true' />
						<AlertTitle>{t('providers.fetchModelsError')}</AlertTitle>
						<AlertDescription className='flex flex-col items-start gap-2'>
							<span>{apiErrorMessage(error, t)}</span>
							{error instanceof Error && error.message ?
								<details className='w-full text-xs'>
									<summary className='cursor-pointer'>{t('providers.errorDetails')}</summary>
									<code className='mt-1 block whitespace-pre-wrap break-words font-mono'>{error.message}</code>
								</details>
							: 	null}
							<Button type='button' size='sm' variant='outline' disabled={isValidating} onClick={() => void retry()}>
								{isValidating ? t('common.loading') : t('common.retry')}
							</Button>
						</AlertDescription>
					</Alert>
				: 	<StackedModelList className='flex min-h-0 flex-1 flex-col' listClassName='max-h-none min-h-[10rem] flex-1 gap-2'>
						{tab === 'new' && loading ?
							<div className='w-full py-8 text-center text-sm text-muted-foreground'>
								{t('providers.fetchingModels')}
							</div>
						: filtered.length === 0 ?
							<div className='w-full py-8 text-center text-sm text-muted-foreground'>
								{emptyText}
							</div>
						: 	filtered.map(model => (
								<label key={model} className='inline-flex cursor-pointer items-center gap-2'>
									<Checkbox
										checked={checked.has(model)}
										onCheckedChange={() => toggleModel(model)}
									/>
									<ModelBadge
										model={model}
										provider={modelProviderMap.get(model)}
										highlightUnpriced={isUnpriced(model)}
									/>
								</label>
							))
						}
					</StackedModelList>
				}
				{anyUnpriced ?
					<p className='mt-2 text-xs text-muted-foreground'>
						{t('providers.unpricedLegend')}{' '}
						<Link to='/dashboard/models?tab=unpriced' className='text-primary underline underline-offset-4'>
							{t('providers.unpricedLegendLink')}
						</Link>
					</p>
				: 	null}
			</div>

			<DialogFooter className='shrink-0 items-center gap-2 sm:justify-between'>
				<p className='text-xs text-muted-foreground'>{hasChanges ? null : t('providers.pickerNoChanges')}</p>
				<div className='flex flex-col-reverse gap-2 sm:flex-row'>
					<Button variant='outline' onClick={() => onOpenChange(false)}>
						{t('common.cancel')}
					</Button>
					<Button
						onClick={() => {
							onConfirm([...checked])
							onOpenChange(false)
						}}
						disabled={!hasChanges}
					>
						{t('providers.pickerApply', { added, removed })}
					</Button>
				</div>
			</DialogFooter>
		</DialogContent>
	)
}
