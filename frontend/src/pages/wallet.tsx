import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useAuth } from "@/hooks/use-auth";
import { PageWrapper, StaggerItem, StaggerList } from "@/components/ui/motion";
import { PageHeader } from "@/components/ui/page-header";
import { rechargeOrdersSWRKey } from "@/lib/swr";
import { BalanceCard } from "./wallet/balance-card";
import { RechargeCard } from "./wallet/recharge-card";
import { OrdersSection } from "./wallet/orders-section";
import { LedgerSection } from "./wallet/ledger-section";
import { PlanCard } from "./wallet/plan-card";

const ORDERS_PAGE_SIZE = 10;

/**
 * `/dashboard/wallet` (recharge-system.spec.md §10): balance card, recharge
 * card, recharge-orders section, ledger section — top to bottom (RC-W1).
 */
export function WalletPage() {
  const { t } = useTranslation();
  const { user } = useAuth();
  const [ordersOffset, setOrdersOffset] = useState(0);

  if (!user) return null;

  return (
    <PageWrapper className="space-y-6">
      <PageHeader title={t("wallet.title")} description={t("wallet.description")} />
      <StaggerList className="flex flex-col gap-6">
        <StaggerItem>
          <div className="grid gap-6 lg:grid-cols-2">
            <BalanceCard user={user} />
            <RechargeCard
              ordersFirstPageKey={rechargeOrdersSWRKey(ORDERS_PAGE_SIZE, 0, {
                username: user.username,
              })}
            />
          </div>
        </StaggerItem>
        <StaggerItem>
          <PlanCard />
        </StaggerItem>
        <StaggerItem>
          <OrdersSection
            pageSize={ORDERS_PAGE_SIZE}
            offset={ordersOffset}
            onOffsetChange={setOrdersOffset}
            username={user.username}
          />
        </StaggerItem>
        <StaggerItem>
          <LedgerSection username={user.username} />
        </StaggerItem>
      </StaggerList>
    </PageWrapper>
  );
}
