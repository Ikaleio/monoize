import { useTranslation } from "react-i18next";
import { PageWrapper } from "@/components/ui/motion";
import { PageHeader } from "@/components/ui/page-header";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { ChannelsTab } from "./payments/channels-tab";
import { OrdersTab } from "./payments/orders-tab";

/**
 * `/dashboard/payments` (recharge-system.spec.md §11): admin page with two
 * tabs — payment-channel CRUD and the global recharge-orders view (RC-M1).
 */
export function PaymentsPage() {
  const { t } = useTranslation();

  return (
    <PageWrapper className="space-y-6">
      <PageHeader
        title={t("payments.title")}
        description={t("payments.description")}
      />
      <Tabs defaultValue="channels">
        <TabsList>
          <TabsTrigger value="channels">{t("payments.channelsTab")}</TabsTrigger>
          <TabsTrigger value="orders">{t("payments.ordersTab")}</TabsTrigger>
        </TabsList>
        <TabsContent value="channels" className="mt-4">
          <ChannelsTab />
        </TabsContent>
        <TabsContent value="orders" className="mt-4">
          <OrdersTab />
        </TabsContent>
      </Tabs>
    </PageWrapper>
  );
}
