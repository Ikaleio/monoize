import { useRef } from "react";
import { useTranslation } from "react-i18next";
import { useSearchParams } from "react-router-dom";
import { motion, useReducedMotion } from "framer-motion";
import { useAuth } from "@/hooks/use-auth";
import { PageWrapper, springs } from "@/components/ui/motion";
import { PageHeader } from "@/components/ui/page-header";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { rechargeOrdersSWRKey } from "@/lib/swr";
import { WalletBalance } from "./wallet/wallet-balance";
import { RechargeDesk } from "./wallet/recharge-desk";
import { PlanAllowance } from "./wallet/plan-allowance";
import { ActivityWorkspace } from "./wallet/activity-workspace";

const ORDERS_PAGE_SIZE = 10;
const WALLET_TABS = ["overview", "recharge", "activity"] as const;
type WalletTab = (typeof WALLET_TABS)[number];

/** Display account balances, funding controls, and wallet activity in persistent tabs. */
export function WalletPage() {
  const { t } = useTranslation();
  const { user } = useAuth();
  const [searchParams, setSearchParams] = useSearchParams();
  const rechargeTabRef = useRef<HTMLButtonElement>(null);
  const reduced = useReducedMotion();
  const requestedTab = searchParams.get("tab");
  const activeTab: WalletTab = WALLET_TABS.includes(requestedTab as WalletTab)
    ? (requestedTab as WalletTab)
    : requestedTab === null && searchParams.has("order_id")
      ? "activity"
      : "overview";

  if (!user) return null;

  const selectTab = (value: string) => {
    setSearchParams((previous) => {
      const next = new URLSearchParams(previous);
      next.set("tab", value);
      return next;
    });
  };

  const openRecharge = () => {
    selectTab("recharge");
    rechargeTabRef.current?.focus();
  };

  return (
    <PageWrapper className="space-y-6 pb-8">
      <PageHeader title={t("wallet.title")} description={t("wallet.description")} />
      <WalletBalance user={user} onRecharge={openRecharge} />
      <Tabs value={activeTab} onValueChange={selectTab} className="min-w-0">
        <div className="border-b">
          <TabsList
            aria-label={t("wallet.title")}
            className="flex h-auto justify-start gap-1 rounded-none bg-transparent p-0 sm:gap-5"
          >
            {WALLET_TABS.map((tab) => (
              <TabsTrigger
                key={tab}
                ref={tab === "recharge" ? rechargeTabRef : undefined}
                value={tab}
                className="relative min-h-11 min-w-0 rounded-none px-3 py-3 text-sm data-[state=active]:bg-transparent data-[state=active]:text-foreground data-[state=active]:shadow-none sm:px-1 sm:text-base"
              >
                {t(`wallet.tabs.${tab}`)}
                {activeTab === tab ? (
                  <motion.span
                    layoutId={reduced ? undefined : "wallet-tab-indicator"}
                    transition={reduced ? { duration: 0 } : springs.snappy}
                    className="absolute inset-x-0 bottom-0 h-0.5 bg-primary"
                  />
                ) : null}
              </TabsTrigger>
            ))}
          </TabsList>
        </div>

        {WALLET_TABS.map((tab) => (
          <TabsContent
            key={tab}
            forceMount
            value={tab}
            className="mt-4 min-w-0 data-[state=inactive]:hidden"
          >
            <motion.div
              initial={false}
              animate={{
                opacity: activeTab === tab ? 1 : 0,
                y: activeTab === tab || reduced ? 0 : 8,
              }}
              transition={reduced ? { duration: 0 } : springs.gentle}
            >
              {tab === "overview" ? (
                <PlanAllowance className="min-w-0 rounded-lg border bg-card" />
              ) : tab === "recharge" ? (
                <div className="min-w-0">
                  <RechargeDesk
                    ordersFirstPageKey={rechargeOrdersSWRKey(
                      ORDERS_PAGE_SIZE,
                      0,
                      { username: user.username },
                    )}
                  />
                </div>
              ) : (
                <ActivityWorkspace
                  pageSize={ORDERS_PAGE_SIZE}
                  username={user.username}
                />
              )}
            </motion.div>
          </TabsContent>
        ))}
      </Tabs>
    </PageWrapper>
  );
}
