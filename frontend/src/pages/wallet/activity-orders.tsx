import { useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import { useSearchParams } from "react-router-dom";
import { motion, useReducedMotion } from "framer-motion";
import { ReceiptText } from "lucide-react";
import { useAuth } from "@/hooks/use-auth";
import { EmptyState } from "@/components/ui/empty-state";
import { Skeleton } from "@/components/ui/skeleton";
import {
  DataList,
  DataListBody,
  DataListCell,
  DataListHead,
  DataListHeader,
  DataListRow,
} from "@/components/ui/data-list";
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
      animate={
        active || reduced ? { opacity: 1, x: 0 } : { opacity: 0, x: -20 }
      }
      transition={reduced ? { duration: 0 } : springs.gentle}
    >
      {isLoading ? (
        <div className="flex flex-col gap-2 p-4" aria-busy="true">
          {Array.from({ length: 5 }).map((_, index) => (
            <Skeleton key={index} className="h-14 w-full" />
          ))}
        </div>
      ) : error && !data ? (
        <div className="p-4">
          <WalletFeedback onRetry={mutate} />
        </div>
      ) : !data?.orders.length ? (
        <EmptyState
          variant="inline"
          className="px-4 py-7"
          icon={<ReceiptText className="size-6" aria-hidden="true" />}
          title={t("wallet.noOrders")}
        />
      ) : (
        <>
          <DataList columns="minmax(0,1.35fr) 7rem 6.5rem 10rem 5.5rem">
            <DataListHeader>
              <DataListHead>{t("wallet.payment")}</DataListHead>
              <DataListHead align="end">{t("wallet.credit")}</DataListHead>
              <DataListHead>{t("wallet.statusCol")}</DataListHead>
              <DataListHead>{t("wallet.createdAt")}</DataListHead>
              <DataListHead>{t("wallet.orderId")}</DataListHead>
            </DataListHeader>
            <TooltipProvider delayDuration={200}>
              <DataListBody aria-label={t("wallet.ordersTitle")}>
                {data.orders.map((order, index) => (
                  <DataListRow
                    key={order.id}
                    asChild
                    className={cn(order.id === highlightedOrderId && "bg-info-soft")}
                  >
                    <motion.li
                      layout={!reduced}
                      initial={reduced ? { opacity: 0 } : { opacity: 0, y: 12 }}
                      animate={{ opacity: 1, y: 0 }}
                      transition={
                        reduced
                          ? { duration: 0 }
                          : { ...springs.gentle, delay: Math.min(index * 0.03, 0.18) }
                      }
                    >
                      <DataListCell primary>
                        <span className="block truncate font-medium" title={order.channel_name}>
                          {order.channel_name}
                        </span>
                        <span className="tabular-nums text-muted-foreground">
                          {order.pay_amount} {order.pay_currency}
                        </span>
                      </DataListCell>
                      <DataListCell label={t("wallet.credit")} align="end">
                        <span className="font-medium tabular-nums">${order.credit_usd}</span>
                      </DataListCell>
                      <DataListCell label={t("wallet.statusCol")}>
                        <OrderStatusBadge status={order.status} />
                      </DataListCell>
                      <DataListCell label={t("wallet.createdAt")}>
                        <span className="tabular-nums text-muted-foreground">
                          {formatTime(order.created_at)}
                        </span>
                      </DataListCell>
                      <DataListCell label={t("wallet.orderId")}>
                        <Tooltip>
                          <TooltipTrigger asChild>
                            <button
                              type="button"
                              className="inline-flex min-h-11 items-center rounded-sm font-mono text-muted-foreground underline-offset-4 hover:text-foreground hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring sm:min-h-0"
                            >
                              {order.id.slice(0, 8)}
                            </button>
                          </TooltipTrigger>
                          <TooltipContent>
                            <span className="font-mono">{order.id}</span>
                          </TooltipContent>
                        </Tooltip>
                      </DataListCell>
                    </motion.li>
                  </DataListRow>
                ))}
              </DataListBody>
            </TooltipProvider>
          </DataList>
          <div className="border-t px-4 pb-3 empty:hidden">
            <PaginationFooter
              total={data.total}
              pageSize={pageSize}
              offset={offset}
              onOffsetChange={onOffsetChange}
            />
          </div>
        </>
      )}
    </motion.section>
  );
}
