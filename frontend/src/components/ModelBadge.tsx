import { Badge } from '@/components/ui/badge'
import { cn } from '@/lib/utils'
import { normalizeMultiplier } from '@/lib/exact-decimal'
import { ModelIcon } from '@/components/ModelIcon'

export interface ModelBadgeProps {
	model: string
	provider?: string | null
	multiplier?: number | string
	redirect?: string | null
	detailTarget?: string
	showDetails?: boolean
	highlightUnpriced?: boolean
	truncateModelText?: boolean
	className?: string
}

export function ModelBadge({
	model,
	provider,
	multiplier = '1',
	redirect,
	detailTarget,
	showDetails = true,
	highlightUnpriced = false,
	truncateModelText = true,
	className
}: ModelBadgeProps) {
	const resolvedTarget = (detailTarget ?? redirect ?? model).trim()
	const normalizedMultiplier = normalizeMultiplier(String(multiplier))
	const hasCustomMultiplier = normalizedMultiplier == null || normalizedMultiplier !== '1'
	const hasRedirectTarget =
		resolvedTarget.length > 0 && resolvedTarget !== model
	const shouldRenderDetails =
		showDetails && (hasCustomMultiplier || hasRedirectTarget)

	return (
		<Badge
			variant='secondary'
			className={cn(
				'h-7 max-w-full shrink-0 flex-nowrap gap-1.5 overflow-hidden border px-2 py-1 font-mono text-xs whitespace-nowrap transition-all',
				highlightUnpriced ?
					'border-warning-border bg-warning-soft text-warning-foreground hover:bg-warning-soft/80'
				:	'bg-sidebar-accent/40 hover:bg-sidebar-accent text-foreground border-transparent hover:border-sidebar-border',
				className
			)}
		>
			<ModelIcon
				model={model}
				provider={provider}
				className='h-3.5 w-3.5 shrink-0'
			/>
			<span
				className={cn(
					'min-w-0',
					truncateModelText ? 'max-w-[220px] truncate' : 'whitespace-nowrap'
				)}
				title={model}
			>
				{model}
			</span>
			{shouldRenderDetails && (
				<span
					className={cn(
						'min-w-0 text-[11px] opacity-60',
						truncateModelText ? 'max-w-[160px] truncate' : 'whitespace-nowrap'
					)}
					title={`[${[
						hasCustomMultiplier ? `${multiplier}x` : null,
						hasRedirectTarget ? resolvedTarget : null
					]
						.filter(Boolean)
						.join(', ')}]`}
				>
					[
					{hasCustomMultiplier && (
						<span className='opacity-80'>{multiplier}x</span>
					)}
					{hasCustomMultiplier && hasRedirectTarget && (
						<span className='mx-1'>,</span>
					)}
					{hasRedirectTarget && (
						<span className='opacity-80'>{resolvedTarget}</span>
					)}
					]
				</span>
			)}
		</Badge>
	)
}
