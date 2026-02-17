import * as React from "react"
import * as SheetPrimitive from "@radix-ui/react-dialog"
import { cva, type VariantProps } from "class-variance-authority"
import { AnimatePresence } from "framer-motion"
import { X } from "lucide-react"

import { easings, motion } from "@/components/ui/motion"
import { cn } from "@/lib/utils"

type SheetProps = React.ComponentPropsWithoutRef<typeof SheetPrimitive.Root>

type SheetStateContextValue = {
  open: boolean
}

const SheetStateContext = React.createContext<SheetStateContextValue | null>(
  null
)

const useSheetState = () => {
  const context = React.useContext(SheetStateContext)

  if (!context) {
    throw new Error("Sheet components must be used within <Sheet>")
  }

  return context
}

const Sheet = ({
  open: openProp,
  defaultOpen,
  onOpenChange,
  children,
  ...props
}: SheetProps) => {
  const isControlled = openProp !== undefined
  const [uncontrolledOpen, setUncontrolledOpen] = React.useState(
    defaultOpen ?? false
  )
  const open = isControlled ? openProp : uncontrolledOpen

  const handleOpenChange = React.useCallback(
    (nextOpen: boolean) => {
      if (!isControlled) {
        setUncontrolledOpen(nextOpen)
      }

      onOpenChange?.(nextOpen)
    },
    [isControlled, onOpenChange]
  )

  return (
    <SheetStateContext.Provider value={{ open }}>
      <SheetPrimitive.Root
        open={open}
        defaultOpen={defaultOpen}
        onOpenChange={handleOpenChange}
        {...props}
      >
        {children}
      </SheetPrimitive.Root>
    </SheetStateContext.Provider>
  )
}

const SheetTrigger = SheetPrimitive.Trigger

const SheetClose = SheetPrimitive.Close

const SheetPortal = SheetPrimitive.Portal

const SheetOverlay = React.forwardRef<
  React.ElementRef<typeof SheetPrimitive.Overlay>,
  React.ComponentPropsWithoutRef<typeof SheetPrimitive.Overlay>
>(({ className, ...props }, ref) => (
  <SheetPrimitive.Overlay
    className={cn("fixed inset-0 z-50 bg-black/80", className)}
    {...props}
    ref={ref}
  />
))
SheetOverlay.displayName = SheetPrimitive.Overlay.displayName

const sheetVariants = cva(
  "fixed z-50 gap-4 bg-background p-6 shadow-lg",
  {
    variants: {
      side: {
        top: "inset-x-0 top-0 border-b",
        bottom:
          "inset-x-0 bottom-0 border-t",
        left: "inset-y-0 left-0 h-full w-3/4 border-r sm:max-w-sm",
        right:
          "inset-y-0 right-0 h-full w-3/4  border-l sm:max-w-sm",
      },
    },
    defaultVariants: {
      side: "right",
    },
  }
)

interface SheetContentProps
  extends React.ComponentPropsWithoutRef<typeof SheetPrimitive.Content>,
    VariantProps<typeof sheetVariants> {}

const getSheetMotionBySide = (side: NonNullable<SheetContentProps["side"]>) => {
  switch (side) {
    case "top":
      return {
        initial: { y: "-100%" },
        animate: {
          y: 0,
          transition: { duration: 0.26, ease: easings.easeOutExpo },
        },
        exit: {
          y: "-100%",
          transition: { duration: 0.2, ease: easings.easeInOutQuart },
        },
      }
    case "bottom":
      return {
        initial: { y: "100%" },
        animate: {
          y: 0,
          transition: { duration: 0.26, ease: easings.easeOutExpo },
        },
        exit: {
          y: "100%",
          transition: { duration: 0.2, ease: easings.easeInOutQuart },
        },
      }
    case "left":
      return {
        initial: { x: "-100%" },
        animate: {
          x: 0,
          transition: { duration: 0.26, ease: easings.easeOutExpo },
        },
        exit: {
          x: "-100%",
          transition: { duration: 0.2, ease: easings.easeInOutQuart },
        },
      }
    case "right":
    default:
      return {
        initial: { x: "100%" },
        animate: {
          x: 0,
          transition: { duration: 0.26, ease: easings.easeOutExpo },
        },
        exit: {
          x: "100%",
          transition: { duration: 0.2, ease: easings.easeInOutQuart },
        },
      }
  }
}

