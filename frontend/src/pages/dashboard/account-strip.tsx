import { useTranslation } from "react-i18next";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { SlideUp } from "@/components/ui/motion";
import { formatNanoUsd, formatUsdDecimal } from "@/lib/exact-decimal";
import type { DashboardAnalytics, User } from "@/lib/api";
import { cn } from "@/lib/utils";
import { formatCompactTokens } from "./utils";

interface AccountStripProps {
  user: User | null | undefined;
  analytics: DashboardAnalytics | undefined;
  loading?: boolean;
}

interface MetricCellProps {
  label: string;
  value: string;
  note: string;
  primary?: boolean;
  mobileDivider?: boolean;
}

interface AccountAnalyticsSummary {
  tokens: number;
  activeModels: number;
}

function summarizeAnalytics(
  analytics: DashboardAnalytics | undefined,
): AccountAnalyticsSummary {
  const modelTotals = new Map<string, number>();

  for (const bucket of analytics?.buckets ?? []) {
    for (const [model, rawTokens] of Object.entries(
      bucket.tokens_by_model ?? {},
    )) {
      const tokens = Number(rawTokens) || 0;
      if (tokens > 0) {
        modelTotals.set(model, (modelTotals.get(model) ?? 0) + tokens);
      }
    }
  }

  return {
    tokens: [...modelTotals.values()].reduce(
      (total, value) => total + value,
      0,
    ),
    activeModels: modelTotals.size,
  };
}

function MetricCell({
  label,
  value,
  note,
  primary,
  mobileDivider,
}: MetricCellProps) {
  return (
    <div
      className={cn(
        "flex min-h-28 min-w-0 flex-col justify-center gap-1 p-4",
        primary
          ? "sm:col-span-2 lg:col-span-1"
          : "border-t lg:border-l lg:border-t-0",
        mobileDivider && "sm:border-l",
      )}
    >
      <p className="truncate text-sm font-medium text-muted-foreground">
        {label}
      </p>
      <p
        className={cn(
          "truncate font-display text-2xl font-semibold tracking-tight tabular-nums",
          primary && "text-3xl",
        )}
        title={value}
      >
        {value}
      </p>
      <p className="truncate text-sm text-muted-foreground" title={note}>
        {note}
      </p>
    </div>
  );
}

function AccountOverviewSkeleton({ title }: { title: string }) {
  return (
    <Card className="overflow-hidden">
      <CardHeader className="sr-only">
        <CardTitle>{title}</CardTitle>
      </CardHeader>
      <CardContent className="grid p-0 sm:grid-cols-2 lg:grid-cols-[1.35fr_repeat(4,minmax(0,1fr))]">
        {Array.from({ length: 5 }).map((_, index) => (
          <div
            key={index}
            className={cn(
              "flex min-h-28 flex-col justify-center gap-2 p-4",
              index === 0
                ? "sm:col-span-2 lg:col-span-1"
                : "border-t lg:border-l lg:border-t-0",
              (index === 2 || index === 4) && "sm:border-l",
            )}
          >
            <Skeleton className="h-4 w-20" />
            <Skeleton className="h-8 w-28" />
            <Skeleton className="h-4 w-32 max-w-full" />
          </div>
        ))}
      </CardContent>
    </Card>
  );
}

export function AccountStrip({ user, analytics, loading }: AccountStripProps) {
  const { t } = useTranslation();
  const overviewTitle = t(
    "dashboard.account.overviewTitle",
    "Account Overview",
  );

  if (loading || !user) {
    return (
      <section aria-label={overviewTitle}>
        <AccountOverviewSkeleton title={overviewTitle} />
      </section>
    );
  }

  const balanceValue = user.balance_unlimited
    ? t("users.unlimited", "Unlimited")
    : formatUsdDecimal(user.balance_usd, 2);
  const balanceNote = user.balance_unlimited
    ? t("dashboard.account.balanceUnlimitedNote", "No account limit")
    : t("dashboard.account.balanceAvailableNote", "Available account balance");
  const summary = summarizeAnalytics(analytics);

  return (
    <SlideUp delay={0.04}>
      <section aria-label={overviewTitle}>
        <Card className="overflow-hidden">
          <CardHeader className="sr-only">
            <CardTitle>{overviewTitle}</CardTitle>
          </CardHeader>
          <CardContent className="grid p-0 sm:grid-cols-2 lg:grid-cols-[1.35fr_repeat(4,minmax(0,1fr))]">
            <MetricCell
              primary
              label={t("dashboard.account.currentBalance", "Current Balance")}
              value={balanceValue}
              note={balanceNote}
            />
            <MetricCell
              label={t("dashboard.account.todaySpend", "Today's Spend")}
              value={formatNanoUsd(analytics?.today_cost_nano_usd, 4)}
              note={t("dashboard.account.todaySpendNote", "USD · so far today")}
            />
            <MetricCell
              mobileDivider
              label={t("dashboard.account.todayRequests", "Today's Requests")}
              value={(analytics?.today_calls ?? 0).toLocaleString()}
              note={t("dashboard.account.callsUnit", "calls")}
            />
            <MetricCell
              label={t("dashboard.account.tokens24h", "24h Tokens")}
              value={formatCompactTokens(summary.tokens)}
              note={t(
                "dashboard.account.tokens24hNote",
                "All token categories",
              )}
            />
            <MetricCell mobileDivider label={t("dashboard.account.activeModels", "Active Models")} value={summary.activeModels.toLocaleString()} note={t("dashboard.account.activeModelsNote", "Past 24 hours")} />
          </CardContent>
        </Card>
      </section>
    </SlideUp>
  );
}
