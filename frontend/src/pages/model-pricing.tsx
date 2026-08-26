import { useTranslation } from "react-i18next";
import {
  CircleDollarSign,
  CloudDownload,
  Hammer,
  Percent,
  SearchX,
} from "lucide-react";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { PageWrapper, motion, transitions } from "@/components/ui/motion";
import { PageHeader } from "@/components/ui/page-header";
import { ModelPricingTab } from "./model-pricing/ModelPricingTab";
import { UnpricedModelsTab } from "./model-pricing/UnpricedModelsTab";
import { ToolPricesTab } from "./model-pricing/ToolPricesTab";
import { UpstreamSyncTab } from "./model-pricing/UpstreamSyncTab";
import { GroupPricingTab } from "./model-pricing/GroupPricingTab";

// /dashboard/models pricing console (model-pricing.spec.md MP-UI1): exactly
// five tabs, in this order.
export function ModelPricingPage() {
  const { t } = useTranslation();

  return (
    <PageWrapper className="space-y-6">
      <motion.div
        initial={{ opacity: 0, y: -10 }}
        animate={{ opacity: 1, y: 0 }}
        transition={transitions.normal}
      >
        <PageHeader
          title={t("modelPricing.title", "Model Pricing")}
          description={t(
            "modelPricing.description",
            "Per-model prices, tool prices, upstream sync, and group billing ratios"
          )}
        />
      </motion.div>

      <Tabs defaultValue="model-pricing" className="space-y-4">
        <TabsList className="max-w-full justify-start overflow-x-auto">
          <TabsTrigger value="model-pricing">
            <CircleDollarSign className="mr-2 h-4 w-4" />
            {t("modelPricing.tabs.modelPricing", "Model Pricing")}
          </TabsTrigger>
          <TabsTrigger value="unpriced-models">
            <SearchX className="mr-2 h-4 w-4" />
            {t("modelPricing.tabs.unpricedModels", "Unpriced Models")}
          </TabsTrigger>
          <TabsTrigger value="tool-prices">
            <Hammer className="mr-2 h-4 w-4" />
            {t("modelPricing.tabs.toolPrices", "Tool Prices")}
          </TabsTrigger>
          <TabsTrigger value="upstream-sync">
            <CloudDownload className="mr-2 h-4 w-4" />
            {t("modelPricing.tabs.upstreamSync", "Upstream Sync")}
          </TabsTrigger>
          <TabsTrigger value="group-pricing">
            <Percent className="mr-2 h-4 w-4" />
            {t("modelPricing.tabs.groupPricing", "Group Pricing")}
          </TabsTrigger>
        </TabsList>

        <motion.div
          initial={{ opacity: 0, y: 20 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ delay: 0.1, ...transitions.normal }}
        >
          <TabsContent value="model-pricing" className="mt-0">
            <ModelPricingTab />
          </TabsContent>
          <TabsContent value="unpriced-models" className="mt-0">
            <UnpricedModelsTab />
          </TabsContent>
          <TabsContent value="tool-prices" className="mt-0">
            <ToolPricesTab />
          </TabsContent>
          <TabsContent value="upstream-sync" className="mt-0">
            <UpstreamSyncTab />
          </TabsContent>
          <TabsContent value="group-pricing" className="mt-0">
            <GroupPricingTab />
          </TabsContent>
        </motion.div>
      </Tabs>
    </PageWrapper>
  );
}
