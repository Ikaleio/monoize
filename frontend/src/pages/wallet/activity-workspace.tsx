import { useState } from "react";
import { useTranslation } from "react-i18next";
import { BookOpenText, ReceiptText } from "lucide-react";
import { Card, CardContent, CardHeader } from "@/components/ui/card";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { ActivityOrders } from "./activity-orders";
import { ActivityLedger } from "./activity-ledger";

type ActivityTab = "orders" | "ledger";

export function ActivityWorkspace({
  pageSize,
  username,
}: {
  pageSize: number;
  username: string;
}) {
  const { t } = useTranslation();
  const [activeTab, setActiveTab] = useState<ActivityTab>("orders");
  const [ordersOffset, setOrdersOffset] = useState(0);

  return (
    <Tabs
      value={activeTab}
      onValueChange={(value) => setActiveTab(value as ActivityTab)}
    >
      <Card role="region" aria-label={t("wallet.activityTitle")}>
        <CardHeader className="border-b p-4">
          <TabsList className="grid h-auto w-full grid-cols-2 sm:w-fit">
            <TabsTrigger
              value="orders"
              className="h-11 gap-2 [&_svg]:size-4 [&_svg]:shrink-0"
            >
              <ReceiptText aria-hidden="true" />
              {t("wallet.ordersTab")}
            </TabsTrigger>
            <TabsTrigger
              value="ledger"
              className="h-11 gap-2 [&_svg]:size-4 [&_svg]:shrink-0"
            >
              <BookOpenText aria-hidden="true" />
              {t("wallet.ledgerTab")}
            </TabsTrigger>
          </TabsList>
        </CardHeader>

        <CardContent className="p-0">
          <TabsContent
            forceMount
            value="orders"
            className="mt-0 data-[state=inactive]:hidden"
          >
            <ActivityOrders
              active={activeTab === "orders"}
              pageSize={pageSize}
              offset={ordersOffset}
              onOffsetChange={setOrdersOffset}
              username={username}
            />
          </TabsContent>
          <TabsContent
            forceMount
            value="ledger"
            className="mt-0 data-[state=inactive]:hidden"
          >
            <ActivityLedger
              active={activeTab === "ledger"}
              username={username}
            />
          </TabsContent>
        </CardContent>
      </Card>
    </Tabs>
  );
}
