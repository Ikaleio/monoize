import { useCallback, useEffect, useId, useMemo, useRef, useState } from 'react'
import { Link } from 'react-router-dom'
import { Virtuoso } from 'react-virtuoso'
import { useTranslation } from 'react-i18next'
import type { TFunction } from 'i18next'
import {
	Activity,
	ArrowLeft,
	Braces,
	ChevronRight,
	CircleGauge,
	Copy,
	GitBranch,
	Layers3,
	Plus,
	Save,
	Server,
	Settings2,
	Trash2
} from 'lucide-react'
import { toast } from 'sonner'
import { TransformChainEditor } from '@/components/transforms/transform-chain-editor'
import { revealTransformRule } from '@/components/transforms/transform-layer'
import { findFirstInvalidTransformRule, formatTransformRuleError } from '@/components/transforms/transform-schema'
import {
	AlertDialog,
	AlertDialogCancel,
	AlertDialogContent,
	AlertDialogDescription,
	AlertDialogFooter,
	AlertDialogHeader,
	AlertDialogTitle
} from '@/components/ui/alert-dialog'
import { Button } from '@/components/ui/button'
import {
	Dialog,
	DialogContent,
	DialogDescription,
	DialogFooter,
	DialogHeader,
	DialogTitle
} from '@/components/ui/dialog'
import { FieldError } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { ListField } from '@/components/ui/list-field'
import { NumberField } from '@/components/ui/number-field'
import { SegmentedControl } from '@/components/ui/segmented-control'
import { Textarea } from '@/components/ui/textarea'
import { Label } from '@/components/ui/label'
import {
	Select,
	SelectContent,
	SelectGroup,
	SelectItem,
	SelectTrigger,
	SelectValue
} from '@/components/ui/select'
import { Separator } from '@/components/ui/separator'
import { Skeleton } from '@/components/ui/skeleton'
import { Switch } from '@/components/ui/switch'
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip'
import type {
	CreateProviderInput,
	AffinityFailbackMode,
	ChatDialect,
	FetchChannelModelsInput,
	ModelMetadataRecord,
	ModelPriceRecord,
	Provider,
	ProviderType,
	SystemSettings,
	TransformRegistryItem
} from '@/lib/api'
import { api } from '@/lib/api'
import { apiErrorParam, toastApiError } from '@/lib/api-error'
import {
	createProviderOptimistic,
	useDashboardGroups,
	useProviderDetail,
	updateProviderOptimistic
} from '@/lib/swr'
import { GroupMultiSelect } from '@/components/groups/GroupPicker'
import { cn } from '@/lib/utils'
import { normalizeMultiplier } from '@/lib/exact-decimal'
import { useChannelTests } from './channel-test-context'
import { ChannelModelEditor } from './ChannelModelEditor'
import { ModelPickerDialog } from './ModelPickerDialog'
import {
	buildPricedModelIdSet,
	CHAT_DIALECTS,
	effectiveChannelStatus,
	effectiveStatusBadge,
	emptyChannelRow,
	emptyForm,
	fromProvider,
	hasTrailingV1,
	type ChannelRow,
	type HealthStatus,
	type ModelRow,
	type ProviderForm,
	PROVIDER_TYPE_CONFIG,
	removeTrailingV1
} from './shared'

type Section = 'provider' | 'channels' | 'routing' | 'transforms' | 'protocol'
type SetForm = React.Dispatch<React.SetStateAction<ProviderForm>>

/**
 * PL26 validation error. `key` is `name`, `transforms`, or `c<index>.<field>` for Channel
 * fields; the focus target of a Channel field is the element id `channel-<field>`.
 */
type ValidationError = { key: string; section: Section; channel?: number; message: string }

const providerTypes = Object.keys(PROVIDER_TYPE_CONFIG) as ProviderType[]

function modelMap(rows: ModelRow[]) {
	return Object.fromEntries(
		rows.map(row => [
			row.model.trim(),
			{
				redirect: row.redirect.trim() || null,
				multiplier: normalizeMultiplier(row.multiplier) ?? row.multiplier.trim()
			}
		])
	)
}

/** Returns the parsed header map, `null` for empty input, or `undefined` when invalid. */
function parseExtraHeaders(raw: string): Record<string, string> | null | undefined {
	const text = raw.trim()
	if (!text) return null
	let parsed: unknown
	try {
		parsed = JSON.parse(text)
	} catch {
		return undefined
	}
	if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) return undefined
	const out: Record<string, string> = {}
	for (const [key, value] of Object.entries(parsed as Record<string, unknown>)) {
		out[key.trim()] = String(value)
	}
	return Object.keys(out).length > 0 ? out : null
}

function buildInput(form: ProviderForm): CreateProviderInput {
	return {
		name: form.name.trim(),
		enabled: form.enabled,
		priority: form.priority,
		max_retries: form.max_retries,
		channel_max_retries: form.channel_max_retries,
		channel_retry_interval_ms: form.channel_retry_interval_ms,
		circuit_breaker_enabled: form.circuit_breaker_enabled,
		per_model_circuit_break: form.per_model_circuit_break,
		channels: form.channels.map(channel => ({
			id: channel.id || undefined,
			name: channel.name.trim(),
			provider_type: channel.provider_type,
			base_url: channel.base_url.trim(),
			api_key: channel.api_key.trim() || undefined,
			weight: channel.weight,
			enabled: channel.enabled,
			models: modelMap(channel.models),
			passive_failure_count_threshold_override: channel.passive_failure_count_threshold_override,
			passive_cooldown_seconds_override: channel.passive_cooldown_seconds_override,
			passive_window_seconds_override: channel.passive_window_seconds_override,
			passive_rate_limit_cooldown_seconds_override: channel.passive_rate_limit_cooldown_seconds_override,
			active_probe_enabled_override: channel.active_probe_enabled_override,
			active_probe_interval_seconds_override: channel.active_probe_interval_seconds_override,
			active_probe_success_threshold_override: channel.active_probe_success_threshold_override,
			active_probe_model_override: channel.active_probe_model_override.trim() || null,
			affinity_enabled_override: channel.affinity_enabled_override,
			affinity_idle_ttl_seconds_override: channel.affinity_idle_ttl_seconds_override,
			affinity_failback_mode_override: channel.affinity_failback_mode_override,
			affinity_failback_delay_seconds_override: channel.affinity_failback_delay_seconds_override,
			proxy_url: channel.proxy_url.trim() || null,
			extra_headers: parseExtraHeaders(channel.extra_headers) ?? null,
			session_affinity_auto: channel.session_affinity_auto,
			websocket_supported: channel.websocket_supported,
			chat_dialect: channel.chat_dialect
		})),
		transforms: form.transforms,
		api_type_overrides: form.api_type_overrides,
		active_probe_enabled_override: form.active_probe_enabled_override,
		active_probe_interval_seconds_override: form.active_probe_interval_seconds_override,
		active_probe_success_threshold_override: form.active_probe_success_threshold_override,
		active_probe_model_override: form.active_probe_model_override.trim() || null,
		request_timeout_ms_override: form.request_timeout_ms_override,
		extra_fields_whitelist: form.extra_fields_whitelist,
		strip_cross_protocol_nested_extra: form.strip_cross_protocol_nested_extra,
		allow_free_when_unpriced_override: form.allow_free_when_unpriced_override,
		allow_free_when_missing_usage_override: form.allow_free_when_missing_usage_override,
		group_ids: form.group_ids
	}
}

function focusField(id: string) {
	// Two frames: the first lets a section/Channel switch commit and mount the target.
	requestAnimationFrame(() => requestAnimationFrame(() => {
		const element = document.getElementById(id)
		if (!element) return
		const details = element.closest('details')
		if (details && !details.open) details.open = true
		element.scrollIntoView({ block: 'center' })
		element.focus()
	}))
}

/** PL25: "继承全局设置" or "N 项覆盖：a · b…" from the set fields of one advanced group. */
function overrideSummary(items: Array<[string, string | number | boolean | null | undefined]>, t: TFunction) {
	const set = items.filter(([, value]) => value !== null && value !== undefined && value !== '')
	if (set.length === 0) return t('providers.editor.inheritGlobalSummary')
	const shown = set.slice(0, 2).map(([label, value]) =>
		t('providers.editor.overrideItem', {
			label,
			value: typeof value === 'boolean' ? (value ? t('common.enabled') : t('common.disabled')) : String(value)
		}))
	return t('providers.editor.overrideSummary', { count: set.length, items: shown.join(' · ') + (set.length > 2 ? '…' : '') })
}

