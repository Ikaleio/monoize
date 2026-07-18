import { useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import useSWR from 'swr'
import {
	ArrowLeft,
	Braces,
	ChevronRight,
	CircleGauge,
	CloudDownload,
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
import { ModelBadge } from '@/components/ModelBadge'
import { TransformChainEditor } from '@/components/transforms/transform-chain-editor'
import { findFirstInvalidTransformRule } from '@/components/transforms/transform-schema'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
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
import { api } from '@/lib/api'
import type {
	CreateProviderInput,
	FetchChannelModelsInput,
	ModelMetadataRecord,
	Provider,
	ProviderType,
	SystemSettings,
	TransformRegistryItem
} from '@/lib/api'
import {
	createProviderOptimistic,
	providerDetailSWRKey,
	updateProviderOptimistic
} from '@/lib/swr'
import { cn } from '@/lib/utils'
import { ModelPickerDialog } from './ModelPickerDialog'
import {
	buildPricedModelIdSet,
	emptyChannelRow,
	emptyForm,
	fromProvider,
	hasBillablePricingModelId,
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
		groups: [...form.groups],
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
				multiplier: Number(row.multiplier)
			}
		])
	)
}

function optionalPositiveInteger(value: string): number | null {
	return value.trim() ? Number(value) : null
}

