import { useState } from 'react'
import { CloudDownload, Layers3, Plus, Trash2 } from 'lucide-react'
import { ModelBadge } from '@/components/ModelBadge'
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
import {
	hasBillablePricingModelId,
	type ModelRow
} from './shared'

type Copy = (zh: string, en: string) => string

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
	c: Copy
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
	reasoningSuffixMap,
	c
}: ChannelModelEditorProps) {
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
			errors.model = c('请输入逻辑模型名称。', 'Enter a logical model name.')
		} else if (models.some((row, index) => index !== editor.index && row.model.trim() === model)) {
			errors.model = c('当前 Channel 已存在同名模型。', 'This channel already contains that model.')
		}

		if (!Number.isFinite(Number(multiplier)) || Number(multiplier) <= 0) {
			errors.multiplier = c('倍率必须是大于 0 的数字。', 'Multiplier must be a number greater than zero.')
		}

		if (errors.model || errors.multiplier) {
			setEditor(current => current ? { ...current, errors } : current)
			return
		}

		const nextModel: ModelRow = {
			model,
			redirect: editor.draft.redirect.trim(),
			multiplier
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
						<h4 className='font-medium'>{c('支持的模型', 'Supported models')}</h4>
						<Badge variant='secondary'>{models.length}</Badge>
					</div>
					<p className='mt-1 text-xs text-muted-foreground'>
						{c('点击模型 badge 编辑重定向与倍率。', 'Click a model badge to edit its redirect and multiplier.')}
					</p>
				</div>
				<div className='flex items-center gap-2'>
					<Button variant='outline' size='sm' onClick={onOpenPicker}>
						<CloudDownload data-icon />
						{c('从上游获取', 'Fetch upstream')}
					</Button>
					<Button size='sm' onClick={openAdd}>
						<Plus data-icon />
						{c('手动添加', 'Add manually')}
					</Button>
				</div>
			</div>

			{models.length === 0 ?
				<Alert>
					<Layers3 className='size-4' />
					<AlertTitle>{c('当前 Channel 不会接收请求', 'This channel will not receive traffic')}</AlertTitle>
					<AlertDescription>{c('从上游获取模型，或手动添加一个逻辑模型。', 'Fetch models from the upstream or add a logical model manually.')}</AlertDescription>
				</Alert>
			: 	<div className='rounded-lg border bg-muted/10 p-3'>
					<div className='flex max-h-52 flex-wrap content-start gap-1.5 overflow-y-auto'>
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
									className='h-auto max-w-full rounded-full p-0 text-left'
									onClick={() => openEdit(index)}
									aria-label={c(`编辑模型 ${modelName}`, `Edit model ${modelName}`)}
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
					</div>
				</div>
			}

			<Dialog open={editor !== null} onOpenChange={open => { if (!open) setEditor(null) }}>
				<DialogContent className='max-w-lg'>
					<DialogHeader>
						<DialogTitle>
							{editor?.index === null ? c('添加模型', 'Add model') : c('编辑模型', 'Edit model')}
						</DialogTitle>
						<DialogDescription>
							{c('设置当前 Channel 的逻辑模型、上游目标和计费倍率。', 'Configure the logical model, upstream target, and billing multiplier for this channel.')}
						</DialogDescription>
					</DialogHeader>

					{editor ?
						<form className='flex flex-col gap-5' onSubmit={event => { event.preventDefault(); saveModel() }}>
							<FieldGroup className='gap-4'>
								<Field data-invalid={Boolean(editor.errors.model)}>
									<FieldLabel htmlFor='channel-model-name'>{c('逻辑模型', 'Logical model')}</FieldLabel>
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
									<FieldLabel htmlFor='channel-model-redirect'>{c('上游模型', 'Upstream model')}</FieldLabel>
									<Input
										id='channel-model-redirect'
										value={editor.draft.redirect}
										onChange={event => updateDraft({ redirect: event.target.value })}
										placeholder={c('同逻辑模型', 'Same as logical model')}
										className='font-mono'
									/>
									<FieldDescription>{c('留空时使用逻辑模型名称。', 'Leave empty to use the logical model name.')}</FieldDescription>
								</Field>

								<Field data-invalid={Boolean(editor.errors.multiplier)}>
									<FieldLabel htmlFor='channel-model-multiplier'>{c('倍率', 'Multiplier')}</FieldLabel>
									<Input
										id='channel-model-multiplier'
										type='number'
										min='0.0001'
										step='any'
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
										{c('删除模型', 'Delete model')}
									</Button>
								: 	<div className='hidden sm:block' />
								}
								<div className='flex flex-col-reverse gap-2 sm:flex-row'>
									<Button type='button' variant='outline' onClick={() => setEditor(null)}>
										{c('取消', 'Cancel')}
									</Button>
									<Button type='submit'>{c('保存模型', 'Save model')}</Button>
								</div>
							</DialogFooter>
						</form>
					: 	null}
				</DialogContent>
			</Dialog>
		</section>
	)
}
