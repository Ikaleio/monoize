import { useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import { useSearchParams } from "react-router-dom";
import { motion, useReducedMotion } from "framer-motion";
import { CalendarClock, CreditCard, ReceiptText } from "lucide-react";
import { useAuth } from "@/hooks/use-auth";
import { EmptyState } from "@/components/ui/empty-state";
import { Skeleton } from "@/components/ui/skeleton";
import { springs } from "@/components/ui/motion";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { OrderStatusBadge } from "@/components/recharge/order-status-badge";
import { formatTime } from "@/pages/request-logs/utils";
import { cn } from "@/lib/utils";
import { revalidateRechargeCaches, useRechargeOrders } from "@/lib/swr";
import type { RechargeOrdersResponse } from "@/lib/api";
import { PaginationFooter } from "./pagination-footer";
import { WalletFeedback } from "./wallet-feedback";

interface ActivityOrdersProps {
  active: boolean;
  pageSize: number;
  offset: number;
  onOffsetChange: (offset: number) => void;
  username: string;
}

export function ActivityOrders({
  active,
  pageSize,
  offset,
  onOffsetChange,
  username,
}: ActivityOrdersProps) {
  const { t } = useTranslation();
  const reduced = useReducedMotion();
  const { refreshUser } = useAuth();
  const [searchParams] = useSearchParams();
  const highlightedOrderId = searchParams.get("order_id");
  const { data, error, isLoading, mutate } = useRechargeOrders(
    pageSize,
    offset,
    { username },
    {
      refreshInterval: (latest: RechargeOrdersResponse | undefined) =>
        latest?.orders.some((order) => order.status === "pending") ? 5000 : 0,
    },
  );

  const pendingIdsRef = useRef<Set<string>>(new Set());
  useEffect(() => {
    if (!data) return;
    const previous = pendingIdsRef.current;
    const settled = data.orders.some(
      (order) => previous.has(order.id) && order.status !== "pending",
    );
    pendingIdsRef.current = new Set(
      data.orders
        .filter((order) => order.status === "pending")
        .map((order) => order.id),
    );
    if (settled) {
      revalidateRechargeCaches();
      void refreshUser();
    }
    // The session refresh function is stable; order changes drive this effect.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [data]);

  return (
    <motion.section
      aria-label={t("wallet.ordersTitle")}
      initial={false}
      animate={active || reduced ? { opacity: 1, x: 0 } : { opacity: 0, x: -20 }}
      transition={reduced ? { duration: 0 } : springs.gentle}
      className="p-5"
    >
      {isLoading ? (
        <div className="flex flex-col gap-2" aria-busy="true">
          {Array.from({ length: 5 }).map((_, index) => (
            <Skeleton key={index} className="h-20 w-full md:h-14" />
          ))}
        </div>
      ) : error && !data ? (
        <WalletFeedback onRetry={mutate} />
      ) : !data?.orders.length ? (
        <EmptyState
          variant="inline"
          className="px-4 py-7"
          icon={<ReceiptText className="size-6" aria-hidden="true" />}
          title={t("wallet.noOrders")}
        />
      ) : (
        <div className="flex flex-col gap-3">
          <div className="hidden grid-cols-[minmax(0,1.35fr)_minmax(7rem,0.55fr)_minmax(7rem,0.55fr)_minmax(9rem,0.75fr)_5rem] gap-4 border-b px-3 pb-2 text-xs font-medium text-muted-foreground md:grid">
            <span>{t("wallet.payment")}</span>
            <span className="text-right">{t("wallet.credit")}</span>
            <span>{t("wallet.statusCol")}</span>
            <span>{t("wallet.createdAt")}</span>
            <span>{t("wallet.orderId")}</span>
          </div>

          <motion.ul layout={!reduced} className="divide-y">
            {data.orders.map((order, index) => (
              <motion.li
                key={order.id}
                layout={!reduced}
                initial={reduced ? { opacity: 0 } : { opacity: 0, y: 12 }}
                animate={{ opacity: 1, y: 0 }}
                transition={
                  reduced
                    ? { duration: 0 }
                    : { ...springs.gentle, delay: Math.min(index * 0.03, 0.18) }
                }
                className={cn(
                  "grid gap-3 rounded-md px-3 py-3 transition-colors hover:bg-muted/50 md:grid-cols-[minmax(0,1.35fr)_minmax(7rem,0.55fr)_minmax(7rem,0.55fr)_minmax(9rem,0.75fr)_5rem] md:items-center md:gap-4",
                  order.id === highlightedOrderId && "bg-info-soft",
                )}
              >
                <div className="flex min-w-0 items-center gap-3">
                  <span className="flex size-9 shrink-0 items-center justify-center rounded-md bg-muted text-muted-foreground">
                    <CreditCard className="size-4" aria-hidden="true" />
                  </span>
                  <div className="flex min-w-0 flex-col gap-0.5">
                    <span className="truncate text-sm font-medium">
                      {order.channel_name}
                    </span>
                    <span className="text-xs text-muted-foreground tabular-nums">
                      {order.pay_amount} {order.pay_currency}
                    </span>
                  </div>
                </div>

                <div className="flex items-baseline justify-between gap-3 md:block md:text-right">
                  <span className="text-sm text-muted-foreground md:hidden">
                    {t("wallet.credit")}
                  </span>
                  <span className="font-display text-lg font-semibold tabular-nums">
                    +${order.credit_usd}
                  </span>
                </div>

                <div className="flex items-center justify-between gap-3 md:block">
                  <span className="text-sm text-muted-foreground md:hidden">
                    {t("wallet.statusCol")}
                  </span>
                  <OrderStatusBadge status={order.status} />
                </div>

                <span className="inline-flex items-center gap-1.5 text-xs text-muted-foreground tabular-nums">
                  <CalendarClock className="size-4" aria-hidden="true" />
                  {formatTime(order.created_at)}
                </span>

                <TooltipProvider delayDuration={200}>
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <button
                        type="button"
                        className="inline-flex min-h-11 items-center font-mono text-xs text-muted-foreground underline-offset-4 hover:text-foreground hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring md:min-h-0"
                      >
                        {order.id.slice(0, 8)}
                      </button>
                    </TooltipTrigger>
                    <TooltipContent>
                      <span className="font-mono">{order.id}</span>
                    </TooltipContent>
                  </Tooltip>
                </TooltipProvider>
              </motion.li>
            ))}
          </motion.ul>

          <PaginationFooter
            total={data.total}
            pageSize={pageSize}
            offset={offset}
            onOffsetChange={onOffsetChange}
          />
        </div>
      )}
    </motion.section>
  );
}
