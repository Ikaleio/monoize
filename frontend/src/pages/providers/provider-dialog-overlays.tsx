import { Download, Trash2 } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Switch } from '@/components/ui/switch'
import {
	Select,
	SelectContent,
	SelectItem,
	SelectTrigger,
	SelectValue
} from '@/components/ui/select'
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
import { Separator } from '@/components/ui/separator'
import type { ProviderType } from '@/lib/api'
import {
	PROVIDER_TYPE_CONFIG,
	type ChannelRow,
	type ModelRow
} from './shared'

type Translator = (key: string) => string

interface BaseUrlPrompt {
	original: string
	trimmed: string
}

interface BaseUrlV1AlertDialogProps {
	prompt: BaseUrlPrompt | null
	t: Translator
	onOpenChange: (open: boolean) => void
	onKeep: () => void
	onRemove: () => void
}

export function BaseUrlV1AlertDialog({
	prompt,
	t,
	onOpenChange,
	onKeep,
	onRemove
}: BaseUrlV1AlertDialogProps) {
	return (
		<AlertDialog open={prompt !== null} onOpenChange={onOpenChange}>
			<AlertDialogContent>
				<AlertDialogHeader>
					<AlertDialogTitle>{t('providers.baseUrlV1Title')}</AlertDialogTitle>
					<AlertDialogDescription>
						{t('providers.baseUrlV1Description')}
					</AlertDialogDescription>
				</AlertDialogHeader>
				<AlertDialogFooter>
					<AlertDialogCancel onClick={onKeep}>
						{t('providers.baseUrlV1Keep')}
					</AlertDialogCancel>
					<AlertDialogAction onClick={onRemove}>
						{t('providers.baseUrlV1Remove')}
					</AlertDialogAction>
				</AlertDialogFooter>
			</AlertDialogContent>
		</AlertDialog>
	)
}

interface UnsavedChangesAlertDialogProps {
	open: boolean
	t: Translator
	onOpenChange: (open: boolean) => void
	onDiscard: () => void
	onSave: () => void
}

export function UnsavedChangesAlertDialog({
	open,
	t,
	onOpenChange,
	onDiscard,
	onSave
}: UnsavedChangesAlertDialogProps) {
	return (
		<AlertDialog open={open} onOpenChange={onOpenChange}>
			<AlertDialogContent>
				<AlertDialogHeader>
					<AlertDialogTitle>{t('providers.unsavedChangesTitle')}</AlertDialogTitle>
					<AlertDialogDescription>
						{t('providers.unsavedChangesDesc')}
					</AlertDialogDescription>
				</AlertDialogHeader>
				<AlertDialogFooter>
					<AlertDialogCancel>{t('common.cancel')}</AlertDialogCancel>
					<Button
						variant='outline'
						className='hover:!bg-destructive hover:!text-destructive-foreground hover:!border-destructive'
						onClick={onDiscard}
					>
						{t('providers.unsavedChangesDiscard')}
					</Button>
					<AlertDialogAction onClick={onSave}>{t('common.save')}</AlertDialogAction>
				</AlertDialogFooter>
			</AlertDialogContent>
		</AlertDialog>
	)
}

interface ModelEditorDialogProps {
	open: boolean
	model: ModelRow | null
	isDraft: boolean
	canDelete: boolean
	t: Translator
	onOpenChange: (open: boolean) => void
	onChangeModel: (value: string) => void
	onChangeRedirect: (value: string) => void
	onChangeMultiplier: (value: string) => void
	onDelete: () => void
	onSave: () => void
}

