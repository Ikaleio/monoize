import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import useSWR from 'swr'
import {
	ArrowDown,
	ArrowUp,
	GripVertical,
	Plus,
	Server,
	Trash2,
	Save,
	Pencil,
	Download,
	Search,
	Layers,
	Settings2,
	X,
	Globe,
	Radio,
	Weight,
	ChevronRight,
	AlertTriangle,
	Zap,
	Activity,
	Check,
	Loader2,
	Play
} from 'lucide-react'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Switch } from '@/components/ui/switch'
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
	AlertDialog,
	AlertDialogAction,
	AlertDialogCancel,
	AlertDialogContent,
	AlertDialogDescription,
	AlertDialogFooter,
	AlertDialogHeader,
	AlertDialogTitle
} from '@/components/ui/alert-dialog'
import {
	Select,
	SelectContent,
	SelectItem,
	SelectTrigger,
	SelectValue
} from '@/components/ui/select'
import {
	Tooltip,
	TooltipContent,
	TooltipProvider,
	TooltipTrigger
} from '@/components/ui/tooltip'
import { Separator } from '@/components/ui/separator'
import { Skeleton } from '@/components/ui/skeleton'
import { toast } from 'sonner'
import { api } from '@/lib/api'
import { ModelBadge } from '@/components/ModelBadge'
import { cn } from '@/lib/utils'
import type {
	CreateProviderInput,
	Provider,
	TransformRuleConfig,
	TransformRegistryItem,
	UpdateProviderInput,
	ModelMetadataRecord,
	ChannelTestResult
} from '@/lib/api'
import {
	useProviders,
	useTransformRegistry,
	createProviderOptimistic,
	updateProviderOptimistic,
	deleteProviderOptimistic,
	reorderProviders
} from '@/lib/swr'
import { PageWrapper, motion, transitions } from '@/components/ui/motion'
import { AnimatePresence } from 'framer-motion'
import { Virtuoso } from 'react-virtuoso'
import { TransformChainEditor } from '@/components/transforms/transform-chain-editor'
import { findFirstInvalidTransformRule } from '@/components/transforms/transform-schema'
import { OpenAI, Anthropic, Google, XAI } from '@lobehub/icons'

const PROVIDER_TYPE_CONFIG: Record<
	ProviderForm['provider_type'],
	{
		label: string
		path: string
		icon: React.ComponentType<{ className?: string }>
	}
> = {
	chat_completion: {
		label: 'Chat Completion',
		path: '/v1/chat/completions',
		icon: OpenAI
	},
	responses: { label: 'Responses', path: '/v1/responses', icon: OpenAI },
	messages: { label: 'Messages', path: '/v1/messages', icon: Anthropic },
	gemini: {
		label: 'Gemini',
		path: '/v1beta/models/{model}:generateContent',
		icon: Google
	},
	grok: { label: 'Responses (Grok)', path: '/v1/responses', icon: XAI }
}

const PROVIDER_CHANNEL_OVERVIEW_ROW_HEIGHT = 40
const PROVIDER_EDIT_CHANNEL_ROW_HEIGHT = 56

type ModelRow = {
	model: string
	redirect: string
	multiplier: string
}

type ChannelRow = {
	id: string
	name: string
	base_url: string
	api_key: string
	weight: string
	enabled: boolean
	passive_failure_threshold_override: string
	passive_cooldown_seconds_override: string
	passive_window_seconds_override: string
	passive_min_samples_override: string
	passive_failure_rate_threshold_override: string
	passive_rate_limit_cooldown_seconds_override: string
}

type ProviderForm = {
	id?: string
	name: string
	provider_type:
		| 'responses'
		| 'chat_completion'
		| 'messages'
		| 'gemini'
		| 'grok'
	enabled: boolean
	max_retries: number
	active_probe_enabled_override: boolean | null
	active_probe_interval_seconds_override: number | null
	active_probe_success_threshold_override: number | null
	active_probe_model_override: string | null
	request_timeout_ms_override: string
	priority?: number
	models: ModelRow[]
	channels: ChannelRow[]
	transforms: TransformRuleConfig[]
}

function emptyForm(): ProviderForm {
	return {
		id: '',
		name: '',
		provider_type: 'chat_completion',
		enabled: true,
		max_retries: -1,
		active_probe_enabled_override: null,
		active_probe_interval_seconds_override: null,
		active_probe_success_threshold_override: null,
		active_probe_model_override: null,
		request_timeout_ms_override: '',
		priority: undefined,
		models: [],
		channels: [],
		transforms: []
	}
}

function fromProvider(provider: Provider): ProviderForm {
	return {
		id: provider.id,
		name: provider.name,
		provider_type: provider.provider_type,
		enabled: provider.enabled,
		max_retries: provider.max_retries,
		active_probe_enabled_override:
			provider.active_probe_enabled_override ?? null,
		active_probe_interval_seconds_override:
			provider.active_probe_interval_seconds_override ?? null,
		active_probe_success_threshold_override:
			provider.active_probe_success_threshold_override ?? null,
		active_probe_model_override: provider.active_probe_model_override ?? null,
		request_timeout_ms_override:
			provider.request_timeout_ms_override != null ?
				String(provider.request_timeout_ms_override)
			:	'',
		priority: provider.priority,
		models: Object.entries(provider.models).map(([model, entry]) => ({
			model,
			redirect: entry.redirect ?? '',
			multiplier: String(entry.multiplier)
		})),
		channels: provider.channels.map(channel => ({
			id: channel.id,
			name: channel.name,
			base_url: channel.base_url,
			api_key: '',
			weight: String(channel.weight),
			enabled: channel.enabled,
			passive_failure_threshold_override:
				channel.passive_failure_threshold_override != null ?
					String(channel.passive_failure_threshold_override)
				:	'',
			passive_cooldown_seconds_override:
				channel.passive_cooldown_seconds_override != null ?
					String(channel.passive_cooldown_seconds_override)
				:	'',
			passive_window_seconds_override:
				channel.passive_window_seconds_override != null ?
					String(channel.passive_window_seconds_override)
				:	'',
			passive_min_samples_override:
				channel.passive_min_samples_override != null ?
					String(channel.passive_min_samples_override)
				:	'',
			passive_failure_rate_threshold_override:
				channel.passive_failure_rate_threshold_override != null ?
					String(channel.passive_failure_rate_threshold_override)
				:	'',
			passive_rate_limit_cooldown_seconds_override:
				channel.passive_rate_limit_cooldown_seconds_override != null ?
					String(channel.passive_rate_limit_cooldown_seconds_override)
				:	''
		})),
		transforms: provider.transforms ?? []
	}
}

function hasTrailingV1(baseUrl: string): boolean {
	return /\/v1\/?$/i.test(baseUrl.trim())
}

function removeTrailingV1(baseUrl: string): string {
	return baseUrl.trim().replace(/\/v1\/?$/i, '')
}

function buildPricedModelIdSet(
	modelMetadata: ModelMetadataRecord[]
): Set<string> {
	const set = new Set<string>()
	for (const item of modelMetadata) {
		if (item.input_cost_per_token_nano && item.output_cost_per_token_nano) {
			set.add(item.model_id)
		}
	}
	return set
}

function resolvePricingModelId(model: string, redirect?: string | null): string {
	const redirectTarget = redirect?.trim()
	return redirectTarget ? redirectTarget : model.trim()
}