export function ProviderDialog({
	open,
	onOpenChange,
	mode,
	current,
	providers,
	transformRegistry,
	transformRegistryLoading,
	modelMetadata,
	modelPrices,
	reasoningSuffixMap,
	settings
}: {
	open: boolean
	onOpenChange: (open: boolean) => void
	mode: 'create' | 'edit'
	current: Provider | null
	providers: Provider[]
	transformRegistry: TransformRegistryItem[]
	transformRegistryLoading?: boolean
	modelMetadata: ModelMetadataRecord[]
	modelPrices: ModelPriceRecord[]
	reasoningSuffixMap: Record<string, string>
	settings?: SystemSettings
}) {
	const { t, i18n } = useTranslation()
	const isEdit = mode === 'edit'
	const titleId = useId()
	const [form, setFormState] = useState<ProviderForm>(emptyForm)
	const [dirty, setDirty] = useState(false)
	const [hydrated, setHydrated] = useState(false)
	const [errors, setErrors] = useState<Record<string, string>>({})
	// A ref, not state: validity is reported from a blur effect and must be visible to the
	// click handler of the same pointer gesture, before React re-renders.
	const invalidNumbers = useRef<Set<string>>(new Set())
	const setForm = useCallback<SetForm>(updater => {
		setDirty(true)
		setFormState(updater)
	}, [])
	const [section, setSection] = useState<Section>('channels')
	const [selectedChannel, setSelectedChannel] = useState(0)
	const [mobileChannelOpen, setMobileChannelOpen] = useState(false)
	const [saving, setSaving] = useState(false)
	const [pickerOpen, setPickerOpen] = useState(false)
	const [closeConfirmOpen, setCloseConfirmOpen] = useState(false)
	// PL17: detail revalidation never reaches the draft; the form hydrates once per open.
	const { data: detail, error: detailError, isValidating: detailValidating } = useProviderDetail(
		open && isEdit && current ? current.id : null,
		{ revalidateOnFocus: false, revalidateOnReconnect: false, refreshInterval: 0 }
	)

	useEffect(() => {
		if (!open) {
			setHydrated(false)
			return
		}
		if (hydrated) return
		let source: Provider | null = null
		if (isEdit) {
			source = detail && !detailValidating ? detail : detailError ? current : null
			if (!source) return
		}
		const next = source ? fromProvider(source) : emptyForm()
		setFormState(next)
		setDirty(false)
		setErrors({})
		invalidNumbers.current = new Set()
		setSelectedChannel(0)
		setSection(isEdit ? 'channels' : 'provider')
		setMobileChannelOpen(isEdit && next.channels.length === 1)
		setHydrated(true)
	}, [open, hydrated, isEdit, detail, detailValidating, detailError, current])

	// Runtime health comes from the polled list and is never part of the draft.
	const healthByChannelId = useMemo(() => {
		const provider = providers.find(item => item.id === current?.id)
		return new Map((provider?.channels ?? []).map(channel => [channel.id, channel._health_status]))
	}, [providers, current?.id])
	const position = current ? providers.findIndex(item => item.id === current.id) : -1

	const activeChannel = form.channels[selectedChannel]
	const pricedModels = useMemo(() => buildPricedModelIdSet(modelPrices), [modelPrices])
	const metadataProvider = useMemo(
		() => new Map(modelMetadata.map(item => [item.model_id, item.models_dev_provider])),
		[modelMetadata]
	)

	const clearErrors = (keys: string[]) => {
		setErrors(previous => {
			if (!keys.some(key => key in previous)) return previous
			const next = { ...previous }
			for (const key of keys) delete next[key]
			return next
		})
	}

	const updateChannel = (index: number, patch: Partial<ChannelRow>) => {
		clearErrors(Object.keys(patch).map(field => `c${index}.${field}`))
		setForm(previous => ({
			...previous,
			channels: previous.channels.map((channel, channelIndex) =>
				channelIndex === index ? { ...channel, ...patch } : channel
			)
		}))
	}

	const trackNumber = (id: string) => (valid: boolean) => {
		if (valid) invalidNumbers.current.delete(id)
		else invalidNumbers.current.add(id)
	}

	const validate = (): ValidationError[] => {
		const out: ValidationError[] = []
		if (!form.name.trim()) out.push({ key: 'name', section: 'provider', message: t('providers.editor.nameRequired') })
		const noModels = !form.channels.some(channel => channel.models.length > 0)
		// Errors are pushed in on-screen field order so `found[0]` is the topmost invalid field.
		for (const [index, channel] of form.channels.entries()) {
			const push = (field: string, message: string) => out.push({ key: `c${index}.${field}`, section: 'channels', channel: index, message })
			if (!channel.name.trim()) push('name', t('providers.editor.channelNameRequired'))
			if (!channel.base_url.trim()) push('base_url', t('providers.editor.baseUrlRequired'))
			if (!channel.id && !channel.api_key.trim()) push('api_key', t('providers.editor.newChannelKeyRequired'))
			const names = channel.models.map(model => model.model.trim())
			if (index === 0 && noModels) {
				push('models', t('providers.editor.modelsRequired'))
			} else if (names.some(name => !name) || new Set(names).size !== names.length) {
				push('models', t('providers.editor.channelModelsInvalid'))
			} else if (channel.models.some(model => normalizeMultiplier(model.multiplier) == null)) {
				push('models', t('providers.editor.channelMultiplierInvalid'))
			}
			if (parseExtraHeaders(channel.extra_headers) === undefined) push('extra_headers', t('providers.editor.extraHeadersInvalid'))
		}
		const invalidTransform = findFirstInvalidTransformRule(form.transforms, transformRegistry)
		if (invalidTransform) {
			out.push({
				key: 'transforms',
				section: 'transforms',
				message: formatTransformRuleError(invalidTransform, form.transforms, transformRegistry, t, i18n.language)
			})
		}
		return out
	}

	const goToError = (error: ValidationError) => {
		setSection(error.section)
		if (error.channel !== undefined) {
			setSelectedChannel(error.channel)
			setMobileChannelOpen(true)
		}
		if (error.key === 'transforms') {
			const invalid = findFirstInvalidTransformRule(form.transforms, transformRegistry)
			if (invalid) requestAnimationFrame(() => requestAnimationFrame(() => revealTransformRule('provider', invalid.index)))
			return
		}
		focusField(error.key === 'name' ? 'provider-name' : `channel-${error.key.split('.')[1]}`)
	}

	const save = async (): Promise<boolean> => {
		if (saving) return false
		if (blockedByInvalidNumber()) return false
		const found = validate()
		setErrors(Object.fromEntries(found.map(error => [error.key, error.message])))
		if (found.length > 0) {
			goToError(found[0])
			return false
		}
		setSaving(true)
		try {
			const input = buildInput(form)
			if (isEdit && current) {
				await updateProviderOptimistic(current.id, input, providers)
			} else {
				await createProviderOptimistic(input, providers)
			}
			toast.success(t('providers.editor.saveSuccess'))
			setDirty(false)
			onOpenChange(false)
			return true
		} catch (error) {
			if (apiErrorParam(error) === 'name') {
				setErrors({ name: t('providers.editor.nameRejected') })
				goToError({ key: 'name', section: 'provider', message: '' })
			} else {
				toastApiError(error, t, t('providers.editor.saveFailed'))
			}
			return false
		} finally {
			setSaving(false)
		}
	}

	const requestClose = () => {
		if (dirty) setCloseConfirmOpen(true)
		else onOpenChange(false)
	}

	// An unmounted NumberField drops its invalid text and the last valid value would be
	// saved silently, so leaving the view of an invalid number field is refused (PL26).
	const blockedByInvalidNumber = () => {
		if (invalidNumbers.current.size === 0) return false
		focusField([...invalidNumbers.current][0])
		return true
	}

	const goToSection = (next: Section) => {
		if (next !== section && blockedByInvalidNumber()) return
		setSection(next)
	}

	const selectChannel = (index: number) => {
		if (index !== selectedChannel && blockedByInvalidNumber()) return
		setSelectedChannel(index)
		setMobileChannelOpen(true)
	}

	const closeMobileChannel = (open: boolean) => {
		if (!open && blockedByInvalidNumber()) return
		setMobileChannelOpen(open)
	}

	const addChannel = () => {
		if (blockedByInvalidNumber()) return
		setErrors({})
		setForm(previous => ({ ...previous, channels: [...previous.channels, emptyChannelRow()] }))
		setSection('channels')
		selectChannel(form.channels.length)
	}

	const duplicateChannel = () => {
		if (!activeChannel || blockedByInvalidNumber()) return
		setErrors({})
		const duplicate: ChannelRow = {
			...activeChannel,
			id: '',
			name: t('providers.editor.duplicateName', { name: activeChannel.name }),
			api_key: '',
			models: activeChannel.models.map(model => ({ ...model }))
		}
		setForm(previous => ({ ...previous, channels: [...previous.channels, duplicate] }))
		setSelectedChannel(form.channels.length)
		focusField('channel-api_key')
	}

	const removeChannel = () => {
		if (!activeChannel || form.channels.length === 1) return
		setErrors({})
		setForm(previous => ({ ...previous, channels: previous.channels.filter((_, index) => index !== selectedChannel) }))
		setSelectedChannel(Math.max(0, selectedChannel - 1))
		setMobileChannelOpen(false)
	}

	const pickerInfo: FetchChannelModelsInput | undefined = activeChannel?.base_url.trim() && (
		activeChannel.api_key.trim() || (isEdit && current && activeChannel.id)
	) ? {
		provider_type: activeChannel.provider_type,
		base_url: activeChannel.base_url.trim(),
		api_key: activeChannel.api_key.trim() || undefined,
		provider_id: activeChannel.api_key.trim() ? undefined : current?.id,
		channel_id: activeChannel.api_key.trim() ? undefined : activeChannel.id
	} : undefined

	const errorKeys = Object.keys(errors)
	const sectionHasError = (id: Section) => errorKeys.some(key =>
		id === 'channels' ? key.startsWith('c') : id === 'provider' ? key === 'name' : key === id)
	const channelHasError = (index: number) => errorKeys.some(key => key.startsWith(`c${index}.`))

	const sections: Array<{ id: Section; icon: typeof Server; label: string; summary: string }> = [
		{ id: 'provider', icon: Server, label: 'Provider', summary: form.name || t('providers.editor.untitled') },
		{ id: 'channels', icon: Layers3, label: 'Channel', summary: t('providers.editor.channelCount', { count: form.channels.length }) },
		{ id: 'routing', icon: GitBranch, label: t('providers.editor.sectionRouting'), summary: form.max_retries < 0 ? t('providers.retryAllChannels') : t('providers.retryAtMost', { count: form.max_retries + 1 }) },
		{ id: 'transforms', icon: Braces, label: t('providers.editor.sectionTransforms'), summary: t('providers.editor.ruleCount', { count: form.transforms.length }) },
		{ id: 'protocol', icon: Settings2, label: t('providers.editor.sectionProtocol'), summary: t('providers.editor.overrideCount', { count: form.api_type_overrides.length }) }
	]

	const footerStatus = dirty
		? t('providers.editor.unsavedChanges')
		: isEdit ? t('providers.editor.noChanges') : t('providers.editor.notSavedYet')

	return (
		<>
			<Dialog open={open} onOpenChange={next => { if (!next) requestClose() }}>
				<DialogContent
					aria-labelledby={titleId}
					className='flex h-[calc(100dvh-2rem)] w-screen max-w-none flex-col gap-0 overflow-hidden rounded-none border-0 p-0 sm:h-[94vh] sm:w-[96vw] sm:max-w-[1500px] sm:rounded-xl sm:border [&>button:last-child]:right-2 [&>button:last-child]:top-2 [&>button:last-child]:flex [&>button:last-child]:size-10 [&>button:last-child]:items-center [&>button:last-child]:justify-center [&>div:first-child]:h-full [&>div:first-child]:min-h-0 [&>div:first-child]:flex-1 [&>div:first-child]:gap-0 [&>div:first-child]:overflow-hidden'
					onPointerDownOutside={event => event.preventDefault()}
					onOpenAutoFocus={event => {
						// PL4.1: never land on the enable switch.
						event.preventDefault()
						requestAnimationFrame(() => document.getElementById(isEdit ? titleId : 'provider-name')?.focus())
					}}
				>
					<form
						noValidate
						className='flex h-full min-h-0 flex-1 flex-col'
						onSubmit={event => {
							// Submits from portaled child dialogs bubble here through React; ignore them.
							if (event.target !== event.currentTarget) return
							event.preventDefault()
							void save()
						}}
					>
					<DialogHeader className='shrink-0 border-b bg-background py-3 pl-4 pr-16 text-left sm:pl-6 sm:pr-16'>
						<div className='flex min-w-0 items-center justify-between gap-3'>
							<div className='min-w-0'>
								<DialogTitle id={titleId} tabIndex={-1} className='truncate text-base outline-none sm:text-lg'>
									{isEdit ? t('providers.editor.editTitle') : t('providers.editor.createTitle')}
									{form.name ? <span className='font-normal text-muted-foreground'> · {form.name}</span> : null}
								</DialogTitle>
								<DialogDescription className='mt-0.5 hidden sm:block'>
									{t('providers.editor.description')}
								</DialogDescription>
							</div>
							<div className='flex shrink-0 items-center gap-2'>
								<Label htmlFor='provider-enabled' className='hidden text-xs text-muted-foreground sm:block'>
									{form.enabled ? t('providers.editor.enableOnSave') : t('providers.editor.disableOnSave')}
								</Label>
								<Switch id='provider-enabled' checked={form.enabled} onCheckedChange={enabled => setForm(previous => ({ ...previous, enabled }))} />
							</div>
						</div>
					</DialogHeader>

					{!hydrated ? (
						<div className='grid flex-1 grid-cols-1 gap-4 overflow-hidden p-4 lg:grid-cols-[220px_1fr]'>
							<Skeleton className='hidden h-full lg:block' />
							<div className='flex flex-col gap-3'><Skeleton className='h-16' /><Skeleton className='h-80' /><Skeleton className='h-20' /></div>
						</div>
					) : (
						<div className='flex min-h-0 flex-1'>
							<nav className='hidden w-56 shrink-0 flex-col gap-1 border-r bg-muted/20 p-3 lg:flex' aria-label={t('providers.editor.sectionsLabel')}>
								{sections.map(item => {
									const Icon = item.icon
									return <button key={item.id} type='button' aria-current={section === item.id ? 'page' : undefined} onClick={() => goToSection(item.id)} className={cn('flex min-h-14 items-center gap-3 rounded-lg px-3 text-left transition-colors', section === item.id ? 'bg-primary/10 text-primary' : 'text-muted-foreground hover:bg-muted hover:text-foreground')}>
										<Icon className='size-4 shrink-0' aria-hidden='true' />
										<span className='min-w-0 flex-1'><span className='flex items-center gap-1.5 text-sm font-medium'>{item.label}{sectionHasError(item.id) ? <ErrorDot label={t('providers.editor.sectionHasErrors')} /> : null}</span><span className='block truncate text-xs opacity-70'>{item.summary}</span></span>
										<ChevronRight className='size-4 shrink-0 opacity-50' aria-hidden='true' />
									</button>
								})}
							</nav>

							<div className='flex min-w-0 flex-1 flex-col'>
								<div className='flex shrink-0 gap-1 overflow-x-auto border-b bg-background px-3 py-2 lg:hidden'>
									{sections.map(item => (
										<Button
											key={item.id}
											type='button'
											size='sm'
											variant={section === item.id ? 'secondary' : 'ghost'}
											aria-current={section === item.id ? 'page' : undefined}
											onClick={() => { if (item.id !== section && blockedByInvalidNumber()) return; setSection(item.id); setMobileChannelOpen(item.id === 'channels' && form.channels.length === 1) }}
											className='relative shrink-0'
										>
											{item.label}
											{sectionHasError(item.id) ? <ErrorDot label={t('providers.editor.sectionHasErrors')} className='absolute right-1 top-1' /> : null}
										</Button>
									))}
								</div>

								<div className={cn('min-h-0 flex-1', section === 'channels' ? 'flex flex-col overflow-hidden' : 'overflow-y-auto')}>
									{section === 'channels' ? (
										<ChannelsWorkbench
											form={form}
											activeChannel={activeChannel}
											selectedChannel={selectedChannel}
											mobileChannelOpen={mobileChannelOpen}
											setMobileChannelOpen={closeMobileChannel}
											selectChannel={selectChannel}
											updateChannel={updateChannel}
											addChannel={addChannel}
											duplicateChannel={duplicateChannel}
											removeChannel={removeChannel}
											openPicker={() => {
												if (!pickerInfo) toast.error(t('providers.editor.pickerNeedsConnection'))
												else setPickerOpen(true)
											}}
											pricedModels={pricedModels}
											metadataProvider={metadataProvider}
											reasoningSuffixMap={reasoningSuffixMap}
											settings={settings}
											errors={errors}
											channelHasError={channelHasError}
											healthByChannelId={healthByChannelId}
											trackNumber={trackNumber}
											dirty={dirty}
											providerId={current?.id}
											providerName={form.name}
										/>
									) : section === 'provider' ? (
										<ProviderBasics form={form} setForm={setForm} nameError={errors.name} clearNameError={() => clearErrors(['name'])} />
									) : section === 'routing' ? (
										<RoutingSettings form={form} setForm={setForm} settings={settings} position={position} trackNumber={trackNumber} />
									) : section === 'transforms' ? (
										<div className='mx-auto flex w-full max-w-4xl flex-col gap-5 p-4 sm:p-6'>
											<SectionHeading title={t('providers.editor.transformsTitle')} description={t('providers.editor.transformsDescription')} />
											{errors.transforms ? <FieldError>{errors.transforms}</FieldError> : null}
											<TransformChainEditor
												layer='provider'
												value={form.transforms}
												registry={transformRegistry}
												loading={transformRegistryLoading}
												onChange={transforms => { clearErrors(['transforms']); setForm(previous => ({ ...previous, transforms })) }}
											/>
										</div>
									) : (
										<ProtocolSettings form={form} setForm={setForm} />
									)}
								</div>
							</div>
						</div>
					)}

					<DialogFooter className='shrink-0 flex-row items-center justify-between gap-2 border-t bg-background px-4 py-3 sm:px-6'>
						<p className='hidden text-xs text-muted-foreground sm:block' aria-live='polite'>{footerStatus}</p>
						<div className='ml-auto flex items-center gap-2'>
							<Button type='button' variant='outline' onClick={requestClose}>{t('common.cancel')}</Button>
							<Button type='submit' disabled={saving || !hydrated}><Save data-icon />{saving ? t('common.saving') : t('providers.editor.save')}</Button>
						</div>
					</DialogFooter>
					</form>
				</DialogContent>
			</Dialog>

			<ModelPickerDialog
				open={pickerOpen}
				onOpenChange={setPickerOpen}
				channelInfo={pickerInfo}
				providerName={`${form.name || t('providers.editor.untitledProvider')} / ${activeChannel?.name || t('providers.editor.untitledChannel')}`}
				existingModels={activeChannel?.models.map(model => model.model) ?? []}
				modelMetadata={modelMetadata}
				modelPrices={modelPrices}
				reasoningSuffixMap={reasoningSuffixMap}
				onConfirm={selected => {
					if (!activeChannel) return
					const existing = new Map(activeChannel.models.map(model => [model.model, model]))
					updateChannel(selectedChannel, { models: selected.sort().map(model => existing.get(model) ?? { model, redirect: '', multiplier: '1' }) })
				}}
			/>

			<AlertDialog open={closeConfirmOpen} onOpenChange={setCloseConfirmOpen}>
				<AlertDialogContent>
					<AlertDialogHeader>
						<AlertDialogTitle>{t('providers.editor.discardTitle')}</AlertDialogTitle>
						<AlertDialogDescription>{t('providers.editor.discardDescription')}</AlertDialogDescription>
					</AlertDialogHeader>
					<AlertDialogFooter className='gap-2 sm:gap-0'>
						<AlertDialogCancel>{t('providers.editor.keepEditing')}</AlertDialogCancel>
						<Button
							type='button'
							className='bg-destructive text-destructive-foreground hover:bg-destructive/90'
							onClick={() => {
								setCloseConfirmOpen(false)
								onOpenChange(false)
							}}
						>
							{t('providers.editor.discard')}
						</Button>
						<Button
							type='button'
							disabled={saving}
							onClick={() => {
								setCloseConfirmOpen(false)
								void save()
							}}
						>
							{t('providers.editor.saveAndClose')}
						</Button>
					</AlertDialogFooter>
				</AlertDialogContent>
			</AlertDialog>
		</>
	)
}

