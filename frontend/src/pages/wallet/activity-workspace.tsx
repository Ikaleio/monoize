import { useState } from "react";
import { useTranslation } from "react-i18next";
import { BookOpenText, ReceiptText } from "lucide-react";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
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
      <Card aria-labelledby="wallet-activity-heading">
        <CardHeader className="grid gap-4 border-b p-5 sm:grid-cols-[minmax(0,1fr)_auto] sm:items-center">
          <div className="flex min-w-0 flex-col gap-1">
            <CardTitle id="wallet-activity-heading" className="font-display text-lg">
              {t("wallet.activityTitle")}
            </CardTitle>
            <CardDescription className="text-pretty leading-relaxed">
              {t("wallet.activityDescription")}
            </CardDescription>
          </div>
          <TabsList className="grid h-12 w-full grid-cols-2 sm:h-9 sm:w-auto">
            <TabsTrigger value="orders" className="h-10 gap-2 sm:h-7">
              <ReceiptText aria-hidden="true" />
              {t("wallet.ordersTab")}
            </TabsTrigger>
            <TabsTrigger value="ledger" className="h-10 gap-2 sm:h-7">
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
            <ActivityLedger active={activeTab === "ledger"} username={username} />
          </TabsContent>
        </CardContent>
      </Card>
    </Tabs>
  );
}
