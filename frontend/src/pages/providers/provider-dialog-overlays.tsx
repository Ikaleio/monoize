import { Trash2 } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Switch } from '@/components/ui/switch'
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
import type { ChannelRow, ModelRow } from './shared'

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
	onOpenChange: (open: boolean) => void
	onChange: (patch: Partial<ChannelRow>) => void
	onBaseUrlChange: (value: string) => void
	onBaseUrlBlur: () => void
	onDelete: () => void
	onSave: () => void
}

export function ChannelEditorDialog({
	open,
	channel,
	t,
	isEdit,
	canDelete,
	onOpenChange,
	onChange,
	onBaseUrlChange,
	onBaseUrlBlur,
	onDelete,
	onSave
}: ChannelEditorDialogProps) {
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
							<Label>{t('providers.passiveFailureThresholdOverride')}</Label>
							<Input
								type='number'
								min='1'
								placeholder={t('providers.inheritGlobal')}
								value={channel.passive_failure_threshold_override}
								onChange={e =>
									onChange({ passive_failure_threshold_override: e.target.value })
								}
							/>
						</div>

						<div className='space-y-2'>
							<Label>{t('providers.passiveCooldownSecondsOverride')}</Label>
							<Input
								type='number'
								min='1'
								placeholder={t('providers.inheritGlobal')}
								value={channel.passive_cooldown_seconds_override}
								onChange={e =>
									onChange({ passive_cooldown_seconds_override: e.target.value })
								}
							/>
						</div>

						<div className='space-y-2'>
							<Label>{t('providers.passiveWindowSecondsOverride')}</Label>
							<Input
								type='number'
								min='1'
								placeholder={t('providers.inheritGlobal')}
								value={channel.passive_window_seconds_override}
								onChange={e =>
									onChange({ passive_window_seconds_override: e.target.value })
								}
							/>
						</div>

						<div className='space-y-2'>
							<Label>{t('providers.passiveMinSamplesOverride')}</Label>
							<Input
								type='number'
								min='1'
								placeholder={t('providers.inheritGlobal')}
								value={channel.passive_min_samples_override}
								onChange={e =>
									onChange({ passive_min_samples_override: e.target.value })
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
								value={channel.passive_failure_rate_threshold_override}
								onChange={e =>
									onChange({
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
								value={channel.passive_rate_limit_cooldown_seconds_override}
								onChange={e =>
									onChange({
										passive_rate_limit_cooldown_seconds_override: e.target.value
									})
								}
							/>
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
