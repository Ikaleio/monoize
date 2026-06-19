import * as React from 'react'
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
	const didMountRef = React.useRef(false)
	const onOpenChangeRef = React.useRef(onOpenChange)
	const lastPointerTypeRef = React.useRef<string | null>(null)
	const suppressPointerFocusOpenRef = React.useRef(false)
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
		setPinnedOpen(!open)
	}, [clearCloseTimer, open, popoverEnabled, setPinnedOpen])

	const isTouchLikeClick = React.useCallback(() => {
		if (lastPointerTypeRef.current === 'touch') return true
		if (lastPointerTypeRef.current) return false
		if (typeof window === 'undefined') return false
		return (
			window.matchMedia('(pointer: coarse)').matches ||
			window.matchMedia('(hover: none)').matches
		)
	}, [])

	const isFinePointerClick = React.useCallback(() => {
		if (lastPointerTypeRef.current === 'touch') return false
		if (lastPointerTypeRef.current) return true
		if (typeof window === 'undefined') return true
		return (
			!window.matchMedia('(pointer: coarse)').matches &&
			!window.matchMedia('(hover: none)').matches
		)
	}, [])

	const handlePointerEnter = React.useCallback(
		(event: React.PointerEvent) => {
			if (event.pointerType === 'touch') return
			openPopover()
		},
		[openPopover]
	)

	React.useEffect(() => {
		onOpenChangeRef.current = onOpenChange
	}, [onOpenChange])

	React.useEffect(() => {
		if (!didMountRef.current) {
			didMountRef.current = true
			return
		}
		onOpenChangeRef.current?.(open)
	}, [open])

	React.useEffect(() => {
		return () => {
			clearCloseTimer()
		}
	}, [clearCloseTimer])

	if (items.length === 0) return null

	const preview = (
		<span
			className={cn(
				'inline-flex min-w-0 max-w-full flex-nowrap items-center gap-1 overflow-hidden whitespace-nowrap align-middle',
				className,
				triggerClassName
			)}
		>
			{visibleItems.map(item => (
				<span
					key={item.key}
					className='inline-flex min-w-0 max-w-full shrink overflow-hidden whitespace-nowrap'
				>
					{item.collapsed}
				</span>
			))}
			{hiddenCount > 0 && (
				<Badge
					variant='secondary'
					className={cn(
						'h-6 shrink-0 flex-nowrap rounded-full border border-border/60 bg-secondary/80 px-2 font-mono text-xs backdrop-blur-sm',
						countClassName
					)}
				>
					+{hiddenCount}
				</Badge>
			)}
		</span>
	)

	if (!popoverEnabled) {
		return (
			<span
				className={cn(
					'inline-flex min-w-0 max-w-full flex-nowrap items-center gap-1 overflow-hidden whitespace-nowrap align-middle',
					className
				)}
			>
				{items.map(item => (
					<span
						key={item.key}
						className='inline-flex min-w-0 max-w-full shrink overflow-hidden whitespace-nowrap'
					>
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
					className='inline-flex min-w-0 max-w-full cursor-default flex-nowrap whitespace-nowrap align-middle'
					onPointerDown={event => {
						lastPointerTypeRef.current = event.pointerType
						suppressPointerFocusOpenRef.current =
							event.pointerType !== 'touch' && isFinePointerClick()
					}}
					onPointerEnter={handlePointerEnter}
					onPointerLeave={scheduleClose}
					onClick={event => {
						event.preventDefault()
						event.stopPropagation()
						if (!isTouchLikeClick()) {
							pinnedOpenRef.current = false
							return
						}
						togglePinnedOpen()
					}}
					onFocus={() => {
						if (suppressPointerFocusOpenRef.current) return
						openPopover()
					}}
					onBlur={() => {
						suppressPointerFocusOpenRef.current = false
						scheduleClose()
					}}
					onKeyDown={event => {
						if (event.key === 'Enter' || event.key === ' ') {
							event.preventDefault()
							event.stopPropagation()
							openPopover()
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
					'w-auto max-w-[min(28rem,calc(100vw-2rem))] rounded-lg border border-border/70 bg-popover/70 p-1.5 shadow-2xl backdrop-blur-xl supports-[backdrop-filter]:bg-popover/65',
					contentClassName
				)}
				onOpenAutoFocus={event => event.preventDefault()}
				onPointerEnter={handlePointerEnter}
				onPointerLeave={scheduleClose}
				onClick={() => setPinnedOpen(false)}
			>
				<div
					className={cn(
						'flex max-h-[18rem] max-w-full flex-col items-start gap-1 overflow-auto overscroll-contain p-0.5 [scrollbar-width:thin]',
						listClassName
					)}
					role='list'
				>
					{items.map(item => (
						<div
							key={item.key}
							className='min-w-max max-w-none shrink-0 rounded-md px-0.5 py-0.5 transition-colors hover:bg-accent/50'
							role='listitem'
						>
							{item.full ?? item.collapsed}
						</div>
					))}
				</div>
			</PopoverContent>
		</Popover>
	)
}
