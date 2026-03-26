import { Layers } from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import {
	Tooltip,
	TooltipContent,
	TooltipProvider,
	TooltipTrigger
} from '@/components/ui/tooltip'
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
	if (groups.length === 0) return null

	if (groups.length === 1) {
		return (
			<Badge variant={variant} className={cn('font-mono text-xs', className)}>
				{groups[0]}
			</Badge>
		)
	}

	return (
		<TooltipProvider>
			<Tooltip>
				<TooltipTrigger asChild>
					<span className='inline-flex'>
						<Badge
							variant={variant}
							className={cn('gap-1 font-mono text-xs', className)}
						>
							<Layers className='h-3 w-3' />
							{groups.length} groups
						</Badge>
					</span>
				</TooltipTrigger>
				<TooltipContent>
					<div className='flex flex-col gap-1'>
						{groups.map(group => (
							<span key={group} className='font-mono text-xs'>
								{group}
							</span>
						))}
					</div>
				</TooltipContent>
			</Tooltip>
		</TooltipProvider>
	)
}
