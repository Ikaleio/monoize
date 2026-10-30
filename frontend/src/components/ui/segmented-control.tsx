import { Button } from "@/components/ui/button";
import { ButtonGroup } from "@/components/ui/button-group";
import { cn } from "@/lib/utils";

interface SegmentedControlProps<T extends string> {
  value: T;
  options: readonly T[];
  onChange: (value: T) => void;
  ariaLabel: string;
  /** Visible label for an option; defaults to the option value. */
  label?: (option: T) => string;
}

/**
 * Single-choice button group for 2–6 short fixed options such as time windows
 * (frontend-design-system.spec.md DS65).
 */
export function SegmentedControl<T extends string>({
  value,
  options,
  onChange,
  ariaLabel,
  label,
}: SegmentedControlProps<T>) {
  return (
    <ButtonGroup aria-label={ariaLabel}>
      {options.map((option) => (
        <Button
          key={option}
          type="button"
          variant="outline"
          aria-pressed={value === option}
          onClick={() => onChange(option)}
          className={cn(
            "h-9 px-3 text-sm tabular-nums",
            value === option && "bg-accent text-accent-foreground",
          )}
        >
          {label ? label(option) : option}
        </Button>
      ))}
    </ButtonGroup>
  );
}