function channelInput(channel: ChannelRow) {
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
		active_probe_model_override: channel.active_probe_model_override.trim() || null
	}
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
		channels: form.channels.map(channelInput),
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
		groups: form.groups
	}
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
	const { i18n } = useTranslation()
	const zh = i18n.language.startsWith('zh')
	const c = (zhText: string, enText: string) => zh ? zhText : enText
	const isEdit = mode === 'edit'
	const [form, setForm] = useState<ProviderForm>(() => current ? fromProvider(current) : emptyForm())
	const [section, setSection] = useState<Section>('channels')
	const [selectedChannel, setSelectedChannel] = useState(0)
	const [mobileChannelOpen, setMobileChannelOpen] = useState(false)
	const [saving, setSaving] = useState(false)
	const [pickerOpen, setPickerOpen] = useState(false)
	const [closeConfirmOpen, setCloseConfirmOpen] = useState(false)
	const [removeV1Open, setRemoveV1Open] = useState(false)
	const [v1ChannelIndex, setV1ChannelIndex] = useState<number | null>(null)
	const initialSnapshot = useRef('')

	const { data: detail, error: detailError, isLoading: detailLoading } = useSWR(
		open && isEdit && current ? providerDetailSWRKey(current.id) : null,
		() => api.getProvider(current!.id),
		{ revalidateOnFocus: false }
	)

	useEffect(() => {
		if (!open) return
		const next = isEdit ? (detail ?? (detailError ? current : null)) : null
		if (isEdit && !next) return
		const hydrated = next ? fromProvider(next) : emptyForm()
		setForm(cloneForm(hydrated))
		initialSnapshot.current = JSON.stringify(hydrated)
		setSelectedChannel(0)
		setSection('channels')
		setMobileChannelOpen(false)
	}, [open, isEdit, detail, detailError, current])

	const dirty = JSON.stringify(form) !== initialSnapshot.current
	const activeChannel = form.channels[selectedChannel]
	const pricedModels = useMemo(() => buildPricedModelIdSet(modelMetadata), [modelMetadata])
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

	const updateModel = (modelIndex: number, patch: Partial<ModelRow>) => {
		if (!activeChannel) return
		updateChannel(selectedChannel, {
			models: activeChannel.models.map((model, index) => index === modelIndex ? { ...model, ...patch } : model)
		})
	}

	const validate = () => {
		if (!form.name.trim()) return c('请输入 Provider 名称', 'Enter a provider name')
		if (!form.channels.length) return c('至少需要一个 Channel', 'At least one channel is required')
		if (!form.channels.some(channel => channel.models.length > 0)) {
			return c('至少为一个 Channel 添加模型', 'Add models to at least one channel')
		}
		for (const [index, channel] of form.channels.entries()) {
			if (!channel.name.trim() || !channel.base_url.trim()) {
				return c(`Channel ${index + 1} 的名称和 Base URL 不能为空`, `Channel ${index + 1} requires a name and base URL`)
			}
			if (!isEdit && !channel.api_key.trim()) {
				return c(`Channel ${index + 1} 需要 API Key`, `Channel ${index + 1} requires an API key`)
			}
			const names = channel.models.map(model => model.model.trim())
			if (names.some(name => !name) || new Set(names).size !== names.length) {
				return c(`Channel ${index + 1} 存在空白或重复模型`, `Channel ${index + 1} has blank or duplicate models`)
			}
			if (channel.models.some(model => !Number.isFinite(Number(model.multiplier)) || Number(model.multiplier) <= 0)) {
				return c(`Channel ${index + 1} 的倍率必须大于 0`, `Channel ${index + 1} multipliers must be greater than zero`)
			}
		}
		const invalidTransform = findFirstInvalidTransformRule(form.transforms, transformRegistry)
		if (invalidTransform) return invalidTransform.message
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
			const input = buildInput(form)
			if (isEdit && current) {
				await updateProviderOptimistic(current.id, input, providers)
			} else {
				await createProviderOptimistic(input, providers)
			}
			toast.success(c('Provider 已保存', 'Provider saved'))
			initialSnapshot.current = JSON.stringify(form)
			onOpenChange(false)
		} catch (error) {
			toast.error(error instanceof Error ? error.message : c('保存失败', 'Save failed'))
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
			name: `${activeChannel.name} ${c('副本', 'copy')}`,
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
		{ id: 'provider', icon: Server, label: 'Provider', summary: form.name || c('未命名', 'Untitled') },
		{ id: 'channels', icon: Layers3, label: 'Channels', summary: `${form.channels.length}` },
		{ id: 'routing', icon: GitBranch, label: c('路由', 'Routing'), summary: `${form.max_retries === -1 ? '∞' : form.max_retries + 1} attempts` },
		{ id: 'transforms', icon: Braces, label: c('转换', 'Transforms'), summary: `${form.transforms.length}` },
		{ id: 'protocol', icon: Settings2, label: c('协议', 'Protocol'), summary: `${form.api_type_overrides.length}` }
	]

	return (
		<>
			<Dialog open={open} onOpenChange={next => { if (!next) requestClose() }}>
				<DialogContent
					className='flex h-[calc(100dvh-2rem)] w-screen max-w-none flex-col gap-0 overflow-hidden rounded-none border-0 p-0 sm:h-[94vh] sm:w-[96vw] sm:max-w-[1500px] sm:rounded-xl sm:border [&>div:first-child]:flex-1 [&>div:first-child]:gap-0'
					onPointerDownOutside={event => event.preventDefault()}
				>
					<DialogHeader className='shrink-0 border-b bg-background px-4 py-3 text-left sm:px-6'>
						<div className='flex min-w-0 items-center justify-between gap-3'>
							<div className='min-w-0'>
								<DialogTitle className='truncate text-base sm:text-lg'>
									{isEdit ? c('编辑 Provider', 'Edit provider') : c('新建 Provider', 'New provider')}
									{form.name ? <span className='font-normal text-muted-foreground'> · {form.name}</span> : null}
								</DialogTitle>
								<DialogDescription className='mt-0.5 hidden sm:block'>
									{c('模型归属于 Channel；在同一个 Provider 内可为不同上游配置独立重定向与倍率。', 'Models belong to channels. Each upstream can use its own redirect and multiplier.')}
								</DialogDescription>
							</div>
							<div className='flex shrink-0 items-center gap-2'>
								<Label htmlFor='provider-enabled' className='hidden text-xs text-muted-foreground sm:block'>{form.enabled ? c('启用', 'Enabled') : c('停用', 'Disabled')}</Label>
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
							<nav className='hidden w-56 shrink-0 flex-col gap-1 border-r bg-muted/20 p-3 lg:flex' aria-label={c('Provider 编辑分区', 'Provider editor sections')}>
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

								<div className='min-h-0 flex-1 overflow-y-auto'>
									{section === 'channels' ? (
										<ChannelsWorkbench
											form={form}
											activeChannel={activeChannel}
											selectedChannel={selectedChannel}
											mobileChannelOpen={mobileChannelOpen}
											setMobileChannelOpen={setMobileChannelOpen}
											setSelectedChannel={setSelectedChannel}
											updateChannel={updateChannel}
											updateModel={updateModel}
											setForm={setForm}
											addChannel={addChannel}
											duplicateChannel={duplicateChannel}
											removeChannel={removeChannel}
											openPicker={() => {
												if (!pickerInfo) toast.error(c('请先填写 Base URL 和 API Key', 'Enter a base URL and API key first'))
												else setPickerOpen(true)
											}}
											pricedModels={pricedModels}
											metadataProvider={metadataProvider}
											reasoningSuffixMap={reasoningSuffixMap}
											settings={settings}
											c={c}
											onBaseUrlBlur={() => {
												if (activeChannel && hasTrailingV1(activeChannel.base_url)) {
													setV1ChannelIndex(selectedChannel)
													setRemoveV1Open(true)
												}
											}}
										/>
									) : section === 'provider' ? (
										<ProviderBasics form={form} setForm={setForm} c={c} />
									) : section === 'routing' ? (
										<RoutingSettings form={form} setForm={setForm} settings={settings} c={c} />
									) : section === 'transforms' ? (
										<div className='mx-auto flex w-full max-w-4xl flex-col gap-5 p-4 sm:p-6'><SectionHeading title={c('请求与响应转换', 'Request and response transforms')} description={c('转换仍属于 Provider，按顺序应用到每个 Channel。', 'Transforms remain provider-scoped and run in order for every channel.')} /><TransformChainEditor value={form.transforms} registry={transformRegistry} onChange={transforms => setForm(previous => ({ ...previous, transforms }))} /></div>
									) : (
										<ProtocolSettings form={form} setForm={setForm} c={c} />
									)}
								</div>
							</div>
						</div>
					)}

					<DialogFooter className='shrink-0 flex-row items-center justify-between gap-2 border-t bg-background px-4 py-3 sm:px-6'>
						<p className='hidden text-xs text-muted-foreground sm:block'>{dirty ? c('有未保存的更改', 'Unsaved changes') : c('所有更改已保存', 'No unsaved changes')}</p>
						<div className='ml-auto flex items-center gap-2'>
							<Button variant='outline' onClick={requestClose}>{c('取消', 'Cancel')}</Button>
							<Button onClick={() => void save()} disabled={saving}><Save data-icon />{saving ? c('保存中…', 'Saving…') : c('保存 Provider', 'Save provider')}</Button>
						</div>
					</DialogFooter>
				</DialogContent>
			</Dialog>

			<ModelPickerDialog
				open={pickerOpen}
				onOpenChange={setPickerOpen}
				channelInfo={pickerInfo}
				providerName={`${form.name || c('未命名 Provider', 'Untitled provider')} / ${activeChannel?.name || c('未命名 Channel', 'Untitled channel')}`}
				existingModels={activeChannel?.models.map(model => model.model) ?? []}
				modelMetadata={modelMetadata}
				reasoningSuffixMap={reasoningSuffixMap}
				onConfirm={selected => {
					if (!activeChannel) return
					const existing = new Map(activeChannel.models.map(model => [model.model, model]))
					updateChannel(selectedChannel, { models: selected.sort().map(model => existing.get(model) ?? { model, redirect: '', multiplier: '1' }) })
				}}
			/>

			<AlertDialog open={closeConfirmOpen} onOpenChange={setCloseConfirmOpen}>
				<AlertDialogContent><AlertDialogHeader><AlertDialogTitle>{c('放弃未保存的更改？', 'Discard unsaved changes?')}</AlertDialogTitle><AlertDialogDescription>{c('本次编辑的内容将不会保存。', 'Your changes in this editor will be lost.')}</AlertDialogDescription></AlertDialogHeader><AlertDialogFooter><AlertDialogCancel>{c('继续编辑', 'Keep editing')}</AlertDialogCancel><AlertDialogAction className='bg-destructive text-destructive-foreground hover:bg-destructive/90' onClick={() => onOpenChange(false)}>{c('放弃', 'Discard')}</AlertDialogAction></AlertDialogFooter></AlertDialogContent>
			</AlertDialog>

			<AlertDialog open={removeV1Open} onOpenChange={setRemoveV1Open}>
				<AlertDialogContent><AlertDialogHeader><AlertDialogTitle>{c('Base URL 包含 /v1', 'Base URL includes /v1')}</AlertDialogTitle><AlertDialogDescription>{c('多数适配器会自动追加 API 路径。建议移除末尾的 /v1。', 'Most adapters append the API path automatically. Removing the trailing /v1 is recommended.')}</AlertDialogDescription></AlertDialogHeader><AlertDialogFooter><AlertDialogCancel>{c('保留 /v1', 'Keep /v1')}</AlertDialogCancel><AlertDialogAction onClick={() => { if (v1ChannelIndex != null) updateChannel(v1ChannelIndex, { base_url: removeTrailingV1(form.channels[v1ChannelIndex]?.base_url ?? '') }) }}>{c('移除 /v1', 'Remove /v1')}</AlertDialogAction></AlertDialogFooter></AlertDialogContent>
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

