import { useMemo, useState } from 'react'
import { ArrowDown, ArrowUp, Download, Globe, Layers, Plus, Radio, Settings2, Trash2, Weight, X } from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Skeleton } from '@/components/ui/skeleton'
import {
	Select,
	SelectContent,
	SelectItem,
	SelectTrigger,
	SelectValue
} from '@/components/ui/select'
import { Separator } from '@/components/ui/separator'
import { Switch } from '@/components/ui/switch'
import { ModelBadge } from '@/components/ModelBadge'
import { TransformChainEditor } from '@/components/transforms/transform-chain-editor'
import { cn } from '@/lib/utils'
import type {
	ProviderType,
	TransformRegistryItem
} from '@/lib/api'
import { Virtuoso } from 'react-virtuoso'
import {
	type ChannelRow,
	type ProviderForm,
	PROVIDER_EDIT_CHANNEL_ROW_HEIGHT,
	PROVIDER_TYPE_CONFIG,
	hasBillablePricingModelId
} from './shared'

type Translator = (key: string) => string

function groupKey(value: string): string {
	return value.trim().toLowerCase()
}

function dedupeChannelGroups(values: string[]): string[] {
	const seen = new Set<string>()
	const next: string[] = []

	for (const value of values) {
		const trimmed = value.trim()
		const key = groupKey(trimmed)
		if (!key || seen.has(key)) {
			continue
		}
		seen.add(key)
		next.push(key)
	}

	return next
}

interface ChannelGroupsInputProps {
	value: string[]
	suggestions: string[]
	suggestionsLoading: boolean
	t: Translator
	onChange: (next: string[]) => void
}

export function ChannelGroupsInput({
	value,
	suggestions,
	suggestionsLoading,
	t,
	onChange
}: ChannelGroupsInputProps) {
	const [draft, setDraft] = useState('')
	const groups = useMemo(() => dedupeChannelGroups(value), [value])
	const draftKey = groupKey(draft)
	const filteredSuggestions = useMemo(
		() =>
			suggestions.filter(suggestion => {
				const suggestionKey = groupKey(suggestion)
				if (!suggestionKey) {
					return false
				}
				if (groups.some(group => groupKey(group) === suggestionKey)) {
					return false
				}
				return !draftKey || suggestionKey.includes(draftKey)
			}),
		[draftKey, groups, suggestions]
	)

	const commitGroups = (nextValues: string[]) => {
		onChange(dedupeChannelGroups(nextValues))
	}

	const flushDraft = () => {
		const parts = draft
			.split(',')
			.map(part => part.trim())
			.filter(Boolean)
		if (parts.length > 0) {
			commitGroups([...groups, ...parts])
		}
		setDraft('')
	}

	const removeGroup = (group: string) => {
		commitGroups(groups.filter(entry => groupKey(entry) !== groupKey(group)))
	}

	const addSuggestion = (group: string) => {
		commitGroups([...groups, group])
		setDraft('')
	}

	return (
		<div className='space-y-2'>
			<div className='flex items-center justify-between gap-2'>
				<Label>{t('providers.groups')}</Label>
				<span className='text-xs text-muted-foreground'>
					{t('providers.optional')}
				</span>
			</div>
			<Input
				value={draft}
				placeholder={t('providers.groupsPlaceholder')}
				onChange={e => setDraft(e.target.value)}
				onBlur={flushDraft}
				onKeyDown={e => {
					if (e.key === 'Enter' || e.key === ',') {
						e.preventDefault()
						flushDraft()
					}
				}}
			/>
			{groups.length > 0 && (
				<div className='flex flex-wrap gap-2'>
					{groups.map(group => (
						<Badge
							key={groupKey(group)}
							variant='secondary'
							className='flex items-center gap-1 font-mono'
						>
							<span>{group}</span>
							<Button
								type='button'
								variant='ghost'
								size='icon'
								className='h-4 w-4'
								onClick={() => removeGroup(group)}
							>
								<X className='h-3 w-3' />
							</Button>
						</Badge>
					))}
				</div>
			)}
			{suggestionsLoading ? (
				<div className='flex flex-wrap gap-2'>
					<Skeleton className='h-7 w-20 rounded-full' />
					<Skeleton className='h-7 w-24 rounded-full' />
					<Skeleton className='h-7 w-16 rounded-full' />
				</div>
			) : filteredSuggestions.length > 0 ? (
				<div className='flex flex-wrap gap-2'>
					{filteredSuggestions.slice(0, 8).map(group => (
						<Button
							key={group}
							type='button'
							variant='outline'
							size='sm'
							className='h-7 rounded-full px-3 font-mono text-xs'
							onClick={() => addSuggestion(group)}
						>
							{group}
						</Button>
					))}
				</div>
			) : null}
		</div>
	)
}

