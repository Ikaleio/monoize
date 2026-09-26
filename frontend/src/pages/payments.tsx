import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Plus } from "lucide-react";
import { Button } from "@/components/ui/button";
import { PageWrapper } from "@/components/ui/motion";
import { PageHeader } from "@/components/ui/page-header";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import type { PaymentChannel } from "@/lib/api";
import { usePaymentChannels } from "@/lib/swr";
import { ChannelDialog } from "./payments/channel-dialog";
import { ChannelsTab } from "./payments/channels-tab";
import { OrderFilters, OrdersTab, type OrderFilterState } from "./payments/orders-tab";

type PaymentsTab = "channels" | "orders";

/**
 * `/dashboard/payments` (recharge-system.spec.md §11): admin page with two
 * tabs — payment-channel CRUD and the global recharge-orders view (RC-M1).
 * The page owns the shared toolbar row, so it also owns the channel dialog and
 * the order query state that the toolbar controls.
 */
export function PaymentsPage() {
  const { t } = useTranslation();
  const [tab, setTab] = useState<PaymentsTab>("channels");
  const { data: channels = [] } = usePaymentChannels();
  const [dialogOpen, setDialogOpen] = useState(false);
  const [editTarget, setEditTarget] = useState<PaymentChannel | null>(null);
  const [orderFilters, setOrderFilters] = useState<OrderFilterState>({ status: "all", username: "" });
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
                setOrderFilters(next);
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