function statusBadge(status?: string) {
	if (status === 'healthy') {
		return (
			<Badge className='bg-emerald-600/15 text-emerald-700 hover:bg-emerald-600/15 dark:bg-emerald-500/15 dark:text-emerald-400 border-0'>
				<span className='mr-1.5 h-1.5 w-1.5 rounded-full bg-emerald-500 inline-block animate-pulse' />
				Healthy
			</Badge>
		)
	}
	if (status === 'probing') {
		return (
			<Badge className='bg-amber-500/15 text-amber-700 hover:bg-amber-500/15 dark:bg-amber-500/15 dark:text-amber-400 border-0'>
				<span className='mr-1.5 h-1.5 w-1.5 rounded-full bg-amber-500 inline-block animate-pulse' />
				Probing
			</Badge>
		)
	}
	return (
		<Badge className='bg-red-500/15 text-red-700 hover:bg-red-500/15 dark:bg-red-500/15 dark:text-red-400 border-0'>
			<span className='mr-1.5 h-1.5 w-1.5 rounded-full bg-red-500 inline-block animate-pulse' />
			Unhealthy
		</Badge>
	)
}

function ModelPickerDialog({
	open,
	onOpenChange,
	providerId,
	channelInfo,
	providerName,
	existingModels,
	modelMetadata,
	onConfirm
}: {
	open: boolean
	onOpenChange: (open: boolean) => void
	providerId?: string
	channelInfo?: { base_url: string; api_key: string }
	providerName: string
	existingModels: string[]
	modelMetadata: ModelMetadataRecord[]
	onConfirm: (checkedModels: string[]) => void
}) {
	const { t } = useTranslation()
	const [loading, setLoading] = useState(false)
	const [fetchedModels, setFetchedModels] = useState<string[]>([])
	const [checked, setChecked] = useState<Set<string>>(
		() => new Set(existingModels)
	)
	const [search, setSearch] = useState('')
	const [tab, setTab] = useState<'new' | 'existing'>('new')
	const initializedForOpenRef = useRef(false)

	const existingSet = useMemo(() => new Set(existingModels), [existingModels])

	const newModels = useMemo(
		() => fetchedModels.filter(m => !existingSet.has(m)),
		[fetchedModels, existingSet]
	)

	const displayModels = tab === 'new' ? newModels : existingModels

	const filtered = useMemo(() => {
		if (!search.trim()) return displayModels
		const q = search.toLowerCase()
		return displayModels.filter(m => m.toLowerCase().includes(q))
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

	useEffect(() => {
		if (!open) return
		if (!providerId && !channelInfo) return
		setLoading(true)
		const promise = providerId
			? api.fetchProviderModels(providerId).then(r => r.models)
			: api.fetchChannelModels(channelInfo!.base_url, channelInfo!.api_key).then(r => r.models)
		promise
			.then(models => {
				setFetchedModels(models)
			})
			.catch(error => {
				toast.error(
					error instanceof Error ?
						error.message
					:	t('providers.fetchModelsError')
				)
			})
			.finally(() => {
				setLoading(false)
			})
	}, [open, providerId, channelInfo, t])

	useEffect(() => {
		if (!open) {
			initializedForOpenRef.current = false
			return
		}
		if (initializedForOpenRef.current) return
		initializedForOpenRef.current = true
		setChecked(new Set(existingModels))
		setFetchedModels([])
		setSearch('')
		setTab('new')
	}, [open, existingModels])

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
		for (const m of checked) {
			if (!existingSet.has(m)) return true
		}
		return false
	}, [checked, existingSet])

	const handleConfirm = () => {
		onConfirm([...checked])
		onOpenChange(false)
	}

	return (
		<Dialog open={open} onOpenChange={onOpenChange}>
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
							onChange={e => setSearch(e.target.value)}
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
												highlightUnpriced={!pricedModelIdSet.has(model)}
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
		</Dialog>
	)
}