interface ProviderBasicsSectionProps {
	form: ProviderForm
	isEdit: boolean
	t: Translator
	onFormChange: (updater: (prev: ProviderForm) => ProviderForm) => void
}

export function ProviderBasicsSection({
	form,
	isEdit,
	t,
	onFormChange
}: ProviderBasicsSectionProps) {
	return (
		<>
			<div className='space-y-2'>
				<Label>{t('providers.name')}</Label>
				<Input
					placeholder={t('providers.namePlaceholder')}
					value={form.name}
					onChange={e =>
						onFormChange(prev => ({ ...prev, name: e.target.value }))
					}
				/>
			</div>
			<div className='space-y-2'>
				<Label>{t('providers.type')}</Label>
				<Select
					value={form.provider_type}
					onValueChange={value =>
						onFormChange(prev => ({
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
						onFormChange(prev => ({
							...prev,
							max_retries: Number(e.target.value) || 0
						}))
					}
				/>
			</div>
			<div className='space-y-2'>
				<Label>{t('providers.channelMaxRetries')}</Label>
				<Input
					type='number'
					min='0'
					value={form.channel_max_retries}
					onChange={e =>
						onFormChange(prev => ({
							...prev,
							channel_max_retries: Number(e.target.value) || 0
						}))
					}
				/>
			</div>
			<div className='space-y-2'>
				<Label>{t('providers.channelRetryIntervalMs')}</Label>
				<Input
					type='number'
					min='0'
					value={form.channel_retry_interval_ms}
					onChange={e =>
						onFormChange(prev => ({
							...prev,
							channel_retry_interval_ms: Math.max(0, Number(e.target.value) || 0)
						}))
					}
				/>
			</div>
			<div className='flex items-center gap-2'>
				<Switch
					checked={form.circuit_breaker_enabled}
					onCheckedChange={checked =>
						onFormChange(prev => ({
							...prev,
							circuit_breaker_enabled: checked,
							per_model_circuit_break:
								checked ? prev.per_model_circuit_break : false
						}))
					}
				/>
				<Label>{t('providers.circuitBreakerEnabled')}</Label>
			</div>
			<div className='flex items-center gap-2'>
				<Switch
					checked={form.per_model_circuit_break}
					disabled={!form.circuit_breaker_enabled}
					onCheckedChange={checked =>
						onFormChange(prev => ({
							...prev,
							per_model_circuit_break: checked
						}))
					}
				/>
				<Label>{t('providers.perModelCircuitBreak')}</Label>
			</div>
		</>
	)
}

interface ProbeSettingsSectionProps {
	form: ProviderForm
	t: Translator
	globalDefaults?: {
		active_probe_enabled?: boolean
		active_probe_interval_seconds?: number
		active_probe_success_threshold?: number
		active_probe_model?: string | null
	}
	onFormChange: (updater: (prev: ProviderForm) => ProviderForm) => void
}

export function ProbeSettingsSection({
	form,
	t,
	globalDefaults,
	onFormChange
}: ProbeSettingsSectionProps) {
	const inheritGlobal = t('providers.inheritGlobal')
	const globalProbeEnabled = globalDefaults?.active_probe_enabled ?? true
	const inheritedProbeLabel = `${inheritGlobal} (${globalProbeEnabled ? t('common.enabled') : t('common.disabled')})`
	const probeModelPlaceholder =
		globalDefaults?.active_probe_model?.trim() ?
			`${inheritGlobal} (${globalDefaults.active_probe_model})`
		:	t('providers.probeModelOverridePlaceholder')

	return (
		<div className='md:col-span-2 rounded-md border p-3 space-y-3'>
			<div className='text-sm font-medium'>{t('providers.probeOverrideTitle')}</div>
			<div className='grid grid-cols-1 md:grid-cols-2 gap-4'>
				<div className='space-y-2'>
					<Label>{t('providers.probeEnabledOverride')}</Label>
					<Select
						value={form.active_probe_enabled_override === null ? 'inherit' : form.active_probe_enabled_override ? 'enabled' : 'disabled'}
						onValueChange={value =>
							onFormChange(prev => ({
								...prev,
								active_probe_enabled_override:
									value === 'inherit' ? null : value === 'enabled'
							}))
						}
					>
						<SelectTrigger>
							<SelectValue />
						</SelectTrigger>
						<SelectContent>
							<SelectItem value='inherit'>{inheritedProbeLabel}</SelectItem>
							<SelectItem value='enabled'>{t('common.enabled')}</SelectItem>
							<SelectItem value='disabled'>{t('common.disabled')}</SelectItem>
						</SelectContent>
					</Select>
				</div>
				<div className='space-y-2'>
					<Label>{t('providers.probeModelOverride')}</Label>
					<Input
						value={form.active_probe_model_override ?? ''}
						onChange={e =>
							onFormChange(prev => ({
								...prev,
								active_probe_model_override: e.target.value
							}))
						}
						placeholder={probeModelPlaceholder}
					/>
				</div>
				<div className='space-y-2'>
					<Label>{t('providers.probeIntervalOverride')}</Label>
					<Input
						type='number'
						min='1'
						value={form.active_probe_interval_seconds_override ?? ''}
						onChange={e =>
							onFormChange(prev => ({
								...prev,
								active_probe_interval_seconds_override:
									e.target.value.trim() ?
										Math.max(1, Number(e.target.value) || 1)
									: 	null
							}))
						}
						placeholder={`${inheritGlobal} (${globalDefaults?.active_probe_interval_seconds ?? 30})`}
					/>
				</div>
				<div className='space-y-2'>
					<Label>{t('providers.probeSuccessThresholdOverride')}</Label>
					<Input
						type='number'
						min='1'
						value={form.active_probe_success_threshold_override ?? ''}
						onChange={e =>
							onFormChange(prev => ({
								...prev,
								active_probe_success_threshold_override:
									e.target.value.trim() ?
										Math.max(1, Number(e.target.value) || 1)
									: 	null
							}))
						}
						placeholder={`${inheritGlobal} (${globalDefaults?.active_probe_success_threshold ?? 1})`}
					/>
				</div>
			</div>
			<p className='text-xs text-muted-foreground'>
				{t('providers.probeOverrideDescription')}
			</p>
		</div>
	)
}

interface TimeoutEnabledSectionProps {
	form: ProviderForm
	t: Translator
	globalDefaults?: {
		request_timeout_ms?: number
	}
	onFormChange: (updater: (prev: ProviderForm) => ProviderForm) => void
}

export function TimeoutEnabledSection({
	form,
	t,
	globalDefaults,
	onFormChange
}: TimeoutEnabledSectionProps) {
	const inheritGlobal = t('providers.inheritGlobal')

	return (
		<>
			<div className='space-y-2'>
				<Label>{t('providers.requestTimeoutMsOverride')}</Label>
				<Input
					type='number'
					min='1'
					placeholder={`${inheritGlobal} (${globalDefaults?.request_timeout_ms ?? 30000})`}
					value={form.request_timeout_ms_override}
					onChange={e =>
						onFormChange(prev => ({
							...prev,
							request_timeout_ms_override: e.target.value
						}))
					}
				/>
				<p className='text-xs text-muted-foreground'>
					{t('providers.requestTimeoutMsOverrideDescription')}
				</p>
			</div>
			<div className='space-y-2'>
				<Label>{t('providers.extraFieldsWhitelist')}</Label>
				<Input
					placeholder={t('providers.extraFieldsWhitelistPlaceholder')}
					value={form.extra_fields_whitelist}
					onChange={e =>
						onFormChange(prev => ({
							...prev,
							extra_fields_whitelist: e.target.value
						}))
					}
					className='font-mono text-sm'
				/>
				<p className='text-xs text-muted-foreground'>
					{t('providers.extraFieldsWhitelistDescription')}
				</p>
			</div>
			<div className='flex items-center gap-2 pt-7'>
				<Switch
					checked={form.enabled}
					onCheckedChange={checked =>
						onFormChange(prev => ({ ...prev, enabled: checked }))
					}
				/>
				<Label>{t('providers.enabled')}</Label>
			</div>
		</>
	)
}

interface ApiTypeOverridesSectionProps {
	form: ProviderForm
	t: Translator
	onFormChange: (updater: (prev: ProviderForm) => ProviderForm) => void
}

export function ApiTypeOverridesSection({
	form,
	t,
	onFormChange
}: ApiTypeOverridesSectionProps) {
	return (
		<div className='md:col-span-2 rounded-lg border p-4 space-y-3'>
			<div className='flex items-center justify-between'>
				<div>
					<h3 className='text-sm font-semibold'>{t('providers.apiTypeOverrides')}</h3>
					<p className='text-xs text-muted-foreground mt-0.5'>
						{t('providers.apiTypeOverridesDesc')}
					</p>
				</div>
				<Button
					type='button'
					variant='outline'
					size='sm'
					onClick={() =>
						onFormChange(prev => ({
							...prev,
							api_type_overrides: [
								...prev.api_type_overrides,
								{ pattern: '', api_type: 'chat_completion' }
							]
						}))
					}
				>
					<Plus className='h-4 w-4 mr-1' />
					{t('providers.addOverride')}
				</Button>
			</div>
			{form.api_type_overrides.length > 0 && (
				<div className='space-y-2'>
					{form.api_type_overrides.map((override, idx) => (
						<div key={idx} className='flex items-center gap-2'>
							<Input
								className='flex-1 font-mono text-sm'
								placeholder='claude-*'
								value={override.pattern}
								onChange={e => {
									const updated = [...form.api_type_overrides]
									updated[idx] = { ...updated[idx], pattern: e.target.value }
									onFormChange(prev => ({ ...prev, api_type_overrides: updated }))
								}}
							/>
							<Select
								value={override.api_type}
								onValueChange={(val: string) => {
									const updated = [...form.api_type_overrides]
									updated[idx] = { ...updated[idx], api_type: val as ProviderType }
									onFormChange(prev => ({ ...prev, api_type_overrides: updated }))
								}}
							>
								<SelectTrigger className='w-[200px]'>
									<SelectValue />
								</SelectTrigger>
								<SelectContent>
									{(Object.keys(PROVIDER_TYPE_CONFIG) as ProviderType[]).map(pt => (
										<SelectItem key={pt} value={pt}>
											{PROVIDER_TYPE_CONFIG[pt].label}
										</SelectItem>
									))}
								</SelectContent>
							</Select>
							<Button
								type='button'
								variant='ghost'
								size='icon'
								className='h-8 w-8'
								disabled={idx === 0}
								onClick={() => {
									const updated = [...form.api_type_overrides]
									;[updated[idx - 1], updated[idx]] = [updated[idx], updated[idx - 1]]
									onFormChange(prev => ({ ...prev, api_type_overrides: updated }))
								}}
							>
								<ArrowUp className='h-4 w-4' />
							</Button>
							<Button
								type='button'
								variant='ghost'
								size='icon'
								className='h-8 w-8'
								disabled={idx === form.api_type_overrides.length - 1}
								onClick={() => {
									const updated = [...form.api_type_overrides]
									;[updated[idx], updated[idx + 1]] = [updated[idx + 1], updated[idx]]
									onFormChange(prev => ({ ...prev, api_type_overrides: updated }))
								}}
							>
								<ArrowDown className='h-4 w-4' />
							</Button>
							<Button
								type='button'
								variant='ghost'
								size='icon'
								className='h-8 w-8 text-destructive'
								onClick={() =>
									onFormChange(prev => ({
										...prev,
										api_type_overrides: prev.api_type_overrides.filter((_, i) => i !== idx)
									}))
								}
							>
								<Trash2 className='h-4 w-4' />
							</Button>
						</div>
					))}
				</div>
			)}
		</div>
	)
}

interface ModelsSectionProps {
	form: ProviderForm
	editingModelIndex: number | null
	modelProviderMap: Map<string, string | undefined>
	pricedModelIdSet: Set<string>
	reasoningSuffixMap: Record<string, string>
	t: Translator
	onFetchModels: () => void
	onAddModel: () => void
	onEditModel: (index: number) => void
	onDeleteModel: (index: number) => void
}

export function ModelsSection({
	form,
	editingModelIndex,
	modelProviderMap,
	pricedModelIdSet,
	reasoningSuffixMap,
	t,
	onFetchModels,
	onAddModel,
	onEditModel,
	onDeleteModel
}: ModelsSectionProps) {
	return (
		<div className='space-y-3'>
			<div className='flex items-center justify-between'>
				<div className='flex items-center gap-2'>
					<Layers className='h-4 w-4 text-muted-foreground' />
					<h3 className='text-base font-semibold'>{t('providers.modelsSection')}</h3>
				</div>
				<div className='flex items-center gap-2'>
					<Button type='button' variant='outline' size='sm' onClick={onFetchModels}>
						<Download className='h-4 w-4 mr-2' />
						{t('providers.fetchModels')}
					</Button>
					<Button type='button' variant='outline' size='sm' onClick={onAddModel}>
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
				: 	<div className='flex flex-wrap content-start gap-2 max-h-[220px] overflow-y-auto'>
						{form.models.map((row, idx) => (
							<div
								key={`model-${row.model || idx}`}
								className='group relative min-w-0 max-w-full shrink-0'
							>
								<button type='button' onClick={() => onEditModel(idx)} className='text-left'>
									<ModelBadge
										model={row.model || '-'}
										provider={modelProviderMap.get(row.model.trim())}
										multiplier={row.multiplier || 1}
										detailTarget={row.redirect.trim() || row.model || '-'}
										highlightUnpriced={!hasBillablePricingModelId(
											pricedModelIdSet,
											row.model,
											row.redirect,
											reasoningSuffixMap
										)}
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
										onDeleteModel(idx)
									}}
								>
									<X className='h-3 w-3' />
								</button>
							</div>
						))}
					</div>
				}
			</div>
		</div>
	)
}

