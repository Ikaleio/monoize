import { useCallback, useEffect, useMemo, useState } from 'react'
import { Virtuoso } from 'react-virtuoso'
import { useTranslation } from 'react-i18next'
import type { TFunction } from 'i18next'
import {
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
import { findFirstInvalidTransformRule } from '@/components/transforms/transform-schema'
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
import { Input } from '@/components/ui/input'
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
import type {
	CreateProviderInput,
	AffinityFailbackMode,
	FetchChannelModelsInput,
	ModelMetadataRecord,
	ModelPriceRecord,
	Provider,
	ProviderType,
	SystemSettings,
	TransformRegistryItem
} from '@/lib/api'
import { api } from '@/lib/api'
import {
	createProviderOptimistic,
	useDashboardGroups,
	useProviderDetail,
	updateProviderOptimistic
} from '@/lib/swr'
import { GroupMultiSelect } from '@/components/groups/GroupPicker'
import { cn } from '@/lib/utils'
import { normalizeMultiplier } from '@/lib/exact-decimal'
import { ChannelModelEditor } from './ChannelModelEditor'
import { ModelPickerDialog } from './ModelPickerDialog'
import {
	buildPricedModelIdSet,
	emptyChannelRow,
	emptyForm,
	fromProvider,
	hasTrailingV1,
	type ChannelRow,
	type ModelRow,
	type ProviderForm,
	PROVIDER_TYPE_CONFIG,
	removeTrailingV1,
	statusBadge
} from './shared'

type Section = 'provider' | 'channels' | 'routing' | 'transforms' | 'protocol'

const providerTypes = Object.keys(PROVIDER_TYPE_CONFIG) as ProviderType[]

function cloneForm(form: ProviderForm): ProviderForm {
	return {
		...form,
		group_ids: [...form.group_ids],
		channels: form.channels.map(channel => ({
			...channel,
			models: channel.models.map(model => ({ ...model }))
		})),
		transforms: form.transforms.map(rule => ({ ...rule, config: { ...rule.config } })),
		api_type_overrides: form.api_type_overrides.map(rule => ({ ...rule }))
	}
}

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

function optionalPositiveInteger(value: string): number | null {
	return value.trim() ? Number(value) : null
}

function channelInput(channel: ChannelRow, t: TFunction) {
	return {
		id: channel.id || undefined,
		name: channel.name.trim(),
		provider_type: channel.provider_type,
		base_url: channel.base_url.trim(),
		api_key: channel.api_key.trim() || undefined,
		weight: Number(channel.weight),
		enabled: channel.enabled,
		models: modelMap(channel.models),
		passive_failure_count_threshold_override: optionalPositiveInteger(channel.passive_failure_count_threshold_override),
		passive_cooldown_seconds_override: optionalPositiveInteger(channel.passive_cooldown_seconds_override),
		passive_window_seconds_override: optionalPositiveInteger(channel.passive_window_seconds_override),
		passive_rate_limit_cooldown_seconds_override: optionalPositiveInteger(channel.passive_rate_limit_cooldown_seconds_override),
		active_probe_enabled_override: channel.active_probe_enabled_override,
		active_probe_interval_seconds_override: optionalPositiveInteger(channel.active_probe_interval_seconds_override),
		active_probe_success_threshold_override: optionalPositiveInteger(channel.active_probe_success_threshold_override),
		active_probe_model_override: channel.active_probe_model_override.trim() || null,
		affinity_enabled_override: channel.affinity_enabled_override,
		affinity_idle_ttl_seconds_override: optionalPositiveInteger(channel.affinity_idle_ttl_seconds_override),
		affinity_failback_mode_override: channel.affinity_failback_mode_override,
		affinity_failback_delay_seconds_override: optionalPositiveInteger(channel.affinity_failback_delay_seconds_override),
		proxy_url: channel.proxy_url.trim() || null,
		extra_headers: parseExtraHeaders(channel.extra_headers, t),
		session_affinity_auto: channel.session_affinity_auto,
		websocket_supported: channel.websocket_supported
	}
}

function parseExtraHeaders(raw: string, t: TFunction): Record<string, string> | null {
	const text = raw.trim()
	if (!text) return null
	let parsed: unknown
	try {
		parsed = JSON.parse(text)
	} catch {
		throw new Error(t('providers.editor.extraHeadersInvalidJson'))
	}
	if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
		throw new Error(t('providers.editor.extraHeadersNotObject'))
	}
	const out: Record<string, string> = {}
	for (const [key, value] of Object.entries(parsed as Record<string, unknown>)) {
		out[key.trim()] = String(value)
	}
	return Object.keys(out).length > 0 ? out : null
}

