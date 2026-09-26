import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { ReceiptText, Undo2 } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Checkbox } from "@/components/ui/checkbox";
import { Skeleton } from "@/components/ui/skeleton";
import { EmptyState } from "@/components/ui/empty-state";
import { Label } from "@/components/ui/label";
import { DataTableShell } from "@/components/ui/data-table-shell";
import {
  DataList,
  DataListActions,
  DataListBody,
  DataListCell,
  DataListHead,
  DataListHeader,
  DataListRow,
} from "@/components/ui/data-list";
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
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
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

export interface OrderFilterState {
  status: RechargeOrderStatus | "all";
  username: string;
}

interface OrderFiltersProps {
  value: OrderFilterState;
  onChange: (next: OrderFilterState) => void;
}

/** RC-M3 status and username filters, rendered in the Payments toolbar row. */
export function OrderFilters({ value, onChange }: OrderFiltersProps) {
  const { t } = useTranslation();
  const [usernameInput, setUsernameInput] = useState(value.username);

  const applyUsername = () => {
    const username = usernameInput.trim();
    if (username !== value.username) onChange({ ...value, username });
  };

  return (
    <div className="flex w-full flex-wrap items-center gap-2 sm:w-auto">
      <Select
        value={value.status}
        onValueChange={(status) =>
          onChange({
            ...value,
            status: ORDER_STATUSES.find((candidate) => candidate === status) ?? "all",
          })
        }
      >
        <SelectTrigger className="w-full sm:w-40" aria-label={t("payments.statusFilter")}>
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="all">{t("payments.statusAll")}</SelectItem>
          {ORDER_STATUSES.map((status) => (
            <SelectItem key={status} value={status}>
              {t(`wallet.status.${status}`)}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
      <Input
        value={usernameInput}
        onChange={(event) => setUsernameInput(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Enter") applyUsername();
        }}
        onBlur={applyUsername}
        placeholder={t("payments.usernameFilterPlaceholder")}
        aria-label={t("payments.usernameFilter")}
        className="w-full sm:w-56"
      />
    </div>
  );
}

interface OrdersTabProps {
  filters: OrderFilterState;
  offset: number;
  onOffsetChange: (offset: number) => void;
}

/**
 * RC-M3 Orders tab: the admin view of all users' orders with a per-row full
 * refund limited to `succeeded` orders. Channels without provider-side refund
 * require the RC-R4 manual acknowledgment.
 */
export function OrdersTab({ filters, offset, onOffsetChange }: OrdersTabProps) {
  const { t } = useTranslation();
  const [refundTarget, setRefundTarget] = useState<RechargeOrder | null>(null);
  const [manualChecked, setManualChecked] = useState(false);
  const [refunding, setRefunding] = useState(false);

  const queryFilters = useMemo(
    () => ({
      status: filters.status === "all" ? undefined : filters.status,
      username: filters.username || undefined,
    }),
    [filters],
  );
  const pageKey = rechargeOrdersSWRKey(PAGE_SIZE, offset, queryFilters);
  const { data, isLoading } = useRechargeOrders(PAGE_SIZE, offset, queryFilters);

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

  if (isLoading && !data) {
    return (
      <DataTableShell aria-busy="true">
        <div className="divide-y">
          {Array.from({ length: 5 }).map((_, index) => (
            <div key={index} className="px-4 py-3">
              <Skeleton className="h-6 w-full" />
            </div>
          ))}
        </div>
      </DataTableShell>
    );
  }

  return (
    <>
      <DataTableShell
        isEmpty={!data || data.orders.length === 0}
        emptyState={
          <EmptyState
            icon={<ReceiptText className="size-10" aria-hidden="true" />}
            title={t("payments.noOrders")}
          />
        }
      >
        <DataList columns="9.5rem minmax(0,1fr) minmax(0,1fr) 5.5rem 7rem 5rem 5.5rem 6.5rem">
          <DataListHeader>
            <DataListHead>{t("common.created")}</DataListHead>
            <DataListHead>{t("payments.user")}</DataListHead>
            <DataListHead>{t("wallet.channelCol")}</DataListHead>
            <DataListHead align="end">{t("wallet.credit")}</DataListHead>
            <DataListHead align="end">{t("wallet.payment")}</DataListHead>
            <DataListHead>{t("common.status")}</DataListHead>
            <DataListHead>{t("wallet.orderId")}</DataListHead>
            <DataListHead align="end">{t("common.actions")}</DataListHead>
          </DataListHeader>
          <TooltipProvider delayDuration={200}>
            <DataListBody aria-label={t("payments.ordersTab")}>
              {data?.orders.map((order) => (
                <DataListRow key={order.id}>
                  <DataListCell label={t("common.created")}>
                    <span className="tabular-nums text-muted-foreground">
                      {formatTime(order.created_at)}
                    </span>
                  </DataListCell>
                  <DataListCell primary>
                    {order.username ? (
                      <span className="block truncate font-medium" title={order.username}>
                        {order.username}
                      </span>
                    ) : (
                      <span className="text-muted-foreground">—</span>
                    )}
                  </DataListCell>
                  <DataListCell label={t("wallet.channelCol")}>
                    <span className="block truncate" title={order.channel_name}>
                      {order.channel_name}
                    </span>
                  </DataListCell>
                  <DataListCell label={t("wallet.credit")} align="end">
                    <span className="tabular-nums">${order.credit_usd}</span>
                  </DataListCell>
                  <DataListCell label={t("wallet.payment")} align="end">
                    <span className="whitespace-nowrap tabular-nums">
                      {order.pay_amount} {order.pay_currency}
                    </span>
                  </DataListCell>
                  <DataListCell label={t("common.status")}>
                    <OrderStatusBadge status={order.status} />
                  </DataListCell>
                  <DataListCell label={t("wallet.orderId")}>
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <button
                          type="button"
                          className="rounded-sm font-mono text-muted-foreground underline-offset-4 hover:text-foreground hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                        >
                          {order.id.slice(0, 8)}
                        </button>
                      </TooltipTrigger>
                      <TooltipContent>
                        <span className="font-mono">{order.id}</span>
                        {order.error_code && (
                          <span className="ml-2 text-error-foreground">{order.error_code}</span>
                        )}
                      </TooltipContent>
                    </Tooltip>
                  </DataListCell>
                  <DataListActions>
                    <Button
                      variant="ghost"
                      className="h-11 px-3 sm:h-9"
                      disabled={order.status !== "succeeded"}
                      onClick={() => {
                        setManualChecked(false);
                        setRefundTarget(order);
                      }}
                    >
                      <Undo2 aria-hidden="true" />
                      {t("payments.refund")}
                    </Button>
                  </DataListActions>
                </DataListRow>
              ))}
            </DataListBody>
          </TooltipProvider>
        </DataList>
        <div className="border-t px-4 pb-3 empty:hidden">
          {data ? (
            <PaginationFooter
              total={data.total}
              pageSize={PAGE_SIZE}
              offset={offset}
              onOffsetChange={onOffsetChange}
            />
          ) : null}
        </div>
      </DataTableShell>

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
    </>
  );
}