interface ChannelsSectionProps {
	form: ProviderForm
	editingChannelIndex: number | null
	isEdit: boolean
	t: Translator
	onAddChannel: () => void
	onSelectChannel: (index: number) => void
	onToggleChannelEnabled: (index: number, enabled: boolean) => void
	onDeleteChannel: (index: number) => void
}

export function ChannelsSection({
	form,
	editingChannelIndex,
	isEdit,
	t,
	onAddChannel,
	onSelectChannel,
	onToggleChannelEnabled,
	onDeleteChannel
}: ChannelsSectionProps) {
	return (
		<div className='space-y-3'>
			<div className='flex items-center justify-between'>
				<div className='flex items-center gap-2'>
					<Radio className='h-4 w-4 text-muted-foreground' />
					<h3 className='text-base font-semibold'>{t('providers.channelsSection')}</h3>
					<Badge variant='secondary' className='text-xs'>
						{form.channels.length}
					</Badge>
				</div>
				<Button type='button' variant='outline' size='sm' onClick={onAddChannel}>
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
				: 	<Virtuoso
						style={{
							height: Math.min(
								form.channels.length * PROVIDER_EDIT_CHANNEL_ROW_HEIGHT,
								300
							)
						}}
						data={form.channels}
						itemContent={(idx, row) => (
							<ChannelListRow
								row={row}
								selected={editingChannelIndex === idx}
								t={t}
								onSelect={() => onSelectChannel(idx)}
								onToggleEnabled={enabled => onToggleChannelEnabled(idx, enabled)}
								onDelete={() => onDeleteChannel(idx)}
							/>
						)}
					/>
				}
			</div>
		</div>
	)
}