function buildInput(form: ProviderForm, t: TFunction): CreateProviderInput {
	return {
		name: form.name.trim(),
		enabled: form.enabled,
		priority: form.priority,
		max_retries: form.max_retries,
		channel_max_retries: form.channel_max_retries,
		channel_retry_interval_ms: form.channel_retry_interval_ms,
		circuit_breaker_enabled: form.circuit_breaker_enabled,
		per_model_circuit_break: form.per_model_circuit_break,
		channels: form.channels.map(channel => channelInput(channel, t)),
		transforms: form.transforms,
		api_type_overrides: form.api_type_overrides,
		active_probe_enabled_override: form.active_probe_enabled_override,
		active_probe_interval_seconds_override: form.active_probe_interval_seconds_override,
		active_probe_success_threshold_override: form.active_probe_success_threshold_override,
		active_probe_model_override: form.active_probe_model_override,
		request_timeout_ms_override: optionalPositiveInteger(form.request_timeout_ms_override),
		extra_fields_whitelist: form.extra_fields_whitelist
			.split(',')
			.map(value => value.trim())
			.filter(Boolean),
		strip_cross_protocol_nested_extra: form.strip_cross_protocol_nested_extra,
		allow_free_when_unpriced_override: form.allow_free_when_unpriced_override,
		allow_free_when_missing_usage_override: form.allow_free_when_missing_usage_override,
		group_ids: form.group_ids
	}
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
	const { t } = useTranslation()
	const isEdit = mode === 'edit'
	const [form, setFormState] = useState<ProviderForm>(() => current ? fromProvider(current) : emptyForm())
	const [dirty, setDirty] = useState(false)
	const setForm = useCallback<React.Dispatch<React.SetStateAction<ProviderForm>>>(updater => {
		setDirty(true)
		setFormState(updater)
	}, [])
	const [section, setSection] = useState<Section>('channels')
	const [selectedChannel, setSelectedChannel] = useState(0)
	const [mobileChannelOpen, setMobileChannelOpen] = useState(false)
	const [saving, setSaving] = useState(false)
	const [pickerOpen, setPickerOpen] = useState(false)
	const [closeConfirmOpen, setCloseConfirmOpen] = useState(false)
	const [removeV1Open, setRemoveV1Open] = useState(false)
	const [v1ChannelIndex, setV1ChannelIndex] = useState<number | null>(null)
	const { data: detail, error: detailError, isLoading: detailLoading } = useProviderDetail(
		open && isEdit && current ? current.id : null,
		{ revalidateOnFocus: false }
	)

	useEffect(() => {
		if (!open) return
		const next = isEdit ? (detail ?? (detailError ? current : null)) : null
		if (isEdit && !next) return
		const hydrated = next ? fromProvider(next) : emptyForm()
		setFormState(cloneForm(hydrated))
		setDirty(false)
		setSelectedChannel(0)
		setSection('channels')
		setMobileChannelOpen(false)
	}, [open, isEdit, detail, detailError, current])

	const activeChannel = form.channels[selectedChannel]
	const pricedModels = useMemo(() => buildPricedModelIdSet(modelPrices), [modelPrices])
	const metadataProvider = useMemo(
		() => new Map(modelMetadata.map(item => [item.model_id, item.models_dev_provider])),
		[modelMetadata]
	)

	const updateChannel = (index: number, patch: Partial<ChannelRow>) => {
		setForm(previous => ({
			...previous,
			channels: previous.channels.map((channel, channelIndex) =>
				channelIndex === index ? { ...channel, ...patch } : channel
			)
		}))
	}

	const validate = () => {
		if (!form.name.trim()) return t('providers.editor.nameRequired')
		if (!form.channels.length) return t('providers.editor.channelRequired')
		if (!form.channels.some(channel => channel.models.length > 0)) {
			return t('providers.editor.modelsRequired')
		}
		for (const [index, channel] of form.channels.entries()) {
			if (!channel.name.trim() || !channel.base_url.trim()) {
				return t('providers.editor.channelNameUrlRequired', { index: index + 1 })
			}
			if (!isEdit && !channel.api_key.trim()) {
				return t('providers.editor.channelKeyRequired', { index: index + 1 })
			}
			const names = channel.models.map(model => model.model.trim())
			if (names.some(name => !name) || new Set(names).size !== names.length) {
				return t('providers.editor.channelModelsInvalid', { index: index + 1 })
			}
			if (channel.models.some(model => normalizeMultiplier(model.multiplier) == null)) {
				return t('providers.editor.channelMultiplierInvalid', { index: index + 1 })
			}
		}
		const invalidTransform = findFirstInvalidTransformRule(form.transforms, transformRegistry)
		if (invalidTransform) return invalidTransform.errors[0]?.message ?? t('providers.editor.transformInvalid')
		return null
	}

	const save = async () => {
		const invalid = validate()
		if (invalid) {
			toast.error(invalid)
			return
		}
		setSaving(true)
		try {
			const input = buildInput(form, t)
			if (isEdit && current) {
				await updateProviderOptimistic(current.id, input, providers)
			} else {
				await createProviderOptimistic(input, providers)
			}
			toast.success(t('providers.editor.saveSuccess'))
			setDirty(false)
			onOpenChange(false)
		} catch (error) {
			toast.error(error instanceof Error ? error.message : t('providers.editor.saveFailed'))
		} finally {
			setSaving(false)
		}
	}

	const requestClose = () => {
		if (dirty) setCloseConfirmOpen(true)
		else onOpenChange(false)
	}

	const addChannel = () => {
		setForm(previous => ({ ...previous, channels: [...previous.channels, emptyChannelRow()] }))
		setSelectedChannel(form.channels.length)
		setSection('channels')
		setMobileChannelOpen(true)
	}

	const duplicateChannel = () => {
		if (!activeChannel) return
		const duplicate = {
			...activeChannel,
			id: '',
			name: t('providers.editor.duplicateName', { name: activeChannel.name }),
			api_key: '',
			models: activeChannel.models.map(model => ({ ...model }))
		}
		setForm(previous => ({ ...previous, channels: [...previous.channels, duplicate] }))
		setSelectedChannel(form.channels.length)
	}

	const removeChannel = () => {
		if (!activeChannel || form.channels.length === 1) return
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

	const sections: Array<{ id: Section; icon: typeof Server; label: string; summary: string }> = [
		{ id: 'provider', icon: Server, label: 'Provider', summary: form.name || t('providers.editor.untitled') },
		{ id: 'channels', icon: Layers3, label: 'Channels', summary: `${form.channels.length}` },
		{ id: 'routing', icon: GitBranch, label: t('providers.editor.sectionRouting'), summary: form.max_retries === -1 ? t('providers.editor.attemptsUnlimited') : t('providers.editor.attempts', { count: form.max_retries + 1 }) },
		{ id: 'transforms', icon: Braces, label: t('providers.editor.sectionTransforms'), summary: `${form.transforms.length}` },
		{ id: 'protocol', icon: Settings2, label: t('providers.editor.sectionProtocol'), summary: `${form.api_type_overrides.length}` }
	]

	return (
		<>
			<Dialog open={open} onOpenChange={next => { if (!next) requestClose() }}>
				<DialogContent
					className='flex h-[calc(100dvh-2rem)] w-screen max-w-none flex-col gap-0 overflow-hidden rounded-none border-0 p-0 sm:h-[94vh] sm:w-[96vw] sm:max-w-[1500px] sm:rounded-xl sm:border [&>button:last-child]:right-2 [&>button:last-child]:top-2 [&>button:last-child]:flex [&>button:last-child]:size-10 [&>button:last-child]:items-center [&>button:last-child]:justify-center [&>div:first-child]:h-full [&>div:first-child]:min-h-0 [&>div:first-child]:flex-1 [&>div:first-child]:gap-0 [&>div:first-child]:overflow-hidden'
					onPointerDownOutside={event => event.preventDefault()}
				>
					<DialogHeader className='shrink-0 border-b bg-background py-3 pl-4 pr-16 text-left sm:pl-6 sm:pr-16'>
						<div className='flex min-w-0 items-center justify-between gap-3'>
							<div className='min-w-0'>
								<DialogTitle className='truncate text-base sm:text-lg'>
									{isEdit ? t('providers.editor.editTitle') : t('providers.editor.createTitle')}
									{form.name ? <span className='font-normal text-muted-foreground'> · {form.name}</span> : null}
								</DialogTitle>
								<DialogDescription className='mt-0.5 hidden sm:block'>
									{t('providers.editor.description')}
								</DialogDescription>
							</div>
							<div className='flex shrink-0 items-center gap-2'>
								<Label htmlFor='provider-enabled' className='hidden text-xs text-muted-foreground sm:block'>{form.enabled ? t('common.enabled') : t('common.disabled')}</Label>
								<Switch id='provider-enabled' checked={form.enabled} onCheckedChange={enabled => setForm(previous => ({ ...previous, enabled }))} />
							</div>
						</div>
					</DialogHeader>

					{isEdit && detailLoading && !detail ? (
						<div className='grid flex-1 grid-cols-1 gap-4 overflow-hidden p-4 lg:grid-cols-[220px_1fr]'>
							<Skeleton className='hidden h-full lg:block' />
							<div className='flex flex-col gap-3'><Skeleton className='h-16' /><Skeleton className='h-80' /><Skeleton className='h-20' /></div>
						</div>
					) : (
						<div className='flex min-h-0 flex-1'>
							<nav className='hidden w-56 shrink-0 flex-col gap-1 border-r bg-muted/20 p-3 lg:flex' aria-label={t('providers.editor.sectionsLabel')}>
								{sections.map(item => {
									const Icon = item.icon
									return <button key={item.id} type='button' onClick={() => setSection(item.id)} className={cn('flex min-h-14 items-center gap-3 rounded-lg px-3 text-left transition-colors', section === item.id ? 'bg-primary/10 text-primary' : 'text-muted-foreground hover:bg-muted hover:text-foreground')}>
										<Icon className='size-4 shrink-0' />
										<span className='min-w-0 flex-1'><span className='block text-sm font-medium'>{item.label}</span><span className='block truncate text-xs opacity-70'>{item.summary}</span></span>
										<ChevronRight className='size-4 shrink-0 opacity-50' />
									</button>
								})}
							</nav>

							<div className='flex min-w-0 flex-1 flex-col'>
								<div className='flex shrink-0 gap-1 overflow-x-auto border-b bg-background px-3 py-2 lg:hidden'>
									{sections.map(item => <Button key={item.id} size='sm' variant={section === item.id ? 'secondary' : 'ghost'} onClick={() => { setSection(item.id); setMobileChannelOpen(false) }} className='shrink-0'>{item.label}</Button>)}
								</div>

								<div className={cn('min-h-0 flex-1', section === 'channels' ? 'flex flex-col overflow-hidden' : 'overflow-y-auto')}>
									{section === 'channels' ? (
										<ChannelsWorkbench
											form={form}
											activeChannel={activeChannel}
											selectedChannel={selectedChannel}
											mobileChannelOpen={mobileChannelOpen}
											setMobileChannelOpen={setMobileChannelOpen}
											setSelectedChannel={setSelectedChannel}
											updateChannel={updateChannel}
											setForm={setForm}
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
											onBaseUrlBlur={() => {
												if (activeChannel && hasTrailingV1(activeChannel.base_url)) {
													setV1ChannelIndex(selectedChannel)
													setRemoveV1Open(true)
												}
											}}
										/>
									) : section === 'provider' ? (
										<ProviderBasics form={form} setForm={setForm} />
									) : section === 'routing' ? (
										<RoutingSettings form={form} setForm={setForm} settings={settings} />
									) : section === 'transforms' ? (
										<div className='mx-auto flex w-full max-w-4xl flex-col gap-5 p-4 sm:p-6'><SectionHeading title={t('providers.editor.transformsTitle')} description={t('providers.editor.transformsDescription')} /><TransformChainEditor value={form.transforms} registry={transformRegistry} loading={transformRegistryLoading} onChange={transforms => setForm(previous => ({ ...previous, transforms }))} /></div>
									) : (
										<ProtocolSettings form={form} setForm={setForm} />
									)}
								</div>
							</div>
						</div>
					)}

					<DialogFooter className='shrink-0 flex-row items-center justify-between gap-2 border-t bg-background px-4 py-3 sm:px-6'>
						<p className='hidden text-xs text-muted-foreground sm:block'>{dirty ? t('providers.editor.unsavedChanges') : t('providers.editor.noUnsavedChanges')}</p>
						<div className='ml-auto flex items-center gap-2'>
							<Button variant='outline' onClick={requestClose}>{t('common.cancel')}</Button>
							<Button onClick={() => void save()} disabled={saving}><Save data-icon />{saving ? t('common.saving') : t('providers.editor.save')}</Button>
						</div>
					</DialogFooter>
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
				<AlertDialogContent><AlertDialogHeader><AlertDialogTitle>{t('providers.editor.discardTitle')}</AlertDialogTitle><AlertDialogDescription>{t('providers.editor.discardDescription')}</AlertDialogDescription></AlertDialogHeader><AlertDialogFooter><AlertDialogCancel>{t('providers.editor.keepEditing')}</AlertDialogCancel><AlertDialogAction className='bg-destructive text-destructive-foreground hover:bg-destructive/90' onClick={() => onOpenChange(false)}>{t('providers.editor.discard')}</AlertDialogAction></AlertDialogFooter></AlertDialogContent>
			</AlertDialog>

			<AlertDialog open={removeV1Open} onOpenChange={setRemoveV1Open}>
				<AlertDialogContent><AlertDialogHeader><AlertDialogTitle>{t('providers.editor.baseUrlV1Title')}</AlertDialogTitle><AlertDialogDescription>{t('providers.editor.baseUrlV1Description')}</AlertDialogDescription></AlertDialogHeader><AlertDialogFooter><AlertDialogCancel>{t('providers.editor.keepV1')}</AlertDialogCancel><AlertDialogAction onClick={() => { if (v1ChannelIndex != null) updateChannel(v1ChannelIndex, { base_url: removeTrailingV1(form.channels[v1ChannelIndex]?.base_url ?? '') }) }}>{t('providers.editor.removeV1')}</AlertDialogAction></AlertDialogFooter></AlertDialogContent>
			</AlertDialog>
		</>
	)
}

function SectionHeading({ title, description }: { title: string; description: string }) {
	return <div><h3 className='text-lg font-semibold'>{title}</h3><p className='mt-1 text-sm text-muted-foreground'>{description}</p></div>
}

function Field({ label, hint, children, className }: { label: string; hint?: string; children: React.ReactNode; className?: string }) {
	return <div className={cn('flex flex-col gap-2', className)}><Label>{label}</Label>{children}{hint ? <p className='text-xs text-muted-foreground'>{hint}</p> : null}</div>
}

function ProviderBasics({ form, setForm }: { form: ProviderForm; setForm: React.Dispatch<React.SetStateAction<ProviderForm>> }) {
	const { t } = useTranslation()
	const { data: groups = [], isLoading: groupsLoading } = useDashboardGroups()
	return <div className='mx-auto flex w-full max-w-3xl flex-col gap-6 p-4 sm:p-6'>
		<SectionHeading title={t('providers.editor.basicsTitle')} description={t('providers.editor.basicsDescription')} />
		<div className='grid gap-5 rounded-xl border bg-card p-4 sm:grid-cols-2 sm:p-5'>
			<Field label={t('common.name')} className='sm:col-span-2'><Input value={form.name} onChange={event => setForm(previous => ({ ...previous, name: event.target.value }))} placeholder='OpenAI production' /></Field>
			<Field label={t('providers.editor.groups')} hint={t('providers.editor.groupsHint')} className='sm:col-span-2'>
				<GroupMultiSelect
					value={form.group_ids}
					groups={groups}
					loading={groupsLoading}
					onChange={group_ids => setForm(previous => ({ ...previous, group_ids }))}
				/>
			</Field>
			<Field label={t('providers.editor.extraFields')} hint={t('providers.editor.extraFieldsHint')}><Input value={form.extra_fields_whitelist} onChange={event => setForm(previous => ({ ...previous, extra_fields_whitelist: event.target.value }))} placeholder='service_tier, metadata' /></Field>
		</div>
		<div className='grid gap-5 rounded-xl border bg-card p-4 sm:grid-cols-2 sm:p-5'>
			<div className='sm:col-span-2'>
				<h4 className='font-medium'>{t('providers.editor.freeSettlement')}</h4>
				<p className='mt-1 text-xs text-muted-foreground'>{t('providers.editor.freeSettlementHint')}</p>
			</div>
			<NullableBoolean
				label={t('providers.editor.freeWhenUnpriced')}
				value={form.allow_free_when_unpriced_override}
				onChange={value => setForm(previous => ({ ...previous, allow_free_when_unpriced_override: value }))}
			/>
			<NullableBoolean
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
	setSelectedChannel: (index: number) => void
	updateChannel: (index: number, patch: Partial<ChannelRow>) => void
	setForm: React.Dispatch<React.SetStateAction<ProviderForm>>
	addChannel: () => void
	duplicateChannel: () => void
	removeChannel: () => void
	openPicker: () => void
	pricedModels: Set<string>
	metadataProvider: Map<string, string | undefined>
	reasoningSuffixMap: Record<string, string>
	settings?: SystemSettings
	onBaseUrlBlur: () => void
}

function ChannelsWorkbench(props: WorkbenchProps) {
	const { form, activeChannel, selectedChannel, mobileChannelOpen, setMobileChannelOpen, setSelectedChannel, addChannel } = props
	const { t } = useTranslation()
	return <div className='grid h-full min-h-0 flex-1 lg:grid-cols-[300px_minmax(0,1fr)]'>
		<div className={cn('flex h-full min-h-0 min-w-0 flex-col overflow-hidden border-r bg-muted/10', mobileChannelOpen ? 'hidden lg:flex' : 'flex')}>
			<div className='flex shrink-0 items-center justify-between border-b px-4 py-3'><div><h3 className='font-semibold'>Channels</h3><p className='text-xs text-muted-foreground'>{t('providers.editor.channelsSubtitle')}</p></div><Button size='icon' variant='outline' className='size-11 touch-manipulation sm:size-9' onClick={addChannel} aria-label={t('providers.editor.addChannel')}><Plus data-icon /></Button></div>
			<div className='relative min-h-0 flex-1'>
				<Virtuoso
					className='absolute inset-0 pt-2'
					style={{ height: '100%' }}
					data={form.channels}
					itemContent={(index, channel) => (
						<div className='px-2 pb-1'>
							<button type='button' onClick={() => { setSelectedChannel(index); setMobileChannelOpen(true) }} className={cn('flex min-h-16 w-full items-center gap-3 rounded-lg border-l-2 px-3 py-2 text-left transition-colors', selectedChannel === index ? 'border-l-primary bg-primary/10' : 'border-l-transparent hover:bg-muted')}>
								<div className='min-w-0 flex-1'><div className='flex items-center gap-2'><span className='truncate text-sm font-medium'>{channel.name || t('providers.editor.untitledChannel')}</span>{!channel.enabled ? <Badge variant='secondary'>{t('common.disabled')}</Badge> : null}</div><p className='mt-1 truncate font-mono text-xs text-muted-foreground'>{channel.base_url || t('providers.editor.noBaseUrl')}</p><p className='mt-1 text-xs text-muted-foreground'>{PROVIDER_TYPE_CONFIG[channel.provider_type]?.label ?? channel.provider_type} · {t('providers.editor.modelCount', { count: channel.models.length })}</p></div>
								<ChevronRight className='size-4 shrink-0 text-muted-foreground' />
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

function ChannelDetail({ form, activeChannel, selectedChannel, setMobileChannelOpen, updateChannel, duplicateChannel, removeChannel, openPicker, pricedModels, metadataProvider, reasoningSuffixMap, settings, onBaseUrlBlur }: WorkbenchProps) {
	const { t } = useTranslation()
	const [probingWebsocket, setProbingWebsocket] = useState(false)
	if (!activeChannel) return null
	return <div className='mx-auto flex w-full max-w-5xl flex-col gap-6 p-4 pb-8 sm:p-6'>
		<div className='flex items-start justify-between gap-3'>
			<div className='flex min-w-0 items-start gap-2'><Button size='icon' variant='ghost' className='-ml-2 size-11 touch-manipulation sm:size-9 lg:hidden' onClick={() => setMobileChannelOpen(false)} aria-label={t('providers.editor.backToChannels')}><ArrowLeft data-icon /></Button><div className='min-w-0'><h3 className='truncate text-lg font-semibold'>{activeChannel.name || t('providers.editor.untitledChannel')}</h3><div className='mt-1'>{activeChannel._health_status ? statusBadge(activeChannel._health_status, t) : <Badge variant='secondary'>{t('providers.editor.unsaved')}</Badge>}</div></div></div>
			<div className='flex items-center gap-1'><Button size='icon' variant='ghost' className='size-11 touch-manipulation sm:size-9' onClick={duplicateChannel} aria-label={t('providers.editor.duplicateChannel')}><Copy data-icon /></Button><Button size='icon' variant='ghost' className='size-11 touch-manipulation sm:size-9' disabled={form.channels.length === 1} onClick={removeChannel} aria-label={t('providers.editor.deleteChannel')}><Trash2 data-icon /></Button><Switch checked={activeChannel.enabled} onCheckedChange={enabled => updateChannel(selectedChannel, { enabled })} /></div>
		</div>

		<section className='flex flex-col gap-4 rounded-xl border bg-card p-4 sm:p-5'>
			<div className='flex items-center gap-2'><Server className='size-4 text-primary' /><h4 className='font-medium'>{t('providers.editor.connection')}</h4></div>
			<div className='grid gap-4 sm:grid-cols-2'>
				<Field label={t('providers.editor.channelName')}><Input value={activeChannel.name} onChange={event => updateChannel(selectedChannel, { name: event.target.value })} /></Field>
				<Field label={t('providers.editor.apiType')}><Select value={activeChannel.provider_type} onValueChange={(provider_type: ProviderType) => updateChannel(selectedChannel, { provider_type })}><SelectTrigger><SelectValue /></SelectTrigger><SelectContent><SelectGroup>{providerTypes.map(type => <SelectItem key={type} value={type}>{PROVIDER_TYPE_CONFIG[type].label}</SelectItem>)}</SelectGroup></SelectContent></Select></Field>
				{activeChannel.provider_type === 'responses' ? (
					<div className='grid gap-2 sm:col-span-2 sm:grid-cols-[1fr_auto] sm:items-end'>
						<NullableBoolean
							label={t('providers.websocketSupported')}
							hint={t('providers.websocketSupportedHint')}
							nullLabel={t('providers.websocketUnknown')}
							value={activeChannel.websocket_supported}
							onChange={value => updateChannel(selectedChannel, { websocket_supported: value })}
							enabledLabel={t('providers.websocketSupportedYes')}
							disabledLabel={t('providers.websocketSupportedNo')}
						/>
						<Button
							type='button'
							variant='outline'
							disabled={
								probingWebsocket
								|| !activeChannel.base_url.trim()
								|| (!activeChannel.api_key.trim() && !activeChannel.id)
							}
							onClick={async () => {
								setProbingWebsocket(true)
								try {
									const result = await api.probeChannelWebsocket({
										provider_type: activeChannel.provider_type,
										base_url: activeChannel.base_url.trim(),
										api_key: activeChannel.api_key.trim() || undefined,
										provider_id: form.id || undefined,
										channel_id: activeChannel.id || undefined,
										model: activeChannel.active_probe_model_override.trim()
											|| activeChannel.models.find(model => model.model.trim())?.model.trim()
									})
									updateChannel(selectedChannel, { websocket_supported: result.supported })
									if (result.supported) {
										toast.success(t('providers.detectWebsocketSuccess'))
									} else {
										toast.error(t('providers.detectWebsocketFailure'))
									}
								} catch (error) {
									updateChannel(selectedChannel, { websocket_supported: false })
									toast.error(error instanceof Error ? error.message : t('providers.detectWebsocketFailure'))
								} finally {
									setProbingWebsocket(false)
								}
							}}
						>
							{probingWebsocket ? t('providers.detectWebsocketRunning') : t('providers.detectWebsocket')}
						</Button>
					</div>
				) : null}
				<Field label='Base URL' className='sm:col-span-2'><Input value={activeChannel.base_url} onChange={event => updateChannel(selectedChannel, { base_url: event.target.value })} onBlur={onBaseUrlBlur} placeholder='https://api.openai.com' className='font-mono' /></Field>
				<Field label='API Key' hint={form.id && activeChannel.id ? t('providers.editor.apiKeyKeepHint') : undefined}><Input type='password' autoComplete='new-password' value={activeChannel.api_key} onChange={event => updateChannel(selectedChannel, { api_key: event.target.value })} placeholder={form.id && activeChannel.id ? '••••••••••••' : 'sk-…'} className='font-mono' /></Field>
				<Field label={t('providers.editor.weight')}><Input type='number' min='0' value={activeChannel.weight} onChange={event => updateChannel(selectedChannel, { weight: event.target.value })} /></Field>
			</div>
		</section>

		<ChannelModelEditor
			key={activeChannel.id || `channel-${selectedChannel}`}
			models={activeChannel.models}
			onChange={models => updateChannel(selectedChannel, { models })}
			onOpenPicker={openPicker}
			pricedModels={pricedModels}
			metadataProvider={metadataProvider}
			reasoningSuffixMap={reasoningSuffixMap}
		/>

		<details className='group rounded-xl border bg-card'>
			<summary className='flex cursor-pointer list-none items-center justify-between gap-3 p-4 sm:p-5'><div className='flex items-center gap-3'><GitBranch className='size-4 text-muted-foreground' /><div><h4 className='font-medium'>{t('providers.editor.affinityTitle')}</h4><p className='mt-0.5 text-xs text-muted-foreground'>{t('providers.editor.affinityDescription')}</p></div></div><ChevronRight className='size-4 transition-transform group-open:rotate-90' /></summary>
			<div className='grid gap-4 border-t p-4 sm:grid-cols-2 sm:p-5'>
				<NullableBoolean label={t('providers.editor.affinityEnabled')} value={activeChannel.affinity_enabled_override} onChange={value => updateChannel(selectedChannel, { affinity_enabled_override: value })} />
				<AffinityModeOverride label={t('providers.editor.recoveryPolicy')} value={activeChannel.affinity_failback_mode_override} onChange={value => updateChannel(selectedChannel, { affinity_failback_mode_override: value })} />
				<NumberOverride label={t('providers.editor.idleExpiry')} value={activeChannel.affinity_idle_ttl_seconds_override} placeholder={settings?.monoize_affinity_idle_ttl_seconds} onChange={value => updateChannel(selectedChannel, { affinity_idle_ttl_seconds_override: value })} />
				<NumberOverride min={0} label={t('providers.editor.failbackDelay')} value={activeChannel.affinity_failback_delay_seconds_override} placeholder={settings?.monoize_affinity_failback_delay_seconds} onChange={value => updateChannel(selectedChannel, { affinity_failback_delay_seconds_override: value })} />
				<Field label={t('providers.editor.egressProxy')} hint={t('providers.editor.egressProxyHint')}>
					<Input value={activeChannel.proxy_url} placeholder='http://proxy:port' onChange={event => updateChannel(selectedChannel, { proxy_url: event.target.value })} />
				</Field>
				<NullableBoolean label={t('providers.editor.autoSessionAffinity')} hint={t('providers.editor.autoSessionAffinityHint')} nullLabel={t('providers.editor.autoDetectByBaseUrl')} value={activeChannel.session_affinity_auto} onChange={value => updateChannel(selectedChannel, { session_affinity_auto: value })} />
				<Field label={t('providers.editor.extraHeaders')} hint={t('providers.editor.extraHeadersHint')} className='sm:col-span-2'>
					<Textarea value={activeChannel.extra_headers} rows={3} placeholder={'{"x-session-affinity": "ses_001"}'} className='font-mono text-xs' onChange={event => updateChannel(selectedChannel, { extra_headers: event.target.value })} />
				</Field>
				<p className='text-xs leading-relaxed text-muted-foreground sm:col-span-2'>
					{t('providers.editor.recoveryPolicyHint')}
				</p>
			</div>
		</details>

		<details className='group rounded-xl border bg-card'>
			<summary className='flex cursor-pointer list-none items-center justify-between gap-3 p-4 sm:p-5'><div className='flex items-center gap-3'><CircleGauge className='size-4 text-muted-foreground' /><div><h4 className='font-medium'>{t('providers.editor.healthTitle')}</h4><p className='mt-0.5 text-xs text-muted-foreground'>{t('providers.editor.affinityDescription')}</p></div></div><ChevronRight className='size-4 transition-transform group-open:rotate-90' /></summary>
			<div className='grid gap-4 border-t p-4 sm:grid-cols-2 sm:p-5'>
				<NullableBoolean label={t('providers.editor.activeProbing')} value={activeChannel.active_probe_enabled_override} onChange={value => updateChannel(selectedChannel, { active_probe_enabled_override: value })} />
				<Field label={t('providers.editor.probeModel')} hint={t('providers.editor.probeModelHint', { model: settings?.monoize_active_probe_model || t('providers.editor.firstChannelModel') })}><Input value={activeChannel.active_probe_model_override} onChange={event => updateChannel(selectedChannel, { active_probe_model_override: event.target.value })} /></Field>
				<NumberOverride label={t('providers.editor.failureThreshold')} value={activeChannel.passive_failure_count_threshold_override} placeholder={settings?.monoize_passive_failure_threshold} onChange={value => updateChannel(selectedChannel, { passive_failure_count_threshold_override: value })} />
				<NumberOverride label={t('providers.editor.windowSeconds')} value={activeChannel.passive_window_seconds_override} placeholder={settings?.monoize_passive_window_seconds} onChange={value => updateChannel(selectedChannel, { passive_window_seconds_override: value })} />
				<NumberOverride label={t('providers.editor.cooldownSeconds')} value={activeChannel.passive_cooldown_seconds_override} placeholder={settings?.monoize_passive_cooldown_seconds} onChange={value => updateChannel(selectedChannel, { passive_cooldown_seconds_override: value })} />
				<NumberOverride label={t('providers.editor.rateLimitCooldownSeconds')} value={activeChannel.passive_rate_limit_cooldown_seconds_override} placeholder={settings?.monoize_passive_rate_limit_cooldown_seconds} onChange={value => updateChannel(selectedChannel, { passive_rate_limit_cooldown_seconds_override: value })} />
			</div>
		</details>
	</div>
}

function NumberOverride({ label, value, placeholder, min = 1, onChange }: { label: string; value: string; placeholder?: number; min?: number; onChange: (value: string) => void }) {
	return <Field label={label}><Input type='number' min={min} value={value} placeholder={placeholder == null ? undefined : String(placeholder)} onChange={event => onChange(event.target.value)} /></Field>
}

function NullableBoolean({ label, hint, nullLabel, enabledLabel, disabledLabel, value, onChange }: { label: string; hint?: string; nullLabel?: string; enabledLabel?: string; disabledLabel?: string; value: boolean | null; onChange: (value: boolean | null) => void }) {
	const { t } = useTranslation()
	return <Field label={label} hint={hint}><Select value={value == null ? 'inherit' : value ? 'enabled' : 'disabled'} onValueChange={next => onChange(next === 'inherit' ? null : next === 'enabled')}><SelectTrigger><SelectValue /></SelectTrigger><SelectContent><SelectGroup><SelectItem value='inherit'>{nullLabel ?? t('providers.editor.inheritGlobal')}</SelectItem><SelectItem value='enabled'>{enabledLabel ?? t('common.enabled')}</SelectItem><SelectItem value='disabled'>{disabledLabel ?? t('common.disabled')}</SelectItem></SelectGroup></SelectContent></Select></Field>
}

function AffinityModeOverride({ label, value, onChange }: { label: string; value: AffinityFailbackMode | null; onChange: (value: AffinityFailbackMode | null) => void }) {
	const { t } = useTranslation()
	return <Field label={label}><Select value={value ?? 'inherit'} onValueChange={next => onChange(next === 'inherit' ? null : next as AffinityFailbackMode)}><SelectTrigger><SelectValue /></SelectTrigger><SelectContent><SelectGroup><SelectItem value='inherit'>{t('providers.editor.inheritGlobal')}</SelectItem><SelectItem value='sticky'>{t('providers.editor.failbackSticky')}</SelectItem><SelectItem value='prefer_higher_priority'>{t('providers.editor.failbackPriority')}</SelectItem></SelectGroup></SelectContent></Select></Field>
}

function RoutingSettings({ form, setForm, settings }: { form: ProviderForm; setForm: React.Dispatch<React.SetStateAction<ProviderForm>>; settings?: SystemSettings }) {
	const { t } = useTranslation()
	return <div className='mx-auto flex w-full max-w-4xl flex-col gap-6 p-4 sm:p-6'><SectionHeading title={t('providers.editor.routingTitle')} description={t('providers.editor.routingDescription')} />
		<div className='grid gap-4 rounded-xl border bg-card p-4 sm:grid-cols-2 sm:p-5'>
			<Field label={t('providers.editor.maxRetries')} hint={t('providers.editor.maxRetriesHint')}><Input type='number' min='-1' value={form.max_retries} onChange={event => setForm(previous => ({ ...previous, max_retries: Number(event.target.value) }))} /></Field>
			<Field label={t('providers.editor.channelRetries')}><Input type='number' min='0' value={form.channel_max_retries} onChange={event => setForm(previous => ({ ...previous, channel_max_retries: Number(event.target.value) }))} /></Field>
			<Field label={t('providers.editor.retryInterval')}><Input type='number' min='0' value={form.channel_retry_interval_ms} onChange={event => setForm(previous => ({ ...previous, channel_retry_interval_ms: Number(event.target.value) }))} /></Field>
			<Field label={t('providers.editor.requestTimeout')} hint={t('providers.editor.requestTimeoutHint', { value: settings?.monoize_request_timeout_ms ?? '—' })}><Input type='number' min='1' value={form.request_timeout_ms_override} onChange={event => setForm(previous => ({ ...previous, request_timeout_ms_override: event.target.value }))} /></Field>
			<div className='flex items-center justify-between gap-4 rounded-lg border p-4'><div><Label>{t('providers.editor.circuitBreaker')}</Label><p className='mt-1 text-xs text-muted-foreground'>{t('providers.editor.circuitBreakerHint')}</p></div><Switch checked={form.circuit_breaker_enabled} onCheckedChange={value => setForm(previous => ({ ...previous, circuit_breaker_enabled: value }))} /></div>
			<div className='flex items-center justify-between gap-4 rounded-lg border p-4'><div><Label>{t('providers.editor.perModelBreaker')}</Label><p className='mt-1 text-xs text-muted-foreground'>{t('providers.editor.perModelBreakerHint')}</p></div><Switch checked={form.per_model_circuit_break} onCheckedChange={value => setForm(previous => ({ ...previous, per_model_circuit_break: value }))} /></div>
		</div>
	</div>
}

function ProtocolSettings({ form, setForm }: { form: ProviderForm; setForm: React.Dispatch<React.SetStateAction<ProviderForm>> }) {
	const { t } = useTranslation()
	return <div className='mx-auto flex w-full max-w-4xl flex-col gap-6 p-4 sm:p-6'><SectionHeading title={t('providers.editor.protocolTitle')} description={t('providers.editor.protocolDescription')} />
		<div className='flex flex-col gap-3'>
			{form.api_type_overrides.map((rule, index) => <div key={index} className='grid gap-2 rounded-xl border bg-card p-3 sm:grid-cols-[1fr_220px_40px] sm:items-center'><Input value={rule.pattern} onChange={event => setForm(previous => ({ ...previous, api_type_overrides: previous.api_type_overrides.map((item, itemIndex) => itemIndex === index ? { ...item, pattern: event.target.value } : item) }))} placeholder='gpt-*' className='font-mono' /><Select value={rule.api_type} onValueChange={(api_type: ProviderType) => setForm(previous => ({ ...previous, api_type_overrides: previous.api_type_overrides.map((item, itemIndex) => itemIndex === index ? { ...item, api_type } : item) }))}><SelectTrigger><SelectValue /></SelectTrigger><SelectContent><SelectGroup>{providerTypes.map(type => <SelectItem key={type} value={type}>{PROVIDER_TYPE_CONFIG[type].label}</SelectItem>)}</SelectGroup></SelectContent></Select><Button size='icon' variant='ghost' className='size-11 touch-manipulation sm:size-9' aria-label={t('providers.editor.deleteOverride')} onClick={() => setForm(previous => ({ ...previous, api_type_overrides: previous.api_type_overrides.filter((_, itemIndex) => itemIndex !== index) }))}><Trash2 data-icon /></Button></div>)}
			<Button variant='outline' className='self-start' onClick={() => setForm(previous => ({ ...previous, api_type_overrides: [...previous.api_type_overrides, { pattern: '', api_type: 'chat_completion' }] }))}><Plus data-icon />{t('providers.editor.addOverride')}</Button>
		</div>
		<Separator />
		<div className='rounded-xl border bg-card p-4 sm:p-5'><NullableBoolean label={t('providers.editor.stripNestedExtras')} value={form.strip_cross_protocol_nested_extra} onChange={value => setForm(previous => ({ ...previous, strip_cross_protocol_nested_extra: value }))} /></div>
	</div>
}
