import { useTranslation } from 'react-i18next'
import { BadgeOverflowList } from '@/components/BadgeOverflowList'
import { Badge } from '@/components/ui/badge'
import { cn } from '@/lib/utils'

interface GroupsBadgeProps {
	groups: string[]
	variant?: 'outline' | 'secondary'
	className?: string
}

export function GroupsBadge({
	groups,
	variant = 'outline',
	className
}: GroupsBadgeProps) {
	const { t } = useTranslation()
	if (groups.length === 0) return null

	const items = groups.map((group, index) => ({
		key: `${group}-${index}`,
		collapsed: (
			<Badge
				variant={variant}
				className={cn(
					'max-w-[10rem] shrink-0 flex-nowrap overflow-hidden font-mono text-xs',
					className
				)}
			>
				<span className='min-w-0 truncate'>{group}</span>
			</Badge>
		),
		full: (
			<Badge
				variant={variant}
				className={cn('max-w-none shrink-0 flex-nowrap font-mono text-xs', className)}
			>
				<span className='whitespace-nowrap'>{group}</span>
			</Badge>
		)
	}))

	return (
		<BadgeOverflowList
			items={items}
			visibleCount={1}
			popoverOnSingle
			ariaLabel={t('groupsBadge.groupsCount', { count: groups.length })}
		/>
	)
}
