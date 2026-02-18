"use client"

import * as React from "react"
import * as TooltipPrimitive from "@radix-ui/react-tooltip"

import { cn } from "@/lib/utils"

const TooltipProvider = TooltipPrimitive.Provider

// Radix Tooltip ignores touch events by design (hover-only).
// On touch devices we take over via controlled open state and click handlers,
// suppressing Radix's internal pointer/focus handlers via preventDefault
// (Radix's composeEventHandlers skips its handler when defaultPrevented).

type TouchTooltipCtx = {
  open: boolean
  setOpen: React.Dispatch<React.SetStateAction<boolean>>
  instanceId: string
} | null

const TouchTooltipContext = React.createContext<TouchTooltipCtx>(null)

function useIsTouchDevice() {
  const [isTouch, setIsTouch] = React.useState(false)
  React.useEffect(() => {
    const mq = window.matchMedia("(pointer: coarse)")
    setIsTouch(mq.matches)
    const handler = (e: MediaQueryListEvent) => setIsTouch(e.matches)
    mq.addEventListener("change", handler)
    return () => mq.removeEventListener("change", handler)
  }, [])
  return isTouch
}

const Tooltip = (props: React.ComponentPropsWithoutRef<typeof TooltipPrimitive.Root>) => {
  const isTouch = useIsTouchDevice()
  const [open, setOpen] = React.useState(false)
  const instanceId = React.useId()

  if (!isTouch) {
    return <TooltipPrimitive.Root {...props} />
  }

  return (
    <TouchTooltipContext.Provider value={{ open, setOpen, instanceId }}>
      <TooltipPrimitive.Root
        {...props}
        open={props.open ?? open}
        onOpenChange={(v) => {
          setOpen(v)
          props.onOpenChange?.(v)
        }}
        delayDuration={0}
      />
    </TouchTooltipContext.Provider>
  )
}

const TooltipTrigger = React.forwardRef<
  React.ElementRef<typeof TooltipPrimitive.Trigger>,
  React.ComponentPropsWithoutRef<typeof TooltipPrimitive.Trigger>
>((props, ref) => {
  const ctx = React.useContext(TouchTooltipContext)

  if (!ctx) {
    return <TooltipPrimitive.Trigger ref={ref} {...props} />
  }

  const suppress = (e: { preventDefault: () => void }) => { e.preventDefault() }

  return (
    <TooltipPrimitive.Trigger
      ref={ref}
      {...props}
      onPointerDown={suppress}
      onPointerMove={suppress}
      onFocus={suppress}
      onBlur={suppress}
      onClick={(e) => {
        // Tag the native event so the document-level close handler can
        // distinguish "tapped own trigger" from "tapped outside".
        ;(e.nativeEvent as any).__tooltipId = ctx.instanceId
        ctx.setOpen(prev => !prev)
        props.onClick?.(e)
      }}
    />
  )
})
TooltipTrigger.displayName = "TooltipTrigger"

const TooltipContent = React.forwardRef<
  React.ElementRef<typeof TooltipPrimitive.Content>,
  React.ComponentPropsWithoutRef<typeof TooltipPrimitive.Content>
>(({ className, sideOffset = 4, ...props }, ref) => {
  const ctx = React.useContext(TouchTooltipContext)
  const contentRef = React.useRef<HTMLDivElement | null>(null)

  // Close on outside tap:
  // 1. Skip if click came from this tooltip's own trigger (tagged with instanceId).
  // 2. Skip if click landed inside tooltip content.
  // 3. Otherwise close — also handles "tap another tooltip's trigger".
  React.useEffect(() => {
    if (!ctx?.open) return
    const handler = (e: MouseEvent) => {
      if ((e as any).__tooltipId === ctx.instanceId) return
      if (contentRef.current?.contains(e.target as Node)) return
      ctx.setOpen(false)
    }
    document.addEventListener("click", handler)
    return () => document.removeEventListener("click", handler)
  }, [ctx, ctx?.open, ctx?.instanceId])

  return (
    <TooltipPrimitive.Portal>
      <TooltipPrimitive.Content
        ref={(node) => {
          contentRef.current = node
          if (typeof ref === "function") ref(node)
          else if (ref) (ref as React.MutableRefObject<HTMLDivElement | null>).current = node
        }}
        sideOffset={sideOffset}
        className={cn(
          "z-50 overflow-hidden rounded-md border bg-popover px-3 py-1.5 text-sm text-popover-foreground shadow-md animate-in fade-in-0 zoom-in-95 data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=closed]:zoom-out-95 data-[side=bottom]:slide-in-from-top-2 data-[side=left]:slide-in-from-right-2 data-[side=right]:slide-in-from-left-2 data-[side=top]:slide-in-from-bottom-2 origin-[--radix-tooltip-content-transform-origin]",
          className
        )}
        {...props}
      />
    </TooltipPrimitive.Portal>
  )
})
TooltipContent.displayName = TooltipPrimitive.Content.displayName

export { Tooltip, TooltipTrigger, TooltipContent, TooltipProvider }
