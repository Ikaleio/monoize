import { useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import { useSearchParams } from "react-router-dom";
import { motion, useReducedMotion } from "framer-motion";
import { ReceiptText } from "lucide-react";
import { useAuth } from "@/hooks/use-auth";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { EmptyState } from "@/components/ui/empty-state";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { springs } from "@/components/ui/motion";
import { OrderStatusBadge } from "@/components/recharge/order-status-badge";
import { PaginationFooter } from "./pagination-footer";
import { formatTime } from "@/pages/request-logs/utils";
import { revalidateRechargeCaches, useRechargeOrders } from "@/lib/swr";
import type { RechargeOrdersResponse } from "@/lib/api";
import { cn } from "@/lib/utils";

interface OrdersSectionProps {
  pageSize: number;
  offset: number;
  onOffsetChange: (offset: number) => void;
}

/**
 * RC-W4/RC-W6: own recharge orders, newest first. Polls at 5s while any
 * displayed order is `pending`; a poll that observes a pending order reach a
 * terminal state revalidates the ledger and the session user caches.
 */
export function OrdersSection({ pageSize, offset, onOffsetChange }: OrdersSectionProps) {
  const { t } = useTranslation();
  const reduced = useReducedMotion();
  const { refreshUser } = useAuth();
  const [searchParams] = useSearchParams();
  const highlightedOrderId = searchParams.get("order_id");

  const { data, isLoading } = useRechargeOrders(pageSize, offset, undefined, {
    refreshInterval: (latest: RechargeOrdersResponse | undefined) =>
      latest?.orders.some((order) => order.status === "pending") ? 5000 : 0,
  });

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
    <Card>
      <CardHeader className="pb-2">
        <CardTitle className="flex items-center gap-2 text-sm font-medium text-muted-foreground">
          <ReceiptText className="h-4 w-4" />
          {t("wallet.ordersTitle")}
        </CardTitle>
      </CardHeader>
      <CardContent>
        {isLoading ? (
          <div className="flex flex-col gap-3" aria-busy="true">
            {Array.from({ length: 4 }).map((_, index) => (
              <Skeleton key={index} className="h-9 w-full" />
            ))}
          </div>
        ) : !data || data.orders.length === 0 ? (
          <EmptyState variant="inline" title={t("wallet.noOrders")} />
        ) : (
          <motion.div
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            transition={{ duration: reduced ? 0 : 0.25 }}
            className="overflow-x-auto"
          >
            <table className="w-full text-sm">
              <thead className="text-left text-muted-foreground">
                <tr className="border-b">
                  <th className="px-3 py-2 font-medium">{t("common.created")}</th>
                  <th className="px-3 py-2 font-medium">{t("wallet.channelCol")}</th>
                  <th className="px-3 py-2 font-medium">{t("wallet.credit")}</th>
                  <th className="px-3 py-2 font-medium">{t("wallet.payment")}</th>
                  <th className="px-3 py-2 font-medium">{t("common.status")}</th>
                  <th className="px-3 py-2 font-medium">{t("wallet.orderId")}</th>
                </tr>
              </thead>
              <tbody>
                {data.orders.map((order) => (
                  <motion.tr
                    key={order.id}
                    layout={!reduced}
                    initial={reduced ? { opacity: 0 } : { opacity: 0, y: -8 }}
                    animate={{ opacity: 1, y: 0 }}
                    transition={reduced ? { duration: 0.15 } : springs.smooth}
                    className={cn(
                      "border-b transition-colors last:border-b-0 hover:bg-accent/40",
                      order.id === highlightedOrderId && "bg-info-soft",
                    )}
                  >
                    <td className="whitespace-nowrap px-3 py-2.5 tabular-nums text-muted-foreground">
                      {formatTime(order.created_at)}
                    </td>
                    <td className="px-3 py-2.5">{order.channel_name}</td>
                    <td className="px-3 py-2.5 tabular-nums">${order.credit_usd}</td>
                    <td className="whitespace-nowrap px-3 py-2.5 tabular-nums">
                      {order.pay_amount} {order.pay_currency}
                    </td>
                    <td className="px-3 py-2.5">
                      <OrderStatusBadge status={order.status} />
                    </td>
                    <td className="px-3 py-2.5">
                      <Tooltip>
                        <TooltipTrigger asChild>
                          <span className="font-mono text-xs text-muted-foreground">
                            {order.id.slice(0, 8)}
                          </span>
                        </TooltipTrigger>
                        <TooltipContent>
                          <span className="font-mono">{order.id}</span>
                        </TooltipContent>
                      </Tooltip>
                    </td>
                  </motion.tr>
                ))}
              </tbody>
            </table>
            <PaginationFooter
              total={data.total}
              pageSize={pageSize}
              offset={offset}
              onOffsetChange={onOffsetChange}
            />
          </motion.div>
        )}
      </CardContent>
    </Card>
  );
}