function ErrorDot({ label, className }: { label: string; className?: string }) {
	return <span role='img' aria-label={label} className={cn('inline-block size-2 shrink-0 rounded-full bg-destructive', className)} />
}

function SectionHeading({ title, description }: { title: string; description: string }) {
	return <div><h3 className='text-lg font-semibold'>{title}</h3><p className='mt-1 text-sm text-muted-foreground'>{description}</p></div>
}

/** Labelled form field: label bound to `id`, optional "必填" marker, hint and inline error. */
function Field({ id, label, hint, error, required, children, className }: { id?: string; label: string; hint?: React.ReactNode; error?: string; required?: boolean; children: React.ReactNode; className?: string }) {
	const { t } = useTranslation()
	return <div className={cn('flex flex-col gap-2', className)}>
		<Label htmlFor={id}>
			{label}
			{required ? <span className='ml-2 text-xs font-normal text-muted-foreground'>{t('providers.editor.required')}</span> : null}
		</Label>
		{children}
		{hint ? <p className='text-xs text-muted-foreground'>{hint}</p> : null}
		{error ? <FieldError>{error}</FieldError> : null}
	</div>
}

function ProviderBasics({ form, setForm, nameError, clearNameError }: { form: ProviderForm; setForm: SetForm; nameError?: string; clearNameError: () => void }) {
	const { t } = useTranslation()
	const { data: groups = [], isLoading: groupsLoading } = useDashboardGroups()
	return <div className='mx-auto flex w-full max-w-3xl flex-col gap-6 p-4 sm:p-6'>
		<SectionHeading title={t('providers.editor.basicsTitle')} description={t('providers.editor.basicsDescription')} />
		<div className='grid gap-5 rounded-xl border bg-card p-4 sm:grid-cols-2 sm:p-5'>
			<Field id='provider-name' label={t('common.name')} required error={nameError} className='sm:col-span-2'>
				<Input id='provider-name' aria-invalid={Boolean(nameError) || undefined} value={form.name} onChange={event => { clearNameError(); setForm(previous => ({ ...previous, name: event.target.value })) }} placeholder='OpenAI production' />
			</Field>
			<Field label={t('providers.editor.groups')} hint={t('providers.editor.groupsHint')} className='sm:col-span-2'>
				<GroupMultiSelect
					value={form.group_ids}
					groups={groups}
					loading={groupsLoading}
					onChange={group_ids => setForm(previous => ({ ...previous, group_ids }))}
				/>
			</Field>
			<Field id='provider-extra-fields' label={t('providers.editor.extraFields')} hint={t('providers.editor.extraFieldsHint')}>
				<ListField id='provider-extra-fields' value={form.extra_fields_whitelist} placeholder='service_tier, metadata' onValueChange={extra_fields_whitelist => setForm(previous => ({ ...previous, extra_fields_whitelist }))} />
			</Field>
		</div>
		<div className='grid gap-5 rounded-xl border bg-card p-4 sm:grid-cols-2 sm:p-5'>
			<div className='sm:col-span-2'>
				<h4 className='font-medium'>{t('providers.editor.freeSettlement')}</h4>
				<p className='mt-1 text-xs text-muted-foreground'>{t('providers.editor.freeSettlementHint')}</p>
			</div>
			<NullableBoolean
				id='provider-free-unpriced'
				label={t('providers.editor.freeWhenUnpriced')}
				value={form.allow_free_when_unpriced_override}
				onChange={value => setForm(previous => ({ ...previous, allow_free_when_unpriced_override: value }))}
			/>
			<NullableBoolean
				id='provider-free-usage'
				label={t('providers.editor.freeWhenUsageMissing')}
				value={form.allow_free_when_missing_usage_override}
				onChange={value => setForm(previous => ({ ...previous, allow_free_when_missing_usage_override: value }))}
			/>
		</div>
	</div>
}