function ProviderBasics({ form, setForm, c }: { form: ProviderForm; setForm: React.Dispatch<React.SetStateAction<ProviderForm>>; c: (zh: string, en: string) => string }) {
	return <div className='mx-auto flex w-full max-w-3xl flex-col gap-6 p-4 sm:p-6'>
		<SectionHeading title={c('Provider 基础信息', 'Provider basics')} description={c('Provider 负责公共路由策略；模型和上游地址在 Channel 中配置。', 'Providers own shared routing policy. Models and upstream endpoints are configured per channel.')} />
		<div className='grid gap-5 rounded-xl border bg-card p-4 sm:grid-cols-2 sm:p-5'>
			<Field label={c('名称', 'Name')} className='sm:col-span-2'><Input value={form.name} onChange={event => setForm(previous => ({ ...previous, name: event.target.value }))} placeholder='OpenAI production' /></Field>
			<Field label={c('访问组', 'Groups')} hint={c('逗号分隔；留空表示公开 Provider。', 'Comma-separated. Empty means public provider.')}><Input value={form.groups.join(', ')} onChange={event => setForm(previous => ({ ...previous, groups: event.target.value.split(',').map(value => value.trim().toLowerCase()).filter(Boolean) }))} placeholder='premium, internal' /></Field>
			<Field label={c('额外字段白名单', 'Extra fields allowlist')} hint={c('逗号分隔，应用到全部 Channel。', 'Comma-separated and shared by all channels.')}><Input value={form.extra_fields_whitelist} onChange={event => setForm(previous => ({ ...previous, extra_fields_whitelist: event.target.value }))} placeholder='service_tier, metadata' /></Field>
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
	updateModel: (index: number, patch: Partial<ModelRow>) => void
	setForm: React.Dispatch<React.SetStateAction<ProviderForm>>
	addChannel: () => void
	duplicateChannel: () => void
	removeChannel: () => void
	openPicker: () => void
	pricedModels: Set<string>
	metadataProvider: Map<string, string | undefined>
	reasoningSuffixMap: Record<string, string>
	settings?: SystemSettings
	c: (zh: string, en: string) => string
	onBaseUrlBlur: () => void
}