function ProviderDialog({
	open,
	onOpenChange,
	mode,
	current,
	providers,
	transformRegistry,
	modelMetadata
}: {
	open: boolean
	onOpenChange: (open: boolean) => void
	mode: 'create' | 'edit'
	current: Provider | null
	providers: Provider[]
	transformRegistry: TransformRegistryItem[]
	modelMetadata: ModelMetadataRecord[]
}) {
	const { t } = useTranslation()
	const [loading, setLoading] = useState(false)
	const [modelPickerOpen, setModelPickerOpen] = useState(false)
	const [editingModelIndex, setEditingModelIndex] = useState<number | null>(
		null
	)
	const [draftModel, setDraftModel] = useState<ModelRow | null>(null)
	const [editingChannelIndex, setEditingChannelIndex] = useState<number | null>(
		null
	)
	const [hydratedKey, setHydratedKey] = useState<string | null>(null)
	const [form, setForm] = useState<ProviderForm>(() =>
		mode === 'edit' && current ? fromProvider(current) : emptyForm()
	)
	const [baseUrlPrompt, setBaseUrlPrompt] = useState<{
		index: number
		original: string
		trimmed: string
	} | null>(null)
	const [v1KeepConfirmed, setV1KeepConfirmed] = useState<
		Record<number, string>
	>({})
	const [unsavedChangesOpen, setUnsavedChangesOpen] = useState(false)
	const initialFormRef = useRef<string | null>(null)

	const isEdit = mode === 'edit'

	const {
		data: editProviderDetail,
		isLoading: isLoadingEditProviderDetail,
		error: editProviderDetailError
	} = useSWR(
		open && isEdit && current ? `provider-detail:${current.id}` : null,
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

	useEffect(() => {
		if (!open) return

		if (isEdit) {
			const source =
				editProviderDetail ?? (editProviderDetailError ? current : null)
			if (!source) return
			const nextHydrationKey = `${source.id}:${source.updated_at}`
			if (hydratedKey === nextHydrationKey) return
			setForm(fromProvider(source))
			setBaseUrlPrompt(null)
			setV1KeepConfirmed({})
			setModelPickerOpen(false)
			setEditingModelIndex(null)
			setDraftModel(null)
			setEditingChannelIndex(null)
			setHydratedKey(nextHydrationKey)
			return
		}

		if (hydratedKey === '__create__') return
		setForm(emptyForm())
		setBaseUrlPrompt(null)
		setV1KeepConfirmed({})
		setModelPickerOpen(false)
		setEditingModelIndex(null)
		setDraftModel(null)
		setEditingChannelIndex(null)
		setHydratedKey('__create__')
	}, [
		open,
		isEdit,
		current,
		editProviderDetail,
		editProviderDetailError,
		hydratedKey
	])

	const isHydratingForm =
		open &&
		(isEdit ?
			!current ||
			(!editProviderDetail &&
				!editProviderDetailError &&
				(isLoadingEditProviderDetail || hydratedKey === null))
		:	hydratedKey !== '__create__')

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
	}, [isDirty, onOpenChange]) // eslint-disable-line react-hooks/exhaustive-deps

	const resetFromCurrent = () => {
		setForm(emptyForm())
		setBaseUrlPrompt(null)
		setV1KeepConfirmed({})
		setModelPickerOpen(false)
		setEditingModelIndex(null)
		setDraftModel(null)
		setEditingChannelIndex(null)
		setHydratedKey(null)
	}

	const handleFetchModels = () => {
		if (isEdit) {
			if (!current) return
			setModelPickerOpen(true)
			return
		}
		const ch = form.channels.find(c => c.base_url.trim() && c.api_key.trim())
		if (!ch) {
			toast.error(t('providers.fetchModelsNeedChannel'))
			return
		}
		setModelPickerOpen(true)
	}

	const fetchChannelInfo = useMemo(() => {
		if (isEdit) return undefined
		const ch = form.channels.find(c => c.base_url.trim() && c.api_key.trim())
		if (!ch) return undefined
		return { base_url: ch.base_url.trim(), api_key: ch.api_key.trim() }
	}, [isEdit, form.channels])

	const existingModelNames = useMemo(
		() => form.models.map(m => m.model.trim()).filter(Boolean),
		[form.models]
	)

	const handleModelsConfirm = (checkedModels: string[]) => {
		const checkedSet = new Set(checkedModels)
		const existingModelNames = form.models.map(m => m.model.trim())

		const kept = form.models.filter(m => checkedSet.has(m.model.trim()))
		const newModels = checkedModels.filter(m => !existingModelNames.includes(m))
		const added = newModels.map(m => ({
			model: m,
			redirect: '',
			multiplier: '1'
		}))

		const removedCount = form.models.length - kept.length
		const addedCount = added.length

		setForm(prev => ({
			...prev,
			models: [...kept, ...added]
		}))
		setEditingModelIndex(null)

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
	}

	const updateModel = (idx: number, patch: Partial<ModelRow>) => {
		setForm(prev => ({
			...prev,
			models: prev.models.map((m, i) => (i === idx ? { ...m, ...patch } : m))
		}))
	}

	const deleteModelAt = (idx: number) => {
		setForm(prev => ({
			...prev,
			models: prev.models.filter((_, i) => i !== idx)
		}))
		setEditingModelIndex(prev => {
			if (prev === null) return null
			if (prev === idx) return null
			if (prev > idx) return prev - 1
			return prev
		})
	}

	const selectedModel =
		(
			editingModelIndex !== null &&
			editingModelIndex >= 0 &&
			editingModelIndex < form.models.length
		) ?
			form.models[editingModelIndex]
		:	null

	const modelDialogModel = draftModel ?? selectedModel
	const modelDialogOpen =
		draftModel !== null || (editingModelIndex !== null && selectedModel !== null)

	const closeModelDialog = () => {
		setDraftModel(null)
		setEditingModelIndex(null)
	}

	const handleModelDialogSave = () => {
		if (draftModel) {
			if (!draftModel.model.trim()) {
				toast.error(t('providers.validationModelRequired'))
				return
			}
			const multiplier = Number(draftModel.multiplier)
			if (!Number.isFinite(multiplier) || multiplier < 0) {
				toast.error(t('providers.validationMultiplier'))
				return
			}
			setForm(prev => ({
				...prev,
				models: [
					...prev.models,
					{
						model: draftModel.model.trim(),
						redirect: draftModel.redirect,
						multiplier: draftModel.multiplier
					}
				]
			}))
			setDraftModel(null)
			return
		}

		setEditingModelIndex(null)
	}

	const selectedChannel =
		(
			editingChannelIndex !== null &&
			editingChannelIndex >= 0 &&
			editingChannelIndex < form.channels.length
		) ?
			form.channels[editingChannelIndex]
		:	null

	const updateChannel = (idx: number, patch: Partial<ChannelRow>) => {
		setForm(prev => ({
			...prev,
			channels: prev.channels.map((c, i) =>
				i === idx ? { ...c, ...patch } : c
			)
		}))
	}

	const deleteChannelAt = (idx: number) => {
		setForm(prev => ({
			...prev,
			channels: prev.channels.filter((_, i) => i !== idx)
		}))
		setEditingChannelIndex(prev => {
			if (prev === null) return null
			if (prev === idx) return null
			if (prev > idx) return prev - 1
			return prev
		})
	}

	const updateChannelBaseUrl = (idx: number, baseUrl: string) => {
		updateChannel(idx, { base_url: baseUrl })
		setV1KeepConfirmed(prev => {
			if (!(idx in prev)) return prev
			const next = { ...prev }
			delete next[idx]
			return next
		})
	}

	const handleBaseUrlBlur = (idx: number) => {
		const raw = form.channels[idx]?.base_url ?? ''
		const trimmed = raw.trim()
		if (!trimmed || !hasTrailingV1(trimmed)) {
			return
		}
		if (v1KeepConfirmed[idx] === trimmed) {
			return
		}
		const normalized = removeTrailingV1(trimmed)
		if (!normalized) {
			return
		}
		setBaseUrlPrompt({ index: idx, original: raw, trimmed: normalized })
	}

	const validateAndBuild = ():
		| CreateProviderInput
		| UpdateProviderInput
		| null => {
		if (!form.name.trim()) {
			toast.error(t('providers.validationNameRequired'))
			return null
		}

		const models: Record<
			string,
			{ redirect: string | null; multiplier: number }
		> = {}
		for (const row of form.models) {
			if (!row.model.trim()) {
				toast.error(t('providers.validationModelRequired'))
				return null
			}
			const multiplier = Number(row.multiplier)
			if (!Number.isFinite(multiplier) || multiplier < 0) {
				toast.error(t('providers.validationMultiplier'))
				return null
			}
			models[row.model.trim()] = {
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
			passive_failure_threshold_override:
				row.passive_failure_threshold_override.trim() ?
					Number(row.passive_failure_threshold_override)
				:	null,
			passive_cooldown_seconds_override:
				row.passive_cooldown_seconds_override.trim() ?
					Number(row.passive_cooldown_seconds_override)
				:	null,
			passive_window_seconds_override:
				row.passive_window_seconds_override.trim() ?
					Number(row.passive_window_seconds_override)
				:	null,
			passive_min_samples_override:
				row.passive_min_samples_override.trim() ?
					Number(row.passive_min_samples_override)
				:	null,
			passive_failure_rate_threshold_override:
				row.passive_failure_rate_threshold_override.trim() ?
					Number(row.passive_failure_rate_threshold_override)
				:	null,
			passive_rate_limit_cooldown_seconds_override:
				row.passive_rate_limit_cooldown_seconds_override.trim() ?
					Number(row.passive_rate_limit_cooldown_seconds_override)
				:	null
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
				channel.passive_failure_threshold_override !== null &&
				(!Number.isFinite(channel.passive_failure_threshold_override) ||
					channel.passive_failure_threshold_override < 1)
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
				channel.passive_min_samples_override !== null &&
				(!Number.isFinite(channel.passive_min_samples_override) ||
					channel.passive_min_samples_override < 1)
			) {
				toast.error(t('providers.validationChannelPassiveSamples'))
				return null
			}
			if (
				channel.passive_failure_rate_threshold_override !== null &&
				(!Number.isFinite(channel.passive_failure_rate_threshold_override) ||
					channel.passive_failure_rate_threshold_override < 0.01 ||
					channel.passive_failure_rate_threshold_override > 1)
			) {
				toast.error(t('providers.validationChannelPassiveRate'))
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
			:	null
		if (
			requestTimeoutMsOverride !== null &&
			(!Number.isFinite(requestTimeoutMsOverride) ||
				requestTimeoutMsOverride < 1)
		) {
			toast.error(t('providers.validationProviderRequestTimeout'))
			return null
		}

		return {
			name: form.name.trim(),
			provider_type: form.provider_type,
			models,
			channels,
			max_retries: form.max_retries,
			transforms: form.transforms,
			active_probe_enabled_override: form.active_probe_enabled_override,
			active_probe_interval_seconds_override:
				form.active_probe_interval_seconds_override,
			active_probe_success_threshold_override:
				form.active_probe_success_threshold_override,
			active_probe_model_override:
				form.active_probe_model_override?.trim() ?
					form.active_probe_model_override.trim()
				: 	null,
			request_timeout_ms_override: requestTimeoutMsOverride,
			enabled: form.enabled,
			priority: form.priority
		}
	}

	const onSubmit = async () => {
		const payload = validateAndBuild()
		if (!payload) return
		setLoading(true)
		try {
			if (isEdit && current) {
				await updateProviderOptimistic(
					current.id,
					payload as UpdateProviderInput,
					providers
				)
				toast.success(t('providers.updateSuccess'))
			} else {
				await createProviderOptimistic(
					payload as CreateProviderInput,
					providers
				)
				toast.success(t('providers.createSuccess'))
			}
			onOpenChange(false)
			resetFromCurrent()
		} catch (error) {
			toast.error(error instanceof Error ? error.message : t('common.error'))
		} finally {
			setLoading(false)
		}
	}

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
							{isEdit ?
								t('providers.editProvider')
							:	t('providers.createProvider')}
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
					:	<div className='space-y-6'>
							<div className='grid grid-cols-1 md:grid-cols-2 gap-4'>
								<div className='space-y-2'>
									<Label>{t('providers.name')}</Label>
									<Input
										placeholder={t('providers.namePlaceholder')}
										value={form.name}
										onChange={e =>
											setForm(prev => ({ ...prev, name: e.target.value }))
										}
									/>
								</div>
								<div className='space-y-2'>
									<Label>{t('providers.type')}</Label>
									<Select
										value={form.provider_type}
										onValueChange={value =>
											setForm(prev => ({
												...prev,
												provider_type: value as ProviderForm['provider_type']
											}))
										}
									>
										<SelectTrigger>
											<SelectValue>
												{(() => {
													const cfg = PROVIDER_TYPE_CONFIG[form.provider_type]
													const Icon = cfg.icon
													return (
														<span className='flex items-center gap-2'>
															<Icon className='h-4 w-4 text-muted-foreground' />
															{cfg.label}
															<span className='text-muted-foreground text-xs'>
																{cfg.path}
															</span>
														</span>
													)
												})()}
											</SelectValue>
										</SelectTrigger>
										<SelectContent>
											{(
												Object.entries(PROVIDER_TYPE_CONFIG) as [
													ProviderForm['provider_type'],
													(typeof PROVIDER_TYPE_CONFIG)[ProviderForm['provider_type']]
												][]
											).map(([value, cfg]) => {
												const Icon = cfg.icon
												return (
													<SelectItem key={value} value={value}>
														<span className='flex items-center gap-2'>
															<Icon className='h-4 w-4' />
															{cfg.label}
															<span className='opacity-60 text-xs'>
																{cfg.path}
															</span>
														</span>
													</SelectItem>
												)
											})}
										</SelectContent>
									</Select>
								</div>
								{isEdit && (
									<div className='space-y-2'>
										<Label>{t('providers.id')}</Label>
										<Input value={form.id || ''} disabled />
									</div>
								)}
							<div className='space-y-2'>
								<Label>{t('providers.maxRetries')}</Label>
								<Input
									type='number'
									value={form.max_retries}
									onChange={e =>
										setForm(prev => ({
											...prev,
											max_retries: Number(e.target.value) || 0
										}))
									}
								/>
							</div>
							<div className='md:col-span-2 rounded-md border p-3 space-y-3'>
								<div className='text-sm font-medium'>
									{t('providers.probeOverrideTitle')}
								</div>
								<div className='grid grid-cols-1 md:grid-cols-2 gap-4'>
									<div className='space-y-2'>
										<Label>{t('providers.probeEnabledOverride')}</Label>
										<Select
											value={
												form.active_probe_enabled_override === null ?
													'inherit'
												: form.active_probe_enabled_override ?
													'true'
												: 	'false'
											}
											onValueChange={value =>
												setForm(prev => ({
													...prev,
													active_probe_enabled_override:
														value === 'inherit' ?
															null
														: value === 'true'
												}))
										}
										>
											<SelectTrigger>
												<SelectValue />
											</SelectTrigger>
											<SelectContent>
												<SelectItem value='inherit'>
													{t('providers.inheritGlobal')}
												</SelectItem>
												<SelectItem value='true'>
													{t('providers.enabled')}
												</SelectItem>
												<SelectItem value='false'>
													{t('common.disabled')}
												</SelectItem>
											</SelectContent>
										</Select>
									</div>
									<div className='space-y-2'>
										<Label>{t('providers.probeModelOverride')}</Label>
										<Input
											value={form.active_probe_model_override ?? ''}
											onChange={e =>
												setForm(prev => ({
													...prev,
													active_probe_model_override:
														e.target.value
												}))
											}
											placeholder={t('providers.probeModelOverridePlaceholder')}
										/>
									</div>
									<div className='space-y-2'>
										<Label>{t('providers.probeIntervalOverride')}</Label>
										<Input
											type='number'
											min='1'
											value={form.active_probe_interval_seconds_override ?? ''}
											onChange={e =>
												setForm(prev => ({
													...prev,
													active_probe_interval_seconds_override:
														e.target.value.trim() ?
															Math.max(1, Number(e.target.value) || 1)
														: 	null
												}))
											}
											placeholder={t('providers.inheritGlobal')}
										/>
									</div>
									<div className='space-y-2'>
										<Label>{t('providers.probeSuccessThresholdOverride')}</Label>
										<Input
											type='number'
											min='1'
											value={form.active_probe_success_threshold_override ?? ''}
											onChange={e =>
												setForm(prev => ({
													...prev,
													active_probe_success_threshold_override:
														e.target.value.trim() ?
															Math.max(1, Number(e.target.value) || 1)
														: 	null
												}))
											}
											placeholder={t('providers.inheritGlobal')}
										/>
									</div>
								</div>
								<p className='text-xs text-muted-foreground'>
									{t('providers.probeOverrideDescription')}
								</p>
							</div>
							<div className='space-y-2'>
								<Label>{t('providers.requestTimeoutMsOverride')}</Label>
								<Input
									type='number'
									min='1'
									placeholder={t('providers.inheritGlobal')}
									value={form.request_timeout_ms_override}
									onChange={e =>
										setForm(prev => ({
											...prev,
											request_timeout_ms_override: e.target.value
										}))
									}
								/>
								<p className='text-xs text-muted-foreground'>
									{t('providers.requestTimeoutMsOverrideDescription')}
								</p>
							</div>
							<div className='flex items-center gap-2 pt-7'>
								<Switch
									checked={form.enabled}
										onCheckedChange={checked =>
											setForm(prev => ({ ...prev, enabled: checked }))
										}
									/>
									<Label>{t('providers.enabled')}</Label>
								</div>
							</div>

							<Separator />

							<div className='space-y-3'>
								<div className='flex items-center justify-between'>
									<div className='flex items-center gap-2'>
										<Layers className='h-4 w-4 text-muted-foreground' />
										<h3 className='text-base font-semibold'>
											{t('providers.modelsSection')}
										</h3>
									</div>
									<div className='flex items-center gap-2'>
										<Button
											type='button'
											variant='outline'
											size='sm'
											onClick={handleFetchModels}
										>
											<Download className='h-4 w-4 mr-2' />
											{t('providers.fetchModels')}
										</Button>
											<Button
												type='button'
												variant='outline'
												size='sm'
												onClick={() => {
													setDraftModel({
														model: '',
														redirect: '',
														multiplier: '1'
													})
													setEditingModelIndex(null)
												}}
											>
											<Plus className='h-4 w-4 mr-2' />
											{t('providers.addModel')}
										</Button>
									</div>
								</div>
								<div className='mt-1 rounded-lg border overflow-hidden px-3 py-2'>
									{form.models.length === 0 ?
										<div className='text-sm text-muted-foreground py-5'>
											{t('providers.validationAtLeastOneModel')}
										</div>
									:	<div className='flex flex-wrap content-start gap-2 max-h-[220px] overflow-y-auto'>
											{form.models.map((row, idx) => {
												const pricingModelId = resolvePricingModelId(
													row.model,
													row.redirect
												)
												return (
													<div
														key={`model-${row.model || idx}`}
														className='group relative min-w-0 max-w-full shrink-0'
													>
														<button
															type='button'
															onClick={() => setEditingModelIndex(idx)}
															className='text-left'
														>
															<ModelBadge
																model={row.model || '-'}
																provider={modelProviderMap.get(row.model.trim())}
																multiplier={row.multiplier || 1}
																detailTarget={
																	row.redirect.trim() || row.model || '-'
																}
																highlightUnpriced={
																	!pricedModelIdSet.has(pricingModelId)
																}
																className={cn(
																	'pr-6 cursor-pointer',
																	editingModelIndex === idx && 'ring-1 ring-primary'
																)}
															/>
														</button>
														<button
															type='button'
															className='absolute right-1 top-1/2 -translate-y-1/2 h-4 w-4 rounded-full flex items-center justify-center opacity-40 hover:opacity-100 hover:bg-destructive/15 hover:text-destructive transition-all'
															onClick={e => {
																e.stopPropagation()
																deleteModelAt(idx)
															}}
														>
															<X className='h-3 w-3' />
														</button>
													</div>
												)
											})}
										</div>
									}
								</div>
							</div>

							<Separator />

							<div className='space-y-3'>
								<div className='flex items-center justify-between'>
									<div className='flex items-center gap-2'>
										<Radio className='h-4 w-4 text-muted-foreground' />
										<h3 className='text-base font-semibold'>
											{t('providers.channelsSection')}
										</h3>
										<Badge variant='secondary' className='text-xs'>
											{form.channels.length}
										</Badge>
									</div>
									<Button
										type='button'
										variant='outline'
										size='sm'
										onClick={() => {
											let nextIndex = 0
											setForm(prev => {
												nextIndex = prev.channels.length
												return {
													...prev,
													channels: [
														...prev.channels,
														{
															id: '',
															name: '',
															base_url: '',
															api_key: '',
															weight: '1',
															enabled: true,
															passive_failure_threshold_override: '',
															passive_cooldown_seconds_override: '',
															passive_window_seconds_override: '',
															passive_min_samples_override: '',
													passive_failure_rate_threshold_override: '',
													passive_rate_limit_cooldown_seconds_override: ''
													}
													]
												}
											})
											setEditingChannelIndex(nextIndex)
										}}
									>
										<Plus className='h-4 w-4 mr-2' />
										{t('providers.addChannel')}
									</Button>
								</div>

								{isEdit && (
									<p className='text-xs text-muted-foreground'>
										{t('providers.editChannelNote')}
									</p>
								)}

								<div className='rounded-lg border overflow-hidden'>
									{form.channels.length === 0 ?
										<div className='text-sm text-muted-foreground py-5 px-3'>
											{t('providers.validationAtLeastOneChannel')}
										</div>
									:	<Virtuoso
											style={{
												height: Math.min(
													form.channels.length * PROVIDER_EDIT_CHANNEL_ROW_HEIGHT,
													300
												)
											}}
											data={form.channels}
											itemContent={(idx, row) => (
												<div
													className={cn(
														'flex h-14 items-center gap-3 px-3 border-b last:border-b-0 cursor-pointer transition-colors hover:bg-muted/50',
														editingChannelIndex === idx ?
															'bg-primary/5'
														:	'bg-background'
													)}
													onClick={() => setEditingChannelIndex(idx)}
												>
													<div className='min-w-0 flex-1'>
														<div className='flex items-center gap-2'>
															<span className='truncate text-sm font-medium'>
																{row.name || t('providers.addChannel')}
															</span>
															{!row.enabled && (
																<Badge
																	variant='secondary'
																	className='text-[10px]'
																>
																	{t('common.disabled')}
																</Badge>
															)}
														</div>
														<div className='flex items-center gap-3 text-xs text-muted-foreground mt-0.5'>
															<span className='flex items-center gap-1 truncate max-w-[280px]'>
																<Globe className='h-3 w-3 shrink-0' />
																{row.base_url || '—'}
															</span>
															<span className='flex items-center gap-1 shrink-0'>
																<Weight className='h-3 w-3' />
																{row.weight}
															</span>
														</div>
													</div>

													<div className='flex items-center gap-1 shrink-0'>
														<Switch
															checked={row.enabled}
															onCheckedChange={checked =>
																updateChannel(idx, { enabled: checked })
															}
															onClick={e => e.stopPropagation()}
														/>
														<Button
															type='button'
															variant='ghost'
															size='icon'
															className='h-7 w-7 text-destructive hover:text-destructive'
															onClick={e => {
																e.stopPropagation()
																deleteChannelAt(idx)
															}}
														>
															<Trash2 className='h-3.5 w-3.5' />
														</Button>
													</div>
												</div>
											)}
										/>
									}
								</div>
							</div>

							<Separator />

							<div className='space-y-3'>
								<div className='flex items-center gap-2'>
									<Settings2 className='h-4 w-4 text-muted-foreground' />
									<h3 className='text-base font-semibold'>
										{t('transforms.titleProvider')}
									</h3>
								</div>
								<TransformChainEditor
									value={form.transforms}
									registry={transformRegistry}
									onChange={next =>
										setForm(prev => ({ ...prev, transforms: next }))
									}
								/>
							</div>
						</div>
					}

					<DialogFooter>
						<Button type='button' variant='outline' onClick={tryClose}>
							{t('common.cancel')}
						</Button>
						<Button
							type='button'
							onClick={onSubmit}
							disabled={loading || isHydratingForm}
						>
							<Save className='h-4 w-4 mr-2' />
							{loading ? t('common.saving') : t('common.save')}
						</Button>
					</DialogFooter>
				</DialogContent>
			</Dialog>

			<AlertDialog
				open={baseUrlPrompt !== null}
				onOpenChange={value => {
					if (!value) {
						setBaseUrlPrompt(null)
					}
				}}
			>
				<AlertDialogContent>
					<AlertDialogHeader>
						<AlertDialogTitle>{t('providers.baseUrlV1Title')}</AlertDialogTitle>
						<AlertDialogDescription>
							{t('providers.baseUrlV1Description')}
						</AlertDialogDescription>
					</AlertDialogHeader>
					<AlertDialogFooter>
						<AlertDialogCancel
							onClick={() => {
								if (!baseUrlPrompt) return
								setV1KeepConfirmed(prev => ({
									...prev,
									[baseUrlPrompt.index]: baseUrlPrompt.original.trim()
								}))
								setBaseUrlPrompt(null)
							}}
						>
							{t('providers.baseUrlV1Keep')}
						</AlertDialogCancel>
						<AlertDialogAction
							onClick={() => {
								if (!baseUrlPrompt) return
								updateChannel(baseUrlPrompt.index, {
									base_url: baseUrlPrompt.trimmed
								})
								setBaseUrlPrompt(null)
							}}
						>
							{t('providers.baseUrlV1Remove')}
						</AlertDialogAction>
					</AlertDialogFooter>
				</AlertDialogContent>
			</AlertDialog>

			<AlertDialog
				open={unsavedChangesOpen}
				onOpenChange={setUnsavedChangesOpen}
			>
				<AlertDialogContent>
					<AlertDialogHeader>
						<AlertDialogTitle>
							{t('providers.unsavedChangesTitle')}
						</AlertDialogTitle>
						<AlertDialogDescription>
							{t('providers.unsavedChangesDesc')}
						</AlertDialogDescription>
					</AlertDialogHeader>
					<AlertDialogFooter>
						<AlertDialogCancel>{t('common.cancel')}</AlertDialogCancel>
						<Button
							variant='outline'
							className='hover:!bg-destructive hover:!text-destructive-foreground hover:!border-destructive'
							onClick={() => {
								setUnsavedChangesOpen(false)
								resetFromCurrent()
								onOpenChange(false)
							}}
						>
							{t('providers.unsavedChangesDiscard')}
						</Button>
						<AlertDialogAction
							onClick={() => {
								setUnsavedChangesOpen(false)
								onSubmit()
							}}
						>
							{t('common.save')}
						</AlertDialogAction>
					</AlertDialogFooter>
				</AlertDialogContent>
			</AlertDialog>

			<ModelPickerDialog
				open={modelPickerOpen}
				onOpenChange={setModelPickerOpen}
				providerId={isEdit ? current?.id : undefined}
				channelInfo={fetchChannelInfo}
				providerName={form.name || current?.name || ''}
				existingModels={existingModelNames}
				modelMetadata={modelMetadata}
				onConfirm={handleModelsConfirm}
			/>

			<Dialog
				open={modelDialogOpen && modelDialogModel !== null}
				onOpenChange={open => {
					if (!open) {
						closeModelDialog()
					}
				}}
			>
				<DialogContent className='max-w-lg'>
					<DialogHeader>
						<DialogTitle>
							{draftModel ? t('providers.addModel') : t('providers.model')}
						</DialogTitle>
						<DialogDescription>
							{modelDialogModel?.model || t('providers.model')}
						</DialogDescription>
					</DialogHeader>

					{modelDialogModel && (
						<div className='space-y-3'>
							<div className='space-y-2'>
								<Label>{t('providers.model')}</Label>
								<Input
									value={modelDialogModel.model}
									onChange={e => {
										const model = e.target.value
										if (draftModel) {
											setDraftModel(prev =>
												prev ? { ...prev, model } : prev
											)
											return
										}
										if (editingModelIndex === null) return
										updateModel(editingModelIndex, { model })
									}}
								/>
							</div>

							<div className='space-y-2'>
								<Label>{t('providers.redirect')}</Label>
								<Input
									value={modelDialogModel.redirect}
									placeholder={t('providers.optional')}
									onChange={e => {
										const redirect = e.target.value
										if (draftModel) {
											setDraftModel(prev =>
												prev ? { ...prev, redirect } : prev
											)
											return
										}
										if (editingModelIndex === null) return
										updateModel(editingModelIndex, { redirect })
									}}
								/>
							</div>

							<div className='space-y-2'>
								<Label>{t('providers.multiplier')}</Label>
								<Input
									type='number'
									min='0'
									step='0.1'
									value={modelDialogModel.multiplier}
									onChange={e => {
										const multiplier = e.target.value
										if (draftModel) {
											setDraftModel(prev =>
												prev ? { ...prev, multiplier } : prev
											)
											return
										}
										if (editingModelIndex === null) return
										updateModel(editingModelIndex, {
											multiplier
										})
									}}
								/>
							</div>
						</div>
					)}

					<DialogFooter>
						<Button type='button' variant='outline' onClick={closeModelDialog}>
							{t('common.cancel')}
						</Button>
						{!draftModel && editingModelIndex !== null && (
							<Button
								type='button'
								variant='outline'
								className='text-destructive border-destructive/30 hover:text-destructive'
								onClick={() => {
									if (editingModelIndex === null) return
									deleteModelAt(editingModelIndex)
								}}
							>
								<Trash2 className='h-4 w-4 mr-2' />
								{t('common.delete')}
							</Button>
						)}
						<Button type='button' onClick={handleModelDialogSave}>
							{t('common.save')}
						</Button>
					</DialogFooter>
				</DialogContent>
			</Dialog>

			<Dialog
				open={editingChannelIndex !== null && selectedChannel !== null}
				onOpenChange={open => {
					if (!open) {
						setEditingChannelIndex(null)
					}
				}}
			>
				<DialogContent className='max-w-lg max-h-[85vh] flex flex-col overflow-hidden'>
					<DialogHeader>
						<DialogTitle>{t('providers.channelsSection')}</DialogTitle>
						<DialogDescription>
							{selectedChannel?.name || t('providers.addChannel')}
						</DialogDescription>
					</DialogHeader>

					{selectedChannel && editingChannelIndex !== null && (
						<div className='space-y-3 overflow-y-auto flex-1 min-h-0 pr-1'>
							<div className='space-y-2'>
								<Label>{t('common.name')}</Label>
								<Input
									value={selectedChannel.name}
									onChange={e =>
										updateChannel(editingChannelIndex, { name: e.target.value })
									}
								/>
							</div>

							<div className='space-y-2'>
								<Label>{t('providers.baseUrl')}</Label>
								<Input
									value={selectedChannel.base_url}
									autoComplete='off'
									onChange={e =>
										updateChannelBaseUrl(editingChannelIndex, e.target.value)
									}
									onBlur={() => handleBaseUrlBlur(editingChannelIndex)}
								/>
							</div>

							<div className='space-y-2'>
								<Label>{t('providers.apiKey')}</Label>
								<Input
									type='password'
									autoComplete='new-password'
									placeholder={
										isEdit && selectedChannel.id ?
											t('providers.apiKeyUnchanged')
										:	undefined
									}
									value={selectedChannel.api_key}
									onChange={e =>
										updateChannel(editingChannelIndex, {
											api_key: e.target.value
										})
									}
								/>
							</div>

							<div className='space-y-2'>
								<Label>{t('providers.weight')}</Label>
								<Input
									type='number'
									min='0'
									value={selectedChannel.weight}
									onChange={e =>
										updateChannel(editingChannelIndex, {
											weight: e.target.value
										})
									}
								/>
							</div>

							<Separator />

							<div className='space-y-2'>
								<Label>{t('providers.passiveFailureThresholdOverride')}</Label>
								<Input
									type='number'
									min='1'
									placeholder={t('providers.inheritGlobal')}
									value={selectedChannel.passive_failure_threshold_override}
									onChange={e =>
										updateChannel(editingChannelIndex, {
											passive_failure_threshold_override: e.target.value
										})
									}
								/>
							</div>

							<div className='space-y-2'>
								<Label>{t('providers.passiveCooldownSecondsOverride')}</Label>
								<Input
									type='number'
									min='1'
									placeholder={t('providers.inheritGlobal')}
									value={selectedChannel.passive_cooldown_seconds_override}
									onChange={e =>
										updateChannel(editingChannelIndex, {
											passive_cooldown_seconds_override: e.target.value
										})
									}
								/>
							</div>

							<div className='space-y-2'>
								<Label>{t('providers.passiveWindowSecondsOverride')}</Label>
								<Input
									type='number'
									min='1'
									placeholder={t('providers.inheritGlobal')}
									value={selectedChannel.passive_window_seconds_override}
									onChange={e =>
										updateChannel(editingChannelIndex, {
											passive_window_seconds_override: e.target.value
										})
									}
								/>
							</div>

							<div className='space-y-2'>
								<Label>{t('providers.passiveMinSamplesOverride')}</Label>
								<Input
									type='number'
									min='1'
									placeholder={t('providers.inheritGlobal')}
									value={selectedChannel.passive_min_samples_override}
									onChange={e =>
										updateChannel(editingChannelIndex, {
											passive_min_samples_override: e.target.value
										})
									}
								/>
							</div>

							<div className='space-y-2'>
								<Label>{t('providers.passiveFailureRateThresholdOverride')}</Label>
								<Input
									type='number'
									min='0.01'
									max='1'
									step='0.01'
									placeholder={t('providers.inheritGlobal')}
									value={selectedChannel.passive_failure_rate_threshold_override}
									onChange={e =>
										updateChannel(editingChannelIndex, {
											passive_failure_rate_threshold_override: e.target.value
										})
									}
								/>
							</div>

							<div className='space-y-2'>
								<Label>{t('providers.passiveRateLimitCooldownSecondsOverride')}</Label>
								<Input
									type='number'
									min='1'
									placeholder={t('providers.inheritGlobal')}
									value={selectedChannel.passive_rate_limit_cooldown_seconds_override}
									onChange={e =>
										updateChannel(editingChannelIndex, {
											passive_rate_limit_cooldown_seconds_override: e.target.value
										})
									}
								/>
							</div>

							<div className='flex items-center gap-2'>
								<Switch
									checked={selectedChannel.enabled}
									onCheckedChange={checked =>
										updateChannel(editingChannelIndex, { enabled: checked })
									}
								/>
								<Label>{t('providers.enabled')}</Label>
							</div>
						</div>
					)}

					<DialogFooter>
						<Button
							type='button'
							variant='outline'
							className='text-destructive border-destructive/30 hover:text-destructive'
							onClick={() => {
								if (editingChannelIndex === null) return
								deleteChannelAt(editingChannelIndex)
								setEditingChannelIndex(null)
							}}
						>
							<Trash2 className='h-4 w-4 mr-2' />
							{t('common.delete')}
						</Button>
						<Button type='button' onClick={() => setEditingChannelIndex(null)}>
							{t('common.save')}
						</Button>
					</DialogFooter>
				</DialogContent>
			</Dialog>
		</>
	)
}

