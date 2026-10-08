import { Check, Copy } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";
import { cn } from "@/lib/utils";

interface CopyButtonProps {
  /** Text written to the clipboard. */
  value: string;
  /** Accessible name; defaults to `common.copy`. */
  label?: string;
  className?: string;
}

/** Icon-only copy control placed next to an identifier (frontend-design-system.spec.md DS58). */
export function CopyButton({ value, label, className }: CopyButtonProps) {
  const { t } = useTranslation();
  const { copied, copy } = useCopyToClipboard();

  return (
    <Button
      type="button"
      variant="ghost"
      size="icon"
      className={cn("size-11 shrink-0 touch-manipulation sm:size-7", className)}
      aria-label={label ?? t("common.copy")}
      onClick={() => void copy(value)}
    >
      {copied ? <Check aria-hidden="true" /> : <Copy aria-hidden="true" />}
    </Button>
  );
}