function ChannelsWorkbench(props: WorkbenchProps) {
	const { form, activeChannel, selectedChannel, mobileChannelOpen, setMobileChannelOpen, setSelectedChannel, addChannel, c } = props
	return <div className='h-full lg:grid lg:grid-cols-[300px_minmax(0,1fr)]'>
		<div className={cn('h-full border-r bg-muted/10', mobileChannelOpen ? 'hidden lg:block' : 'block')}>
			<div className='flex items-center justify-between border-b px-4 py-3'><div><h3 className='font-semibold'>Channels</h3><p className='text-xs text-muted-foreground'>{c('每个上游独立配置模型能力', 'Models are configured per upstream')}</p></div><Button size='icon' variant='outline' onClick={addChannel} aria-label={c('添加 Channel', 'Add channel')}><Plus data-icon /></Button></div>
			<div className='flex flex-col gap-1 p-2'>
				{form.channels.map((channel, index) => <button type='button' key={channel.id || index} onClick={() => { setSelectedChannel(index); setMobileChannelOpen(true) }} className={cn('flex min-h-16 items-center gap-3 rounded-lg border-l-2 px-3 py-2 text-left transition-colors', selectedChannel === index ? 'border-l-primary bg-primary/10' : 'border-l-transparent hover:bg-muted')}>
					<div className='min-w-0 flex-1'><div className='flex items-center gap-2'><span className='truncate text-sm font-medium'>{channel.name || c('未命名 Channel', 'Untitled channel')}</span>{!channel.enabled ? <Badge variant='secondary'>{c('停用', 'Off')}</Badge> : null}</div><p className='mt-1 truncate font-mono text-xs text-muted-foreground'>{channel.base_url || c('尚未填写 Base URL', 'No base URL')}</p><p className='mt-1 text-xs text-muted-foreground'>{PROVIDER_TYPE_CONFIG[channel.provider_type].label} · {channel.models.length} {c('个模型', 'models')}</p></div>
					<ChevronRight className='size-4 shrink-0 text-muted-foreground' />
				</button>)}
			</div>
		</div>

		<div className={cn('min-w-0', mobileChannelOpen ? 'block' : 'hidden lg:block')}>
			{activeChannel ? <ChannelDetail {...props} /> : <div className='grid h-full place-items-center p-6 text-center text-sm text-muted-foreground'>{c('选择一个 Channel 开始配置', 'Select a channel to start configuring')}</div>}
		</div>
	</div>
}

