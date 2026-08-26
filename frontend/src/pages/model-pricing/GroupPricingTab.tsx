import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Percent, Save, Users } from "lucide-react";
import { toast } from "sonner";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { EmptyState } from "@/components/ui/empty-state";
import { TablePageSkeleton } from "@/components/ui/page-skeleton";
import {
  DataTableShell,
  VirtualTableCell,
  VirtualTableHeaderCell,
} from "@/components/ui/data-table-shell";
import { updateGroupOptimistic, useDashboardGroups } from "@/lib/swr";
import { isValidUsdDecimal } from "./shared";

export function GroupPricingTab() {
  const { t } = useTranslation();
  const { data: groups = [], isLoading } = useDashboardGroups();
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const [savingId, setSavingId] = useState<string | null>(null);

  useEffect(() => {
    setDrafts((previous) => {
      const next = { ...previous };
      for (const group of groups) {
        if (!(group.id in next)) next[group.id] = group.billing_ratio;
      }
      return next;
    });
  }, [groups]);

  const saveRatio = async (groupId: string) => {
    const value = (drafts[groupId] ?? "").trim();
    if (!isValidUsdDecimal(value)) {
      toast.error(
        t(
          "modelPricing.groupPricing.invalidRatio",
          "Billing ratio must be a non-negative decimal with at most 9 fractional digits"
        )
      );
      return;
    }
    setSavingId(groupId);
    try {
      const updated = await updateGroupOptimistic(
        groupId,
        { billing_ratio: value },
        groups,
        (error) =>
          toast.error(t("modelPricing.groupPricing.saveFailed", "Failed to save ratio"), {
            description: error.message,
          })
      );
      setDrafts((previous) => ({ ...previous, [groupId]: updated.billing_ratio }));
      toast.success(t("modelPricing.groupPricing.saveSuccess", "Billing ratio saved"));
    } catch {
      return;
    } finally {
      setSavingId(null);
    }
  };

  if (isLoading) {
    return <TablePageSkeleton showToolbar />;
  }

  return (
    <DataTableShell
      toolbar={
        <div className="flex items-center gap-2 text-base font-semibold">
          <Percent className="h-5 w-5" />
          {t("modelPricing.tabs.groupPricing", "Group Pricing")}
        </div>
      }
      isEmpty={groups.length === 0}
      emptyState={
        <EmptyState
          icon={<Users className="h-12 w-12" />}
          title={t("modelPricing.groupPricing.noGroups", "No groups")}
          description={t(
            "modelPricing.groupPricing.noGroupsDesc",
            "Create groups on the Groups page to set per-group billing ratios."
          )}
        />
      }
    >
      <div className="overflow-x-auto">
        <table className="w-full caption-bottom text-sm">
          <thead className="[&_tr]:border-b">
            <tr className="border-b">
              <VirtualTableHeaderCell className="min-w-[200px]">
                {t("modelPricing.groupPricing.group", "Group")}
              </VirtualTableHeaderCell>
              <VirtualTableHeaderCell>
                {t("modelPricing.groupPricing.description", "Description")}
              </VirtualTableHeaderCell>
              <VirtualTableHeaderCell className="w-[180px]">
                {t("modelPricing.groupPricing.ratio", "Billing ratio")}
              </VirtualTableHeaderCell>
              <VirtualTableHeaderCell className="w-[120px]">
                {t("common.actions", "Actions")}
              </VirtualTableHeaderCell>
            </tr>
          </thead>
          <tbody className="[&_tr:last-child]:border-0">
            {groups.map((group) => {
              const draft = drafts[group.id] ?? group.billing_ratio;
              const dirty = draft.trim() !== group.billing_ratio;
              return (
                <tr key={group.id} className="border-b">
                  <VirtualTableCell>
                    <div className="flex items-center gap-2">
                      <span className="font-medium">{group.name}</span>
                      {group.is_default ? (
                        <Badge variant="secondary" className="text-xs">
                          {t("modelPricing.groupPricing.default", "Default")}
                        </Badge>
                      ) : null}
                    </div>
                  </VirtualTableCell>
                  <VirtualTableCell className="text-sm text-muted-foreground">
                    {group.description || "—"}
                  </VirtualTableCell>
                  <VirtualTableCell>
                    <Input
                      inputMode="decimal"
                      value={draft}
                      onChange={(event) =>
                        setDrafts((previous) => ({
                          ...previous,
                          [group.id]: event.target.value,
                        }))
                      }
                      className="font-mono"
                    />
                  </VirtualTableCell>
                  <VirtualTableCell>
                    <Button
                      variant="outline"
                      size="sm"
                      disabled={!dirty || savingId === group.id}
                      onClick={() => void saveRatio(group.id)}
                    >
                      <Save className="mr-1.5 h-3.5 w-3.5" />
                      {savingId === group.id
                        ? t("common.saving", "Saving...")
                        : t("common.save", "Save")}
                    </Button>
                  </VirtualTableCell>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    </DataTableShell>
  );
}
