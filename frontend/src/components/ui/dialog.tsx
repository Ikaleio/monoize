import * as React from "react"
import * as DialogPrimitive from "@radix-ui/react-dialog"
import { AnimatePresence } from "framer-motion"
import { X } from "lucide-react"

import { easings, motion } from "@/components/ui/motion"
import { cn } from "@/lib/utils"

type DialogProps = React.ComponentPropsWithoutRef<typeof DialogPrimitive.Root>

type DialogStateContextValue = {
  open: boolean
}

const DialogStateContext = React.createContext<DialogStateContextValue | null>(
  null
)

const useDialogState = () => {
  const context = React.useContext(DialogStateContext)

  if (!context) {
    throw new Error("Dialog components must be used within <Dialog>")
  }

  return context
}

const Dialog = ({
  open: openProp,
  defaultOpen,
  onOpenChange,
  children,
  ...props
}: DialogProps) => {
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
    <DialogStateContext.Provider value={{ open }}>
      <DialogPrimitive.Root
        open={open}
        defaultOpen={defaultOpen}
        onOpenChange={handleOpenChange}
        {...props}
      >
        {children}
      </DialogPrimitive.Root>
    </DialogStateContext.Provider>
  )
}

const DialogTrigger = DialogPrimitive.Trigger

const DialogPortal = DialogPrimitive.Portal

const DialogClose = DialogPrimitive.Close

const DialogOverlay = React.forwardRef<
  React.ElementRef<typeof DialogPrimitive.Overlay>,
  React.ComponentPropsWithoutRef<typeof DialogPrimitive.Overlay>
>(({ className, ...props }, ref) => (
  <DialogPrimitive.Overlay
    ref={ref}
    className={cn("fixed inset-0 z-50 bg-black/80", className)}
    {...props}
  />
))
DialogOverlay.displayName = DialogPrimitive.Overlay.displayName

const DialogContent = React.forwardRef<
  React.ElementRef<typeof DialogPrimitive.Content>,
  React.ComponentPropsWithoutRef<typeof DialogPrimitive.Content>
>(({ className, children, ...props }, ref) => {
  const { open } = useDialogState()

  return (
    <DialogPortal forceMount>
      <AnimatePresence>
        {open ? (
          <DialogPrimitive.Overlay forceMount asChild>
            <motion.div
              key="dialog-overlay"
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
          </DialogPrimitive.Overlay>
        ) : null}
      </AnimatePresence>

      <AnimatePresence>
        {open ? (
          <DialogPrimitive.Content forceMount asChild {...props}>
            <motion.div
              key="dialog-content"
              ref={ref}
              initial={{ opacity: 0, scale: 0.96, x: "-50%", y: "-47%" }}
              animate={{
                opacity: 1,
                scale: 1,
                x: "-50%",
                y: "-50%",
                transition: { duration: 0.24, ease: easings.easeOutExpo },
              }}
              exit={{
                opacity: 0,
                scale: 0.96,
                x: "-50%",
                y: "-47%",
                transition: { duration: 0.18, ease: easings.easeInOutQuart },
              }}
              className={cn(
                "fixed left-[50%] top-[50%] z-50 grid w-full max-w-lg gap-4 border bg-background p-6 shadow-lg sm:rounded-lg [&_*]:ring-offset-background",
                className
              )}
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
                className="grid gap-4"
              >
                {children}
              </motion.div>
              <DialogPrimitive.Close className="absolute right-4 top-4 rounded-sm opacity-70 ring-offset-background transition-opacity hover:opacity-100 focus:outline-none focus:ring-2 focus:ring-ring focus:ring-offset-2 disabled:pointer-events-none data-[state=open]:bg-accent data-[state=open]:text-muted-foreground">
                <X className="h-4 w-4" />
                <span className="sr-only">Close</span>
              </DialogPrimitive.Close>
            </motion.div>
          </DialogPrimitive.Content>
        ) : null}
      </AnimatePresence>
    </DialogPortal>
  )
})
DialogContent.displayName = DialogPrimitive.Content.displayName

const DialogHeader = ({
  className,
  ...props
}: React.HTMLAttributes<HTMLDivElement>) => (
  <div
    className={cn(
      "flex flex-col space-y-1.5 text-center sm:text-left",
      className
    )}
    {...props}
  />
)
DialogHeader.displayName = "DialogHeader"

const DialogFooter = ({
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
DialogFooter.displayName = "DialogFooter"

const DialogTitle = React.forwardRef<
  React.ElementRef<typeof DialogPrimitive.Title>,
  React.ComponentPropsWithoutRef<typeof DialogPrimitive.Title>
>(({ className, ...props }, ref) => (
  <DialogPrimitive.Title
    ref={ref}
    className={cn(
      "text-lg font-semibold leading-none tracking-tight",
      className
    )}
    {...props}
  />
))
DialogTitle.displayName = DialogPrimitive.Title.displayName

const DialogDescription = React.forwardRef<
  React.ElementRef<typeof DialogPrimitive.Description>,
  React.ComponentPropsWithoutRef<typeof DialogPrimitive.Description>
>(({ className, ...props }, ref) => (
  <DialogPrimitive.Description
    ref={ref}
    className={cn("text-sm text-muted-foreground", className)}
    {...props}
  />
))
DialogDescription.displayName = DialogPrimitive.Description.displayName

export {
  Dialog,
  DialogPortal,
  DialogOverlay,
  DialogClose,
  DialogTrigger,
  DialogContent,
  DialogHeader,
  DialogFooter,
  DialogTitle,
  DialogDescription,
}
