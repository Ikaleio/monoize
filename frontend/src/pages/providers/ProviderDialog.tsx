import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import useSWR from 'swr'
import { Save } from 'lucide-react'
import { Button } from '@/components/ui/button'
import {
	Dialog,
	DialogContent,
	DialogDescription,
	DialogFooter,
	DialogHeader,
	DialogTitle
} from '@/components/ui/dialog'
import { Skeleton } from '@/components/ui/skeleton'
import { toast } from 'sonner'
import { api } from '@/lib/api'
import type {
	CreateProviderInput,
	ModelMetadataRecord,
	Provider,
	SystemSettings,
	TransformRegistryItem
} from '@/lib/api'
import {
	createProviderOptimistic,
	providerDetailSWRKey,
	updateProviderOptimistic,
	useDashboardGroups
} from '@/lib/swr'
import { findFirstInvalidTransformRule } from '@/components/transforms/transform-schema'
import { ModelPickerDialog } from './ModelPickerDialog'
import {
	ApiTypeOverridesSection,
	ChannelsSection,
	ModelsSection,
	ProbeSettingsSection,
	ProviderBasicsSection,
	ProviderDialogSectionsDivider,
	TimeoutEnabledSection,
	TransformsSection
} from './provider-dialog-sections'
import {
	BaseUrlV1AlertDialog,
	ChannelEditorDialog,
	ModelEditorDialog,
	UnsavedChangesAlertDialog
} from './provider-dialog-overlays'
import {
	buildPricedModelIdSet,
	type ChannelRow,
	emptyChannelRow,
	emptyForm,
	emptyModelRow,
	fromProvider,
	hasTrailingV1,
	type ModelRow,
	type ProviderForm,
	removeTrailingV1
} from './shared'

function cloneModelRow(row: ModelRow): ModelRow {
	return { ...row }
}

function cloneChannelRow(row: ChannelRow): ChannelRow {
	return { ...row, groups: [...row.groups] }
}

function hasModelNameConflict(
	models: ModelRow[],
	modelName: string,
	excludeIndex: number | null
): boolean {
	const trimmed = modelName.trim()
	if (!trimmed) return false
	return models.some(
		(row, index) => index !== excludeIndex && row.model.trim() === trimmed
	)
}

