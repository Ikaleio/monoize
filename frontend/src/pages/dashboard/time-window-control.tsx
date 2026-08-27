import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { ButtonGroup } from "@/components/ui/button-group";
import { cn } from "@/lib/utils";
import { USAGE_WINDOWS, type UsageWindow } from "@/lib/usage-window";

interface TimeWindowControlProps {
  value: UsageWindow;
  onChange: (window: UsageWindow) => void;
}

/**
 * Segmented 1h/24h/7d/30d selector (dashboard-home-overview.spec.md DH-6i).
 * Window labels are canonical product tokens and stay ASCII in every locale.
 */
export function TimeWindowControl({ value, onChange }: TimeWindowControlProps) {
  const { t } = useTranslation();
  return (
    <ButtonGroup aria-label={t("dashboard.usage.timeRange", "Time range")}>
      {USAGE_WINDOWS.map((window) => (
        <Button
          key={window}
          type="button"
          size="sm"
          variant="outline"
          aria-pressed={value === window}
          onClick={() => onChange(window)}
          className={cn(
            "h-7 px-2.5 text-xs tabular-nums",
            value === window && "bg-accent text-accent-foreground"
          )}
        >
          {window}
        </Button>
      ))}
    </ButtonGroup>
  );
}