type ChannelTestState = Record<
	string,
	{ status: 'idle' | 'testing' | 'passed' | 'failed'; latency_ms?: number; error?: string }
>

function ChannelTestDialog({
	open,
	onOpenChange,
	providerId,
	channelId,
	channelName,
	providerName,
	models
}: {
	open: boolean
	onOpenChange: (open: boolean) => void
	providerId: string
	channelId: string
	channelName: string
	providerName: string
	models: string[]
}) {
	const { t } = useTranslation()
	const [testState, setTestState] = useState<ChannelTestState>({})
	const [testingAll, setTestingAll] = useState(false)
	const abortRef = useRef(false)

	useEffect(() => {
		if (open) {
			setTestState({})
			setTestingAll(false)
			abortRef.current = false
		} else {
			abortRef.current = true
		}
	}, [open])

	const runSingleTest = async (model: string) => {
		setTestState(prev => ({
			...prev,
			[model]: { status: 'testing' }
		}))
		try {
			const result: ChannelTestResult = await api.testChannel(providerId, channelId, model)
			setTestState(prev => ({
				...prev,
				[model]: {
					status: result.success ? 'passed' : 'failed',
					latency_ms: result.latency_ms,
					error: result.error ?? undefined
				}
			}))
		} catch (err) {
			setTestState(prev => ({
				...prev,
				[model]: {
					status: 'failed',
					error: err instanceof Error ? err.message : 'Unknown error'
				}
			}))
		}
	}

	const runAllTests = async () => {
		setTestingAll(true)
		abortRef.current = false
		for (const model of models) {
			if (abortRef.current) break
			await runSingleTest(model)
		}
		setTestingAll(false)
	}

	const testedCount = Object.values(testState).filter(
		s => s.status === 'passed' || s.status === 'failed'
	).length
	const passedCount = Object.values(testState).filter(
		s => s.status === 'passed'
	).length

	return (
		<Dialog open={open} onOpenChange={onOpenChange}>
			<DialogContent className='max-h-[85vh] flex flex-col overflow-hidden max-w-2xl'>
				<DialogHeader>
					<DialogTitle className='flex items-center gap-2'>
						<Activity className='h-5 w-5 text-muted-foreground' />
						{t('providers.testChannelTitle')}
					</DialogTitle>
					<DialogDescription>
						{t('providers.testChannelDesc', {
							channel: channelName,
							provider: providerName
						})}
					</DialogDescription>
				</DialogHeader>

				<div className='flex items-center justify-between'>
					<div className='text-sm text-muted-foreground'>
						{testedCount > 0 && (
							<span>
								{passedCount}/{testedCount} {t('providers.testPassed').toLowerCase()}
							</span>
						)}
					</div>
					<Button
						size='sm'
						variant='outline'
						disabled={testingAll || models.length === 0}
						onClick={runAllTests}
					>
						{testingAll ?
							<>
								<Loader2 className='h-4 w-4 mr-2 animate-spin' />
								{t('providers.testAllRunning')}
							</>
						:	<>
								<Play className='h-4 w-4 mr-2' />
								{t('providers.testAll')} ({models.length})
							</>
						}
					</Button>
				</div>

				<div className='border rounded-lg overflow-hidden'>
					{models.length === 0 ?
						<div className='text-sm text-muted-foreground py-8 text-center'>
							{t('providers.validationAtLeastOneModel')}
						</div>
					:	<Virtuoso
							style={{ height: Math.min(models.length * 44, 352) }}
							data={models}
							computeItemKey={(_idx, model) => model}
							itemContent={(_idx, model) => {
								const state = testState[model]
								const status = state?.status ?? 'idle'
								return (
									<div className='flex h-11 items-center gap-3 px-3 border-b last:border-b-0 hover:bg-muted/50 transition-colors'>
										<span className='text-sm font-mono truncate min-w-0 flex-1'>
											{model}
										</span>

										<span className='flex items-center gap-2 shrink-0'>
											{status === 'passed' && (
												<Badge className='bg-emerald-600/15 text-emerald-700 hover:bg-emerald-600/15 dark:bg-emerald-500/15 dark:text-emerald-400 border-0 gap-1'>
													<Check className='h-3 w-3' />
													{t('providers.testLatency', { ms: state?.latency_ms ?? 0 })}
												</Badge>
											)}
											{status === 'failed' && (
												<TooltipProvider delayDuration={0}>
													<Tooltip>
														<TooltipTrigger asChild>
															<Badge className='bg-red-500/15 text-red-700 hover:bg-red-500/15 dark:bg-red-500/15 dark:text-red-400 border-0 gap-1'>
																<X className='h-3 w-3' />
																{state?.latency_ms != null ?
																	t('providers.testLatency', { ms: state.latency_ms })
																:	t('providers.testFailed')
																}
															</Badge>
														</TooltipTrigger>
														{state?.error && (
															<TooltipContent side='left' className='max-w-xs'>
																{state.error}
															</TooltipContent>
														)}
													</Tooltip>
												</TooltipProvider>
											)}
											{status === 'testing' && (
												<Badge variant='secondary' className='gap-1 border-0'>
													<Loader2 className='h-3 w-3 animate-spin' />
													{t('providers.testing')}
												</Badge>
											)}
											{status === 'idle' && (
												<span className='text-xs text-muted-foreground'>
													{t('providers.testIdle')}
												</span>
											)}
										</span>

										<Button
											variant='ghost'
											size='sm'
											className='h-7 px-2 shrink-0'
											disabled={status === 'testing' || testingAll}
											onClick={() => runSingleTest(model)}
										>
											{status === 'testing' ?
												<Loader2 className='h-3.5 w-3.5 animate-spin' />
											:	<Zap className='h-3.5 w-3.5' />
											}
										</Button>
									</div>
								)
							}}
						/>
					}
				</div>
			</DialogContent>
		</Dialog>
	)
}