export function ProviderDialog({
	open,
	onOpenChange,
	mode,
	current,
	providers,
	transformRegistry,
	modelMetadata,
	reasoningSuffixMap,
	settings
}: {
	open: boolean
	onOpenChange: (open: boolean) => void
	mode: 'create' | 'edit'
	current: Provider | null
	providers: Provider[]
	transformRegistry: TransformRegistryItem[]
	modelMetadata: ModelMetadataRecord[]
	reasoningSuffixMap: Record<string, string>
	settings?: SystemSettings
}) {
	const { t } = useTranslation()
	const [loading, setLoading] = useState(false)
	const [modelPickerOpen, setModelPickerOpen] = useState(false)
	const [editingModelIndex, setEditingModelIndex] = useState<number | null>(null)
	const [modelDraft, setModelDraft] = useState<ModelRow | null>(null)
	const [editingChannelIndex, setEditingChannelIndex] = useState<number | null>(null)
	const [channelDraft, setChannelDraft] = useState<ChannelRow | null>(null)
	const [hydratedKey, setHydratedKey] = useState<string | null>(null)
	const [form, setForm] = useState<ProviderForm>(() =>
		mode === 'edit' && current ? fromProvider(current) : emptyForm()
	)
	const [baseUrlPrompt, setBaseUrlPrompt] = useState<{
		original: string
		trimmed: string
	} | null>(null)
	const [v1KeepConfirmed, setV1KeepConfirmed] = useState<string | null>(null)
	const [unsavedChangesOpen, setUnsavedChangesOpen] = useState(false)
	const initialFormRef = useRef<string | null>(null)

	const isEdit = mode === 'edit'
	const { data: dashboardGroups = [], isLoading: isDashboardGroupsLoading } =
		useDashboardGroups(open)

	const {
		data: editProviderDetail,
		isLoading: isLoadingEditProviderDetail,
		error: editProviderDetailError
	} = useSWR(
		open && isEdit && current ? providerDetailSWRKey(current.id) : null,
		() => api.getProvider(current!.id)
	)

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
	const probeGlobalDefaults = useMemo(
		() => ({
			active_probe_enabled: settings?.monoize_active_probe_enabled,
			active_probe_interval_seconds:
				settings?.monoize_active_probe_interval_seconds,
			active_probe_success_threshold:
				settings?.monoize_active_probe_success_threshold,
			active_probe_model: settings?.monoize_active_probe_model ?? null
		}),
		[settings]
	)
	const channelGlobalDefaults = useMemo(
		() => ({
			passive_failure_count_threshold:
				settings?.monoize_passive_failure_threshold,
			passive_window_seconds: settings?.monoize_passive_window_seconds,
			passive_cooldown_seconds: settings?.monoize_passive_cooldown_seconds,
			passive_rate_limit_cooldown_seconds:
				settings?.monoize_passive_rate_limit_cooldown_seconds,
		}),
		[settings]
	)
	const timeoutGlobalDefaults = useMemo(
		() => ({ request_timeout_ms: settings?.monoize_request_timeout_ms }),
		[settings]
	)

	const closeModelDialog = useCallback(() => {
		setEditingModelIndex(null)
		setModelDraft(null)
	}, [])

	const closeChannelDialog = useCallback(() => {
		setEditingChannelIndex(null)
		setChannelDraft(null)
		setBaseUrlPrompt(null)
		setV1KeepConfirmed(null)
	}, [])

	const resetFromCurrent = useCallback(() => {
		setForm(emptyForm())
		setModelPickerOpen(false)
		setHydratedKey(null)
		closeModelDialog()
		closeChannelDialog()
	}, [closeChannelDialog, closeModelDialog])

	const applyFormUpdate = useCallback(
		(updater: (prev: ProviderForm) => ProviderForm) => {
			setForm(updater)
		},
		[]
	)

	useEffect(() => {
		if (!open) return

		if (isEdit) {
			const source = editProviderDetail ?? (editProviderDetailError ? current : null)
			if (!source) return
			const nextHydrationKey = `${source.id}:${source.updated_at}`
			if (hydratedKey === nextHydrationKey) return
			setForm(fromProvider(source))
			setModelPickerOpen(false)
			closeModelDialog()
			closeChannelDialog()
			setHydratedKey(nextHydrationKey)
			return
		}

		if (hydratedKey === '__create__') return
		setForm(emptyForm())
		setModelPickerOpen(false)
		closeModelDialog()
		closeChannelDialog()
		setHydratedKey('__create__')
	}, [
		open,
		isEdit,
		current,
		editProviderDetail,
		editProviderDetailError,
		hydratedKey,
		closeChannelDialog,
		closeModelDialog
	])

	const isHydratingForm =
		open &&
		(isEdit ?
			!current ||
			(!editProviderDetail &&
				!editProviderDetailError &&
				(isLoadingEditProviderDetail || hydratedKey === null))
		: 	hydratedKey !== '__create__')

	useEffect(() => {
		if (open && !isHydratingForm) {
			initialFormRef.current = JSON.stringify(form)
		}
		if (!open) {
			initialFormRef.current = null
		}
	}, [open, isHydratingForm, hydratedKey]) // eslint-disable-line react-hooks/exhaustive-deps

	const isDirty = useCallback(() => {
		if (!initialFormRef.current) return false
		return JSON.stringify(form) !== initialFormRef.current
	}, [form])

	const tryClose = useCallback(() => {
		if (isDirty()) {
			setUnsavedChangesOpen(true)
		} else {
			resetFromCurrent()
			onOpenChange(false)
		}
	}, [isDirty, onOpenChange, resetFromCurrent])

	const handleFetchModels = useCallback(() => {
		if (isEdit) {
			if (!current) return
			setModelPickerOpen(true)
			return
		}
		const channel = form.channels.find(c => c.base_url.trim() && c.api_key.trim())
		if (!channel) {
			toast.error(t('providers.fetchModelsNeedChannel'))
			return
		}
		setModelPickerOpen(true)
	}, [current, form.channels, isEdit, t])

	const fetchChannelInfo = useMemo(() => {
		if (isEdit) return undefined
		const channel = form.channels.find(c => c.base_url.trim() && c.api_key.trim())
		if (!channel) return undefined
		return { base_url: channel.base_url.trim(), api_key: channel.api_key.trim() }
	}, [isEdit, form.channels])

	const existingModelNames = useMemo(
		() => form.models.map(m => m.model.trim()).filter(Boolean),
		[form.models]
	)

	const handleModelsConfirm = useCallback(
		(checkedModels: string[]) => {
			const checkedSet = new Set(checkedModels)
			const currentModelNames = form.models.map(m => m.model.trim())
			const kept = form.models.filter(m => checkedSet.has(m.model.trim()))
			const newModels = checkedModels.filter(m => !currentModelNames.includes(m))
			const added = newModels.map(model => ({
				...emptyModelRow(),
				model
			}))

			const removedCount = form.models.length - kept.length
			const addedCount = added.length

			setForm(prev => ({
				...prev,
				models: [...kept, ...added]
			}))
			closeModelDialog()

			if (addedCount > 0 && removedCount > 0) {
				toast.success(
					t('providers.modelsAdded', { count: addedCount }) +
						', ' +
						t('providers.modelsRemoved', { count: removedCount })
				)
			} else if (addedCount > 0) {
				toast.success(t('providers.modelsAdded', { count: addedCount }))
			} else if (removedCount > 0) {
				toast.success(t('providers.modelsRemoved', { count: removedCount }))
			}
		},
		[closeModelDialog, form.models, t]
	)

	const deleteModelAt = useCallback(
		(idx: number) => {
			setForm(prev => ({
				...prev,
				models: prev.models.filter((_, index) => index !== idx)
			}))
			setEditingModelIndex(prev => {
				if (prev === null) return null
				if (prev === idx) return null
				if (prev > idx) return prev - 1
				return prev
			})
			setModelDraft(prev => {
				if (editingModelIndex !== idx) return prev
				return null
			})
		},
		[editingModelIndex]
	)

	const openCreateModelDialog = useCallback(() => {
		setEditingModelIndex(null)
		setModelDraft(emptyModelRow())
	}, [])

	const openEditModelDialog = useCallback(
		(index: number) => {
			const row = form.models[index]
			if (!row) return
			setEditingModelIndex(index)
			setModelDraft(cloneModelRow(row))
		},
		[form.models]
	)

	const handleModelDialogSave = useCallback(() => {
		if (!modelDraft) return

		const trimmedModel = modelDraft.model.trim()
		if (!trimmedModel) {
			toast.error(t('providers.validationModelRequired'))
			return
		}

		if (hasModelNameConflict(form.models, trimmedModel, editingModelIndex)) {
			toast.error(t('providers.validationDuplicateModel'))
			return
		}

		const multiplier = Number(modelDraft.multiplier)
		if (!Number.isFinite(multiplier) || multiplier < 0) {
			toast.error(t('providers.validationMultiplier'))
			return
		}

		const nextRow: ModelRow = {
			model: trimmedModel,
			redirect: modelDraft.redirect,
			multiplier: modelDraft.multiplier
		}

		setForm(prev => ({
			...prev,
			models:
				editingModelIndex === null ?
					[...prev.models, nextRow]
				: 	prev.models.map((row, index) =>
						index === editingModelIndex ? nextRow : row
					)
		}))
		closeModelDialog()
	}, [closeModelDialog, editingModelIndex, form.models, modelDraft, t])

	const deleteChannelAt = useCallback(
		(idx: number) => {
			setForm(prev => ({
				...prev,
				channels: prev.channels.filter((_, index) => index !== idx)
			}))
			setEditingChannelIndex(prev => {
				if (prev === null) return null
				if (prev === idx) return null
				if (prev > idx) return prev - 1
				return prev
			})
			setChannelDraft(prev => {
				if (editingChannelIndex !== idx) return prev
				return null
			})
			setBaseUrlPrompt(null)
			setV1KeepConfirmed(null)
		},
		[editingChannelIndex]
	)

	const openCreateChannelDialog = useCallback(() => {
		setEditingChannelIndex(null)
		setChannelDraft(emptyChannelRow())
		setBaseUrlPrompt(null)
		setV1KeepConfirmed(null)
	}, [])

	const openEditChannelDialog = useCallback(
		(index: number) => {
			const row = form.channels[index]
			if (!row) return
			setEditingChannelIndex(index)
			setChannelDraft(cloneChannelRow(row))
			setBaseUrlPrompt(null)
			setV1KeepConfirmed(null)
		},
		[form.channels]
	)

	const updateChannelDraft = useCallback((patch: Partial<ChannelRow>) => {
		setChannelDraft(prev => (prev ? { ...prev, ...patch } : prev))
	}, [])

	const updateChannelDraftBaseUrl = useCallback((baseUrl: string) => {
		setChannelDraft(prev => (prev ? { ...prev, base_url: baseUrl } : prev))
		setV1KeepConfirmed(null)
	}, [])

	const handleBaseUrlBlur = useCallback(() => {
		const raw = channelDraft?.base_url ?? ''
		const trimmed = raw.trim()
		if (!trimmed || !hasTrailingV1(trimmed)) {
			return
		}
		if (v1KeepConfirmed === trimmed) {
			return
		}
		const normalized = removeTrailingV1(trimmed)
		if (!normalized) {
			return
		}
		setBaseUrlPrompt({ original: raw, trimmed: normalized })
	}, [channelDraft?.base_url, v1KeepConfirmed])

	const handleChannelDialogSave = useCallback(() => {
		if (!channelDraft) return

		const nextRow = cloneChannelRow(channelDraft)
		setForm(prev => ({
			...prev,
			channels:
				editingChannelIndex === null ?
					[...prev.channels, nextRow]
				: 	prev.channels.map((row, index) =>
						index === editingChannelIndex ? nextRow : row
					)
		}))
		closeChannelDialog()
	}, [channelDraft, closeChannelDialog, editingChannelIndex])

	const validateAndBuild = useCallback((): CreateProviderInput | null => {
		if (!form.name.trim()) {
			toast.error(t('providers.validationNameRequired'))
			return null
		}

		const models: Record<string, { redirect: string | null; multiplier: number }> = {}
		const seenModelNames = new Set<string>()
		for (const row of form.models) {
			const trimmedModel = row.model.trim()
			if (!trimmedModel) {
				toast.error(t('providers.validationModelRequired'))
				return null
			}
			if (seenModelNames.has(trimmedModel)) {
				toast.error(t('providers.validationDuplicateModel'))
				return null
			}
			seenModelNames.add(trimmedModel)

			const multiplier = Number(row.multiplier)
			if (!Number.isFinite(multiplier) || multiplier < 0) {
				toast.error(t('providers.validationMultiplier'))
				return null
			}
			models[trimmedModel] = {
				redirect: row.redirect.trim() ? row.redirect.trim() : null,
				multiplier
			}
		}
		if (Object.keys(models).length === 0) {
			toast.error(t('providers.validationAtLeastOneModel'))
			return null
		}

		const channels = form.channels.map(row => ({
			id: row.id.trim() || undefined,
			name: row.name.trim(),
			base_url: row.base_url.trim(),
			api_key: row.api_key.trim() || undefined,
			weight: Number(row.weight),
			enabled: row.enabled,
			groups: row.groups.map(group => group.trim()).filter(Boolean),
			passive_failure_count_threshold_override:
				row.passive_failure_count_threshold_override.trim() ?
					Number(row.passive_failure_count_threshold_override)
				: null,
			passive_cooldown_seconds_override:
				row.passive_cooldown_seconds_override.trim() ?
					Number(row.passive_cooldown_seconds_override)
				: null,
			passive_window_seconds_override:
				row.passive_window_seconds_override.trim() ?
					Number(row.passive_window_seconds_override)
				: null,
			passive_rate_limit_cooldown_seconds_override:
				row.passive_rate_limit_cooldown_seconds_override.trim() ?
					Number(row.passive_rate_limit_cooldown_seconds_override)
				: null
		}))

		for (const channel of channels) {
			if (!channel.name) {
				toast.error(t('providers.validationChannelName'))
				return null
			}
			if (!channel.base_url) {
				toast.error(t('providers.validationChannelUrl'))
				return null
			}
			if (!channel.api_key && !(isEdit && channel.id)) {
				toast.error(t('providers.validationChannelKey'))
				return null
			}
			if (!Number.isFinite(channel.weight) || channel.weight < 0) {
				toast.error(t('providers.validationChannelWeight'))
				return null
			}
			if (
				channel.passive_failure_count_threshold_override !== null &&
				(!Number.isFinite(channel.passive_failure_count_threshold_override) ||
					channel.passive_failure_count_threshold_override < 1)
			) {
				toast.error(t('providers.validationChannelPassiveThreshold'))
				return null
			}
			if (
				channel.passive_cooldown_seconds_override !== null &&
				(!Number.isFinite(channel.passive_cooldown_seconds_override) ||
					channel.passive_cooldown_seconds_override < 1)
			) {
				toast.error(t('providers.validationChannelPassiveCooldown'))
				return null
			}
			if (
				channel.passive_window_seconds_override !== null &&
				(!Number.isFinite(channel.passive_window_seconds_override) ||
					channel.passive_window_seconds_override < 1)
			) {
				toast.error(t('providers.validationChannelPassiveWindow'))
				return null
			}
			if (
				channel.passive_rate_limit_cooldown_seconds_override !== null &&
				(!Number.isFinite(channel.passive_rate_limit_cooldown_seconds_override) ||
					channel.passive_rate_limit_cooldown_seconds_override < 1)
			) {
				toast.error(t('providers.validationChannelRateLimitCooldown'))
				return null
			}
		}

		if (channels.length === 0) {
			toast.error(t('providers.validationAtLeastOneChannel'))
			return null
		}

		const invalidRule = findFirstInvalidTransformRule(
			form.transforms,
			transformRegistry
		)
		if (invalidRule) {
			const firstError = invalidRule.errors[0]
			toast.error(
				t('transforms.validationRuleInvalid', {
					index: invalidRule.index + 1,
					reason: `${firstError.field} ${firstError.message}`
				})
			)
			return null
		}

		const requestTimeoutMsOverride =
			form.request_timeout_ms_override.trim() ?
				Number(form.request_timeout_ms_override)
			: null
		if (
			!Number.isFinite(form.channel_retry_interval_ms) ||
			form.channel_retry_interval_ms < 0
		) {
			toast.error(t('providers.validationChannelRetryInterval'))
			return null
		}
		if (
			requestTimeoutMsOverride !== null &&
			(!Number.isFinite(requestTimeoutMsOverride) || requestTimeoutMsOverride < 1)
		) {
			toast.error(t('providers.validationProviderRequestTimeout'))
			return null
		}

		const apiTypeOverrides = form.api_type_overrides.map(override => ({
			pattern: override.pattern.trim(),
			api_type: override.api_type
		}))
		for (const override of apiTypeOverrides) {
			if (!override.pattern) {
				toast.error(t('providers.validationApiTypeOverridePattern'))
				return null
			}
		}

		return {
			name: form.name.trim(),
			provider_type: form.provider_type,
			api_type_overrides: apiTypeOverrides,
			models,
			channels,
			max_retries: form.max_retries,
			channel_max_retries: form.channel_max_retries,
			channel_retry_interval_ms: form.channel_retry_interval_ms,
			circuit_breaker_enabled: form.circuit_breaker_enabled,
			per_model_circuit_break: form.per_model_circuit_break,
			transforms: form.transforms,
			active_probe_enabled_override: form.active_probe_enabled_override,
			active_probe_interval_seconds_override:
				form.active_probe_interval_seconds_override,
			active_probe_success_threshold_override:
				form.active_probe_success_threshold_override,
			active_probe_model_override:
				form.active_probe_model_override?.trim() ?
					form.active_probe_model_override.trim()
				: null,
			request_timeout_ms_override: requestTimeoutMsOverride,
			enabled: form.enabled,
			priority: form.priority
		}
	}, [form, isEdit, t, transformRegistry])

	const onSubmit = useCallback(async () => {
		const payload = validateAndBuild()
		if (!payload) return
		setLoading(true)
		try {
			if (isEdit && current) {
				await updateProviderOptimistic(current.id, payload, providers)
				toast.success(t('providers.updateSuccess'))
			} else {
				await createProviderOptimistic(payload, providers)
				toast.success(t('providers.createSuccess'))
			}
			onOpenChange(false)
			resetFromCurrent()
		} catch (error) {
			toast.error(error instanceof Error ? error.message : t('common.error'))
		} finally {
			setLoading(false)
		}
	}, [current, isEdit, onOpenChange, providers, resetFromCurrent, t, validateAndBuild])

	return (
		<>
			<Dialog
				open={open}
				onOpenChange={value => {
					if (!value) {
						tryClose()
						return
					}
					onOpenChange(value)
				}}
			>
				<DialogContent className='max-h-[85vh] overflow-y-auto w-[min(96vw,1200px)] max-w-[1200px]'>
					<DialogHeader>
						<DialogTitle>
							{isEdit ? t('providers.editProvider') : t('providers.createProvider')}
						</DialogTitle>
						<DialogDescription>
							{t('providers.createProviderDesc')}
						</DialogDescription>
					</DialogHeader>

					{isHydratingForm ?
						<div className='space-y-4 py-2'>
							<Skeleton className='h-10 w-full' />
							<Skeleton className='h-10 w-full' />
							<Skeleton className='h-32 w-full' />
							<Skeleton className='h-32 w-full' />
						</div>
					: 	<div className='space-y-6'>
							<div className='grid grid-cols-1 md:grid-cols-2 gap-4'>
								<ProviderBasicsSection
									form={form}
									isEdit={isEdit}
									t={t}
									onFormChange={applyFormUpdate}
								/>
								<ProbeSettingsSection
									form={form}
									t={t}
									globalDefaults={probeGlobalDefaults}
									onFormChange={applyFormUpdate}
								/>
								<TimeoutEnabledSection
									form={form}
									t={t}
									globalDefaults={timeoutGlobalDefaults}
									onFormChange={applyFormUpdate}
								/>
							</div>

							<ApiTypeOverridesSection
								form={form}
								t={t}
								onFormChange={applyFormUpdate}
							/>

							<ProviderDialogSectionsDivider />

							<ModelsSection
								form={form}
								editingModelIndex={editingModelIndex}
								modelProviderMap={modelProviderMap}
								pricedModelIdSet={pricedModelIdSet}
								reasoningSuffixMap={reasoningSuffixMap}
								t={t}
								onFetchModels={handleFetchModels}
								onAddModel={openCreateModelDialog}
								onEditModel={openEditModelDialog}
								onDeleteModel={deleteModelAt}
							/>

							<ProviderDialogSectionsDivider />

							<ChannelsSection
								form={form}
								editingChannelIndex={editingChannelIndex}
								isEdit={isEdit}
								t={t}
								onAddChannel={openCreateChannelDialog}
								onSelectChannel={openEditChannelDialog}
								onToggleChannelEnabled={(idx, enabled) =>
									setForm(prev => ({
										...prev,
										channels: prev.channels.map((channel, index) =>
											index === idx ? { ...channel, enabled } : channel
										)
									}))
								}
								onDeleteChannel={deleteChannelAt}
							/>

							<ProviderDialogSectionsDivider />

							<TransformsSection
								transformRegistry={transformRegistry}
								transforms={form.transforms}
								t={t}
								onChange={next =>
									setForm(prev => ({ ...prev, transforms: next }))
								}
							/>
						</div>
					}

					<DialogFooter>
						<Button type='button' variant='outline' onClick={tryClose}>
							{t('common.cancel')}
						</Button>
						<Button
							type='button'
							onClick={() => void onSubmit()}
							disabled={loading || isHydratingForm}
						>
							<Save className='h-4 w-4 mr-2' />
							{loading ? t('common.saving') : t('common.save')}
						</Button>
					</DialogFooter>
				</DialogContent>
			</Dialog>

			<BaseUrlV1AlertDialog
				prompt={baseUrlPrompt}
				t={t}
				onOpenChange={value => {
					if (!value) {
						setBaseUrlPrompt(null)
					}
				}}
				onKeep={() => {
					if (!baseUrlPrompt) return
					setV1KeepConfirmed(baseUrlPrompt.original.trim())
					setBaseUrlPrompt(null)
				}}
				onRemove={() => {
					if (!baseUrlPrompt) return
					setChannelDraft(prev =>
						prev ? { ...prev, base_url: baseUrlPrompt.trimmed } : prev
					)
					setBaseUrlPrompt(null)
					setV1KeepConfirmed(null)
				}}
			/>

			<UnsavedChangesAlertDialog
				open={unsavedChangesOpen}
				t={t}
				onOpenChange={setUnsavedChangesOpen}
				onDiscard={() => {
					setUnsavedChangesOpen(false)
					resetFromCurrent()
					onOpenChange(false)
				}}
				onSave={() => {
					setUnsavedChangesOpen(false)
					void onSubmit()
				}}
			/>

			<ModelPickerDialog
				open={modelPickerOpen}
				onOpenChange={setModelPickerOpen}
				providerId={isEdit ? current?.id : undefined}
				channelInfo={fetchChannelInfo}
				providerName={form.name || current?.name || ''}
				existingModels={existingModelNames}
				modelMetadata={modelMetadata}
				reasoningSuffixMap={reasoningSuffixMap}
				onConfirm={handleModelsConfirm}
			/>

			<ModelEditorDialog
				open={modelDraft !== null}
				model={modelDraft}
				isDraft={editingModelIndex === null}
				canDelete={editingModelIndex !== null}
				t={t}
				onOpenChange={value => {
					if (!value) {
						closeModelDialog()
					}
				}}
				onChangeModel={value => {
					setModelDraft(prev => (prev ? { ...prev, model: value } : prev))
				}}
				onChangeRedirect={value => {
					setModelDraft(prev => (prev ? { ...prev, redirect: value } : prev))
				}}
				onChangeMultiplier={value => {
					setModelDraft(prev => (prev ? { ...prev, multiplier: value } : prev))
				}}
				onDelete={() => {
					if (editingModelIndex === null) return
					deleteModelAt(editingModelIndex)
					closeModelDialog()
				}}
				onSave={handleModelDialogSave}
			/>

			<ChannelEditorDialog
				open={channelDraft !== null}
				channel={channelDraft}
				t={t}
				isEdit={isEdit}
				canDelete={editingChannelIndex !== null}
				globalDefaults={channelGlobalDefaults}
				groupSuggestions={dashboardGroups}
				groupSuggestionsLoading={isDashboardGroupsLoading}
				onOpenChange={value => {
					if (!value) {
						closeChannelDialog()
					}
				}}
				onChange={updateChannelDraft}
				onBaseUrlChange={updateChannelDraftBaseUrl}
				onBaseUrlBlur={handleBaseUrlBlur}
				onDelete={() => {
					if (editingChannelIndex === null) return
					deleteChannelAt(editingChannelIndex)
					closeChannelDialog()
				}}
				onSave={handleChannelDialogSave}
			/>
		</>
	)
}
