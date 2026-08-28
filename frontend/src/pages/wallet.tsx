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
      <CardHeader className="flex flex-col gap-4 pb-2 sm:flex-row sm:items-center sm:justify-between">
        <div className="flex min-w-0 flex-col gap-1.5">
          <CardTitle className="text-balance font-display text-2xl">
            {t("wallet.activityTitle")}
          </CardTitle>
          <CardDescription className="text-pretty">
            {t("wallet.activityDescription")}
          </CardDescription>
        </div>
        <TabsList className="grid w-full grid-cols-2 sm:w-auto">
          <TabsTrigger value="orders" className="gap-2">
            <ReceiptText aria-hidden="true" />
            {t("wallet.ordersTab")}
          </TabsTrigger>
          <TabsTrigger value="ledger" className="gap-2">
            <BookOpenText aria-hidden="true" />
            {t("wallet.ledgerTab")}
          </TabsTrigger>
        </TabsList>
      </CardHeader>
      <CardContent>
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
    <PageWrapper className="flex flex-col gap-8 pb-10">
      <header className="flex min-w-0 flex-col gap-2">
        <h1 className="text-balance font-display text-3xl font-semibold tracking-tight sm:text-4xl">
          {t("wallet.title")}
        </h1>
        <p className="max-w-2xl text-pretty text-sm leading-relaxed text-muted-foreground">
          {t("wallet.description")}
        </p>
      </header>

      <StaggerList className="flex flex-col gap-6">
        <StaggerItem>
          <Card className="overflow-hidden">
            <CardContent className="grid p-0 lg:grid-cols-[minmax(0,0.9fr)_minmax(0,1.1fr)]">
              <BalanceCard user={user} />
              <RechargeCard
                ordersFirstPageKey={rechargeOrdersSWRKey(ORDERS_PAGE_SIZE, 0, {
                  username: user.username,
                })}
              />
            </CardContent>
          </Card>
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