function ChannelDetail({ form, activeChannel, selectedChannel, setMobileChannelOpen, updateChannel, updateModel, duplicateChannel, removeChannel, openPicker, pricedModels, metadataProvider, reasoningSuffixMap, settings, c, onBaseUrlBlur }: WorkbenchProps) {
	if (!activeChannel) return null
	const addModel = () => updateChannel(selectedChannel, { models: [...activeChannel.models, { model: '', redirect: '', multiplier: '1' }] })
	return <div className='mx-auto flex w-full max-w-5xl flex-col gap-6 p-4 pb-8 sm:p-6'>
		<div className='flex items-start justify-between gap-3'>
			<div className='flex min-w-0 items-start gap-2'><Button size='icon' variant='ghost' className='-ml-2 lg:hidden' onClick={() => setMobileChannelOpen(false)} aria-label={c('返回 Channel 列表', 'Back to channels')}><ArrowLeft data-icon /></Button><div className='min-w-0'><h3 className='truncate text-lg font-semibold'>{activeChannel.name || c('未命名 Channel', 'Untitled channel')}</h3><div className='mt-1'>{activeChannel._health_status ? statusBadge(activeChannel._health_status) : <Badge variant='secondary'>{c('未保存', 'Unsaved')}</Badge>}</div></div></div>
			<div className='flex items-center gap-1'><Button size='icon' variant='ghost' onClick={duplicateChannel} aria-label={c('复制 Channel', 'Duplicate channel')}><Copy data-icon /></Button><Button size='icon' variant='ghost' disabled={form.channels.length === 1} onClick={removeChannel} aria-label={c('删除 Channel', 'Delete channel')}><Trash2 data-icon /></Button><Switch checked={activeChannel.enabled} onCheckedChange={enabled => updateChannel(selectedChannel, { enabled })} /></div>
		</div>

		<section className='flex flex-col gap-4 rounded-xl border bg-card p-4 sm:p-5'>
			<div className='flex items-center gap-2'><Server className='size-4 text-primary' /><h4 className='font-medium'>{c('连接', 'Connection')}</h4></div>
			<div className='grid gap-4 sm:grid-cols-2'>
				<Field label={c('Channel 名称', 'Channel name')}><Input value={activeChannel.name} onChange={event => updateChannel(selectedChannel, { name: event.target.value })} /></Field>
				<Field label={c('接口类型', 'API type')}><Select value={activeChannel.provider_type} onValueChange={(provider_type: ProviderType) => updateChannel(selectedChannel, { provider_type })}><SelectTrigger><SelectValue /></SelectTrigger><SelectContent><SelectGroup>{providerTypes.map(type => <SelectItem key={type} value={type}>{PROVIDER_TYPE_CONFIG[type].label}</SelectItem>)}</SelectGroup></SelectContent></Select></Field>
				<Field label='Base URL' className='sm:col-span-2'><Input value={activeChannel.base_url} onChange={event => updateChannel(selectedChannel, { base_url: event.target.value })} onBlur={onBaseUrlBlur} placeholder='https://api.openai.com' className='font-mono' /></Field>
				<Field label='API Key' hint={form.id && activeChannel.id ? c('留空保留现有密钥。', 'Leave blank to preserve the stored key.') : undefined}><Input type='password' autoComplete='new-password' value={activeChannel.api_key} onChange={event => updateChannel(selectedChannel, { api_key: event.target.value })} placeholder={form.id && activeChannel.id ? '••••••••••••' : 'sk-…'} className='font-mono' /></Field>
				<Field label={c('流量权重', 'Traffic weight')}><Input type='number' min='0' value={activeChannel.weight} onChange={event => updateChannel(selectedChannel, { weight: event.target.value })} /></Field>
			</div>
		</section>

		<section className='flex flex-col gap-4 rounded-xl border bg-card p-4 sm:p-5'>
			<div className='flex flex-wrap items-center justify-between gap-3'><div><div className='flex items-center gap-2'><Layers3 className='size-4 text-primary' /><h4 className='font-medium'>{c('支持的模型', 'Supported models')}</h4><Badge variant='secondary'>{activeChannel.models.length}</Badge></div><p className='mt-1 text-xs text-muted-foreground'>{c('逻辑模型、上游重定向和计费倍率都只作用于当前 Channel。', 'Logical model, upstream redirect, and billing multiplier apply only to this channel.')}</p></div><div className='flex items-center gap-2'><Button variant='outline' size='sm' onClick={openPicker}><CloudDownload data-icon />{c('从上游获取', 'Fetch upstream')}</Button><Button size='sm' onClick={addModel}><Plus data-icon />{c('手动添加', 'Add manually')}</Button></div></div>
			{activeChannel.models.length === 0 ? <Alert><Layers3 className='size-4' /><AlertTitle>{c('当前 Channel 不会接收请求', 'This channel will not receive traffic')}</AlertTitle><AlertDescription>{c('从上游获取模型，或手动添加一个逻辑模型。', 'Fetch models from the upstream or add a logical model manually.')}</AlertDescription></Alert> : null}
			<div className='flex flex-col gap-2'>
				<div className='hidden grid-cols-[minmax(180px,1fr)_minmax(180px,1fr)_100px_36px] gap-2 px-2 text-xs font-medium text-muted-foreground md:grid'><span>{c('逻辑模型', 'Logical model')}</span><span>{c('上游模型（可选）', 'Upstream model (optional)')}</span><span>{c('倍率', 'Multiplier')}</span><span /></div>
				{activeChannel.models.map((model, index) => {
					const unpriced = model.model.trim() && !hasBillablePricingModelId(pricedModels, model.model, model.redirect, reasoningSuffixMap)
					return <div key={`${model.model}-${index}`} className={cn('grid gap-2 rounded-lg border p-3 md:grid-cols-[minmax(180px,1fr)_minmax(180px,1fr)_100px_36px] md:items-center md:p-2', unpriced && 'border-status-warning/40 bg-status-warning/5')}>
						<div className='flex min-w-0 items-center gap-2'><ModelBadge model={model.model || c('新模型', 'New model')} provider={metadataProvider.get(model.model)} highlightUnpriced={Boolean(unpriced)} /><Input aria-label={c('逻辑模型', 'Logical model')} value={model.model} onChange={event => updateModel(index, { model: event.target.value })} className='min-w-0 font-mono md:hidden' /></div>
						<Input aria-label={c('逻辑模型', 'Logical model')} value={model.model} onChange={event => updateModel(index, { model: event.target.value })} className='hidden min-w-0 font-mono md:block' />
						<Input aria-label={c('上游模型', 'Upstream model')} value={model.redirect} onChange={event => updateModel(index, { redirect: event.target.value })} placeholder={c('同逻辑模型', 'Same as logical')} className='min-w-0 font-mono' />
						<Input aria-label={c('倍率', 'Multiplier')} type='number' min='0.0001' step='0.1' value={model.multiplier} onChange={event => updateModel(index, { multiplier: event.target.value })} />
						<Button size='icon' variant='ghost' onClick={() => updateChannel(selectedChannel, { models: activeChannel.models.filter((_, modelIndex) => modelIndex !== index) })} aria-label={c('删除模型', 'Delete model')}><Trash2 data-icon /></Button>
					</div>
				})}
			</div>
		</section>

		<details className='group rounded-xl border bg-card'>
			<summary className='flex cursor-pointer list-none items-center justify-between gap-3 p-4 sm:p-5'><div className='flex items-center gap-3'><CircleGauge className='size-4 text-muted-foreground' /><div><h4 className='font-medium'>{c('健康检查与熔断', 'Health and circuit breaker')}</h4><p className='mt-0.5 text-xs text-muted-foreground'>{c('默认继承全局设置', 'Inherits global settings by default')}</p></div></div><ChevronRight className='size-4 transition-transform group-open:rotate-90' /></summary>
			<div className='grid gap-4 border-t p-4 sm:grid-cols-2 sm:p-5'>
				<NullableBoolean label={c('主动探测', 'Active probing')} value={activeChannel.active_probe_enabled_override} onChange={value => updateChannel(selectedChannel, { active_probe_enabled_override: value })} c={c} />
				<Field label={c('探测模型', 'Probe model')} hint={c(`留空继承：${settings?.monoize_active_probe_model || '首个 Channel 模型'}`, `Empty inherits: ${settings?.monoize_active_probe_model || 'first channel model'}`)}><Input value={activeChannel.active_probe_model_override} onChange={event => updateChannel(selectedChannel, { active_probe_model_override: event.target.value })} /></Field>
				<NumberOverride label={c('失败次数阈值', 'Failure count threshold')} value={activeChannel.passive_failure_count_threshold_override} placeholder={settings?.monoize_passive_failure_threshold} onChange={value => updateChannel(selectedChannel, { passive_failure_count_threshold_override: value })} />
				<NumberOverride label={c('统计窗口（秒）', 'Window (seconds)')} value={activeChannel.passive_window_seconds_override} placeholder={settings?.monoize_passive_window_seconds} onChange={value => updateChannel(selectedChannel, { passive_window_seconds_override: value })} />
				<NumberOverride label={c('冷却时间（秒）', 'Cooldown (seconds)')} value={activeChannel.passive_cooldown_seconds_override} placeholder={settings?.monoize_passive_cooldown_seconds} onChange={value => updateChannel(selectedChannel, { passive_cooldown_seconds_override: value })} />
				<NumberOverride label={c('限流冷却（秒）', 'Rate-limit cooldown (seconds)')} value={activeChannel.passive_rate_limit_cooldown_seconds_override} placeholder={settings?.monoize_passive_rate_limit_cooldown_seconds} onChange={value => updateChannel(selectedChannel, { passive_rate_limit_cooldown_seconds_override: value })} />
			</div>
		</details>
	</div>
}

