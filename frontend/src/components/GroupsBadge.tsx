import { useMemo } from 'react'
import { useTranslation } from 'react-i18next'
import { BadgeOverflowList } from '@/components/BadgeOverflowList'
import { Badge } from '@/components/ui/badge'
import { useDashboardGroups } from '@/lib/swr'
import { cn } from '@/lib/utils'

interface GroupsBadgeProps {
	/** Ordered registry ids; unknown ids fall back to a truncated id label. */
	groupIds: string[]
	variant?: 'outline' | 'secondary'
	className?: string
}

export function GroupsBadge({
	groupIds,
	variant = 'outline',
	className
}: GroupsBadgeProps) {
	const { t } = useTranslation()
	const { data: groups = [] } = useDashboardGroups(groupIds.length > 0)
	const nameById = useMemo(
		() => new Map(groups.map(group => [group.id, group.name])),
		[groups]
	)
	if (groupIds.length === 0) return null

	const items = groupIds.map((groupId, index) => {
		const label = nameById.get(groupId) ?? `${groupId.slice(0, 8)}…`
		return {
			key: `${groupId}-${index}`,
			collapsed: (
				<Badge
					variant={variant}
					className={cn(
						'max-w-[10rem] shrink-0 flex-nowrap overflow-hidden text-xs',
						className
					)}
				>
					<span className='min-w-0 truncate'>{label}</span>
				</Badge>
			),
			full: (
				<Badge
					variant={variant}
					className={cn('max-w-none shrink-0 flex-nowrap text-xs', className)}
				>
					<span className='whitespace-nowrap'>{label}</span>
				</Badge>
			)
		}
	})

	return (
		<BadgeOverflowList
			items={items}
			visibleCount={1}
			popoverOnSingle
			ariaLabel={t('groupsBadge.groupsCount', { count: groupIds.length })}
		/>
	)
}
