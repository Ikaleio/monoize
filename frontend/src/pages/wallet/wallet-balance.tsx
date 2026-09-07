import { useTranslation } from "react-i18next";
import { ArrowUpRight } from "lucide-react";
import { Button } from "@/components/ui/button";
import { StatusBadge } from "@/components/ui/status";
import { formatUsdDecimal } from "@/lib/exact-decimal";
import { cn } from "@/lib/utils";
import type { User } from "@/lib/api";

export function WalletBalance({
  user,
  onRecharge,
  className,
}: {
  user: User;
  onRecharge: () => void;
  className?: string;
}) {
  const { t } = useTranslation();
  return (
    <section
      aria-labelledby="wallet-balance-heading"
      className={cn("rounded-lg border bg-card px-5 py-5 sm:px-6", className)}
    >
      <div className="flex flex-wrap items-center justify-between gap-5">
        <div className="min-w-0">
          <div className="mb-2 flex flex-wrap items-center gap-3">
            <h2
              id="wallet-balance-heading"
              className="text-base font-medium text-muted-foreground"
            >
              {t("wallet.balanceTitle")}
            </h2>
            {user.balance_unlimited ? (
              <StatusBadge variant="info">{t("wallet.unlimited")}</StatusBadge>
            ) : null}
          </div>
          <div
            className="flex flex-wrap items-baseline gap-3"
            aria-live="polite"
            aria-atomic="true"
          >
            <span className="min-w-0 break-all text-4xl font-semibold tracking-tight tabular-nums">
              {formatUsdDecimal(user.balance_usd, 2)}
            </span>
            <span className="text-sm text-muted-foreground">USD</span>
          </div>
        </div>
        <Button
          type="button"
          className="h-11 bg-wallet-action px-5 text-wallet-action-foreground hover:bg-wallet-action/90"
          onClick={onRecharge}
        >
          {t("wallet.rechargeTitle")}
          <ArrowUpRight data-icon="inline-end" aria-hidden="true" />
        </Button>
      </div>
      <p className="mt-4 border-t pt-3 text-sm leading-relaxed text-muted-foreground">
        {t("wallet.prepaidBalanceHelp")}
      </p>
    </section>
  );
}
