import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { CloudDownload, Layers3, Plus, Trash2 } from 'lucide-react'
import { ModelBadge } from '@/components/ModelBadge'
import { StackedModelList } from '@/components/StackedModelList'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
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
import { normalizeMultiplier } from '@/lib/exact-decimal'
import {
	hasBillablePricingModelId,
	type ModelRow
} from './shared'

type ModelEditorState = {
	index: number | null
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
}

const emptyModel = (): ModelRow => ({
	model: '',
	redirect: '',
	multiplier: '1'
})

export function ChannelModelEditor({
	models,
	onChange,
	onOpenPicker,
	pricedModels,
	metadataProvider,
	reasoningSuffixMap
}: ChannelModelEditorProps) {
	const { t } = useTranslation()
	const [editor, setEditor] = useState<ModelEditorState | null>(null)

	const openAdd = () => {
		setEditor({ index: null, draft: emptyModel(), errors: {} })
	}

	const openEdit = (index: number) => {
		const model = models[index]
		if (!model) return
		setEditor({ index, draft: { ...model }, errors: {} })
	}

	const updateDraft = (patch: Partial<ModelRow>) => {
		setEditor(current => {
			if (!current) return current
			return {
				...current,
				draft: { ...current.draft, ...patch },
				errors: {
					...current.errors,
					...(patch.model !== undefined ? { model: undefined } : {}),
					...(patch.multiplier !== undefined ? { multiplier: undefined } : {})
				}
			}
		})
	}

	const saveModel = () => {
		if (!editor) return
		const model = editor.draft.model.trim()
		const multiplier = editor.draft.multiplier.trim()
		const errors: ModelEditorState['errors'] = {}

		if (!model) {
			errors.model = t('providers.editor.modelNameRequired')
		} else if (models.some((row, index) => index !== editor.index && row.model.trim() === model)) {
			errors.model = t('providers.editor.modelDuplicate')
		}

		const normalizedMultiplier = normalizeMultiplier(multiplier)
		if (normalizedMultiplier == null) {
			errors.multiplier = t('providers.editor.multiplierInvalid')
		}

		if (errors.model || errors.multiplier) {
			setEditor(current => current ? { ...current, errors } : current)
			return
		}

		const nextModel: ModelRow = {
			model,
			redirect: editor.draft.redirect.trim(),
			multiplier: normalizedMultiplier ?? multiplier
		}
		onChange(
			editor.index === null ?
				[...models, nextModel]
			: models.map((row, index) => index === editor.index ? nextModel : row)
		)
		setEditor(null)
	}

	const deleteModel = () => {
		if (editor?.index === null || editor?.index === undefined) return
		onChange(models.filter((_, index) => index !== editor.index))
		setEditor(null)
	}

	return (
		<section className='flex flex-col gap-4 rounded-xl border bg-card p-4 sm:p-5'>
			<div className='flex flex-wrap items-center justify-between gap-3'>
				<div>
					<div className='flex items-center gap-2'>
						<Layers3 className='size-4 text-primary' />
						<h4 className='font-medium'>{t('providers.editor.supportedModels')}</h4>
						<Badge variant='secondary'>{models.length}</Badge>
					</div>
					<p className='mt-1 text-xs text-muted-foreground'>
						{t('providers.editor.supportedModelsHint')}
					</p>
				</div>
				<div className='flex items-center gap-2'>
					<Button variant='outline' size='sm' onClick={onOpenPicker}>
						<CloudDownload data-icon />
						{t('providers.editor.fetchUpstream')}
					</Button>
					<Button size='sm' onClick={openAdd}>
						<Plus data-icon />
						{t('providers.editor.addManually')}
					</Button>
				</div>
			</div>

			{models.length === 0 ?
				<Alert>
					<Layers3 className='size-4' />
					<AlertTitle>{t('providers.editor.noTraffic')}</AlertTitle>
					<AlertDescription>{t('providers.editor.noModelsHint')}</AlertDescription>
				</Alert>
			: 	<StackedModelList>
					{models.map((model, index) => {
						const modelName = model.model.trim()
						const unpriced = Boolean(modelName) && !hasBillablePricingModelId(
							pricedModels,
							modelName,
							model.redirect,
							reasoningSuffixMap
						)
						return (
							<Button
								key={`${modelName}-${index}`}
								type='button'
								variant='ghost'
								className='h-auto max-w-full rounded-md p-0 text-left'
								onClick={() => openEdit(index)}
								aria-label={t('common.editItem', { name: modelName })}
							>
								<ModelBadge
									model={modelName}
									provider={metadataProvider.get(modelName)}
									multiplier={model.multiplier}
									redirect={model.redirect}
									highlightUnpriced={unpriced}
									className='pointer-events-none'
								/>
							</Button>
						)
					})}
				</StackedModelList>
			}

			<Dialog open={editor !== null} onOpenChange={open => { if (!open) setEditor(null) }}>
				<DialogContent className='max-w-lg'>
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
									<FieldLabel htmlFor='channel-model-name'>{t('providers.editor.logicalModel')}</FieldLabel>
									<Input
										id='channel-model-name'
										value={editor.draft.model}
										onChange={event => updateDraft({ model: event.target.value })}
										aria-invalid={Boolean(editor.errors.model)}
										className='font-mono'
										autoFocus
									/>
									<FieldError>{editor.errors.model}</FieldError>
								</Field>

								<Field>
									<FieldLabel htmlFor='channel-model-redirect'>{t('providers.editor.upstreamModel')}</FieldLabel>
									<Input
										id='channel-model-redirect'
										value={editor.draft.redirect}
										onChange={event => updateDraft({ redirect: event.target.value })}
										placeholder={t('providers.editor.sameAsLogical')}
										className='font-mono'
									/>
									<FieldDescription>{t('providers.editor.upstreamModelHint')}</FieldDescription>
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
