import { useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { CheckSquare, CloudDownload, Layers3, Plus, Search, Trash2, X } from 'lucide-react'
import { ModelBadge } from '@/components/ModelBadge'
import { StackedModelList } from '@/components/StackedModelList'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Badge } from '@/components/ui/badge'
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
import {
	Field,
	FieldDescription,
	FieldError,
	FieldGroup,
	FieldLabel
} from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { normalizeMultiplier } from '@/lib/exact-decimal'
import {
	hasBillablePricingModelId,
	type ModelRow
} from './shared'

type ModelEditorState = {
	index: number | null
	initial: ModelRow
	draft: ModelRow
	errors: {
		model?: string
		multiplier?: string
	}
}

type ChannelModelEditorProps = {
	models: ModelRow[]
	onChange: (models: ModelRow[]) => void
	onOpenPicker: () => void
	pricedModels: Set<string>
	metadataProvider: Map<string, string | undefined>
	reasoningSuffixMap: Record<string, string>
	/** PL26: validation message for the Channel model list, focus target `channel-models`. */
	error?: string
}

const emptyModel = (): ModelRow => ({ model: '', redirect: '', multiplier: '1' })

function splitModelIds(text: string): string[] {
	return Array.from(new Set(text.split(/[\n,]/).map(value => value.trim()).filter(Boolean)))
}