const SheetContent = React.forwardRef<
  React.ElementRef<typeof SheetPrimitive.Content>,
  SheetContentProps
>(({ side = "right", className, children, ...props }, ref) => {
  const { open } = useSheetState()
  const resolvedSide = side ?? "right"
  const motionBySide = React.useMemo(
    () => getSheetMotionBySide(resolvedSide),
    [resolvedSide]
  )

  return (
    <SheetPortal forceMount>
      <AnimatePresence>
        {open ? (
          <SheetPrimitive.Overlay forceMount asChild>
            <motion.div
              key="sheet-overlay"
              initial={{ opacity: 0 }}
              animate={{
                opacity: 1,
                transition: { duration: 0.22, ease: easings.easeOutExpo },
              }}
              exit={{
                opacity: 0,
                transition: { duration: 0.16, ease: easings.easeInOutQuart },
              }}
              className="fixed inset-0 z-50 bg-black/80"
            />
          </SheetPrimitive.Overlay>
        ) : null}
      </AnimatePresence>

      <AnimatePresence>
        {open ? (
          <SheetPrimitive.Content forceMount asChild {...props}>
            <motion.div
              key={`sheet-content-${side}`}
              ref={ref}
              initial={motionBySide.initial}
              animate={motionBySide.animate}
              exit={motionBySide.exit}
              className={cn(sheetVariants({ side: resolvedSide }), className)}
            >
              <motion.div
                initial={{ opacity: 0, y: 8 }}
                animate={{
                  opacity: 1,
                  y: 0,
                  transition: {
                    duration: 0.2,
                    ease: easings.easeOutExpo,
                    delay: 0.03,
                  },
                }}
                exit={{
                  opacity: 0,
                  y: 4,
                  transition: { duration: 0.16, ease: easings.easeInOutQuart },
                }}
                className="h-full"
              >
                {children}
              </motion.div>
              <SheetPrimitive.Close className="absolute right-4 top-4 rounded-sm opacity-70 ring-offset-background transition-opacity hover:opacity-100 focus:outline-none focus:ring-2 focus:ring-ring focus:ring-offset-2 disabled:pointer-events-none data-[state=open]:bg-secondary">
                <X className="h-4 w-4" />
                <span className="sr-only">Close</span>
              </SheetPrimitive.Close>
            </motion.div>
          </SheetPrimitive.Content>
        ) : null}
      </AnimatePresence>
    </SheetPortal>
  )
})
SheetContent.displayName = SheetPrimitive.Content.displayName

const SheetHeader = ({
  className,
  ...props
}: React.HTMLAttributes<HTMLDivElement>) => (
  <div
    className={cn(
      "flex flex-col space-y-2 text-center sm:text-left",
      className
    )}
    {...props}
  />
)
SheetHeader.displayName = "SheetHeader"

const SheetFooter = ({
  className,
  ...props
}: React.HTMLAttributes<HTMLDivElement>) => (
  <div
    className={cn(
      "flex flex-col-reverse sm:flex-row sm:justify-end sm:space-x-2",
      className
    )}
    {...props}
  />
)
SheetFooter.displayName = "SheetFooter"

const SheetTitle = React.forwardRef<
  React.ElementRef<typeof SheetPrimitive.Title>,
  React.ComponentPropsWithoutRef<typeof SheetPrimitive.Title>
>(({ className, ...props }, ref) => (
  <SheetPrimitive.Title
    ref={ref}
    className={cn("text-lg font-semibold text-foreground", className)}
    {...props}
  />
))
SheetTitle.displayName = SheetPrimitive.Title.displayName

const SheetDescription = React.forwardRef<
  React.ElementRef<typeof SheetPrimitive.Description>,
  React.ComponentPropsWithoutRef<typeof SheetPrimitive.Description>
>(({ className, ...props }, ref) => (
  <SheetPrimitive.Description
    ref={ref}
    className={cn("text-sm text-muted-foreground", className)}
    {...props}
  />
))
SheetDescription.displayName = SheetPrimitive.Description.displayName

export {
  Sheet,
  SheetPortal,
  SheetOverlay,
  SheetTrigger,
  SheetClose,
  SheetContent,
  SheetHeader,
  SheetFooter,
  SheetTitle,
  SheetDescription,
}
