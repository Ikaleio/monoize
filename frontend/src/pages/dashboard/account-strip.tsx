import { useTranslation } from "react-i18next";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { motion, springs, transitions } from "@/components/ui/motion";
import { formatUsdDecimal } from "@/lib/exact-decimal";
import { planRemainingFraction } from "@/lib/live-usage";
import type { User } from "@/lib/api";
import { cn } from "@/lib/utils";

interface AccountStripProps {
  user: User | null | undefined;
  loading?: boolean;
}

function MetricCardSkeleton() {
  return (
    <Card>
      <CardHeader className="p-4 pb-2">
        <Skeleton className="h-4 w-24" />
      </CardHeader>
      <CardContent className="space-y-2 p-4 pt-0">
        <Skeleton className="h-8 w-32" />
        <Skeleton className="h-3 w-40" />
      </CardContent>
    </Card>
  );
}

export function AccountStrip({ user, loading }: AccountStripProps) {
  const { t } = useTranslation();

  if (loading || !user) {
    return (
      <section className="grid gap-3 md:grid-cols-2">
        <MetricCardSkeleton />
        <MetricCardSkeleton />
      </section>
    );
  }

  const balanceValue = user.balance_unlimited
    ? t("users.unlimited", "Unlimited")
    : formatUsdDecimal(user.balance_usd, 2);

  const plan = user.billing_plan;
  const remainingFraction =
    plan && !user.balance_unlimited
      ? planRemainingFraction(user.balance_nano_usd, plan.grant_amount_nano_usd)
      : null;

  const remainingLabel = user.balance_unlimited
    ? t("users.unlimited", "Unlimited")
    : plan
      ? `${formatUsdDecimal(user.balance_usd, 2)} / ${formatUsdDecimal(plan.grant_amount_usd, 2)}`
      : t("dashboard.cards.noPlan", "No plan");

  const resetLabel = plan?.next_grant_at
    ? new Date(plan.next_grant_at).toLocaleString()
    : t("dashboard.subscription.resetUnavailable", "Not scheduled");

  return (
    <section className="grid gap-3 md:grid-cols-2">
      <motion.div
        initial={{ opacity: 0, y: 16, scale: 0.98 }}
        animate={{ opacity: 1, y: 0, scale: 1 }}
        transition={{ delay: 0.04, ...transitions.normal }}
        whileHover={{ y: -2, transition: springs.snappy }}
        className="h-full"
      >
        <Card className="h-full">
          <CardHeader className="p-4 pb-2">
            <CardTitle className="text-base font-semibold leading-none tracking-tight">
              {t("dashboard.account.balanceTitle", "Account Balance")}
            </CardTitle>
          </CardHeader>
          <CardContent className="p-4 pt-0">
            <p className="font-display text-3xl font-semibold tracking-tight tabular-nums">
              {balanceValue}
            </p>
            <p className="mt-1 text-xs text-muted-foreground">
              {t("dashboard.cards.currentBalance", "Current Balance")}
            </p>
          </CardContent>
        </Card>
      </motion.div>

      <motion.div
        initial={{ opacity: 0, y: 16, scale: 0.98 }}
        animate={{ opacity: 1, y: 0, scale: 1 }}
        transition={{ delay: 0.1, ...transitions.normal }}
        whileHover={{ y: -2, transition: springs.snappy }}
        className="h-full"
      >
        <Card className="h-full">
          <CardHeader className="p-4 pb-2">
            <CardTitle className="text-base font-semibold leading-none tracking-tight">
              {t("dashboard.account.subscriptionTitle", "Subscription")}
            </CardTitle>
          </CardHeader>
          <CardContent className="space-y-3 p-4 pt-0">
            {!plan ? (
              <p className="text-xl font-semibold">
                {t("dashboard.cards.noPlan", "No plan")}
              </p>
            ) : (
              <>
                <div className="flex items-baseline justify-between gap-3">
                  <p className="truncate text-xl font-semibold">{plan.name}</p>
                  <p className="shrink-0 font-mono text-xs text-muted-foreground">
                    {plan.schedule}
                  </p>
                </div>
                <div className="space-y-1.5">
                  <div className="flex items-center justify-between gap-2 text-xs">
                    <span className="text-muted-foreground">
                      {t("dashboard.subscription.remaining", "Remaining quota")}
                    </span>
                    <span className="font-medium tabular-nums">{remainingLabel}</span>
                  </div>
                  {remainingFraction != null && (
                    <div className="h-1.5 overflow-hidden rounded-md bg-muted">
                      <div
                        className={cn(
                          "h-full rounded-md bg-primary transition-[width] duration-500 ease-out"
                        )}
                        style={{ width: `${Math.round(remainingFraction * 100)}%` }}
                      />
                    </div>
                  )}
                </div>
                <div className="flex items-center justify-between gap-2 text-xs">
                  <span className="text-muted-foreground">
                    {t("dashboard.subscription.reset", "Resets")}
                  </span>
                  <span className="truncate tabular-nums text-foreground">{resetLabel}</span>
                </div>
              </>
            )}
          </CardContent>
        </Card>
      </motion.div>
    </section>
  );
}
