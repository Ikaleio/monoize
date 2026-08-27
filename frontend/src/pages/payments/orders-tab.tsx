import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { motion, useReducedMotion } from "framer-motion";
import { Undo2 } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Checkbox } from "@/components/ui/checkbox";
import { Skeleton } from "@/components/ui/skeleton";
import { EmptyState } from "@/components/ui/empty-state";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { springs } from "@/components/ui/motion";
import { OrderStatusBadge } from "@/components/recharge/order-status-badge";
import { PaginationFooter } from "@/pages/wallet/pagination-footer";
import { formatTime } from "@/pages/request-logs/utils";
import { DashboardApiError } from "@/lib/api";
import type { RechargeOrder, RechargeOrderStatus } from "@/lib/api";
import { ORDER_STATUSES, SUPPORTS_REFUND } from "@/lib/recharge";
import {
  rechargeOrdersSWRKey,
  refundRechargeOrderOptimistic,
  useRechargeOrders,
} from "@/lib/swr";

const PAGE_SIZE = 20;

/**
 * RC-M3 Orders tab: the admin view of all users' orders with status/username
 * filters and a per-row full refund limited to `succeeded` orders. Channels
 * without provider-side refund require the RC-R4 manual acknowledgment.
 */
export function OrdersTab() {
  const { t } = useTranslation();
  const reduced = useReducedMotion();
  const [status, setStatus] = useState<string>("all");
  const [usernameInput, setUsernameInput] = useState("");
  const [username, setUsername] = useState("");
  const [offset, setOffset] = useState(0);
  const [refundTarget, setRefundTarget] = useState<RechargeOrder | null>(null);
  const [manualChecked, setManualChecked] = useState(false);
  const [refunding, setRefunding] = useState(false);

  const filters = useMemo(
    () => ({
      status: status === "all" ? undefined : (status as RechargeOrderStatus),
      username: username || undefined,
    }),
    [status, username],
  );
  const pageKey = rechargeOrdersSWRKey(PAGE_SIZE, offset, filters);
  const { data, isLoading } = useRechargeOrders(PAGE_SIZE, offset, filters);

  const needsManual = refundTarget
    ? !SUPPORTS_REFUND[refundTarget.channel_type_id]
    : false;

  const handleRefund = async () => {
    if (!refundTarget || refunding) return;
    if (needsManual && !manualChecked) return;
    setRefunding(true);
    try {
      await refundRechargeOrderOptimistic(
        refundTarget,
        needsManual,
        pageKey,
        (error) => {
          const message =
            error instanceof DashboardApiError
              ? t(`payments.errors.${error.code}`, { defaultValue: error.message })
              : error.message;
          toast.error(message);
        },
      );
      toast.success(t("payments.refunded"));
      setRefundTarget(null);
      setManualChecked(false);
    } catch {
      // optimistic helper already rolled back and toasted; keep dialog open
    } finally {
      setRefunding(false);
    }
  };

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center gap-2">
        <Select
          value={status}
          onValueChange={(value) => {
            setStatus(value);
            setOffset(0);
          }}
        >
          <SelectTrigger className="h-9 w-40" aria-label={t("payments.statusFilter")}>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="all">{t("payments.statusAll")}</SelectItem>
            {ORDER_STATUSES.map((value) => (
              <SelectItem key={value} value={value}>
                {t(`wallet.status.${value}`)}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Input
          value={usernameInput}
          onChange={(e) => setUsernameInput(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              setUsername(usernameInput.trim());
              setOffset(0);
            }
          }}
          onBlur={() => {
            setUsername(usernameInput.trim());
            setOffset(0);
          }}
          placeholder={t("payments.usernameFilterPlaceholder")}
          aria-label={t("payments.usernameFilter")}
          className="h-9 w-56"
        />
      </div>

      {isLoading && !data ? (
        <div className="flex flex-col gap-3" aria-busy="true">
          {Array.from({ length: 5 }).map((_, index) => (
            <Skeleton key={index} className="h-10 w-full" />
          ))}
        </div>
      ) : !data || data.orders.length === 0 ? (
        <EmptyState variant="card" title={t("payments.noOrders")} />
      ) : (
        <motion.div
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          transition={{ duration: reduced ? 0 : 0.25 }}
          className="overflow-hidden rounded-lg border"
        >
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead className="bg-muted/50 text-left text-muted-foreground">
                <tr>
                  <th className="px-4 py-2.5 font-medium">{t("common.created")}</th>
                  <th className="px-4 py-2.5 font-medium">{t("payments.user")}</th>
                  <th className="px-4 py-2.5 font-medium">{t("wallet.channelCol")}</th>
                  <th className="px-4 py-2.5 font-medium">{t("wallet.credit")}</th>
                  <th className="px-4 py-2.5 font-medium">{t("wallet.payment")}</th>
                  <th className="px-4 py-2.5 font-medium">{t("common.status")}</th>
                  <th className="px-4 py-2.5 font-medium">{t("wallet.orderId")}</th>
                  <th className="px-4 py-2.5" />
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
                    className="border-t transition-colors hover:bg-accent/40"
                  >
                    <td className="whitespace-nowrap px-4 py-2.5 tabular-nums text-muted-foreground">
                      {formatTime(order.created_at)}
                    </td>
                    <td className="px-4 py-2.5">
                      {order.username ?? (
                        <span className="text-muted-foreground">—</span>
                      )}
                    </td>
                    <td className="px-4 py-2.5">{order.channel_name}</td>
                    <td className="px-4 py-2.5 tabular-nums">${order.credit_usd}</td>
                    <td className="whitespace-nowrap px-4 py-2.5 tabular-nums">
                      {order.pay_amount} {order.pay_currency}
                    </td>
                    <td className="px-4 py-2.5">
                      <OrderStatusBadge status={order.status} />
                    </td>
                    <td className="px-4 py-2.5">
                      <Tooltip>
                        <TooltipTrigger asChild>
                          <span className="font-mono text-xs text-muted-foreground">
                            {order.id.slice(0, 8)}
                          </span>
                        </TooltipTrigger>
                        <TooltipContent>
                          <span className="font-mono">{order.id}</span>
                          {order.error_code && (
                            <span className="ml-2 text-destructive">
                              {order.error_code}
                            </span>
                          )}
                        </TooltipContent>
                      </Tooltip>
                    </td>
                    <td className="px-4 py-2.5 text-right">
                      <Button
                        variant="ghost"
                        size="sm"
                        className="h-8 px-2"
                        disabled={order.status !== "succeeded"}
                        onClick={() => {
                          setManualChecked(false);
                          setRefundTarget(order);
                        }}
                      >
                        <Undo2 className="mr-1 h-4 w-4" />
                        {t("payments.refund")}
                      </Button>
                    </td>
                  </motion.tr>
                ))}
              </tbody>
            </table>
          </div>
          <div className="px-4 pb-3">
            <PaginationFooter
              total={data.total}
              pageSize={PAGE_SIZE}
              offset={offset}
              onOffsetChange={setOffset}
            />
          </div>
        </motion.div>
      )}

      <AlertDialog
        open={refundTarget !== null}
        onOpenChange={(open) => {
          if (!open && !refunding) setRefundTarget(null);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("payments.refundTitle")}</AlertDialogTitle>
            <AlertDialogDescription>
              {t("payments.refundDescription", {
                credit: `$${refundTarget?.credit_usd ?? ""}`,
                username: refundTarget?.username ?? refundTarget?.user_id ?? "",
              })}
            </AlertDialogDescription>
          </AlertDialogHeader>
          {needsManual && (
            <div className="flex items-start gap-3 rounded-lg border p-3">
              <Checkbox
                id="manual-refund"
                checked={manualChecked}
                onCheckedChange={(checked) => setManualChecked(checked === true)}
              />
              <Label
                htmlFor="manual-refund"
                className="text-sm font-normal leading-snug"
              >
                {t("payments.manualRefund")}
              </Label>
            </div>
          )}
          <AlertDialogFooter>
            <AlertDialogCancel disabled={refunding}>
              {t("common.cancel")}
            </AlertDialogCancel>
            <Button
              variant="destructive"
              disabled={refunding || (needsManual && !manualChecked)}
              onClick={() => void handleRefund()}
            >
              {refunding ? t("common.loading") : t("payments.refund")}
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