type WorkbenchProps = {
	form: ProviderForm
	activeChannel?: ChannelRow
	selectedChannel: number
	mobileChannelOpen: boolean
	setMobileChannelOpen: (value: boolean) => void
	selectChannel: (index: number) => void
	updateChannel: (index: number, patch: Partial<ChannelRow>) => void
	addChannel: () => void
	duplicateChannel: () => void
	removeChannel: () => void
	openPicker: () => void
	pricedModels: Set<string>
	metadataProvider: Map<string, string | undefined>
	reasoningSuffixMap: Record<string, string>
	settings?: SystemSettings
	errors: Record<string, string>
	channelHasError: (index: number) => boolean
	healthByChannelId: Map<string, HealthStatus | undefined>
	trackNumber: (id: string) => (valid: boolean) => void
	dirty: boolean
	providerId?: string
	providerName: string
}

function channelStatus(channel: ChannelRow, health: Map<string, HealthStatus | undefined>) {
	return effectiveChannelStatus(channel.enabled, channel.weight, channel.id ? health.get(channel.id) ?? 'healthy' : undefined)
}

function ChannelsWorkbench(props: WorkbenchProps) {
	const { form, selectedChannel, mobileChannelOpen, selectChannel, addChannel, channelHasError, healthByChannelId, activeChannel } = props
	const { t } = useTranslation()
	return <div className='grid h-full min-h-0 flex-1 lg:grid-cols-[300px_minmax(0,1fr)]'>
		<div className={cn('flex h-full min-h-0 min-w-0 flex-col overflow-hidden border-r bg-muted/10', mobileChannelOpen ? 'hidden lg:flex' : 'flex')}>
			<div className='flex shrink-0 items-center justify-between border-b px-4 py-3'><div><h3 className='font-semibold'>Channel</h3><p className='text-xs text-muted-foreground'>{t('providers.editor.channelsSubtitle')}</p></div><Button type='button' size='icon' variant='outline' className='size-11 touch-manipulation sm:size-9' onClick={addChannel} aria-label={t('providers.editor.addChannel')}><Plus data-icon /></Button></div>
			<div className='relative min-h-0 flex-1'>
				<Virtuoso
					className='absolute inset-0 pt-2'
					style={{ height: '100%' }}
					data={form.channels}
					itemContent={(index, channel) => (
						<div className='px-2 pb-1'>
							<button type='button' aria-current={selectedChannel === index ? 'true' : undefined} onClick={() => selectChannel(index)} className={cn('flex min-h-16 w-full items-center gap-3 rounded-lg border-l-2 px-3 py-2 text-left transition-colors', selectedChannel === index ? 'border-l-primary bg-primary/10' : 'border-l-transparent hover:bg-muted')}>
								<div className='min-w-0 flex-1'>
									<div className='flex items-center gap-2'>
										<span className='truncate text-sm font-medium'>{channel.name || t('providers.editor.untitledChannel')}</span>
										{channelHasError(index) ? <ErrorDot label={t('providers.editor.channelHasErrors')} /> : null}
										{effectiveStatusBadge(channelStatus(channel, healthByChannelId), t)}
									</div>
									<p className='mt-1 truncate font-mono text-xs text-muted-foreground'>{channel.base_url || t('providers.editor.noBaseUrl')}</p>
									<p className='mt-1 text-xs text-muted-foreground'>{PROVIDER_TYPE_CONFIG[channel.provider_type]?.label ?? channel.provider_type} · {t('providers.modelCount', { count: channel.models.length })} · {t('providers.weightValue', { weight: channel.weight })}</p>
								</div>
								<ChevronRight className='size-4 shrink-0 text-muted-foreground' aria-hidden='true' />
							</button>
						</div>
					)}
				/>
			</div>
		</div>

		<div className={cn('min-h-0 min-w-0 overflow-y-auto', mobileChannelOpen ? 'block' : 'hidden lg:block')}>
			{activeChannel ? <ChannelDetail {...props} /> : <div className='grid h-full place-items-center p-6 text-center text-sm text-muted-foreground'>{t('providers.editor.selectChannel')}</div>}
		</div>
	</div>
}