export function ModelEditorDialog({
	open,
	model,
	isDraft,
	canDelete,
	t,
	onOpenChange,
	onChangeModel,
	onChangeRedirect,
	onChangeMultiplier,
	onDelete,
	onSave
}: ModelEditorDialogProps) {
	return (
		<Dialog open={open && model !== null} onOpenChange={onOpenChange}>
			<DialogContent className='max-w-lg'>
				<DialogHeader>
					<DialogTitle>
						{isDraft ? t('providers.addModel') : t('providers.model')}
					</DialogTitle>
					<DialogDescription>
						{model?.model || t('providers.model')}
					</DialogDescription>
				</DialogHeader>

				{model && (
					<div className='space-y-3'>
						<div className='space-y-2'>
							<Label>{t('providers.model')}</Label>
							<Input value={model.model} onChange={e => onChangeModel(e.target.value)} />
						</div>

						<div className='space-y-2'>
							<Label>{t('providers.redirect')}</Label>
							<Input
								value={model.redirect}
								placeholder={t('providers.optional')}
								onChange={e => onChangeRedirect(e.target.value)}
							/>
						</div>

						<div className='space-y-2'>
							<Label>{t('providers.multiplier')}</Label>
							<Input
								type='number'
								min='0'
								step='0.1'
								value={model.multiplier}
								onChange={e => onChangeMultiplier(e.target.value)}
							/>
						</div>
					</div>
				)}

				<DialogFooter>
					<Button type='button' variant='outline' onClick={() => onOpenChange(false)}>
						{t('common.cancel')}
					</Button>
					{canDelete && (
						<Button
							type='button'
							variant='outline'
							className='text-destructive border-destructive/30 hover:text-destructive'
							onClick={onDelete}
						>
							<Trash2 className='h-4 w-4 mr-2' />
							{t('common.delete')}
						</Button>
					)}
					<Button type='button' onClick={onSave}>
						{t('common.save')}
					</Button>
				</DialogFooter>
			</DialogContent>
		</Dialog>
	)
}

interface ChannelEditorDialogProps {
	open: boolean
	channel: ChannelRow | null
	t: Translator
	isEdit: boolean
	canDelete: boolean
	globalDefaults?: {
		passive_failure_count_threshold?: number
		passive_window_seconds?: number
		passive_cooldown_seconds?: number
		passive_rate_limit_cooldown_seconds?: number
		active_probe_enabled?: boolean
		active_probe_interval_seconds?: number
		active_probe_success_threshold?: number
		active_probe_model?: string | null
	}
	providerModels: ModelRow[]
	onOpenChange: (open: boolean) => void
	onChange: (patch: Partial<ChannelRow>) => void
	onBaseUrlChange: (value: string) => void
	onBaseUrlBlur: () => void
	onFetchModels: () => void
	onDelete: () => void
	onSave: () => void
}

