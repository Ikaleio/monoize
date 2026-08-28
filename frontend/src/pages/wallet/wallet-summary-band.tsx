import { useTranslation } from "react-i18next";
import { AnimatePresence, motion, useReducedMotion } from "framer-motion";
import { ArrowDownToLine, WalletCards } from "lucide-react";
import { Button } from "@/components/ui/button";
import { StatusBadge } from "@/components/ui/status";
import { springs } from "@/components/ui/motion";
import { formatUsdDecimal } from "@/lib/exact-decimal";
import type { User } from "@/lib/api";

export function WalletSummaryBand({
  user,
  onRecharge,
}: {
  user: User;
  onRecharge: () => void;
}) {
  const { t } = useTranslation();
  const reduced = useReducedMotion();
  const balance = formatUsdDecimal(user.balance_usd, 2);

  return (
    <section
      aria-labelledby="wallet-balance-heading"
      className="grid overflow-hidden rounded-lg bg-wallet text-wallet-foreground md:grid-cols-[minmax(0,1.35fr)_minmax(17rem,0.65fr)]"
    >
      <div className="flex min-w-0 flex-col gap-6 p-5 sm:p-6">
        <div className="flex flex-wrap items-center gap-3">
          <span className="flex size-9 items-center justify-center rounded-md bg-wallet-foreground/10">
            <WalletCards className="size-5" aria-hidden="true" />
          </span>
          <h2
            id="wallet-balance-heading"
            className="text-sm font-medium text-wallet-muted"
          >
            {t("wallet.balanceTitle")}
          </h2>
          {user.balance_unlimited ? (
            <StatusBadge variant="info">{t("wallet.unlimited")}</StatusBadge>
          ) : null}
        </div>

        <div className="flex min-w-0 items-end gap-3" aria-live="polite">
          <AnimatePresence mode="popLayout" initial={false}>
            <motion.span
              key={balance}
              initial={reduced ? { opacity: 0 } : { opacity: 0, y: 20 }}
              animate={{ opacity: 1, y: 0 }}
              exit={reduced ? { opacity: 0 } : { opacity: 0, y: -20 }}
              transition={reduced ? { duration: 0 } : springs.gentle}
              className="min-w-0 truncate font-display text-4xl font-semibold tracking-tight tabular-nums sm:text-5xl"
            >
              {balance}
            </motion.span>
          </AnimatePresence>
          <span className="pb-1 font-mono text-sm uppercase tracking-widest text-wallet-muted">
            USD
          </span>
        </div>
      </div>

      <div className="flex flex-col items-start justify-center gap-4 border-t border-wallet-foreground/10 p-5 sm:p-6 md:border-l md:border-t-0">
        <p className="max-w-md text-pretty text-sm leading-relaxed text-wallet-muted">
          {t("wallet.prepaidBalanceHelp")}
        </p>
        <Button
          type="button"
          variant="outline"
          className="h-11 border-wallet-foreground/20 bg-wallet-foreground/10 text-wallet-foreground hover:bg-wallet-foreground/15 hover:text-wallet-foreground sm:h-9"
          onClick={onRecharge}
        >
          <ArrowDownToLine data-icon="inline-start" aria-hidden="true" />
          {t("wallet.rechargeTitle")}
        </Button>
      </div>
    </section>
  );
}
