import { Fragment, useState } from "react";
import { useTranslation } from "react-i18next";
import { motion, useReducedMotion } from "framer-motion";
import { CalendarClock, Gauge, LoaderCircle, ShoppingCart } from "lucide-react";
import { toast } from "sonner";
import { GroupsBadge } from "@/components/GroupsBadge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Separator } from "@/components/ui/separator";
import { Skeleton } from "@/components/ui/skeleton";
import { springs } from "@/components/ui/motion";
import { formatNanoUsd } from "@/lib/exact-decimal";
import {
  purchaseBillingPlanOptimistic,
  useBillingPlanMarketplace,
  useBillingPlanSubscription,
} from "@/lib/swr";
import type { BillingPlanWindowUsage } from "@/lib/api";
import { WalletFeedback } from "./wallet-feedback";

function PlanAllowanceSkeleton({ className }: { className?: string }) {
  return (
    <Card className={className} aria-busy="true">
      <CardHeader className="border-b p-5">
        <Skeleton className="h-6 w-40" />
        <Skeleton className="h-4 w-64 max-w-full" />
      </CardHeader>
      <CardContent className="grid gap-5 p-5 sm:grid-cols-2">
        {Array.from({ length: 4 }).map((_, index) => (
          <div key={index} className="flex flex-col gap-2">
            <Skeleton className="h-4 w-28" />
            <Skeleton className="h-2 w-full" />
          </div>
        ))}
      </CardContent>
    </Card>
  );
}

function AllowanceMeter({
  label,
  window,
}: {
  label: string;
  window: BillingPlanWindowUsage;
}) {
  const reduced = useReducedMotion();
  const remaining = BigInt(window.remaining_nano_usd);
  const limit = BigInt(window.limit_nano_usd);
  const percent =
    limit <= 0n
      ? 0
      : Math.max(0, Math.min(100, Number((remaining * 10000n) / limit) / 100));

  return (
    <div className="flex flex-col gap-2 rounded-md bg-muted/55 p-3">
      <div className="flex items-baseline justify-between gap-3">
        <span className="text-sm font-medium">{label}</span>
        <span className="font-mono text-sm tabular-nums">
          {formatNanoUsd(window.remaining_nano_usd, 2)}
        </span>
      </div>
      <div
        className="h-1.5 overflow-hidden rounded-full bg-background"
        role="progressbar"
        aria-label={label}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={Math.round(percent)}
      >
        <motion.div
          initial={reduced ? false : { width: 0 }}
          animate={{ width: `${percent}%` }}
          transition={reduced ? { duration: 0 } : springs.gentle}
          className="h-full rounded-full bg-primary"
        />
      </div>
      <span className="text-xs text-muted-foreground tabular-nums">
        {formatNanoUsd(window.remaining_nano_usd, 2)} /{" "}
        {formatNanoUsd(window.limit_nano_usd, 2)}
      </span>
    </div>
  );
}