export function ChannelEditorDialog({
	open,
	channel,
	t,
	isEdit,
	canDelete,
	globalDefaults,
	providerModels,
	onOpenChange,
	onChange,
	onBaseUrlChange,
	onBaseUrlBlur,
	onFetchModels,
	onDelete,
	onSave
}: ChannelEditorDialogProps) {
	const inheritGlobal = t('providers.inheritGlobal')
	const providerModelNames = providerModels.map(row => row.model.trim()).filter(Boolean)
	const selectedSet = new Set(channel?.supported_models ?? [])
	const selectedValidModels = providerModelNames.filter(model => selectedSet.has(model))

	return (
		<Dialog open={open && channel !== null} onOpenChange={onOpenChange}>
			<DialogContent className='max-w-lg max-h-[85vh] flex flex-col overflow-hidden'>
				<DialogHeader>
					<DialogTitle>{t('providers.channelsSection')}</DialogTitle>
					<DialogDescription>
						{channel?.name || t('providers.addChannel')}
					</DialogDescription>
				</DialogHeader>

				{channel && (
					<div className='space-y-3 overflow-y-auto flex-1 min-h-0 pr-1'>
						<div className='space-y-2'>
							<Label>{t('common.name')}</Label>
							<Input
								value={channel.name}
								onChange={e => onChange({ name: e.target.value })}
							/>
						</div>

						<div className='space-y-2'>
							<Label>{t('providers.type')}</Label>
							<Select
								value={channel.provider_type}
								onValueChange={value =>
									onChange({ provider_type: value as ProviderType })
								}
							>
								<SelectTrigger>
									<SelectValue />
								</SelectTrigger>
								<SelectContent>
									{(Object.keys(PROVIDER_TYPE_CONFIG) as ProviderType[]).map(type => {
										const cfg = PROVIDER_TYPE_CONFIG[type]
										const Icon = cfg.icon
										return (
											<SelectItem key={type} value={type}>
												<span className='flex items-center gap-2'>
													<Icon className='h-4 w-4' />
													{cfg.label}
													<span className='text-xs opacity-60'>{cfg.path}</span>
												</span>
											</SelectItem>
										)
									})}
								</SelectContent>
							</Select>
						</div>

						<div className='space-y-2'>
							<Label>{t('providers.baseUrl')}</Label>
							<Input
								value={channel.base_url}
								autoComplete='off'
								onChange={e => onBaseUrlChange(e.target.value)}
								onBlur={onBaseUrlBlur}
							/>
						</div>

						<div className='space-y-2'>
							<Label>{t('providers.apiKey')}</Label>
							<Input
								type='password'
								autoComplete='new-password'
								placeholder={
									isEdit && channel.id ? t('providers.apiKeyUnchanged') : undefined
								}
								value={channel.api_key}
								onChange={e => onChange({ api_key: e.target.value })}
							/>
						</div>

						<div className='space-y-2'>
							<Label>{t('providers.weight')}</Label>
							<Input
								type='number'
								min='0'
								value={channel.weight}
								onChange={e => onChange({ weight: e.target.value })}
							/>
						</div>

						<Separator />

						<div className='space-y-2'>
							<div className='flex items-start justify-between gap-2'>
								<Label>{t('providers.supportedModels')}</Label>
								<div className='flex flex-wrap items-center justify-end gap-2'>
									<Button
										type='button'
										variant='outline'
										size='sm'
										onClick={onFetchModels}
									>
										<Download className='h-4 w-4 mr-2' />
										{t('providers.fetchModels')}
									</Button>
									<Button
										type='button'
										variant='outline'
										size='sm'
										onClick={() => onChange({ supported_models: providerModelNames })}
										disabled={providerModelNames.length === 0}
									>
										{t('providers.selectAll')}
									</Button>
									<Button
										type='button'
										variant='outline'
										size='sm'
										onClick={() => onChange({ supported_models: [] })}
									>
										{t('providers.deselectAll')}
									</Button>
								</div>
							</div>
							{providerModelNames.length === 0 ? (
								<p className='text-sm text-muted-foreground'>
									{t('providers.validationAtLeastOneModel')}
								</p>
							) : (
								<div className='max-h-44 overflow-y-auto rounded-md border p-2 space-y-1'>
									{providerModelNames.map(model => (
										<label
											key={model}
											className='flex min-h-8 items-center gap-2 rounded px-2 text-sm hover:bg-muted/50'
										>
											<Checkbox
												checked={selectedSet.has(model)}
												onCheckedChange={checked => {
													const next = new Set(selectedValidModels)
													if (checked) next.add(model)
													else next.delete(model)
													onChange({ supported_models: providerModelNames.filter(item => next.has(item)) })
												}}
											/>
											<span className='truncate font-mono text-xs'>{model}</span>
										</label>
									))}
								</div>
							)}
						</div>

						<div className='space-y-2'>
							<Label>{t('providers.passiveFailureCountThresholdOverride')}</Label>
							<Input
								type='number'
								min='1'
								placeholder={`${inheritGlobal} (${globalDefaults?.passive_failure_count_threshold ?? 3})`}
								value={channel.passive_failure_count_threshold_override}
								onChange={e =>
									onChange({
										passive_failure_count_threshold_override: e.target.value
									})
								}
							/>
						</div>

						<div className='space-y-2'>
							<Label>{t('providers.passiveWindowSecondsOverride')}</Label>
							<Input
								type='number'
								min='1'
								placeholder={`${inheritGlobal} (${globalDefaults?.passive_window_seconds ?? 30})`}
								value={channel.passive_window_seconds_override}
								onChange={e =>
									onChange({ passive_window_seconds_override: e.target.value })
								}
							/>
						</div>

						<div className='space-y-2'>
							<Label>{t('providers.passiveCooldownSecondsOverride')}</Label>
							<Input
								type='number'
								min='1'
								placeholder={`${inheritGlobal} (${globalDefaults?.passive_cooldown_seconds ?? 60})`}
								value={channel.passive_cooldown_seconds_override}
								onChange={e =>
									onChange({ passive_cooldown_seconds_override: e.target.value })
								}
							/>
						</div>

						<div className='space-y-2'>
							<Label>{t('providers.passiveRateLimitCooldownSecondsOverride')}</Label>
							<Input
								type='number'
								min='1'
								placeholder={`${inheritGlobal} (${globalDefaults?.passive_rate_limit_cooldown_seconds ?? 15})`}
								value={channel.passive_rate_limit_cooldown_seconds_override}
								onChange={e =>
									onChange({
										passive_rate_limit_cooldown_seconds_override: e.target.value
									})
								}
							/>
						</div>

						<Separator />

						<div className='space-y-2'>
							<Label>{t('providers.probeEnabledOverride')}</Label>
							<Select
								value={
									channel.active_probe_enabled_override === null ?
										'inherit'
									: channel.active_probe_enabled_override ?
										'enabled'
									:	'disabled'
								}
								onValueChange={value =>
									onChange({
										active_probe_enabled_override:
											value === 'inherit' ? null : value === 'enabled'
									})
								}
							>
								<SelectTrigger>
									<SelectValue />
								</SelectTrigger>
								<SelectContent>
									<SelectItem value='inherit'>
										{inheritGlobal} ({globalDefaults?.active_probe_enabled ?? true ? t('common.enabled') : t('common.disabled')})
									</SelectItem>
									<SelectItem value='enabled'>{t('common.enabled')}</SelectItem>
									<SelectItem value='disabled'>{t('common.disabled')}</SelectItem>
								</SelectContent>
							</Select>
						</div>

						<div className='space-y-2'>
							<Label>{t('providers.probeModelOverride')}</Label>
							<Input
								value={channel.active_probe_model_override}
								placeholder={
									globalDefaults?.active_probe_model?.trim() ?
										`${inheritGlobal} (${globalDefaults.active_probe_model})`
									:	t('providers.probeModelOverridePlaceholder')
								}
								onChange={e => onChange({ active_probe_model_override: e.target.value })}
							/>
						</div>

						<div className='grid grid-cols-1 sm:grid-cols-2 gap-3'>
							<div className='space-y-2'>
								<Label>{t('providers.probeIntervalOverride')}</Label>
								<Input
									type='number'
									min='1'
									placeholder={`${inheritGlobal} (${globalDefaults?.active_probe_interval_seconds ?? 30})`}
									value={channel.active_probe_interval_seconds_override}
									onChange={e => onChange({ active_probe_interval_seconds_override: e.target.value })}
								/>
							</div>
							<div className='space-y-2'>
								<Label>{t('providers.probeSuccessThresholdOverride')}</Label>
								<Input
									type='number'
									min='1'
									placeholder={`${inheritGlobal} (${globalDefaults?.active_probe_success_threshold ?? 1})`}
									value={channel.active_probe_success_threshold_override}
									onChange={e => onChange({ active_probe_success_threshold_override: e.target.value })}
								/>
							</div>
						</div>

						<div className='flex items-center gap-2'>
							<Switch
								checked={channel.enabled}
								onCheckedChange={checked => onChange({ enabled: checked })}
							/>
							<Label>{t('providers.enabled')}</Label>
						</div>
					</div>
				)}

				<DialogFooter>
					<Button type='button' variant='outline' onClick={() => onOpenChange(false)}>
						{t('common.cancel')}
					</Button>
					{canDelete && (
						<Button
							type='button'
							variant='outline'
							className='text-destructive border-destructive/30 hover:text-destructive'
							onClick={onDelete}
						>
							<Trash2 className='h-4 w-4 mr-2' />
							{t('common.delete')}
						</Button>
					)}
					<Button type='button' onClick={onSave}>
						{t('common.save')}
					</Button>
				</DialogFooter>
			</DialogContent>
		</Dialog>
	)
}
