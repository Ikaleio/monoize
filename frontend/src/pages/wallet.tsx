import { useState } from "react";
import { useTranslation } from "react-i18next";
import { BookOpenText, ReceiptText } from "lucide-react";
import { useAuth } from "@/hooks/use-auth";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { PageHeader } from "@/components/ui/page-header";
import { PageWrapper, StaggerItem, StaggerList } from "@/components/ui/motion";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { rechargeOrdersSWRKey } from "@/lib/swr";
import { BalanceCard } from "./wallet/balance-card";
import { LedgerSection } from "./wallet/ledger-section";
import { OrdersSection } from "./wallet/orders-section";
import { PlanCard } from "./wallet/plan-card";
import { RechargeCard } from "./wallet/recharge-card";

const ORDERS_PAGE_SIZE = 10;

type ActivityTab = "orders" | "ledger";

interface ActivityCardProps {
  activeTab: ActivityTab;
  ordersOffset: number;
  onOrdersOffsetChange: (offset: number) => void;
  username: string;
}

function ActivityCard({
  activeTab,
  ordersOffset,
  onOrdersOffsetChange,
  username,
}: ActivityCardProps) {
  const { t } = useTranslation();

  return (
    <Card>
      <CardHeader className="flex flex-col gap-4 p-5 pb-3 sm:flex-row sm:items-center sm:justify-between">
        <div className="flex min-w-0 flex-col gap-1.5">
          <CardTitle className="text-balance font-display text-lg">
            {t("wallet.activityTitle")}
          </CardTitle>
          <CardDescription className="text-pretty">
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
      <CardContent className="p-5 pt-2">
        <TabsContent
          forceMount
          value="orders"
          className="mt-0 data-[state=inactive]:hidden"
        >
          <OrdersSection
            active={activeTab === "orders"}
            pageSize={ORDERS_PAGE_SIZE}
            offset={ordersOffset}
            onOffsetChange={onOrdersOffsetChange}
            username={username}
          />
        </TabsContent>
        <TabsContent
          forceMount
          value="ledger"
          className="mt-0 data-[state=inactive]:hidden"
        >
          <LedgerSection active={activeTab === "ledger"} username={username} />
        </TabsContent>
      </CardContent>
    </Card>
  );
}

/** `/dashboard/wallet` implements the wallet-stage and activity-tab contract in RC-W1. */
export function WalletPage() {
  const { t } = useTranslation();
  const { user } = useAuth();
  const [activityTab, setActivityTab] = useState<ActivityTab>("orders");
  const [ordersOffset, setOrdersOffset] = useState(0);

  if (!user) return null;

  return (
    <PageWrapper className="flex flex-col gap-6 pb-8">
      <PageHeader
        title={t("wallet.title")}
        description={t("wallet.description")}
      />

      <StaggerList className="flex flex-col gap-5">
        <StaggerItem>
          <section
            aria-label={t("wallet.title")}
            className="grid items-stretch gap-4 lg:grid-cols-12"
          >
            <BalanceCard user={user} />
            <RechargeCard
              ordersFirstPageKey={rechargeOrdersSWRKey(ORDERS_PAGE_SIZE, 0, {
                username: user.username,
              })}
            />
          </section>
        </StaggerItem>

        <StaggerItem>
          <PlanCard />
        </StaggerItem>

        <StaggerItem>
          <Tabs
            value={activityTab}
            onValueChange={(value) => setActivityTab(value as ActivityTab)}
          >
            <ActivityCard
              activeTab={activityTab}
              ordersOffset={ordersOffset}
              onOrdersOffsetChange={setOrdersOffset}
              username={user.username}
            />
          </Tabs>
        </StaggerItem>
      </StaggerList>
    </PageWrapper>
  );
}