export function PlanAllowance({ className }: { className?: string }) {
  const { t } = useTranslation();
  const {
    data: subscription,
    error: subscriptionError,
    isLoading,
    mutate: mutateSubscription,
  } = useBillingPlanSubscription();
  const {
    data: plans,
    error: plansError,
    isLoading: plansLoading,
    mutate: mutatePlans,
  } = useBillingPlanMarketplace();
  const [purchasing, setPurchasing] = useState<string | null>(null);

  if (isLoading || plansLoading) {
    return <PlanAllowanceSkeleton className={className} />;
  }

  const hasBlockingError =
    (subscriptionError && subscription === undefined) ||
    (plansError && plans === undefined);

  const windows = subscription
    ? [
        ["5h", subscription.windows.five_hour],
        ["24h", subscription.windows.twenty_four_hour],
        ["7d", subscription.windows.seven_day],
        ["30d", subscription.windows.thirty_day],
      ].filter(
        (entry): entry is [string, BillingPlanWindowUsage] => entry[1] !== null,
      )
    : [];

  return (
    <Card className={className} aria-labelledby="wallet-plan-heading">
      <CardHeader className="border-b p-5">
        <div className="flex items-start gap-3">
          <span className="flex size-9 shrink-0 items-center justify-center rounded-md bg-muted text-foreground">
            <Gauge className="size-5" aria-hidden="true" />
          </span>
          <div className="flex min-w-0 flex-col gap-1">
            <CardTitle id="wallet-plan-heading" className="font-display text-lg">
              {t("wallet.planTitle")}
            </CardTitle>
            <CardDescription className="text-pretty leading-relaxed">
              {subscription?.plan_description || t("wallet.noPlan")}
            </CardDescription>
          </div>
        </div>
      </CardHeader>

      <CardContent className="p-5">
        {hasBlockingError ? (
          <WalletFeedback
            onRetry={() => Promise.all([mutateSubscription(), mutatePlans()])}
          />
        ) : subscription ? (
          <div className="flex flex-col gap-5">
            <div className="grid gap-4 sm:grid-cols-[minmax(0,1fr)_auto] sm:items-start">
              <div className="flex min-w-0 flex-col gap-1">
                <span className="text-sm text-muted-foreground">
                  {t("wallet.currentPlan")}
                </span>
                <span className="truncate text-lg font-semibold">
                  {subscription.plan_name}
                </span>
              </div>
              <div className="flex flex-col items-start gap-2 sm:items-end">
                <GroupsBadge groupIds={subscription.group_ids} />
                <span className="inline-flex items-center gap-1.5 text-xs text-muted-foreground">
                  <CalendarClock className="size-4" aria-hidden="true" />
                  {t("wallet.planExpires", {
                    time: new Date(subscription.expires_at).toLocaleString(),
                  })}
                </span>
              </div>
            </div>

            <div className="grid gap-3 sm:grid-cols-2">
              {windows.map(([label, window]) => (
                <AllowanceMeter key={label} label={label} window={window} />
              ))}
            </div>
          </div>
        ) : !plans?.length ? (
          <p className="text-sm text-muted-foreground">
            {t("wallet.noListedPlans")}
          </p>
        ) : (
          <div className="flex flex-col">
            {plans.map((plan, index) => (
              <Fragment key={plan.id}>
                {index > 0 ? <Separator className="my-4" /> : null}
                <div className="grid gap-4 md:grid-cols-[minmax(0,1fr)_auto] md:items-center">
                  <div className="flex min-w-0 flex-col gap-1.5">
                    <h3 className="font-semibold">{plan.name}</h3>
                    <p className="max-w-2xl text-pretty text-sm leading-relaxed text-muted-foreground">
                      {plan.description}
                    </p>
                  </div>
                  <div className="flex flex-wrap gap-2 md:justify-end">
                    {plan.prices.map((price) => (
                      <Button
                        key={price.id}
                        type="button"
                        variant="outline"
                        className="h-11 sm:h-9"
                        disabled={purchasing !== null}
                        onClick={async () => {
                          setPurchasing(price.id);
                          try {
                            await purchaseBillingPlanOptimistic(
                              plan.id,
                              price.id,
                              price.price_nano_usd,
                            );
                            toast.success(t("wallet.purchaseSuccess"));
                          } catch {
                            toast.error(t("wallet.errors.request_failed"));
                          } finally {
                            setPurchasing(null);
                          }
                        }}
                      >
                        {purchasing === price.id ? (
                          <LoaderCircle
                            data-icon="inline-start"
                            className="animate-spin"
                            aria-hidden="true"
                          />
                        ) : (
                          <ShoppingCart data-icon="inline-start" aria-hidden="true" />
                        )}
                        <span className="tabular-nums">
                          {price.price_usd} USD / {Math.round(price.duration_seconds / 86400)}d
                        </span>
                      </Button>
                    ))}
                  </div>
                </div>
              </Fragment>
            ))}
          </div>
        )}
      </CardContent>
    </Card>
  );
}
