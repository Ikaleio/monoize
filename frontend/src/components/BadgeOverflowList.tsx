import * as React from 'react'
import { Layers } from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import {
	Popover,
	PopoverContent,
	PopoverTrigger
} from '@/components/ui/popover'
import { cn } from '@/lib/utils'

export interface BadgeOverflowListItem {
	key: React.Key
	collapsed: React.ReactNode
	direct?: React.ReactNode
	full?: React.ReactNode
}

interface BadgeOverflowListProps {
	items: BadgeOverflowListItem[]
	visibleCount?: number
	popoverOnSingle?: boolean
	ariaLabel?: string
	className?: string
	triggerClassName?: string
	contentClassName?: string
	listClassName?: string
	countClassName?: string
	align?: React.ComponentProps<typeof PopoverContent>['align']
	side?: React.ComponentProps<typeof PopoverContent>['side']
	onOpenChange?: (open: boolean) => void
}

export function BadgeOverflowList({
	items,
	visibleCount = 1,
	popoverOnSingle = false,
	ariaLabel,
	className,
	triggerClassName,
	contentClassName,
	listClassName,
	countClassName,
	align = 'start',
	side = 'bottom',
	onOpenChange
}: BadgeOverflowListProps) {
	const [open, setOpen] = React.useState(false)
	const pinnedOpenRef = React.useRef(false)
	const didNotifyOpenRef = React.useRef(false)
	const openRef = React.useRef(false)
	const closeTimerRef = React.useRef<ReturnType<typeof setTimeout> | null>(null)
	const safeVisibleCount = Math.max(1, visibleCount)
	const visibleItems = items.slice(0, safeVisibleCount)
	const hiddenCount = Math.max(0, items.length - safeVisibleCount)
	const popoverEnabled =
		items.length > safeVisibleCount || (popoverOnSingle && items.length > 0)

	const clearCloseTimer = React.useCallback(() => {
		if (closeTimerRef.current) {
			clearTimeout(closeTimerRef.current)
			closeTimerRef.current = null
		}
	}, [])

	const openPopover = React.useCallback(() => {
		if (!popoverEnabled) return
		clearCloseTimer()
		setOpen(true)
	}, [clearCloseTimer, popoverEnabled])

	const scheduleClose = React.useCallback(() => {
		if (!popoverEnabled || pinnedOpenRef.current) return
		clearCloseTimer()
		closeTimerRef.current = setTimeout(() => setOpen(false), 120)
	}, [clearCloseTimer, popoverEnabled])

	const setPinnedOpen = React.useCallback((next: boolean) => {
		pinnedOpenRef.current = next
		setOpen(next)
	}, [])

	const togglePinnedOpen = React.useCallback(() => {
		if (!popoverEnabled) return
		clearCloseTimer()
		setPinnedOpen(!(open && pinnedOpenRef.current))
	}, [clearCloseTimer, open, popoverEnabled, setPinnedOpen])

	React.useEffect(() => {
		openRef.current = open
		if (didNotifyOpenRef.current) {
			onOpenChange?.(open)
			return
		}
		didNotifyOpenRef.current = true
	}, [onOpenChange, open])

	React.useEffect(() => {
		return () => {
			clearCloseTimer()
			if (openRef.current) onOpenChange?.(false)
		}
	}, [clearCloseTimer, onOpenChange])

	if (items.length === 0) return null

	const preview = (
		<span
			className={cn(
				'inline-flex min-w-0 max-w-full items-center gap-1 overflow-hidden whitespace-nowrap align-middle',
				className,
				triggerClassName
			)}
		>
			{visibleItems.map(item => (
				<span
					key={item.key}
					className='inline-flex min-w-0 max-w-full shrink overflow-hidden'
				>
					{item.collapsed}
				</span>
			))}
			{hiddenCount > 0 && (
				<Badge
					variant='secondary'
					className={cn('shrink-0 gap-1 px-2 font-mono text-xs', countClassName)}
				>
					<Layers className='h-3 w-3 shrink-0' />
					+{hiddenCount}
				</Badge>
			)}
		</span>
	)

	if (!popoverEnabled) {
		return (
			<span
				className={cn(
					'inline-flex min-w-0 max-w-full items-center gap-1 overflow-hidden whitespace-nowrap align-middle',
					className
				)}
			>
				{items.map(item => (
					<span key={item.key} className='inline-flex min-w-0 max-w-full shrink'>
						{item.direct ?? item.full ?? item.collapsed}
					</span>
				))}
			</span>
		)
	}

	return (
		<Popover
			open={open}
			onOpenChange={next => {
				if (!next) pinnedOpenRef.current = false
				setOpen(next)
			}}
		>
			<PopoverTrigger asChild>
				<span
					role='button'
					tabIndex={0}
					aria-label={ariaLabel}
					className='inline-flex min-w-0 max-w-full cursor-default'
					onPointerEnter={openPopover}
					onPointerLeave={scheduleClose}
					onClick={event => {
						event.preventDefault()
						event.stopPropagation()
						togglePinnedOpen()
					}}
					onFocus={openPopover}
					onBlur={scheduleClose}
					onKeyDown={event => {
						if (event.key === 'Enter' || event.key === ' ') {
							event.preventDefault()
							event.stopPropagation()
							togglePinnedOpen()
						}
						if (event.key === 'Escape') {
							setPinnedOpen(false)
						}
					}}
				>
					{preview}
				</span>
			</PopoverTrigger>
			<PopoverContent
				side={side}
				align={align}
				className={cn(
					'w-auto max-w-[min(28rem,calc(100vw-2rem))] p-2',
					contentClassName
				)}
				onOpenAutoFocus={event => event.preventDefault()}
				onPointerEnter={openPopover}
				onPointerLeave={scheduleClose}
				onClick={() => setPinnedOpen(false)}
			>
				<div
					className={cn(
						'flex max-h-[18rem] max-w-full flex-col items-start gap-1 overflow-auto',
						listClassName
					)}
				>
					{items.map(item => (
						<div key={item.key} className='max-w-none shrink-0'>
							{item.full ?? item.collapsed}
						</div>
					))}
				</div>
			</PopoverContent>
		</Popover>
	)
}