function ChannelDetail({ form, activeChannel, selectedChannel, setMobileChannelOpen, updateChannel, duplicateChannel, removeChannel, openPicker, pricedModels, metadataProvider, reasoningSuffixMap, settings, errors, healthByChannelId, trackNumber, dirty, providerId, providerName }: WorkbenchProps) {
	const { t } = useTranslation()
	const { openTest } = useChannelTests()
	const [probingWebsocket, setProbingWebsocket] = useState(false)
	if (!activeChannel) return null
	const error = (field: string) => errors[`c${selectedChannel}.${field}`]
	const update = (patch: Partial<ChannelRow>) => updateChannel(selectedChannel, patch)
	const canTest = Boolean(providerId && activeChannel.id && !dirty)
	const ch = activeChannel
	const headerCount = Object.keys(parseExtraHeaders(ch.extra_headers) ?? {}).length

	return <div className='mx-auto flex w-full max-w-5xl flex-col gap-6 p-4 pb-8 sm:p-6'>
		<div className='flex items-start justify-between gap-3'>
			<div className='flex min-w-0 items-start gap-2'>
				<Button type='button' size='icon' variant='ghost' className='-ml-2 size-11 touch-manipulation sm:size-9 lg:hidden' onClick={() => setMobileChannelOpen(false)} aria-label={t('providers.editor.backToChannels')}><ArrowLeft data-icon /></Button>
				<div className='min-w-0'>
					<h3 className='truncate text-lg font-semibold'>{ch.name || t('providers.editor.untitledChannel')}</h3>
					<div className='mt-1'>{effectiveStatusBadge(channelStatus(ch, healthByChannelId), t)}</div>
				</div>
			</div>
			<div className='flex items-center gap-1'>
				<TooltipProvider delayDuration={200}>
					<Tooltip>
						<TooltipTrigger asChild>
							{/* A disabled button emits no pointer events, so the tooltip anchors on a wrapper. */}
							<span tabIndex={canTest ? undefined : 0}>
								<Button
									type='button'
									variant='outline'
									size='sm'
									className='h-11 sm:h-9'
									disabled={!canTest}
									onClick={() => providerId && openTest({
										providerId,
										providerName,
										channelId: ch.id,
										channelName: ch.name,
										providerType: ch.provider_type,
										models: ch.models.map(model => model.model).sort()
									})}
								>
									<Activity data-icon />
									{t('providers.editor.testChannel')}
								</Button>
							</span>
						</TooltipTrigger>
						{canTest ? null : <TooltipContent>{t('providers.editor.testNeedsSave')}</TooltipContent>}
					</Tooltip>
				</TooltipProvider>
				<Button type='button' size='icon' variant='ghost' className='size-11 touch-manipulation sm:size-9' onClick={duplicateChannel} aria-label={t('providers.editor.duplicateChannel')}><Copy data-icon /></Button>
				<Button type='button' size='icon' variant='ghost' className='size-11 touch-manipulation sm:size-9' disabled={form.channels.length === 1} onClick={removeChannel} aria-label={t('providers.editor.deleteChannel')}><Trash2 data-icon /></Button>
				<div className='flex items-center gap-2'>
					<Label htmlFor='channel-enabled' className='hidden text-xs text-muted-foreground sm:block'>{ch.enabled ? t('common.enabled') : t('common.disabled')}</Label>
					<Switch id='channel-enabled' checked={ch.enabled} onCheckedChange={enabled => update({ enabled })} />
				</div>
			</div>
		</div>

		<section className='flex flex-col gap-4 rounded-xl border bg-card p-4 sm:p-5'>
			<div className='flex items-center gap-2'><Server className='size-4 text-primary' aria-hidden='true' /><h4 className='font-medium'>{t('providers.editor.connection')}</h4></div>
			<div className='grid gap-4 sm:grid-cols-2'>
				<Field id='channel-name' label={t('providers.editor.channelName')} required error={error('name')}>
					<Input id='channel-name' aria-invalid={Boolean(error('name')) || undefined} value={ch.name} onChange={event => update({ name: event.target.value })} />
				</Field>
				<Field id='channel-provider-type' label={t('providers.editor.apiType')}>
					<Select value={ch.provider_type} onValueChange={(provider_type: ProviderType) => update({ provider_type, chat_dialect: provider_type === 'chat_completion' ? ch.chat_dialect ?? 'generic' : null })}><SelectTrigger id='channel-provider-type'><SelectValue /></SelectTrigger><SelectContent><SelectGroup>{providerTypes.map(type => <SelectItem key={type} value={type}>{PROVIDER_TYPE_CONFIG[type].label}</SelectItem>)}</SelectGroup></SelectContent></Select>
				</Field>
				{ch.provider_type === 'chat_completion' ? (
					<Field id='channel-chat-dialect' label={t('providers.chatDialect')} hint={t('providers.chatDialectHint')} className='sm:col-span-2'>
						<Select value={ch.chat_dialect ?? 'generic'} onValueChange={(chat_dialect: ChatDialect) => update({ chat_dialect })}><SelectTrigger id='channel-chat-dialect'><SelectValue /></SelectTrigger><SelectContent><SelectGroup>{CHAT_DIALECTS.map(dialect => <SelectItem key={dialect} value={dialect}>{t(`providers.chatDialects.${dialect}`)}</SelectItem>)}</SelectGroup></SelectContent></Select>
					</Field>
				) : null}
				{ch.provider_type === 'responses' ? (
					<div className='grid gap-2 sm:col-span-2 sm:grid-cols-[1fr_auto] sm:items-end'>
						<NullableBoolean
							id='channel-websocket'
							label={t('providers.websocketSupported')}
							hint={t('providers.websocketSupportedHint')}
							nullLabel={t('providers.websocketUnknown')}
							value={ch.websocket_supported}
							onChange={value => update({ websocket_supported: value })}
							enabledLabel={t('providers.websocketSupportedYes')}
							disabledLabel={t('providers.websocketSupportedNo')}
						/>
						<Button
							type='button'
							variant='outline'
							disabled={probingWebsocket || !ch.base_url.trim() || (!ch.api_key.trim() && !ch.id)}
							onClick={async () => {
								setProbingWebsocket(true)
								try {
									const result = await api.probeChannelWebsocket({
										provider_type: ch.provider_type,
										base_url: ch.base_url.trim(),
										api_key: ch.api_key.trim() || undefined,
										provider_id: form.id || undefined,
										channel_id: ch.id || undefined,
										model: ch.active_probe_model_override.trim()
											|| ch.models.find(model => model.model.trim())?.model.trim()
									})
									update({ websocket_supported: result.supported })
									if (result.supported) toast.success(t('providers.detectWebsocketSuccess'))
									else toast.error(t('providers.detectWebsocketFailure'))
								} catch (probeError) {
									update({ websocket_supported: false })
									toastApiError(probeError, t, t('providers.detectWebsocketFailure'))
								} finally {
									setProbingWebsocket(false)
								}
							}}
						>
							{probingWebsocket ? t('providers.detectWebsocketRunning') : t('providers.detectWebsocket')}
						</Button>
					</div>
				) : null}
				<Field id='channel-base_url' label='Base URL' required error={error('base_url')} className='sm:col-span-2'>
					<Input id='channel-base_url' aria-invalid={Boolean(error('base_url')) || undefined} value={ch.base_url} onChange={event => update({ base_url: event.target.value })} placeholder='https://api.openai.com' className='font-mono' />
					{hasTrailingV1(ch.base_url) ? (
						<div className='flex flex-wrap items-center gap-2 rounded-md border border-warning-border bg-warning-soft px-3 py-2 text-xs text-warning-foreground'>
							<span className='min-w-0 flex-1'>{t('providers.editor.baseUrlV1Warning')}</span>
							<Button type='button' size='sm' variant='outline' className='h-8' onClick={() => update({ base_url: removeTrailingV1(ch.base_url) })}>{t('providers.editor.removeV1')}</Button>
						</div>
					) : null}
				</Field>
				<Field id='channel-api_key' label='API Key' required={!ch.id} error={error('api_key')} hint={form.id && ch.id ? t('providers.editor.apiKeyKeepHint') : undefined}>
					<Input id='channel-api_key' type='password' autoComplete='new-password' aria-invalid={Boolean(error('api_key')) || undefined} value={ch.api_key} onChange={event => update({ api_key: event.target.value })} placeholder={form.id && ch.id ? '••••••••••••' : 'sk-…'} className='font-mono' />
				</Field>
				<Field
					id='channel-weight'
					label={t('providers.editor.weight')}
					hint={<>{t('providers.editor.weightHint')}{ch.weight === 0 ? <span className='mt-1 block text-warning-foreground'>{t('providers.editor.weightZeroHint')}</span> : null}</>}
				>
					<NumberField id='channel-weight' min={0} value={ch.weight} onValueChange={value => update({ weight: value ?? 0 })} onValidityChange={trackNumber('channel-weight')} />
				</Field>
			</div>
		</section>

		<ChannelModelEditor
			key={ch.id || `channel-${selectedChannel}`}
			models={ch.models}
			onChange={models => update({ models })}
			onOpenPicker={openPicker}
			pricedModels={pricedModels}
			metadataProvider={metadataProvider}
			reasoningSuffixMap={reasoningSuffixMap}
			error={error('models')}
		/>

		<AdvancedGroup
			icon={GitBranch}
			title={t('providers.editor.affinityTitle')}
			forceOpen={Boolean(error('extra_headers'))}
			summary={overrideSummary([
				[t('providers.editor.affinityEnabled'), ch.affinity_enabled_override],
				[t('providers.editor.recoveryPolicy'), ch.affinity_failback_mode_override ? t(ch.affinity_failback_mode_override === 'sticky' ? 'providers.editor.failbackSticky' : 'providers.editor.failbackPriority') : null],
				[t('providers.editor.idleExpiry'), ch.affinity_idle_ttl_seconds_override],
				[t('providers.editor.failbackDelay'), ch.affinity_failback_delay_seconds_override],
				[t('providers.editor.egressProxy'), ch.proxy_url.trim()],
				[t('providers.editor.autoSessionAffinity'), ch.session_affinity_auto],
				[t('providers.editor.extraHeaders'), headerCount > 0 ? t('providers.editor.headerCount', { count: headerCount }) : null]
			], t)}
		>
			<NullableBoolean id='channel-affinity-enabled' label={t('providers.editor.affinityEnabled')} value={ch.affinity_enabled_override} onChange={value => update({ affinity_enabled_override: value })} />
			<AffinityModeOverride id='channel-affinity-mode' label={t('providers.editor.recoveryPolicy')} value={ch.affinity_failback_mode_override} onChange={value => update({ affinity_failback_mode_override: value })} />
			<NumberOverride id='channel-affinity-ttl' label={t('providers.editor.idleExpiry')} value={ch.affinity_idle_ttl_seconds_override} placeholder={settings?.monoize_affinity_idle_ttl_seconds} onChange={value => update({ affinity_idle_ttl_seconds_override: value })} trackNumber={trackNumber} />
			<NumberOverride id='channel-affinity-delay' min={0} label={t('providers.editor.failbackDelay')} value={ch.affinity_failback_delay_seconds_override} placeholder={settings?.monoize_affinity_failback_delay_seconds} onChange={value => update({ affinity_failback_delay_seconds_override: value })} trackNumber={trackNumber} />
			<Field id='channel-proxy_url' label={t('providers.editor.egressProxy')} hint={t('providers.editor.egressProxyHint')}>
				<Input id='channel-proxy_url' value={ch.proxy_url} placeholder='http://proxy:port' onChange={event => update({ proxy_url: event.target.value })} />
			</Field>
			<NullableBoolean id='channel-session-affinity' label={t('providers.editor.autoSessionAffinity')} hint={t('providers.editor.autoSessionAffinityHint')} nullLabel={t('providers.editor.autoDetectByBaseUrl')} value={ch.session_affinity_auto} onChange={value => update({ session_affinity_auto: value })} />
			<Field id='channel-extra_headers' label={t('providers.editor.extraHeaders')} hint={t('providers.editor.extraHeadersHint')} error={error('extra_headers')} className='sm:col-span-2'>
				<Textarea id='channel-extra_headers' aria-invalid={Boolean(error('extra_headers')) || undefined} value={ch.extra_headers} rows={3} placeholder={'{"x-session-affinity": "ses_001"}'} className='font-mono text-xs' onChange={event => update({ extra_headers: event.target.value })} />
			</Field>
			<p className='text-xs leading-relaxed text-muted-foreground sm:col-span-2'>{t('providers.editor.recoveryPolicyHint')}</p>
		</AdvancedGroup>

		<AdvancedGroup
			icon={CircleGauge}
			title={t('providers.editor.healthTitle')}
			summary={overrideSummary([
				[t('providers.editor.activeProbing'), ch.active_probe_enabled_override],
				[t('providers.editor.probeModel'), ch.active_probe_model_override.trim()],
				[t('providers.editor.probeInterval'), ch.active_probe_interval_seconds_override],
				[t('providers.editor.probeSuccessThreshold'), ch.active_probe_success_threshold_override],
				[t('providers.editor.failureThreshold'), ch.passive_failure_count_threshold_override],
				[t('providers.editor.windowSeconds'), ch.passive_window_seconds_override],
				[t('providers.editor.cooldownSeconds'), ch.passive_cooldown_seconds_override],
				[t('providers.editor.rateLimitCooldownSeconds'), ch.passive_rate_limit_cooldown_seconds_override]
			], t)}
		>
			<NullableBoolean id='channel-probe-enabled' label={t('providers.editor.activeProbing')} value={ch.active_probe_enabled_override} onChange={value => update({ active_probe_enabled_override: value })} />
			<Field id='channel-probe-model' label={t('providers.editor.probeModel')} hint={t('providers.editor.probeModelHint', { model: form.active_probe_model_override.trim() || settings?.monoize_active_probe_model || t('providers.editor.firstChannelModel') })}>
				<Input id='channel-probe-model' value={ch.active_probe_model_override} onChange={event => update({ active_probe_model_override: event.target.value })} />
			</Field>
			<NumberOverride id='channel-probe-interval' label={t('providers.editor.probeInterval')} value={ch.active_probe_interval_seconds_override} placeholder={form.active_probe_interval_seconds_override ?? settings?.monoize_active_probe_interval_seconds} onChange={value => update({ active_probe_interval_seconds_override: value })} trackNumber={trackNumber} />
			<NumberOverride id='channel-probe-threshold' label={t('providers.editor.probeSuccessThreshold')} value={ch.active_probe_success_threshold_override} placeholder={form.active_probe_success_threshold_override ?? settings?.monoize_active_probe_success_threshold} onChange={value => update({ active_probe_success_threshold_override: value })} trackNumber={trackNumber} />
			<NumberOverride id='channel-failure-threshold' label={t('providers.editor.failureThreshold')} value={ch.passive_failure_count_threshold_override} placeholder={settings?.monoize_passive_failure_threshold} onChange={value => update({ passive_failure_count_threshold_override: value })} trackNumber={trackNumber} />
			<NumberOverride id='channel-window' label={t('providers.editor.windowSeconds')} value={ch.passive_window_seconds_override} placeholder={settings?.monoize_passive_window_seconds} onChange={value => update({ passive_window_seconds_override: value })} trackNumber={trackNumber} />
			<NumberOverride id='channel-cooldown' label={t('providers.editor.cooldownSeconds')} value={ch.passive_cooldown_seconds_override} placeholder={settings?.monoize_passive_cooldown_seconds} onChange={value => update({ passive_cooldown_seconds_override: value })} trackNumber={trackNumber} />
			<NumberOverride id='channel-rate-limit-cooldown' label={t('providers.editor.rateLimitCooldownSeconds')} value={ch.passive_rate_limit_cooldown_seconds_override} placeholder={settings?.monoize_passive_rate_limit_cooldown_seconds} onChange={value => update({ passive_rate_limit_cooldown_seconds_override: value })} trackNumber={trackNumber} />
		</AdvancedGroup>
	</div>
}

