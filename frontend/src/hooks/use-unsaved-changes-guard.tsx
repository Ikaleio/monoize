import { useEffect, type ReactNode } from "react";
import { useBlocker } from "react-router-dom";
import { useTranslation } from "react-i18next";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { buttonVariants } from "@/components/ui/button";

/**
 * Guards a page-level draft. While `dirty`, in-app navigation to another path asks for
 * confirmation and closing or reloading the tab triggers the browser prompt. Search and
 * hash changes on the same path (tabs, filters, categories) never block. The caller
 * renders the returned node.
 */
export function useUnsavedChangesGuard(dirty: boolean): ReactNode {
  const { t } = useTranslation();
  const blocker = useBlocker(
    ({ currentLocation, nextLocation }) =>
      dirty && currentLocation.pathname !== nextLocation.pathname,
  );

  useEffect(() => {
    if (!dirty) return;
    const onBeforeUnload = (event: BeforeUnloadEvent) => {
      event.preventDefault();
      // Older browsers show the prompt only when returnValue is set.
      event.returnValue = "";
    };
    window.addEventListener("beforeunload", onBeforeUnload);
    return () => window.removeEventListener("beforeunload", onBeforeUnload);
  }, [dirty]);

  // A save that finishes while the prompt is open clears the draft; the pending
  // navigation then no longer needs confirmation.
  useEffect(() => {
    if (!dirty && blocker.state === "blocked") blocker.proceed();
  }, [blocker, dirty]);

  return (
    <AlertDialog
      open={blocker.state === "blocked"}
      onOpenChange={(open) => {
        if (!open && blocker.state === "blocked") blocker.reset();
      }}
    >
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{t("guards.leaveTitle")}</AlertDialogTitle>
          <AlertDialogDescription>{t("guards.leaveDescription")}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>{t("guards.keepEditing")}</AlertDialogCancel>
          <AlertDialogAction
            className={buttonVariants({ variant: "destructive" })}
            onClick={(event) => {
              // Closing through Radix would call reset() before the router leaves "blocked".
              event.preventDefault();
              blocker.proceed?.();
            }}
          >
            {t("guards.discardAndLeave")}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
