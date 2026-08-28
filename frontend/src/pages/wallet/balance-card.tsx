import { useTranslation } from "react-i18next";
import { AnimatePresence, motion, useReducedMotion } from "framer-motion";
import { Wallet } from "lucide-react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { StatusBadge } from "@/components/ui/status";
import { springs } from "@/components/ui/motion";
import { formatUsdDecimal } from "@/lib/exact-decimal";
import type { User } from "@/lib/api";

/**
 * RC-W2: renders the prepaid balance from the session user object. The figure animates
 * with a vertical spring slide whenever the value changes (poll-driven).
 */
export function BalanceCard({ user }: { user: User }) {
  const { t } = useTranslation();
  const reduced = useReducedMotion();
  const balance = formatUsdDecimal(user.balance_usd, 2);

  return (
    <Card className="flex flex-col">
      <CardHeader className="flex flex-row items-center justify-between gap-2 pb-2">
        <CardTitle className="flex items-center gap-2 text-sm font-medium text-muted-foreground">
          <Wallet className="h-4 w-4" />
          {t("wallet.balanceTitle")}
        </CardTitle>
        {user.balance_unlimited && (
          <StatusBadge variant="info">{t("wallet.unlimited")}</StatusBadge>
        )}
      </CardHeader>
      <CardContent className="flex flex-1 flex-col justify-between gap-4">
        <div className="relative overflow-hidden">
          <AnimatePresence mode="popLayout" initial={false}>
            <motion.p
              key={balance}
              initial={reduced ? { opacity: 0 } : { opacity: 0, y: 18 }}
              animate={{ opacity: 1, y: 0 }}
              exit={reduced ? { opacity: 0 } : { opacity: 0, y: -18 }}
              transition={reduced ? { duration: 0.15 } : springs.smooth}
              className="font-display text-4xl font-semibold tracking-tight tabular-nums"
            >
              {balance}
            </motion.p>
          </AnimatePresence>
        </div>
        <p className="text-sm text-muted-foreground">{t("wallet.prepaidBalanceHelp")}</p>
      </CardContent>
    </Card>
  );
}