function NumberOverride({ label, value, placeholder, onChange }: { label: string; value: string; placeholder?: number; onChange: (value: string) => void }) {
	return <Field label={label}><Input type='number' min='1' value={value} placeholder={placeholder == null ? undefined : String(placeholder)} onChange={event => onChange(event.target.value)} /></Field>
}

function NullableBoolean({ label, value, onChange, c }: { label: string; value: boolean | null; onChange: (value: boolean | null) => void; c: (zh: string, en: string) => string }) {
	return <Field label={label}><Select value={value == null ? 'inherit' : value ? 'enabled' : 'disabled'} onValueChange={next => onChange(next === 'inherit' ? null : next === 'enabled')}><SelectTrigger><SelectValue /></SelectTrigger><SelectContent><SelectGroup><SelectItem value='inherit'>{c('继承全局', 'Inherit global')}</SelectItem><SelectItem value='enabled'>{c('启用', 'Enabled')}</SelectItem><SelectItem value='disabled'>{c('停用', 'Disabled')}</SelectItem></SelectGroup></SelectContent></Select></Field>
}

function RoutingSettings({ form, setForm, settings, c }: { form: ProviderForm; setForm: React.Dispatch<React.SetStateAction<ProviderForm>>; settings?: SystemSettings; c: (zh: string, en: string) => string }) {
	return <div className='mx-auto flex w-full max-w-4xl flex-col gap-6 p-4 sm:p-6'><SectionHeading title={c('路由与重试', 'Routing and retries')} description={c('这些策略应用到 Provider 下的全部 Channel。', 'These policies apply to every channel in the provider.')} />
		<div className='grid gap-4 rounded-xl border bg-card p-4 sm:grid-cols-2 sm:p-5'>
			<Field label={c('Provider 最大重试', 'Provider max retries')} hint={c('-1 表示尝试全部可用 Channel。', '-1 tries every eligible channel.')}><Input type='number' min='-1' value={form.max_retries} onChange={event => setForm(previous => ({ ...previous, max_retries: Number(event.target.value) }))} /></Field>
			<Field label={c('单 Channel 重试', 'Retries per channel')}><Input type='number' min='0' value={form.channel_max_retries} onChange={event => setForm(previous => ({ ...previous, channel_max_retries: Number(event.target.value) }))} /></Field>
			<Field label={c('重试间隔（毫秒）', 'Retry interval (ms)')}><Input type='number' min='0' value={form.channel_retry_interval_ms} onChange={event => setForm(previous => ({ ...previous, channel_retry_interval_ms: Number(event.target.value) }))} /></Field>
			<Field label={c('请求超时覆盖（毫秒）', 'Request timeout override (ms)')} hint={c(`留空继承全局 ${settings?.monoize_request_timeout_ms ?? '—'}`, `Empty inherits global ${settings?.monoize_request_timeout_ms ?? '—'}`)}><Input type='number' min='1' value={form.request_timeout_ms_override} onChange={event => setForm(previous => ({ ...previous, request_timeout_ms_override: event.target.value }))} /></Field>
			<div className='flex items-center justify-between gap-4 rounded-lg border p-4'><div><Label>{c('启用熔断器', 'Circuit breaker')}</Label><p className='mt-1 text-xs text-muted-foreground'>{c('根据失败状态暂时移除 Channel。', 'Temporarily removes failing channels.')}</p></div><Switch checked={form.circuit_breaker_enabled} onCheckedChange={value => setForm(previous => ({ ...previous, circuit_breaker_enabled: value }))} /></div>
			<div className='flex items-center justify-between gap-4 rounded-lg border p-4'><div><Label>{c('按模型隔离熔断', 'Per-model circuit breaker')}</Label><p className='mt-1 text-xs text-muted-foreground'>{c('同一 Channel 的模型分别维护健康状态。', 'Tracks health separately per model.')}</p></div><Switch checked={form.per_model_circuit_break} onCheckedChange={value => setForm(previous => ({ ...previous, per_model_circuit_break: value }))} /></div>
		</div>
	</div>
}