function ProviderCard({
	provider,
	index,
	total,
	onEdit,
	onDelete,
	onMove,
	onToggle,
	onDragStart,
	onDrop,
	modelMetadata
}: {
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
}) {
	const { t } = useTranslation()
	const [expanded, setExpanded] = useState(false)
	const [testDialogOpen, setTestDialogOpen] = useState(false)
	const [testDialogChannel, setTestDialogChannel] = useState<{
		id: string
		name: string
	} | null>(null)
	const [quickTestingChannelId, setQuickTestingChannelId] = useState<string | null>(null)
	const modelEntries = useMemo(
		() =>
			Object.entries(provider.models).sort(([a], [b]) => a.localeCompare(b)),
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

	const modelNames = useMemo(
		() => Object.keys(provider.models).sort(),
		[provider.models]
	)

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
		}
	}

	return (
		<motion.div
			initial={{ opacity: 0, y: 20 }}
			animate={{ opacity: 1, y: 0 }}
			transition={{ delay: index * 0.08, ...transitions.normal }}
			whileHover={{ y: -2, transition: { duration: 0.2 } }}
		>
			<Card
				className='transition-shadow hover:shadow-md'
				draggable
				onDragStart={() => onDragStart(provider.id)}
				onDragOver={e => e.preventDefault()}
				onDrop={() => onDrop(provider.id)}
			>
				<CardHeader
					className={cn('cursor-pointer select-none py-3', expanded && 'pb-4')}
					onClick={() => setExpanded(v => !v)}
				>
					<div className='flex items-center justify-between gap-3'>
						<div className='flex items-center gap-3 min-w-0'>
							<GripVertical
								className='h-4 w-4 text-muted-foreground/50 hover:text-muted-foreground cursor-grab transition-colors shrink-0'
								onClick={e => e.stopPropagation()}
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
								<CardTitle className='text-base leading-normal -translate-y-px'>{provider.name}</CardTitle>
								<Badge variant='outline' className='font-mono text-xs'>
									{provider.provider_type}
								</Badge>
								<Badge
									variant={provider.enabled ? 'default' : 'secondary'}
									className={
										provider.enabled ?
											'bg-emerald-600/15 text-emerald-700 hover:bg-emerald-600/15 dark:bg-emerald-500/15 dark:text-emerald-400 border-0'
										:	'border-0'
									}
								>
									{provider.enabled ?
										t('common.enabled')
									:	t('common.disabled')}
								</Badge>
							{unpricedCount > 0 && (
								<Badge className='bg-amber-500/15 text-amber-700 hover:bg-amber-500/15 dark:bg-amber-500/15 dark:text-amber-400 border-0'>
									<AlertTriangle className='h-3 w-3 mr-1' />
									{t('providers.unpricedModels', { count: unpricedCount })}
								</Badge>
							)}
								<span className='hidden lg:inline text-xs text-muted-foreground whitespace-nowrap'>
									[{t('providers.priority')}: {provider.priority} ·{' '}
									{t('providers.maxRetriesLabel')}: {provider.max_retries}]
								</span>
							</div>
						</div>
						<div
							className='flex items-center gap-4'
							onClick={e => e.stopPropagation()}
						>
							<div className='hidden md:flex items-center gap-2'>
								<Switch
									checked={provider.enabled}
									onCheckedChange={v => onToggle(provider, v)}
								/>
							</div>
							<TooltipProvider delayDuration={300}>
								<div className='flex items-center gap-1'>
									<Tooltip>
										<TooltipTrigger asChild>
											<Button
												variant='ghost'
												size='icon'
												className='h-8 w-8'
												onClick={() => onMove(index, index - 1)}
												disabled={index === 0}
											>
												<ArrowUp className='h-4 w-4' />
											</Button>
										</TooltipTrigger>
										<TooltipContent>{t('providers.moveUp')}</TooltipContent>
									</Tooltip>
									<Tooltip>
										<TooltipTrigger asChild>
											<Button
												variant='ghost'
												size='icon'
												className='h-8 w-8'
												onClick={() => onMove(index, index + 1)}
												disabled={index === total - 1}
											>
												<ArrowDown className='h-4 w-4' />
											</Button>
										</TooltipTrigger>
										<TooltipContent>{t('providers.moveDown')}</TooltipContent>
									</Tooltip>

									<Separator orientation='vertical' className='h-6 mx-1' />

									<Tooltip>
										<TooltipTrigger asChild>
											<Button
												variant='ghost'
												size='icon'
												className='h-8 w-8'
												onClick={() => onEdit(provider)}
											>
												<Pencil className='h-4 w-4' />
											</Button>
										</TooltipTrigger>
										<TooltipContent>{t('common.edit')}</TooltipContent>
									</Tooltip>
									<Tooltip>
										<TooltipTrigger asChild>
											<Button
												variant='ghost'
												size='icon'
												className='h-8 w-8 text-destructive hover:text-destructive'
												onClick={() => onDelete(provider)}
											>
												<Trash2 className='h-4 w-4' />
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
							onClick={e => e.stopPropagation()}
						>
							<Switch
								checked={provider.enabled}
								onCheckedChange={v => onToggle(provider, v)}
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
										<div className='flex flex-wrap content-start gap-1.5 max-h-[220px] overflow-y-auto'>
											{modelEntries.map(([model, modelEntry]) => {
												const pricingModelId = resolvePricingModelId(
													model,
													modelEntry.redirect
												)
												const meta = modelMetadataById.get(model)
												return (
													<div key={model} className='min-w-0 max-w-full shrink-0'>
														<ModelBadge
															model={model}
															provider={meta?.models_dev_provider}
															multiplier={modelEntry.multiplier}
															redirect={modelEntry.redirect}
															highlightUnpriced={
																!pricedModelIdSet.has(pricingModelId)
															}
														/>
													</div>
												)
											})}
										</div>
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
												provider.channels.length *
													PROVIDER_CHANNEL_OVERVIEW_ROW_HEIGHT,
												190
											)
										}}
										data={provider.channels}
										itemContent={(_idx, channel) => (
											<div
												className={cn(
													'flex min-h-10 items-center gap-3 px-3 py-1.5 text-sm hover:bg-muted/50 transition-colors border-b last:border-b-0'
												)}
											>
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
																	name: channel.name
																})
																setTestDialogOpen(true)
															}}
														>
															<ChevronRight className='h-3.5 w-3.5 text-muted-foreground' />
														</button>
													</span>
													<span className='text-xs text-muted-foreground'>
														W:{channel.weight}
													</span>
													<Badge
														variant={
															channel.enabled ? 'default' : 'secondary'
														}
														className={cn(
															'text-xs',
															channel.enabled ?
																'bg-emerald-600/15 text-emerald-700 hover:bg-emerald-600/15 dark:bg-emerald-500/15 dark:text-emerald-400 border-0'
															:	'border-0'
														)}
													>
														{channel.enabled ?
															t('common.enabled')
														:	t('common.disabled')}
													</Badge>
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
					models={modelNames}
				/>
			)}
		</motion.div>
	)
}

