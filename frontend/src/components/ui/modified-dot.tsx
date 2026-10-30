import { cn } from "@/lib/utils";

/**
 * Small decorative dot marking customized settings or active filters. The adjacent
 * control's accessible name must state the condition, because the dot is hidden from
 * assistive technology.
 */
export function ModifiedDot({ className }: { className?: string }) {
  return (
    <span
      aria-hidden="true"
      className={cn("inline-block size-1.5 shrink-0 rounded-full bg-primary", className)}
    />
  );
}
