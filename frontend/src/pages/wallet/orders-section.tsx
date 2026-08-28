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
import { WalletSectionError } from "./section-error";

interface OrdersSectionProps {
  active: boolean;
  pageSize: number;
  offset: number;
  onOffsetChange: (offset: number) => void;
  username: string;
}

/** RC-W4/RC-W6: own recharge orders with pending-state polling. */
export function OrdersSection({
  active,
  pageSize,
  offset,
  onOffsetChange,
  username,
}: OrdersSectionProps) {
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
    // refreshUser is stable in practice; re-running on data only is intended.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [data]);

  return (
    <motion.section
      aria-label={t("wallet.ordersTitle")}
      initial={false}
      animate={
        active || reduced ? { opacity: 1, x: 0 } : { opacity: 0, x: -24 }
      }
      transition={reduced ? { duration: 0 } : springs.gentle}
    >
      {isLoading ? (
        <div className="flex flex-col gap-3" aria-busy="true">
          {Array.from({ length: 4 }).map((_, index) => (
            <Skeleton key={index} className="h-24 w-full" />
          ))}
        </div>
      ) : error && !data ? (
        <WalletSectionError onRetry={mutate} />
      ) : !data || data.orders.length === 0 ? (
        <EmptyState
          variant="inline"
          className="px-4 py-6"
          icon={<ReceiptText className="size-6" aria-hidden="true" />}
          title={t("wallet.noOrders")}
        />
      ) : (
        <div className="flex flex-col gap-2">
          <motion.ul layout={!reduced} className="divide-y">
            {data.orders.map((order, index) => (
              <motion.li
                key={order.id}
                layout={!reduced}
                initial={reduced ? { opacity: 0 } : { opacity: 0, y: 16 }}
                animate={{ opacity: 1, y: 0 }}
                transition={
                  reduced
                    ? { duration: 0 }
                    : { ...springs.gentle, delay: Math.min(index * 0.035, 0.2) }
                }
                className={cn(
                  "flex flex-col gap-3 px-1 py-3 transition-colors hover:bg-muted/50 sm:px-2 sm:py-4",
                  order.id === highlightedOrderId && "bg-info-soft",
                )}
              >
                <div className="flex items-start justify-between gap-4">
                  <div className="flex min-w-0 items-start gap-3">
                    <span className="flex size-9 shrink-0 items-center justify-center rounded-full bg-muted text-muted-foreground">
                      <CreditCard aria-hidden="true" />
                    </span>
                    <div className="flex min-w-0 flex-col gap-1">
                      <span className="truncate font-medium">
                        {order.channel_name}
                      </span>
                      <span className="text-sm text-muted-foreground tabular-nums">
                        {order.pay_amount} {order.pay_currency}
                      </span>
                    </div>
                  </div>
                  <div className="flex shrink-0 flex-col items-end gap-2">
                    <span className="font-display text-xl font-semibold tabular-nums">
                      +${order.credit_usd}
                    </span>
                    <OrderStatusBadge status={order.status} />
                  </div>
                </div>

                <div className="flex flex-wrap items-center gap-x-4 gap-y-2 text-xs text-muted-foreground">
                  <span className="inline-flex items-center gap-1.5 tabular-nums">
                    <CalendarClock className="size-4" aria-hidden="true" />
                    {formatTime(order.created_at)}
                  </span>
                  <TooltipProvider delayDuration={200}>
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <button
                          type="button"
                          className="inline-flex min-h-11 items-center font-mono underline-offset-4 hover:text-foreground hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring sm:min-h-0"
                        >
                          {order.id.slice(0, 8)}
                        </button>
                      </TooltipTrigger>
                      <TooltipContent>
                        <span className="font-mono">{order.id}</span>
                      </TooltipContent>
                    </Tooltip>
                  </TooltipProvider>
                </div>
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
