import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

const COPIED_FEEDBACK_MS = 2000;

/**
 * Clipboard write with the shared feedback contract (frontend-design-system.spec.md DS58):
 * `copied` stays true for 2 s after a successful write; a failed write shows one error toast.
 */
export function useCopyToClipboard() {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  const timer = useRef<number | undefined>(undefined);

  useEffect(() => () => window.clearTimeout(timer.current), []);

  const copy = useCallback(
    async (text: string) => {
      try {
        await navigator.clipboard.writeText(text);
      } catch {
        toast.error(t("common.copyFailed"));
        return;
      }
      setCopied(true);
      window.clearTimeout(timer.current);
      timer.current = window.setTimeout(() => setCopied(false), COPIED_FEEDBACK_MS);
    },
    [t],
  );

  return { copied, copy };
}