function AdvancedGroup({ icon: Icon, title, summary, forceOpen, children }: { icon: typeof Server; title: string; summary: string; forceOpen?: boolean; children: React.ReactNode }) {
	return <details className='group rounded-xl border bg-card' open={forceOpen || undefined}>
		<summary className='flex cursor-pointer list-none items-center justify-between gap-3 p-4 sm:p-5'>
			<div className='flex items-center gap-3'>
				<Icon className='size-4 text-muted-foreground' aria-hidden='true' />
				<div><h4 className='font-medium'>{title}</h4><p className='mt-0.5 text-xs text-muted-foreground'>{summary}</p></div>
			</div>
			<ChevronRight className='size-4 transition-transform group-open:rotate-90' aria-hidden='true' />
		</summary>
		<div className='grid gap-4 border-t p-4 sm:grid-cols-2 sm:p-5'>{children}</div>
	</details>
}

function NumberOverride({ id, label, value, placeholder, min = 1, unit, hint, onChange, trackNumber }: { id: string; label: string; value: number | null; placeholder?: number | null; min?: number; unit?: string; hint?: React.ReactNode; onChange: (value: number | null) => void; trackNumber: (id: string) => (valid: boolean) => void }) {
	return <Field id={id} label={label} hint={hint}>
		<NumberField id={id} allowEmpty min={min} unit={unit} value={value} placeholder={placeholder == null ? undefined : String(placeholder)} onValueChange={onChange} onValidityChange={trackNumber(id)} />
	</Field>
}