export function ChannelModelEditor({
	models,
	onChange,
	onOpenPicker,
	pricedModels,
	metadataProvider,
	reasoningSuffixMap,
	error
}: ChannelModelEditorProps) {
	const { t } = useTranslation()
	const [editor, setEditor] = useState<ModelEditorState | null>(null)
	const [selecting, setSelecting] = useState(false)
	const [selected, setSelected] = useState<Set<string>>(new Set())
	const [filter, setFilter] = useState('')
	const [bulkMultiplier, setBulkMultiplier] = useState('')
	const [bulkError, setBulkError] = useState<string | null>(null)

	const visibleModels = useMemo(() => {
		const query = filter.trim().toLowerCase()
		return models
			.map((row, index) => ({ row, index }))
			.filter(({ row }) => !query || row.model.toLowerCase().includes(query))
	}, [filter, models])

	const openEditor = (index: number | null) => {
		const initial = index === null ? emptyModel() : { ...models[index] }
		setEditor({ index, initial, draft: { ...initial }, errors: {} })
	}

	const updateDraft = (patch: Partial<ModelRow>) => {
		setEditor(current => current && {
			...current,
			draft: { ...current.draft, ...patch },
			errors: {
				...current.errors,
				...(patch.model !== undefined ? { model: undefined } : {}),
				...(patch.multiplier !== undefined ? { multiplier: undefined } : {})
			}
		})
	}

	const saveModel = () => {
		if (!editor) return
		const errors: ModelEditorState['errors'] = {}
		const ids = editor.index === null ? splitModelIds(editor.draft.model) : [editor.draft.model.trim()].filter(Boolean)
		const duplicates = ids.filter(id => models.some((row, index) => index !== editor.index && row.model.trim() === id))
		if (ids.length === 0) {
			errors.model = t('providers.editor.modelNameRequired')
		} else if (duplicates.length > 0) {
			errors.model = t('providers.editor.modelDuplicateList', { models: duplicates.join(', ') })
		}
		const multiplier = normalizeMultiplier(editor.draft.multiplier.trim())
		if (multiplier == null) errors.multiplier = t('providers.editor.multiplierInvalid')
		if (errors.model || errors.multiplier || multiplier == null) {
			setEditor(current => current && { ...current, errors })
			return
		}
		const redirect = ids.length === 1 ? editor.draft.redirect.trim() : ''
		const rows = ids.map(model => ({ model, redirect, multiplier }))
		onChange(
			editor.index === null ?
				[...models, ...rows]
			: 	models.map((row, index) => index === editor.index ? rows[0] : row)
		)
		setEditor(null)
	}

	const deleteModel = () => {
		if (editor?.index == null) return
		onChange(models.filter((_, index) => index !== editor.index))
		setEditor(null)
	}

	const exitSelection = () => {
		setSelecting(false)
		setSelected(new Set())
		setFilter('')
		setBulkMultiplier('')
		setBulkError(null)
	}

	const toggleSelected = (model: string) => {
		setSelected(previous => {
			const next = new Set(previous)
			if (next.has(model)) next.delete(model)
			else next.add(model)
			return next
		})
	}

	const applyBulkMultiplier = () => {
		const multiplier = normalizeMultiplier(bulkMultiplier.trim())
		if (multiplier == null) {
			setBulkError(t('providers.editor.multiplierInvalid'))
			return
		}
		onChange(models.map(row => selected.has(row.model) ? { ...row, multiplier } : row))
		setBulkError(null)
	}

	const dirty = editor !== null && (
		editor.draft.model !== editor.initial.model
		|| editor.draft.redirect !== editor.initial.redirect
		|| editor.draft.multiplier !== editor.initial.multiplier
	)
	const draftIdCount = editor?.index === null ? splitModelIds(editor.draft.model).length : 1

	return (
		<section className='flex flex-col gap-4 rounded-xl border bg-card p-4 sm:p-5' aria-labelledby='channel-models-title'>
			<div className='flex flex-wrap items-center justify-between gap-3'>
				<div>
					<div className='flex items-center gap-2'>
						<Layers3 className='size-4 text-primary' aria-hidden='true' />
						<h4 id='channel-models-title' className='font-medium'>{t('providers.editor.supportedModels')}</h4>
						<Badge variant='secondary'>{models.length}</Badge>
					</div>
					<p className='mt-1 text-xs text-muted-foreground'>
						{t('providers.editor.supportedModelsHint')}
					</p>
				</div>
				<div className='flex flex-wrap items-center gap-2'>
					{models.length > 0 ?
						<Button type='button' variant={selecting ? 'secondary' : 'ghost'} size='sm' onClick={() => selecting ? exitSelection() : setSelecting(true)}>
							{selecting ? <X data-icon /> : <CheckSquare data-icon />}
							{selecting ? t('providers.editor.exitSelection') : t('providers.editor.selectMode')}
						</Button>
					: 	null}
					<Button id='channel-models' type='button' variant='outline' size='sm' onClick={onOpenPicker} aria-invalid={Boolean(error)}>
						<CloudDownload data-icon />
						{t('providers.editor.fetchUpstream')}
					</Button>
					<Button type='button' size='sm' onClick={() => openEditor(null)}>
						<Plus data-icon />
						{t('providers.editor.addManually')}
					</Button>
				</div>
			</div>
			{error ? <FieldError>{error}</FieldError> : null}

			{selecting ?
				<div className='flex flex-col gap-2 rounded-lg border bg-muted/30 p-3'>
					<div className='flex flex-col gap-2 sm:flex-row sm:items-center'>
						<div className='relative min-w-0 flex-1'>
							<Search className='absolute left-3 top-1/2 size-4 -translate-y-1/2 text-muted-foreground' aria-hidden='true' />
							<Input className='pl-10' value={filter} placeholder={t('providers.searchModels')} aria-label={t('providers.searchModels')} onChange={event => setFilter(event.target.value)} />
						</div>
						<Button type='button' variant='outline' size='sm' onClick={() => setSelected(new Set(visibleModels.map(({ row }) => row.model)))}>
							{t('providers.editor.selectAllFiltered')}
						</Button>
					</div>
					<div className='flex flex-wrap items-center gap-2'>
						<span className='text-xs text-muted-foreground'>{t('providers.editor.selectedCount', { count: selected.size })}</span>
						<Button
							type='button'
							variant='outline'
							size='sm'
							className='text-error-foreground'
							disabled={selected.size === 0}
							onClick={() => {
								onChange(models.filter(row => !selected.has(row.model)))
								setSelected(new Set())
							}}
						>
							<Trash2 data-icon />
							{t('providers.editor.deleteSelected')}
						</Button>
						<Input
							className='h-8 w-28'
							inputMode='decimal'
							value={bulkMultiplier}
							placeholder={t('providers.editor.multiplier')}
							aria-label={t('providers.editor.bulkMultiplier')}
							aria-invalid={Boolean(bulkError)}
							onChange={event => { setBulkMultiplier(event.target.value); setBulkError(null) }}
						/>
						<Button type='button' variant='outline' size='sm' disabled={selected.size === 0 || !bulkMultiplier.trim()} onClick={applyBulkMultiplier}>
							{t('providers.editor.applyMultiplier')}
						</Button>
					</div>
					{bulkError ? <FieldError>{bulkError}</FieldError> : null}
				</div>
			: 	null}

			{models.length === 0 ?
				<Alert>
					<Layers3 className='size-4' aria-hidden='true' />
					<AlertTitle>{t('providers.editor.noTraffic')}</AlertTitle>
					<AlertDescription>{t('providers.editor.noModelsHint')}</AlertDescription>
				</Alert>
			: 	<StackedModelList>
					{visibleModels.map(({ row, index }) => {
						const modelName = row.model.trim()
						const unpriced = Boolean(modelName) && !hasBillablePricingModelId(
							pricedModels,
							modelName,
							row.redirect,
							reasoningSuffixMap
						)
						const badge = (
							<ModelBadge
								model={modelName}
								provider={metadataProvider.get(modelName)}
								multiplier={row.multiplier}
								redirect={row.redirect}
								highlightUnpriced={unpriced}
								className='pointer-events-none'
							/>
						)
						return selecting ?
								<label key={`${modelName}-${index}`} className='inline-flex cursor-pointer items-center gap-2'>
									<Checkbox checked={selected.has(row.model)} onCheckedChange={() => toggleSelected(row.model)} />
									{badge}
								</label>
							: 	<Button
									key={`${modelName}-${index}`}
									type='button'
									variant='ghost'
									className='h-auto max-w-full rounded-md p-0 text-left'
									onClick={() => openEditor(index)}
									aria-label={t('common.editItem', { name: modelName })}
								>
									{badge}
								</Button>
					})}
				</StackedModelList>
			}

			<Dialog open={editor !== null} onOpenChange={open => { if (!open) setEditor(null) }}>
				<DialogContent className='max-w-lg' dirty={dirty}>
					<DialogHeader>
						<DialogTitle>
							{editor?.index === null ? t('providers.editor.addModel') : t('providers.editor.editModel')}
						</DialogTitle>
						<DialogDescription>
							{t('providers.editor.modelDialogDescription')}
						</DialogDescription>
					</DialogHeader>

					{editor ?
						<form className='flex flex-col gap-5' onSubmit={event => { event.preventDefault(); saveModel() }}>
							<FieldGroup className='gap-4'>
								<Field data-invalid={Boolean(editor.errors.model)}>
									<FieldLabel htmlFor='channel-model-name'>
										{editor.index === null ? t('providers.editor.logicalModels') : t('providers.editor.logicalModel')}
									</FieldLabel>
									{editor.index === null ?
										<Textarea
											id='channel-model-name'
											rows={4}
											value={editor.draft.model}
											onChange={event => updateDraft({ model: event.target.value })}
											aria-invalid={Boolean(editor.errors.model)}
											className='font-mono'
											autoFocus
										/>
									: 	<Input
											id='channel-model-name'
											value={editor.draft.model}
											onChange={event => updateDraft({ model: event.target.value })}
											aria-invalid={Boolean(editor.errors.model)}
											className='font-mono'
											autoFocus
										/>
									}
									{editor.index === null ? <FieldDescription>{t('providers.editor.logicalModelsHint')}</FieldDescription> : null}
									<FieldError>{editor.errors.model}</FieldError>
								</Field>

								<Field data-disabled={draftIdCount > 1}>
									<FieldLabel htmlFor='channel-model-redirect'>{t('providers.editor.upstreamModel')}</FieldLabel>
									<Input
										id='channel-model-redirect'
										value={editor.draft.redirect}
										disabled={draftIdCount > 1}
										onChange={event => updateDraft({ redirect: event.target.value })}
										placeholder={t('providers.editor.sameAsLogical')}
										className='font-mono'
									/>
									<FieldDescription>
										{draftIdCount > 1 ? t('providers.editor.upstreamModelMultiHint') : t('providers.editor.upstreamModelHint')}
									</FieldDescription>
								</Field>

								<Field data-invalid={Boolean(editor.errors.multiplier)}>
									<FieldLabel htmlFor='channel-model-multiplier'>{t('providers.editor.multiplier')}</FieldLabel>
									<Input
										id='channel-model-multiplier'
										type='text'
										inputMode='decimal'
										value={editor.draft.multiplier}
										onChange={event => updateDraft({ multiplier: event.target.value })}
										aria-invalid={Boolean(editor.errors.multiplier)}
									/>
									<FieldDescription>{t('providers.editor.multiplierHint')}</FieldDescription>
									<FieldError>{editor.errors.multiplier}</FieldError>
								</Field>
							</FieldGroup>

							<DialogFooter className='gap-2 sm:justify-between sm:space-x-0'>
								{editor.index !== null ?
									<Button type='button' variant='destructive' onClick={deleteModel}>
										<Trash2 data-icon />
										{t('providers.editor.deleteModel')}
									</Button>
								: 	<div className='hidden sm:block' />
								}
								<div className='flex flex-col-reverse gap-2 sm:flex-row'>
									<Button type='button' variant='outline' onClick={() => setEditor(null)}>
										{t('common.cancel')}
									</Button>
									<Button type='submit'>{t('providers.editor.saveModel')}</Button>
								</div>
							</DialogFooter>
						</form>
					: 	null}
				</DialogContent>
			</Dialog>
		</section>
	)
}
