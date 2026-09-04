import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { ButtonGroup } from "@/components/ui/button-group";
import { cn } from "@/lib/utils";
import { SPEND_WINDOWS, type SpendWindow } from "@/lib/spend-window";

interface SpendWindowControlProps {
  value: SpendWindow;
  onChange: (window: SpendWindow) => void;
}

/**
 * Segmented 24h/3d/7d/14d/30d selector (admin-dashboard.spec.md ADF-5).
 * Window labels are canonical product tokens and stay ASCII in every locale.
 */
export function SpendWindowControl({ value, onChange }: SpendWindowControlProps) {
  const { t } = useTranslation();
  return (
    <ButtonGroup aria-label={t("adminDashboard.spendWindowAria", "Spend window")}>
      {SPEND_WINDOWS.map((window) => (
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