function NullableBoolean({ id, label, hint, nullLabel, enabledLabel, disabledLabel, value, onChange }: { id: string; label: string; hint?: string; nullLabel?: string; enabledLabel?: string; disabledLabel?: string; value: boolean | null; onChange: (value: boolean | null) => void }) {
	const { t } = useTranslation()
	return <Field id={id} label={label} hint={hint}><Select value={value == null ? 'inherit' : value ? 'enabled' : 'disabled'} onValueChange={next => onChange(next === 'inherit' ? null : next === 'enabled')}><SelectTrigger id={id}><SelectValue /></SelectTrigger><SelectContent><SelectGroup><SelectItem value='inherit'>{nullLabel ?? t('providers.editor.inheritGlobal')}</SelectItem><SelectItem value='enabled'>{enabledLabel ?? t('common.enabled')}</SelectItem><SelectItem value='disabled'>{disabledLabel ?? t('common.disabled')}</SelectItem></SelectGroup></SelectContent></Select></Field>
}

function AffinityModeOverride({ id, label, value, onChange }: { id: string; label: string; value: AffinityFailbackMode | null; onChange: (value: AffinityFailbackMode | null) => void }) {
	const { t } = useTranslation()
	return <Field id={id} label={label}><Select value={value ?? 'inherit'} onValueChange={next => onChange(next === 'inherit' ? null : next as AffinityFailbackMode)}><SelectTrigger id={id}><SelectValue /></SelectTrigger><SelectContent><SelectGroup><SelectItem value='inherit'>{t('providers.editor.inheritGlobal')}</SelectItem><SelectItem value='sticky'>{t('providers.editor.failbackSticky')}</SelectItem><SelectItem value='prefer_higher_priority'>{t('providers.editor.failbackPriority')}</SelectItem></SelectGroup></SelectContent></Select></Field>
}

const RETRY_MODES = ['all', 'count'] as const