function ProtocolSettings({ form, setForm, c }: { form: ProviderForm; setForm: React.Dispatch<React.SetStateAction<ProviderForm>>; c: (zh: string, en: string) => string }) {
	return <div className='mx-auto flex w-full max-w-4xl flex-col gap-6 p-4 sm:p-6'><SectionHeading title={c('协议覆盖', 'Protocol overrides')} description={c('按逻辑模型 glob 覆盖 Channel 默认接口类型。第一条匹配规则生效。', 'Override a channel default API type by logical-model glob. First match wins.')} />
		<div className='flex flex-col gap-3'>
			{form.api_type_overrides.map((rule, index) => <div key={index} className='grid gap-2 rounded-xl border bg-card p-3 sm:grid-cols-[1fr_220px_40px] sm:items-center'><Input value={rule.pattern} onChange={event => setForm(previous => ({ ...previous, api_type_overrides: previous.api_type_overrides.map((item, itemIndex) => itemIndex === index ? { ...item, pattern: event.target.value } : item) }))} placeholder='gpt-*' className='font-mono' /><Select value={rule.api_type} onValueChange={(api_type: ProviderType) => setForm(previous => ({ ...previous, api_type_overrides: previous.api_type_overrides.map((item, itemIndex) => itemIndex === index ? { ...item, api_type } : item) }))}><SelectTrigger><SelectValue /></SelectTrigger><SelectContent><SelectGroup>{providerTypes.map(type => <SelectItem key={type} value={type}>{PROVIDER_TYPE_CONFIG[type].label}</SelectItem>)}</SelectGroup></SelectContent></Select><Button size='icon' variant='ghost' onClick={() => setForm(previous => ({ ...previous, api_type_overrides: previous.api_type_overrides.filter((_, itemIndex) => itemIndex !== index) }))}><Trash2 data-icon /></Button></div>)}
			<Button variant='outline' className='self-start' onClick={() => setForm(previous => ({ ...previous, api_type_overrides: [...previous.api_type_overrides, { pattern: '', api_type: 'chat_completion' }] }))}><Plus data-icon />{c('添加覆盖规则', 'Add override')}</Button>
		</div>
		<Separator />
		<div className='rounded-xl border bg-card p-4 sm:p-5'><NullableBoolean label={c('剥离跨协议嵌套额外字段', 'Strip cross-protocol nested extras')} value={form.strip_cross_protocol_nested_extra} onChange={value => setForm(previous => ({ ...previous, strip_cross_protocol_nested_extra: value }))} c={c} /></div>
	</div>
}
