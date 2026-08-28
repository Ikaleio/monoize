import { useTranslation } from "react-i18next";
import { AnimatePresence, motion, useReducedMotion } from "framer-motion";
import { WalletCards } from "lucide-react";
import { StatusBadge } from "@/components/ui/status";
import { springs } from "@/components/ui/motion";
import { formatUsdDecimal } from "@/lib/exact-decimal";
import type { User } from "@/lib/api";

/** RC-W2: session-backed prepaid balance with a bounded value replacement. */
export function BalanceCard({ user }: { user: User }) {
  const { t } = useTranslation();
  const reduced = useReducedMotion();
  const balance = formatUsdDecimal(user.balance_usd, 2);

  return (
    <section
      className="flex min-h-80 flex-col justify-between gap-10 bg-wallet p-6 text-wallet-foreground sm:p-8 lg:min-h-full lg:p-10"
      aria-labelledby="wallet-balance-title"
    >
      <header className="flex items-start justify-between gap-4">
        <div className="flex items-center gap-3">
          <span className="flex size-10 items-center justify-center rounded-full bg-wallet-foreground/10">
            <WalletCards className="size-5" aria-hidden="true" />
          </span>
          <div className="flex flex-col gap-1">
            <h2
              id="wallet-balance-title"
              className="text-sm font-medium text-wallet-muted"
            >
              {t("wallet.balanceTitle")}
            </h2>
            <span className="font-mono text-xs uppercase tracking-widest text-wallet-muted">
              USD
            </span>
          </div>
        </div>
        {user.balance_unlimited ? (
          <StatusBadge variant="info">{t("wallet.unlimited")}</StatusBadge>
        ) : null}
      </header>

      <div className="flex flex-col gap-4">
        <div className="relative min-h-16 overflow-hidden" aria-live="polite">
          <AnimatePresence mode="popLayout" initial={false}>
            <motion.p
              key={balance}
              initial={
                reduced ? { opacity: 0 } : { opacity: 0, y: 28, scale: 0.96 }
              }
              animate={{ opacity: 1, y: 0, scale: 1 }}
              exit={
                reduced ? { opacity: 0 } : { opacity: 0, y: -28, scale: 0.96 }
              }
              transition={reduced ? { duration: 0 } : springs.gentle}
              className="font-display text-5xl font-semibold tracking-tight tabular-nums sm:text-6xl"
            >
              {balance}
            </motion.p>
          </AnimatePresence>
        </div>
        <p className="max-w-md text-pretty text-sm leading-relaxed text-wallet-muted">
          {t("wallet.prepaidBalanceHelp")}
        </p>
      </div>
    </section>
  );
}
