import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { CircleDollarSign, CloudDownload, Hammer, SearchX } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { ModifiedDot } from "@/components/ui/modified-dot";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { PageWrapper, motion, transitions } from "@/components/ui/motion";
import { PageHeader } from "@/components/ui/page-header";
import { useSearchParamState } from "@/hooks/use-search-param-state";
import { useUnpricedModels } from "@/lib/swr";
import { cn } from "@/lib/utils";
import { ModelPricingTab } from "./model-pricing/ModelPricingTab";
import { UnpricedModelsTab } from "./model-pricing/UnpricedModelsTab";
import { ToolPricesTab } from "./model-pricing/ToolPricesTab";
import { UpstreamSyncTab } from "./model-pricing/UpstreamSyncTab";

const TABS = ["pricing", "unpriced", "tools", "sync"] as const;
type PricingTab = (typeof TABS)[number];

/** Which edges of a horizontally scrollable element hide content. */
function useHorizontalOverflow<T extends HTMLElement>() {
  const ref = useRef<T>(null);
  const [overflow, setOverflow] = useState({ start: false, end: false });
  useEffect(() => {
    const element = ref.current;
    if (!element) return;
    const update = () =>
      setOverflow({
        start: element.scrollLeft > 1,
        end: element.scrollLeft + element.clientWidth < element.scrollWidth - 1,
      });
    update();
    element.addEventListener("scroll", update, { passive: true });
    const observer = new ResizeObserver(update);
    observer.observe(element);
    return () => {
      element.removeEventListener("scroll", update);
      observer.disconnect();
    };
  }, []);
  return [ref, overflow] as const;
}

// /dashboard/models pricing console (model-pricing.spec.md MP-UI1): exactly
// four tabs, in this order; the selected tab lives in `?tab=`.
export function ModelPricingPage() {
  const { t } = useTranslation();
  const [tab, setTab] = useSearchParamState<PricingTab>("tab", "pricing", TABS);
  const { data: unpriced } = useUnpricedModels();
  const unpricedCount = unpriced?.models.length ?? 0;
  const [toolsDirty, setToolsDirty] = useState(false);
  const [tabListRef, overflow] = useHorizontalOverflow<HTMLDivElement>();

  const triggerClass = "focus-visible:ring-inset focus-visible:ring-offset-0";

  return (
    <PageWrapper className="space-y-6">
      <motion.div
        initial={{ opacity: 0, y: -10 }}
        animate={{ opacity: 1, y: 0 }}
        transition={transitions.normal}
      >
        <PageHeader title={t("modelPricing.title")} description={t("modelPricing.description")} />
      </motion.div>

      <Tabs value={tab} onValueChange={(value) => setTab(value as PricingTab)} className="space-y-4">
        {/* MP-UI1a: edge fades show that the narrow-screen tab row scrolls. */}
        <TabsList
          ref={tabListRef}
          className={cn(
            "max-w-full justify-start overflow-x-auto",
            overflow.start && overflow.end
              ? "[mask-image:linear-gradient(to_right,transparent,black_1.5rem,black_calc(100%-1.5rem),transparent)]"
              : overflow.end
                ? "[mask-image:linear-gradient(to_right,black_calc(100%-1.5rem),transparent)]"
                : overflow.start
                  ? "[mask-image:linear-gradient(to_right,transparent,black_1.5rem)]"
                  : undefined,
          )}
        >
          <TabsTrigger value="pricing" className={triggerClass}>
            <CircleDollarSign className="mr-2 h-4 w-4" aria-hidden="true" />
            {t("modelPricing.tabs.modelPricing")}
          </TabsTrigger>
          <TabsTrigger
            value="unpriced"
            className={triggerClass}
            aria-label={
              unpricedCount > 0
                ? t("modelPricing.tabs.unpricedModelsCount", { count: unpricedCount })
                : undefined
            }
          >
            <SearchX className="mr-2 h-4 w-4" aria-hidden="true" />
            {t("modelPricing.tabs.unpricedModels")}
            {unpricedCount > 0 ? (
              <Badge variant="secondary" className="ml-2 px-1.5 tabular-nums" aria-hidden="true">
                {unpricedCount}
              </Badge>
            ) : null}
          </TabsTrigger>
          <TabsTrigger
            value="tools"
            className={triggerClass}
            aria-label={
              toolsDirty ? t("modelPricing.tabs.toolPricesUnsaved") : undefined
            }
          >
            <Hammer className="mr-2 h-4 w-4" aria-hidden="true" />
            {t("modelPricing.tabs.toolPrices")}
            {toolsDirty ? <ModifiedDot className="ml-2" /> : null}
          </TabsTrigger>
          <TabsTrigger value="sync" className={triggerClass}>
            <CloudDownload className="mr-2 h-4 w-4" aria-hidden="true" />
            {t("modelPricing.tabs.upstreamSync")}
          </TabsTrigger>
        </TabsList>

        <motion.div
          initial={{ opacity: 0, y: 20 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ delay: 0.1, ...transitions.normal }}
        >
          <TabsContent value="pricing" className="mt-0">
            <ModelPricingTab />
          </TabsContent>
          <TabsContent value="unpriced" className="mt-0">
            <UnpricedModelsTab />
          </TabsContent>
          {/* MP-UI5a: kept mounted so an unsaved tool-price draft survives tab switches. */}
          <TabsContent value="tools" forceMount className="mt-0 data-[state=inactive]:hidden">
            <ToolPricesTab onDirtyChange={setToolsDirty} />
          </TabsContent>
          <TabsContent value="sync" className="mt-0">
            <UpstreamSyncTab />
          </TabsContent>
        </motion.div>
      </Tabs>
    </PageWrapper>
  );
}
