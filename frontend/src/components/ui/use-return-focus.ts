import { useCallback, useRef } from "react"

type AutoFocusHandler = (event: Event) => void

/**
 * Returns focus to the element that was focused when a Radix dialog opened, when that
 * element is still connected on close. Radix alone only refocuses a `*Trigger` element,
 * so dialogs opened from row actions or controlled state would drop focus to `body`.
 */
export function useReturnFocus(
  onOpenAutoFocus?: AutoFocusHandler,
  onCloseAutoFocus?: AutoFocusHandler,
) {
  const returnTargetRef = useRef<HTMLElement | null>(null)

  const handleOpenAutoFocus = useCallback(
    (event: Event) => {
      // Radix dispatches this before it moves focus into the dialog.
      const active = document.activeElement
      returnTargetRef.current =
        active instanceof HTMLElement && active !== document.body ? active : null
      onOpenAutoFocus?.(event)
    },
    [onOpenAutoFocus],
  )

  const handleCloseAutoFocus = useCallback(
    (event: Event) => {
      onCloseAutoFocus?.(event)
      const target = returnTargetRef.current
      returnTargetRef.current = null
      if (event.defaultPrevented || !target?.isConnected) return
      event.preventDefault()
      target.focus({ preventScroll: true })
    },
    [onCloseAutoFocus],
  )

  return { onOpenAutoFocus: handleOpenAutoFocus, onCloseAutoFocus: handleCloseAutoFocus }
}
