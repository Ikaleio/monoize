import { Fragment, useState } from "react";
import { useTranslation } from "react-i18next";
import { motion, useReducedMotion } from "framer-motion";
import { Gauge, LoaderCircle, ShoppingCart } from "lucide-react";
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
import { AnimatedButton, springs } from "@/components/ui/motion";
import { Separator } from "@/components/ui/separator";
import { Skeleton } from "@/components/ui/skeleton";
import { formatNanoUsd } from "@/lib/exact-decimal";
import {
  purchaseBillingPlanOptimistic,
  useBillingPlanMarketplace,
  useBillingPlanSubscription,
} from "@/lib/swr";
import type { BillingPlanWindowUsage } from "@/lib/api";

function PlanCardSkeleton() {
  return (
    <Card aria-busy="true">
      <CardHeader>
        <Skeleton className="h-6 w-40" />
        <Skeleton className="h-4 w-64 max-w-full" />
      </CardHeader>
      <CardContent className="grid gap-5 sm:grid-cols-2">
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

function WindowRow({
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
    <div className="flex flex-col gap-2">
      <div className="flex items-center justify-between gap-4 text-sm">
        <span className="font-medium">{label}</span>
        <span className="font-mono text-xs tabular-nums text-muted-foreground">
          {formatNanoUsd(window.remaining_nano_usd)} /{" "}
          {formatNanoUsd(window.limit_nano_usd)}
        </span>
      </div>
      <div
        className="h-2 overflow-hidden rounded-full bg-muted"
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
    </div>
  );
}

export function PlanCard() {
  const { t } = useTranslation();
  const { data: subscription, isLoading } = useBillingPlanSubscription();
  const { data: plans = [], isLoading: plansLoading } =
    useBillingPlanMarketplace();
  const [purchasing, setPurchasing] = useState<string | null>(null);

  if (isLoading || plansLoading) return <PlanCardSkeleton />;

  return (
    <Card>
      <CardHeader className="flex flex-row items-start gap-3">
        <span className="flex size-10 shrink-0 items-center justify-center rounded-full bg-muted text-foreground">
          <Gauge className="size-5" aria-hidden="true" />
        </span>
        <div className="flex min-w-0 flex-col gap-1.5">
          <CardTitle className="text-balance font-display text-xl">
            {t("wallet.planTitle")}
          </CardTitle>
          <CardDescription className="text-pretty">
            {subscription?.plan_description || t("wallet.noPlan")}
          </CardDescription>
        </div>
      </CardHeader>
      <CardContent>
        {subscription ? (
          <div className="flex flex-col gap-6">
            <div className="flex flex-wrap items-start justify-between gap-4">
              <div className="flex flex-col gap-1">
                <span className="text-sm text-muted-foreground">
                  {t("wallet.currentPlan")}
                </span>
                <span className="text-lg font-semibold">
                  {subscription.plan_name}
                </span>
              </div>
              <div className="flex flex-col items-start gap-2 sm:items-end">
                <GroupsBadge groupIds={subscription.group_ids} />
                <span className="text-xs text-muted-foreground">
                  {t("wallet.planExpires", {
                    time: new Date(subscription.expires_at).toLocaleString(),
                  })}
                </span>
              </div>
            </div>

            <div className="grid gap-5 sm:grid-cols-2">
              {subscription.windows.five_hour ? (
                <WindowRow label="5h" window={subscription.windows.five_hour} />
              ) : null}
              {subscription.windows.twenty_four_hour ? (
                <WindowRow
                  label="24h"
                  window={subscription.windows.twenty_four_hour}
                />
              ) : null}
              {subscription.windows.seven_day ? (
                <WindowRow label="7d" window={subscription.windows.seven_day} />
              ) : null}
              {subscription.windows.thirty_day ? (
                <WindowRow
                  label="30d"
                  window={subscription.windows.thirty_day}
                />
              ) : null}
            </div>
          </div>
        ) : plans.length === 0 ? (
          <p className="text-sm text-muted-foreground">
            {t("wallet.noListedPlans")}
          </p>
        ) : (
          <div className="flex flex-col gap-5">
            {plans.map((plan, index) => (
              <Fragment key={plan.id}>
                {index > 0 ? <Separator /> : null}
                <div className="flex flex-col gap-4 md:flex-row md:items-center md:justify-between">
                  <div className="flex min-w-0 flex-col gap-1">
                    <h3 className="font-semibold">{plan.name}</h3>
                    <p className="max-w-2xl text-pretty text-sm leading-relaxed text-muted-foreground">
                      {plan.description}
                    </p>
                  </div>
                  <div className="flex shrink-0 flex-wrap gap-2">
                    {plan.prices.map((price) => (
                      <AnimatedButton key={price.id}>
                        <Button
                          size="sm"
                          variant="outline"
                          disabled={purchasing !== null}
                          onClick={async () => {
                            setPurchasing(price.id);
                            try {
                              await purchaseBillingPlanOptimistic(
                                plan.id,
                                price.id,
                                price.price_nano_usd,
                                (error) => toast.error(error.message),
                              );
                              toast.success(t("wallet.purchaseSuccess"));
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
                            <ShoppingCart
                              data-icon="inline-start"
                              aria-hidden="true"
                            />
                          )}
                          {price.price_usd} USD /{" "}
                          {Math.round(price.duration_seconds / 86400)}d
                        </Button>
                      </AnimatedButton>
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
