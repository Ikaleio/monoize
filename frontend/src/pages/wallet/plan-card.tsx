import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Gauge, ShoppingCart } from "lucide-react";
import { toast } from "sonner";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { GroupsBadge } from "@/components/GroupsBadge";
import { formatNanoUsd } from "@/lib/exact-decimal";
import { purchaseBillingPlanOptimistic, useBillingPlanMarketplace, useBillingPlanSubscription } from "@/lib/swr";
import type { BillingPlanWindowUsage } from "@/lib/api";

function WindowRow({ label, window }: { label: string; window: BillingPlanWindowUsage }) {
  const used = BigInt(window.used_nano_usd);
  const limit = BigInt(window.limit_nano_usd);
  const percent = limit === 0n ? 100 : Number((used * 10000n) / limit) / 100;
  return <div className="space-y-1"><div className="flex justify-between text-xs"><span>{label}</span><span>{formatNanoUsd(window.remaining_nano_usd)} / {formatNanoUsd(window.limit_nano_usd)}</span></div><div className="h-2 overflow-hidden rounded-full bg-muted"><div className="h-full bg-primary" style={{ width: `${Math.min(100, percent)}%` }} /></div></div>;
}

export function PlanCard() {
  const { t } = useTranslation();
  const { data: subscription, isLoading } = useBillingPlanSubscription();
  const { data: plans = [], isLoading: plansLoading } = useBillingPlanMarketplace();
  const [purchasing, setPurchasing] = useState<string | null>(null);

  if (isLoading || plansLoading) return <Skeleton className="h-64 w-full rounded-xl" />;
  return <Card><CardHeader><CardTitle className="flex items-center gap-2 text-sm font-medium"><Gauge className="h-4 w-4" />{t("wallet.planTitle")}</CardTitle></CardHeader><CardContent className="space-y-5">
    {subscription ? <div className="space-y-4">
      <div><div className="font-medium">{subscription.plan_name}</div><p className="text-sm text-muted-foreground">{subscription.plan_description}</p></div>
      <div className="grid gap-3 sm:grid-cols-2">{subscription.windows.five_hour && <WindowRow label="5h" window={subscription.windows.five_hour} />}{subscription.windows.twenty_four_hour && <WindowRow label="24h" window={subscription.windows.twenty_four_hour} />}{subscription.windows.seven_day && <WindowRow label="7d" window={subscription.windows.seven_day} />}{subscription.windows.thirty_day && <WindowRow label="30d" window={subscription.windows.thirty_day} />}</div>
      <div className="flex flex-wrap items-center justify-between gap-2 text-xs text-muted-foreground"><GroupsBadge groupIds={subscription.group_ids} /><span>{t("wallet.planExpires", { time: new Date(subscription.expires_at).toLocaleString() })}</span></div>
    </div> : <div className="space-y-4"><p className="text-sm text-muted-foreground">{t("wallet.noPlan")}</p>{plans.length === 0 ? <p className="text-sm text-muted-foreground">{t("wallet.noListedPlans")}</p> : plans.map((plan) => <div key={plan.id} className="rounded-lg border p-4"><div className="font-medium">{plan.name}</div><p className="mb-3 text-sm text-muted-foreground">{plan.description}</p><div className="flex flex-wrap gap-2">{plan.prices.map((price) => <Button key={price.id} size="sm" variant="outline" disabled={purchasing !== null} onClick={async () => { setPurchasing(price.id); try { await purchaseBillingPlanOptimistic(plan.id, price.id, price.price_nano_usd, (error) => toast.error(error.message)); toast.success(t("wallet.purchaseSuccess")); } finally { setPurchasing(null); } }}><ShoppingCart className="mr-1.5 h-3.5 w-3.5" />{price.price_usd} USD / {Math.round(price.duration_seconds / 86400)}d</Button>)}</div></div>)}</div>}
  </CardContent></Card>;
}
