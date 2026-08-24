import * as React from 'react'
import { cn } from '@/lib/utils'

/**
 * Shared stacked model badge list container (spec PL19a).
 *
 * Renders a bounded, vertically scrollable, wrapping badge collection with
 * identical border, padding, gap, and max-height on every surface that shows
 * a stacked model list (provider-card overview and the Channel model editor).
 */
export interface StackedModelListProps extends React.HTMLAttributes<HTMLDivElement> {
	/** Class overrides for the inner wrapping flex row (e.g. custom max-height). */
	listClassName?: string
}

export function StackedModelList({
	className,
	listClassName,
	children,
	...props
}: StackedModelListProps) {
	return (
		<div
			className={cn('rounded-lg border bg-muted/10 p-3', className)}
			{...props}
		>
			<div
				className={cn(
					'flex max-h-52 flex-wrap content-start gap-1.5 overflow-y-auto',
					listClassName
				)}
			>
				{children}
			</div>
		</div>
	)
}
