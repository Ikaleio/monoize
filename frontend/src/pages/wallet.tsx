import { useRef } from "react";
import { useTranslation } from "react-i18next";
import { useAuth } from "@/hooks/use-auth";
import { PageHeader } from "@/components/ui/page-header";
import { PageWrapper, StaggerItem, StaggerList } from "@/components/ui/motion";
import { rechargeOrdersSWRKey } from "@/lib/swr";
import { WalletSummaryBand } from "./wallet/wallet-summary-band";
import { RechargeDesk } from "./wallet/recharge-desk";
import { PlanAllowance } from "./wallet/plan-allowance";
import { ActivityWorkspace } from "./wallet/activity-workspace";

const ORDERS_PAGE_SIZE = 10;

/** `/dashboard/wallet` implements the wallet workspace contract in RC-W1. */
export function WalletPage() {
  const { t } = useTranslation();
  const { user } = useAuth();
  const rechargeRegionRef = useRef<HTMLElement>(null);

  if (!user) return null;

  const focusRecharge = () => {
    rechargeRegionRef.current?.scrollIntoView({
      behavior: "smooth",
      block: "start",
    });
    rechargeRegionRef.current?.focus({ preventScroll: true });
  };

  return (
    <PageWrapper className="flex flex-col gap-6 pb-8">
      <PageHeader
        title={t("wallet.title")}
        description={t("wallet.description")}
      />

      <StaggerList className="flex flex-col gap-6">
        <StaggerItem>
          <WalletSummaryBand user={user} onRecharge={focusRecharge} />
        </StaggerItem>

        <StaggerItem>
          <section
            aria-label={t("wallet.rechargeTitle")}
            className="grid items-start gap-5 lg:grid-cols-12"
          >
            <RechargeDesk
              ref={rechargeRegionRef}
              className="min-w-0 lg:col-span-5"
              ordersFirstPageKey={rechargeOrdersSWRKey(ORDERS_PAGE_SIZE, 0, {
                username: user.username,
              })}
            />
            <PlanAllowance className="min-w-0 lg:col-span-7" />
          </section>
        </StaggerItem>

        <StaggerItem>
          <ActivityWorkspace
            pageSize={ORDERS_PAGE_SIZE}
            username={user.username}
          />
        </StaggerItem>
      </StaggerList>
    </PageWrapper>
  );
}
