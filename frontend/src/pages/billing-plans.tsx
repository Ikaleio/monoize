import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { CalendarClock, Coins, Pencil, Plus, RotateCcw, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { GroupsBadge } from "@/components/GroupsBadge";
import { toast } from "sonner";
import { PageWrapper, motion, transitions } from "@/components/ui/motion";
import { PageHeader } from "@/components/ui/page-header";
import { TablePageSkeleton } from "@/components/ui/page-skeleton";
import { EmptyState } from "@/components/ui/empty-state";
import {
  deleteBillingPlanOptimistic,
  updateBillingPlanOptimistic,
  useBillingPlans,
  createBillingPlanOptimistic,
  resetBillingPlanOptimistic,
} from "@/lib/swr";
import type { BillingPlan } from "@/lib/api";

const NANO_PER_USD = 1_000_000_000n;

function parseUsdToNanoBigInt(usd: string): bigint | null {
  const trimmed = usd.trim();
  if (!trimmed) return null;
  const negative = trimmed.startsWith("-");
  const unsigned = negative ? trimmed.slice(1) : trimmed;
  const [wholeRaw, fracRaw = ""] = unsigned.split(".");
  if (!/^\d*$/.test(wholeRaw) || !/^\d*$/.test(fracRaw)) return null;
  if (!wholeRaw && !fracRaw) return null;
  const frac = (fracRaw + "000000000").slice(0, 9);
  try {
    const value = BigInt(wholeRaw || "0") * NANO_PER_USD + BigInt(frac);
    return negative ? -value : value;
  } catch {
    return null;
  }
}

interface PlanFormState {
  name: string;
  amount_usd: string;
  schedule: string;
  groups_text: string;
  enabled: boolean;
}

const EMPTY_FORM: PlanFormState = {
  name: "",
  amount_usd: "",
  schedule: "0 0 * * *",
  groups_text: "",
  enabled: true,
};

function formFromPlan(plan: BillingPlan): PlanFormState {
  return {
    name: plan.name,
    amount_usd: plan.grant_amount_usd,
    schedule: plan.schedule,
    groups_text: plan.allowed_groups.join(", "),
    enabled: plan.enabled,
  };
}

export function BillingPlansPage() {
  const { t } = useTranslation();
  const { data, isLoading } = useBillingPlans();
  const plans = useMemo(() => data ?? [], [data]);

  const [createOpen, setCreateOpen] = useState(false);
  const [form, setForm] = useState<PlanFormState>(EMPTY_FORM);
  const [editTarget, setEditTarget] = useState<BillingPlan | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<BillingPlan | null>(null);
  const [resetTarget, setResetTarget] = useState<BillingPlan | null>(null);
  const [saving, setSaving] = useState(false);
  const [resetting, setResetting] = useState(false);

  const openCreate = () => {
    setForm(EMPTY_FORM);
    setCreateOpen(true);
  };

  const buildInput = () => {
    const amount = parseUsdToNanoBigInt(form.amount_usd);
    const schedule = form.schedule.trim().split(/\s+/).filter(Boolean).join(" ");
    if (amount === null || amount < 0n) {
      toast.error(t("billingPlans.invalidAmount"));
      return null;
    }
    if (schedule.split(" ").length !== 5) {
      toast.error(t("billingPlans.invalidSchedule"));
      return null;
    }
    const name = form.name.trim();
    if (!name) {
      toast.error(t("billingPlans.nameRequired"));
      return null;
    }
    return {
      name,
      grant_amount_nano_usd: amount.toString(),
      schedule,
      allowed_groups: form.groups_text
        .split(",")
        .map((g) => g.trim().toLowerCase())
        .filter(Boolean),
      enabled: form.enabled,
    };
  };

  const handleCreate = async () => {
    const input = buildInput();
    if (!input || saving) return;
    setSaving(true);
    try {
      await createBillingPlanOptimistic(input, plans, (error) =>
        toast.error(error.message)
      );
      setCreateOpen(false);
      toast.success(t("common.success"));
    } catch {
      // optimistic helper already rolled back and toasted
    } finally {
      setSaving(false);
    }
  };

  const handleUpdate = async () => {
    if (!editTarget) return;
    const input = buildInput();
    if (!input || saving) return;
    setSaving(true);
    try {
      await updateBillingPlanOptimistic(editTarget.id, input, plans, (error) =>
        toast.error(error.message)
      );
      setEditTarget(null);
      toast.success(t("common.success"));
    } catch {
      // optimistic helper already rolled back and toasted
    } finally {
      setSaving(false);
    }
  };

  const handleDelete = async () => {
    if (!deleteTarget) return;
    try {
      await deleteBillingPlanOptimistic(deleteTarget.id, plans, (error) =>
        toast.error(error.message)
      );
      toast.success(t("common.success"));
    } catch {
      // optimistic helper already rolled back and toasted
    } finally {
      setDeleteTarget(null);
    }
  };

  const handleReset = async () => {
    if (!resetTarget || resetting) return;
    setResetting(true);
    try {
      const result = await resetBillingPlanOptimistic(resetTarget, (error) =>
        toast.error(error.message)
      );
      toast.success(t("billingPlans.resetSuccess", { count: result.reset_count }));
      setResetTarget(null);
    } catch {
      // optimistic helper already rolled back and toasted; keep dialog open
    } finally {
      setResetting(false);
    }
  };

  const toggleEnabled = async (plan: BillingPlan, enabled: boolean) => {
    await updateBillingPlanOptimistic(
      plan.id,
      {
        name: plan.name,
        grant_amount_nano_usd: plan.grant_amount_nano_usd,
        schedule: plan.schedule,
        allowed_groups: plan.allowed_groups,
        enabled,
      },
      plans,
      (error) => toast.error(error.message)
    ).catch(() => undefined);
  };

  const renderForm = (onSubmit: () => void) => (
    <>
      <div className="grid gap-4 py-4">
        <div className="grid gap-2">
          <Label htmlFor="plan-name">{t("billingPlans.name")}</Label>
          <Input
            id="plan-name"
            value={form.name}
            onChange={(e) => setForm({ ...form, name: e.target.value })}
            placeholder={t("billingPlans.namePlaceholder")}
          />
        </div>
        <div className="grid gap-2">
          <Label htmlFor="plan-amount">{t("billingPlans.amount")}</Label>
          <Input
            id="plan-amount"
            inputMode="decimal"
            value={form.amount_usd}
            onChange={(e) => setForm({ ...form, amount_usd: e.target.value })}
            placeholder="5"
          />
          <p className="text-xs text-muted-foreground">
            {t("billingPlans.amountHelp")}
          </p>
        </div>
        <div className="grid gap-2">
          <Label htmlFor="plan-schedule">{t("billingPlans.schedule")}</Label>
          <Input
            id="plan-schedule"
            value={form.schedule}
            onChange={(e) => setForm({ ...form, schedule: e.target.value })}
            placeholder="0 0 * * *"
            className="font-mono"
          />
          <div className="flex flex-wrap gap-1.5">
            {[
              { value: "0 0 * * *", label: t("billingPlans.scheduleDaily") },
              { value: "0 * * * *", label: t("billingPlans.scheduleHourly") },
              { value: "0 0 * * 1", label: t("billingPlans.scheduleWeekly") },
            ].map((preset) => (
              <Button
                key={preset.value}
                type="button"
                variant={form.schedule.trim() === preset.value ? "secondary" : "outline"}
                size="sm"
                className="h-7 px-2 text-xs"
                onClick={() => setForm({ ...form, schedule: preset.value })}
              >
                {preset.label}
              </Button>
            ))}
          </div>
          <p className="text-xs text-muted-foreground">{t("billingPlans.scheduleHelp")}</p>
        </div>
        <div className="grid gap-2">
          <Label htmlFor="plan-groups">{t("users.allowedGroups")}</Label>
          <Input
            id="plan-groups"
            value={form.groups_text}
            onChange={(e) => setForm({ ...form, groups_text: e.target.value })}
            placeholder="team-a, team-b"
          />
          <p className="text-xs text-muted-foreground">{t("billingPlans.groupsHelp")}</p>
        </div>
        <div className="flex items-center justify-between rounded-lg border p-3">
          <Label htmlFor="plan-enabled">{t("billingPlans.enabled")}</Label>
          <Switch
            id="plan-enabled"
            checked={form.enabled}
            onCheckedChange={(checked) => setForm({ ...form, enabled: checked })}
          />
        </div>
      </div>
      <DialogFooter>
        <Button type="button" onClick={onSubmit} disabled={saving}>
          {saving ? t("common.loading") : t("common.save")}
        </Button>
      </DialogFooter>
    </>
  );

  return (
    <PageWrapper>
      <motion.div
        initial={{ opacity: 0, y: 12 }}
        animate={{ opacity: 1, y: 0 }}
        transition={transitions.normal}
        className="space-y-6"
      >
        <PageHeader
          title={t("billingPlans.title")}
          description={t("billingPlans.description")}
          actions={
            <Button onClick={openCreate}>
              <Plus className="mr-2 h-4 w-4" />
              {t("billingPlans.create")}
            </Button>
          }
        />

        {isLoading ? (
          <TablePageSkeleton />
        ) : plans.length === 0 ? (
          <EmptyState
            variant="card"
            icon={<Coins className="h-10 w-10 text-muted-foreground" />}
            title={t("billingPlans.emptyTitle")}
            description={t("billingPlans.emptyDescription")}
          />
        ) : (
          <div className="overflow-hidden rounded-lg border">
            <table className="w-full text-sm">
              <thead className="bg-muted/50 text-left text-muted-foreground">
                <tr>
                  <th className="px-4 py-2.5 font-medium">{t("billingPlans.name")}</th>
                  <th className="px-4 py-2.5 font-medium">{t("billingPlans.amount")}</th>
                  <th className="px-4 py-2.5 font-medium">{t("billingPlans.schedule")}</th>
                  <th className="px-4 py-2.5 font-medium">{t("users.allowedGroups")}</th>
                  <th className="px-4 py-2.5 font-medium">{t("billingPlans.enabled")}</th>
                  <th className="px-4 py-2.5" />
                </tr>
              </thead>
              <tbody>
                {plans.map((plan) => (
                  <tr key={plan.id} className="border-t transition-colors hover:bg-accent/40">
                    <td className="px-4 py-3 font-medium">{plan.name}</td>
                    <td className="px-4 py-3 tabular-nums">${plan.grant_amount_usd}</td>
                    <td className="px-4 py-3">
                      <span className="inline-flex items-center gap-1.5">
                        <CalendarClock className="h-3.5 w-3.5 text-muted-foreground" />
                        <span className="font-mono">{plan.schedule}</span>
                      </span>
                    </td>
                    <td className="px-4 py-3">
                      <GroupsBadge groups={plan.allowed_groups} />
                    </td>
                    <td className="px-4 py-3">
                      <Switch
                        checked={plan.enabled}
                        onCheckedChange={(checked) => toggleEnabled(plan, checked)}
                      />
                    </td>
                    <td className="px-4 py-3">
                      <div className="flex items-center justify-end gap-1">
                        <Button
                          variant="ghost"
                          size="sm"
                          className="h-8 px-2"
                          title={t("billingPlans.reset")}
                          onClick={() => setResetTarget(plan)}
                        >
                          <RotateCcw className="mr-1 h-4 w-4" />
                          {t("billingPlans.reset")}
                        </Button>
                        <Button
                          variant="ghost"
                          size="icon"
                          className="size-11 touch-manipulation sm:size-9"
                          aria-label={t("common.edit")}
                          onClick={() => {
                            setForm(formFromPlan(plan));
                            setEditTarget(plan);
                          }}
                        >
                          <Pencil className="h-4 w-4" />
                        </Button>
                        <Button
                          variant="ghost"
                          size="icon"
                          className="size-11 touch-manipulation sm:size-9"
                          aria-label={t("common.delete")}
                          onClick={() => setDeleteTarget(plan)}
                        >
                          <Trash2 className="h-4 w-4 text-destructive" />
                        </Button>
                      </div>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}

        {/* Create dialog */}
        <Dialog open={createOpen} onOpenChange={setCreateOpen}>
          <DialogContent>
            <DialogHeader>
              <DialogTitle>{t("billingPlans.create")}</DialogTitle>
              <DialogDescription>{t("billingPlans.description")}</DialogDescription>
            </DialogHeader>
            {renderForm(handleCreate)}
          </DialogContent>
        </Dialog>

        {/* Edit dialog */}
        <Dialog open={editTarget !== null} onOpenChange={(open) => !open && setEditTarget(null)}>
          <DialogContent>
            <DialogHeader>
              <DialogTitle>{t("billingPlans.edit")}</DialogTitle>
              <DialogDescription>{editTarget?.name}</DialogDescription>
            </DialogHeader>
            {renderForm(handleUpdate)}
          </DialogContent>
        </Dialog>

        {/* Delete confirm */}
        <AlertDialog open={deleteTarget !== null} onOpenChange={(open) => !open && setDeleteTarget(null)}>
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>{t("billingPlans.deleteTitle")}</AlertDialogTitle>
              <AlertDialogDescription>
                {t("billingPlans.deleteDescription", { name: deleteTarget?.name })}
              </AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel>{t("common.cancel")}</AlertDialogCancel>
              <AlertDialogAction onClick={handleDelete}>
                {t("common.delete")}
              </AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>

        <AlertDialog
          open={resetTarget !== null}
          onOpenChange={(open) => {
            if (!open && !resetting) setResetTarget(null);
          }}
        >
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>{t("billingPlans.resetTitle")}</AlertDialogTitle>
              <AlertDialogDescription>
                {t("billingPlans.resetDescription", { name: resetTarget?.name })}
              </AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel disabled={resetting}>{t("common.cancel")}</AlertDialogCancel>
              <AlertDialogAction
                disabled={resetting}
                onClick={(event) => {
                  event.preventDefault();
                  void handleReset();
                }}
              >
                {resetting ? t("common.loading") : t("billingPlans.reset")}
              </AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>

      </motion.div>
    </PageWrapper>
  );
}
