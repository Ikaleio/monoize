import { useReducedMotionPreference } from "@/hooks/use-reduced-motion"
import { useTranslation } from "react-i18next"
import * as React from "react"
import * as DialogPrimitive from "@radix-ui/react-dialog"
import { AnimatePresence } from "framer-motion"
import { X } from "lucide-react"

import { easings, motion } from "@/components/ui/motion"
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog"
import { buttonVariants } from "@/components/ui/button"
import { useReturnFocus } from "@/components/ui/use-return-focus"
import { cn } from "@/lib/utils"

type DialogProps = React.ComponentPropsWithoutRef<typeof DialogPrimitive.Root>

type DialogStateContextValue = {
  open: boolean
  onOpenChange: (open: boolean) => void
}

const DialogStateContext = React.createContext<DialogStateContextValue | null>(
  null,
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
    defaultOpen ?? false,
  )
  const open = isControlled ? openProp : uncontrolledOpen

  const handleOpenChange = React.useCallback(
    (nextOpen: boolean) => {
      if (!isControlled) {
        setUncontrolledOpen(nextOpen)
      }

      onOpenChange?.(nextOpen)
    },
    [isControlled, onOpenChange],
  )

  return (
    <DialogStateContext.Provider value={{ open, onOpenChange: handleOpenChange }}>
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

type DialogContentProps = React.ComponentPropsWithoutRef<
  typeof DialogPrimitive.Content
> & {
  /**
   * While true, outside click, Escape and the close button ask before discarding
   * unsaved edits instead of closing the dialog.
   */
  dirty?: boolean
}

const DialogContent = React.forwardRef<
  React.ElementRef<typeof DialogPrimitive.Content>,
  DialogContentProps
>(
  (
    {
      className,
      children,
      dirty = false,
      onOpenAutoFocus,
      onCloseAutoFocus,
      onEscapeKeyDown,
      onInteractOutside,
      ...props
    },
    ref,
  ) => {
  const { open, onOpenChange } = useDialogState()
  const reduced = useReducedMotionPreference()
  const { t } = useTranslation()
  const [confirmDiscardOpen, setConfirmDiscardOpen] = React.useState(false)
  const returnFocus = useReturnFocus(onOpenAutoFocus, onCloseAutoFocus)

  React.useEffect(() => {
    if (!open) setConfirmDiscardOpen(false)
  }, [open])
  const touchScrollStyle: React.CSSProperties = {
    WebkitOverflowScrolling: "touch",
  }

  const interceptDismiss = (event: Event) => {
    if (!dirty || event.defaultPrevented) return
    event.preventDefault()
    setConfirmDiscardOpen(true)
  }

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
              className="fixed inset-0 z-50 bg-black/50 backdrop-blur-[2px]"
            />
          </DialogPrimitive.Overlay>
        ) : null}
      </AnimatePresence>

      <AnimatePresence>
        {open ? (
          <div className="fixed inset-0 z-50 overflow-y-auto overscroll-contain p-4 sm:p-6">
            <div className="flex min-h-full items-start justify-center sm:items-center">
              <DialogPrimitive.Content
                forceMount
                asChild
                {...props}
                onOpenAutoFocus={returnFocus.onOpenAutoFocus}
                onCloseAutoFocus={returnFocus.onCloseAutoFocus}
                onEscapeKeyDown={(event) => {
                  onEscapeKeyDown?.(event)
                  interceptDismiss(event)
                }}
                onInteractOutside={(event) => {
                  // Runs after onPointerDownOutside; modal dialogs already prevent focus-outside.
                  onInteractOutside?.(event)
                  interceptDismiss(event)
                }}
              >
                <motion.div
                  key="dialog-content"
                  ref={ref}
                  initial={
                    reduced
                      ? { opacity: 0 }
                      : { opacity: 0, scale: 0.96, y: 12 }
                  }
                  animate={{
                    opacity: 1,
                    scale: 1,
                    y: 0,
                    transition: { duration: 0.24, ease: easings.easeOutExpo },
                  }}
                  exit={{
                    opacity: 0,
                    scale: reduced ? 1 : 0.96,
                    y: reduced ? 0 : 12,
                    transition: {
                      duration: 0.18,
                      ease: easings.easeInOutQuart,
                    },
                  }}
                  style={touchScrollStyle}
                  className={cn(
                    "relative z-50 flex w-full max-w-lg flex-col overflow-y-auto overscroll-contain rounded-lg border bg-background p-6 shadow-lg [&_*]:ring-offset-background",
                    className,
                  )}
                >
                  <motion.div
                    initial={reduced ? { opacity: 0 } : { opacity: 0, y: 8 }}
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
                      y: reduced ? 0 : 4,
                      transition: {
                        duration: 0.16,
                        ease: easings.easeInOutQuart,
                      },
                    }}
                    className="flex h-full min-h-0 flex-1 flex-col gap-4 overflow-hidden"
                  >
                    {children}
                  </motion.div>
                  <DialogPrimitive.Close
                    onClick={(event) => {
                      if (!dirty) return
                      event.preventDefault()
                      setConfirmDiscardOpen(true)
                    }}
                    className="absolute right-2 top-2 inline-flex size-11 items-center justify-center rounded-sm sm:size-9 opacity-70 ring-offset-background transition-opacity hover:opacity-100 focus:outline-none focus:ring-2 focus:ring-ring focus:ring-offset-2 disabled:pointer-events-none data-[state=open]:bg-accent data-[state=open]:text-muted-foreground"
                  >
                    <X className="h-4 w-4" />
                    <span className="sr-only">{t("common.close")}</span>
                  </DialogPrimitive.Close>
                </motion.div>
              </DialogPrimitive.Content>
            </div>
          </div>
        ) : null}
      </AnimatePresence>
      <AlertDialog
        open={open && confirmDiscardOpen}
        onOpenChange={setConfirmDiscardOpen}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("guards.discardTitle")}</AlertDialogTitle>
            <AlertDialogDescription>
              {t("guards.discardDescription")}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("guards.keepEditing")}</AlertDialogCancel>
            <AlertDialogAction
              className={buttonVariants({ variant: "destructive" })}
              onClick={() => onOpenChange(false)}
            >
              {t("guards.discard")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
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
      className,
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
      className,
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
      "pr-8 text-lg font-semibold leading-none tracking-tight",
      className,
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