interface ChannelListRowProps {
	row: ChannelRow
	selected: boolean
	t: Translator
	onSelect: () => void
	onToggleEnabled: (enabled: boolean) => void
	onDelete: () => void
}

function ChannelListRow({
	row,
	selected,
	t,
	onSelect,
	onToggleEnabled,
	onDelete
}: ChannelListRowProps) {
	return (
		<div
			className={cn(
				'flex h-14 items-center gap-3 px-3 border-b last:border-b-0 cursor-pointer transition-colors hover:bg-muted/50',
				selected ? 'bg-primary/5' : 'bg-background'
			)}
			onClick={onSelect}
		>
			<div className='min-w-0 flex-1'>
				<div className='flex items-center gap-2'>
					<span className='truncate text-sm font-medium'>
						{row.name || t('providers.addChannel')}
					</span>
					{!row.enabled && (
						<Badge variant='secondary' className='text-[10px]'>
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
					onCheckedChange={onToggleEnabled}
					onClick={e => e.stopPropagation()}
				/>
				<Button
					type='button'
					variant='ghost'
					size='icon'
					className='h-7 w-7 text-destructive hover:text-destructive'
					onClick={e => {
						e.stopPropagation()
						onDelete()
					}}
				>
					<Trash2 className='h-3.5 w-3.5' />
				</Button>
			</div>
		</div>
	)
}

interface TransformsSectionProps {
	transformRegistry: TransformRegistryItem[]
	transforms: ProviderForm['transforms']
	t: Translator
	onChange: (next: ProviderForm['transforms']) => void
}

export function TransformsSection({
	transformRegistry,
	transforms,
	t,
	onChange
}: TransformsSectionProps) {
	return (
		<div className='space-y-3'>
			<div className='flex items-center gap-2'>
				<Settings2 className='h-4 w-4 text-muted-foreground' />
				<h3 className='text-base font-semibold'>{t('transforms.titleProvider')}</h3>
			</div>
			<TransformChainEditor value={transforms} registry={transformRegistry} onChange={onChange} />
		</div>
	)
}

export function ProviderDialogSectionsDivider() {
	return <Separator />
}
