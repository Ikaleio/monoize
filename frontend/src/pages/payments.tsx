import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Plus } from "lucide-react";
import { Button } from "@/components/ui/button";
import { PageWrapper } from "@/components/ui/motion";
import { PageHeader } from "@/components/ui/page-header";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { useSearchParamState } from "@/hooks/use-search-param-state";
import type { PaymentChannel } from "@/lib/api";
import { ORDER_STATUSES } from "@/lib/recharge";
import { usePaymentChannels } from "@/lib/swr";
import { ChannelDialog } from "./payments/channel-dialog";
import { ChannelsTab } from "./payments/channels-tab";
import { OrderFilters, OrdersTab, type OrderFilterState } from "./payments/orders-tab";

const PAYMENTS_TABS = ["channels", "orders"] as const;
const STATUS_FILTERS: readonly OrderFilterState["status"][] = ["all", ...ORDER_STATUSES];

/**
 * `/dashboard/payments` (recharge-system.spec.md §11): admin page with two
 * tabs — payment-channel CRUD and the global recharge-orders view (RC-M1).
 * The page owns the shared toolbar row, so it also owns the channel dialog and
 * the order query state that the toolbar controls. Tab and filters live in
 * the URL (RC-M1).
 */
export function PaymentsPage() {
  const { t } = useTranslation();
  const [tab, setTab] = useSearchParamState<(typeof PAYMENTS_TABS)[number]>(
    "tab",
    "channels",
    PAYMENTS_TABS,
  );
  const [status, setStatus] = useSearchParamState<OrderFilterState["status"]>(
    "status",
    "all",
    STATUS_FILTERS,
  );
  const [username, setUsername] = useSearchParamState<string>("username", "");
  const { data: channels = [] } = usePaymentChannels();
  const [dialogOpen, setDialogOpen] = useState(false);
  const [editTarget, setEditTarget] = useState<PaymentChannel | null>(null);
  const orderFilters: OrderFilterState = { status, username };
  const [ordersOffset, setOrdersOffset] = useState(0);

  const openChannelDialog = (channel: PaymentChannel | null) => {
    setEditTarget(channel);
    setDialogOpen(true);
  };

  return (
    <PageWrapper className="space-y-6">
      <PageHeader title={t("payments.title")} description={t("payments.description")} />
      <Tabs
        value={tab}
        onValueChange={(value) => setTab(value === "orders" ? "orders" : "channels")}
        className="space-y-3"
      >
        <div className="flex flex-wrap items-center justify-between gap-3">
          <TabsList>
            <TabsTrigger value="channels">{t("payments.channelsTab")}</TabsTrigger>
            <TabsTrigger value="orders">{t("payments.ordersTab")}</TabsTrigger>
          </TabsList>
          {tab === "channels" ? (
            <Button onClick={() => openChannelDialog(null)}>
              <Plus aria-hidden="true" />
              {t("payments.create")}
            </Button>
          ) : (
            <OrderFilters
              value={orderFilters}
              onChange={(next) => {
                // Filters change one at a time; write only the changed param.
                if (next.status !== status) setStatus(next.status);
                if (next.username !== username) setUsername(next.username);
                setOrdersOffset(0);
              }}
            />
          )}
        </div>
        <TabsContent value="channels" className="mt-0">
          <ChannelsTab onCreate={() => openChannelDialog(null)} onEdit={openChannelDialog} />
        </TabsContent>
        <TabsContent value="orders" className="mt-0">
          <OrdersTab filters={orderFilters} offset={ordersOffset} onOffsetChange={setOrdersOffset} />
        </TabsContent>
      </Tabs>
      <ChannelDialog
        open={dialogOpen}
        onOpenChange={setDialogOpen}
        channel={editTarget}
        channels={channels}
      />
    </PageWrapper>
  );
}