export function ProvidersPage() {
	const { t } = useTranslation()
	const { data: providers = [], isLoading } = useProviders()
	const { data: transformRegistry = [] } = useTransformRegistry()
	const { data: modelMetadata = [] } = useSWR('model-metadata', () =>
		api.listModelMetadata()
	)
	const [createOpen, setCreateOpen] = useState(false)
	const [editProvider, setEditProvider] = useState<Provider | null>(null)
	const [draggingProviderId, setDraggingProviderId] = useState<string | null>(
		null
	)

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
		await applyReorder(next.map(p => p.id))
	}

	const handleDrop = async (targetProviderId: string) => {
		if (!draggingProviderId || draggingProviderId === targetProviderId) {
			return
		}
		const next = [...providers]
		const from = next.findIndex(p => p.id === draggingProviderId)
		const to = next.findIndex(p => p.id === targetProviderId)
		if (from < 0 || to < 0) {
			return
		}
		const [item] = next.splice(from, 1)
		next.splice(to, 0, item)
		setDraggingProviderId(null)
		await applyReorder(next.map(p => p.id))
	}

	const handleDelete = async (provider: Provider) => {
		try {
			await deleteProviderOptimistic(provider.id, providers)
			toast.success(t('providers.deleteSuccess'))
		} catch (error) {
			toast.error(error instanceof Error ? error.message : t('common.error'))
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
					{[...Array(3)].map((_, i) => (
						<Skeleton key={i} className='h-48 w-full' />
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

				{providers.map((provider, idx) => (
					<ProviderCard
						key={provider.id}
						provider={provider}
						index={idx}
						total={providers.length}
						onEdit={p => setEditProvider(p)}
						onDelete={handleDelete}
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
				modelMetadata={modelMetadata}
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
			/>
		</PageWrapper>
	)
}
