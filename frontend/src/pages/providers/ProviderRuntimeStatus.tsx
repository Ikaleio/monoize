import { CircleDollarSign, Clock3, RadioTower, TriangleAlert } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { BadgeOverflowList } from '@/components/BadgeOverflowList'
import { ModelBadge } from '@/components/ModelBadge'
import { StatusBadge } from '@/components/ui/status'
import type {
	ModelMetadataRecord,
	MonoizeChannel,
	ProviderModelRuntimeStatus
} from '@/lib/api'
import { statusBadge } from './shared'

function formatTimestamp(value: string | undefined, locale: string) {
	if (!value) return null
	const timestamp = new Date(value)
	if (Number.isNaN(timestamp.getTime())) return value
	return timestamp.toLocaleString(locale)
}

export function ProviderModelRuntimeBadge({
	model,
	metadata,
	status,
	highlightUnpriced
}: {
	model: string
	metadata?: ModelMetadataRecord
	status?: ProviderModelRuntimeStatus
	highlightUnpriced: boolean
}) {
	const { t } = useTranslation()
	const severity =
		status?.availability_status === 'unavailable' ? 'destructive'
		: status?.availability_status === 'degraded' ||
			status?.pricing_status !== 'complete' ||
			highlightUnpriced ?
			'warning'
		: 	'default'
	const hasDetails = severity !== 'default'
	const badge = (
		<ModelBadge
			model={model}
			provider={metadata?.models_dev_provider}
			status={severity}
			highlightUnpriced={highlightUnpriced}
		/>
	)

	if (!hasDetails || !status) return badge

	const availabilityVariant =
		status.availability_status === 'unavailable' ? 'destructive'
		: status.availability_status === 'degraded' ? 'warning'
		: 'success'

	return (
		<BadgeOverflowList
			items={[
				{
					key: model,
					collapsed: badge,
					full: (
						<div className='flex min-w-64 max-w-sm flex-col gap-3 p-2'>
							<div className='flex items-center justify-between gap-3'>
								<span className='min-w-0 truncate font-mono text-sm font-medium'>
									{model}
								</span>
								<StatusBadge variant={availabilityVariant}>
									{t(`providers.modelAvailability.${status.availability_status}`)}
								</StatusBadge>
							</div>
							<div className='flex items-start gap-2 text-sm'>
								<RadioTower className='mt-0.5 size-4 shrink-0 text-muted-foreground' />
								<div className='flex min-w-0 flex-col gap-1'>
									<span className='font-medium'>{t('providers.availability')}</span>
									<span className='text-muted-foreground'>
										{t('providers.availableChannelsCount', {
											available: status.available_channel_count,
											total: status.eligible_channel_count
										})}
									</span>
									{status.breaker_channels.length > 0 && (
										<div className='flex flex-col gap-1 text-destructive'>
											<span>{t('providers.trippedChannels')}</span>
											{status.breaker_channels.map(channel => (
												<span key={channel.channel_id} className='break-words font-mono text-sm'>
													{channel.channel_name}
													{channel.cooldown_until ?
														` · ${t('providers.until', { time: formatTimestamp(channel.cooldown_until, navigator.language) })}`
													: 	null}
												</span>
											))}
										</div>
									)}
								</div>
							</div>
							{status.pricing_status !== 'complete' && (
								<div className='flex items-start gap-2 text-sm'>
									<CircleDollarSign className='mt-0.5 size-4 shrink-0 text-warning' />
									<div className='flex min-w-0 flex-col gap-1'>
										<span className='font-medium'>{t('providers.pricingConfiguration')}</span>
										<span className='text-warning-foreground'>
											{t(`providers.modelPricing.${status.pricing_status}`)}
										</span>
										<span className='break-words font-mono text-sm text-muted-foreground'>
											{status.unpriced_channels.map(channel => channel.channel_name).join(', ')}
										</span>
									</div>
								</div>
							)}
						</div>
					)
				}
			]}
			visibleCount={1}
			popoverOnSingle
			ariaLabel={t('providers.modelStatusAria', { model })}
			contentClassName='p-1.5'
		/>
	)
}

export function ChannelRuntimeStatus({
	channel,
	perModelCircuitBreak
}: {
	channel: MonoizeChannel
	perModelCircuitBreak: boolean
}) {
	const { t } = useTranslation()
	const status = channel._health_status ?? 'healthy'
	const models =
		status === 'unhealthy' ? channel._unhealthy_models ?? []
		: status === 'probing' ? channel._probing_models ?? []
		: []
	if (status === 'healthy') return statusBadge(status, t)

	return (
		<BadgeOverflowList
			items={[
				{
					key: status,
					collapsed: statusBadge(status, t),
					full: (
						<div className='flex min-w-56 max-w-sm flex-col gap-2 p-2 text-sm'>
							<div className='flex items-center gap-2 font-medium'>
								{status === 'unhealthy' ?
									<TriangleAlert className='size-4 text-destructive' />
								: 	<RadioTower className='size-4 text-warning' />}
								{perModelCircuitBreak ?
									t(
										status === 'unhealthy' ?
											'providers.trippedModels'
										: 	'providers.probingModels'
									)
								: 	t(
										status === 'unhealthy' ?
											'providers.channelBreakerOpen'
										: 	'providers.channelProbing'
									)}
							</div>
							{perModelCircuitBreak && models.length > 0 && (
								<span className='break-words font-mono text-sm text-muted-foreground'>
									{models.join(', ')}
								</span>
							)}
							{channel._cooldown_until && (
								<span className='flex items-center gap-2 text-muted-foreground'>
									<Clock3 className='size-4' />
									{t('providers.cooldownUntil', {
										time: formatTimestamp(channel._cooldown_until, navigator.language)
									})}
								</span>
							)}
						</div>
					)
				}
			]}
			visibleCount={1}
			popoverOnSingle
			ariaLabel={t('providers.channelStatusAria', { channel: channel.name })}
			contentClassName='p-1.5'
		/>
	)
}