function RoutingSettings({ form, setForm, settings, position, trackNumber }: { form: ProviderForm; setForm: SetForm; settings?: SystemSettings; position: number; trackNumber: (id: string) => (valid: boolean) => void }) {
	const { t } = useTranslation()
	const retryMode = form.max_retries < 0 ? 'all' : 'count'
	return <div className='mx-auto flex w-full max-w-4xl flex-col gap-6 p-4 sm:p-6'>
		<SectionHeading title={t('providers.editor.routingTitle')} description={t('providers.editor.routingDescription')} />
		<p className='rounded-lg border bg-muted/30 px-4 py-3 text-sm text-muted-foreground'>
			{position >= 0 ? t('providers.editor.routingPosition', { position: position + 1 }) : t('providers.editor.routingPositionNew')}
		</p>
		<div className='grid gap-4 rounded-xl border bg-card p-4 sm:grid-cols-2 sm:p-5'>
			<Field id='provider-max-retries' label={t('providers.editor.maxRetries')} hint={t('providers.editor.maxRetriesHint')}>
				<div className='flex flex-wrap items-center gap-3'>
					<SegmentedControl
						value={retryMode}
						options={RETRY_MODES}
						ariaLabel={t('providers.editor.maxRetries')}
						label={option => option === 'all' ? t('providers.editor.retryAll') : t('providers.editor.retryCount')}
						onChange={next => setForm(previous => ({ ...previous, max_retries: next === 'all' ? -1 : 2 }))}
					/>
					{retryMode === 'count' ? (
						<div className='w-40'>
							<NumberField
								id='provider-max-retries'
								min={1}
								unit={t('providers.editor.attemptsUnit')}
								aria-label={t('providers.editor.attemptCount')}
								value={form.max_retries + 1}
								onValueChange={value => value != null && setForm(previous => ({ ...previous, max_retries: value - 1 }))}
								onValidityChange={trackNumber('provider-max-retries')}
							/>
						</div>
					) : null}
				</div>
			</Field>
			<Field id='provider-channel-retries' label={t('providers.editor.channelRetries')}>
				<NumberField id='provider-channel-retries' min={0} value={form.channel_max_retries} onValueChange={value => value != null && setForm(previous => ({ ...previous, channel_max_retries: value }))} onValidityChange={trackNumber('provider-channel-retries')} />
			</Field>
			<Field id='provider-retry-interval' label={t('providers.editor.retryInterval')}>
				<NumberField id='provider-retry-interval' min={0} unit='ms' value={form.channel_retry_interval_ms} onValueChange={value => value != null && setForm(previous => ({ ...previous, channel_retry_interval_ms: value }))} onValidityChange={trackNumber('provider-retry-interval')} />
			</Field>
			<NumberOverride
				id='provider-request-timeout'
				label={t('providers.editor.requestTimeout')}
				unit='ms'
				value={form.request_timeout_ms_override}
				placeholder={settings?.monoize_request_timeout_ms}
				onChange={value => setForm(previous => ({ ...previous, request_timeout_ms_override: value }))}
				trackNumber={trackNumber}
				hint={<>
					{t('providers.editor.requestTimeoutHint', { value: settings?.monoize_request_timeout_ms ?? '—' })}
					{' · '}
					<Link to='/dashboard/admin-settings?setting=monoize_request_timeout_ms' className='underline underline-offset-4'>{t('providers.editor.changeGlobal')}</Link>
				</>}
			/>
			<div className='flex items-center justify-between gap-4 rounded-lg border p-4'><div><Label htmlFor='provider-circuit-breaker'>{t('providers.editor.circuitBreaker')}</Label><p className='mt-1 text-xs text-muted-foreground'>{t('providers.editor.circuitBreakerHint')}</p></div><Switch id='provider-circuit-breaker' checked={form.circuit_breaker_enabled} onCheckedChange={value => setForm(previous => ({ ...previous, circuit_breaker_enabled: value }))} /></div>
			<div className='flex items-center justify-between gap-4 rounded-lg border p-4'><div><Label htmlFor='provider-per-model-breaker'>{t('providers.editor.perModelBreaker')}</Label><p className='mt-1 text-xs text-muted-foreground'>{t('providers.editor.perModelBreakerHint')}</p></div><Switch id='provider-per-model-breaker' checked={form.per_model_circuit_break} onCheckedChange={value => setForm(previous => ({ ...previous, per_model_circuit_break: value }))} /></div>
		</div>
		<div className='grid gap-4 rounded-xl border bg-card p-4 sm:grid-cols-2 sm:p-5'>
			<div className='sm:col-span-2'>
				<h4 className='font-medium'>{t('providers.editor.providerHealthTitle')}</h4>
				<p className='mt-1 text-xs text-muted-foreground'>{t('providers.editor.providerHealthHint')}</p>
			</div>
			<NullableBoolean id='provider-probe-enabled' label={t('providers.editor.activeProbing')} value={form.active_probe_enabled_override} onChange={value => setForm(previous => ({ ...previous, active_probe_enabled_override: value }))} />
			<Field id='provider-probe-model' label={t('providers.editor.probeModel')} hint={t('providers.editor.probeModelHint', { model: settings?.monoize_active_probe_model || t('providers.editor.firstChannelModel') })}>
				<Input id='provider-probe-model' value={form.active_probe_model_override} onChange={event => setForm(previous => ({ ...previous, active_probe_model_override: event.target.value }))} />
			</Field>
			<NumberOverride id='provider-probe-interval' label={t('providers.editor.probeInterval')} value={form.active_probe_interval_seconds_override} placeholder={settings?.monoize_active_probe_interval_seconds} onChange={value => setForm(previous => ({ ...previous, active_probe_interval_seconds_override: value }))} trackNumber={trackNumber} />
			<NumberOverride id='provider-probe-threshold' label={t('providers.editor.probeSuccessThreshold')} value={form.active_probe_success_threshold_override} placeholder={settings?.monoize_active_probe_success_threshold} onChange={value => setForm(previous => ({ ...previous, active_probe_success_threshold_override: value }))} trackNumber={trackNumber} />
		</div>
	</div>
}

function ProtocolSettings({ form, setForm }: { form: ProviderForm; setForm: SetForm }) {
	const { t } = useTranslation()
	return <div className='mx-auto flex w-full max-w-4xl flex-col gap-6 p-4 sm:p-6'><SectionHeading title={t('providers.editor.protocolTitle')} description={t('providers.editor.protocolDescription')} />
		<div className='flex flex-col gap-3'>
			{form.api_type_overrides.map((rule, index) => <div key={index} className='grid gap-2 rounded-xl border bg-card p-3 sm:grid-cols-[1fr_220px_40px] sm:items-center'><Input aria-label={t('providers.editor.overridePattern')} value={rule.pattern} onChange={event => setForm(previous => ({ ...previous, api_type_overrides: previous.api_type_overrides.map((item, itemIndex) => itemIndex === index ? { ...item, pattern: event.target.value } : item) }))} placeholder='gpt-*' className='font-mono' /><Select value={rule.api_type} onValueChange={(api_type: ProviderType) => setForm(previous => ({ ...previous, api_type_overrides: previous.api_type_overrides.map((item, itemIndex) => itemIndex === index ? { ...item, api_type } : item) }))}><SelectTrigger aria-label={t('providers.editor.apiType')}><SelectValue /></SelectTrigger><SelectContent><SelectGroup>{providerTypes.map(type => <SelectItem key={type} value={type}>{PROVIDER_TYPE_CONFIG[type].label}</SelectItem>)}</SelectGroup></SelectContent></Select><Button type='button' size='icon' variant='ghost' className='size-11 touch-manipulation sm:size-9' aria-label={t('providers.editor.deleteOverride')} onClick={() => setForm(previous => ({ ...previous, api_type_overrides: previous.api_type_overrides.filter((_, itemIndex) => itemIndex !== index) }))}><Trash2 data-icon /></Button></div>)}
			<Button type='button' variant='outline' className='self-start' onClick={() => setForm(previous => ({ ...previous, api_type_overrides: [...previous.api_type_overrides, { pattern: '', api_type: 'chat_completion' }] }))}><Plus data-icon />{t('providers.editor.addOverride')}</Button>
		</div>
		<Separator />
		<div className='rounded-xl border bg-card p-4 sm:p-5'>
			<NullableBoolean id='provider-strip-nested' label={t('providers.editor.stripNestedExtras')} value={form.strip_cross_protocol_nested_extra} onChange={value => setForm(previous => ({ ...previous, strip_cross_protocol_nested_extra: value }))} />
		</div>
	</div>
}
