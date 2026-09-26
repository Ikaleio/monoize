import { Fragment, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { CalendarClock, Pencil, Plus, Trash2, X } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
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
import {
  DataList,
  DataListActions,
  DataListBody,
  DataListCell,
  DataListHead,
  DataListHeader,
  DataListRow,
} from "@/components/ui/data-list";
import { DataTableShell } from "@/components/ui/data-table-shell";
import { StatusBadge } from "@/components/ui/status";
import { GroupsBadge } from "@/components/GroupsBadge";
import { GroupMultiSelect } from "@/components/groups/GroupPicker";
import { PageWrapper } from "@/components/ui/motion";
import { PageHeader } from "@/components/ui/page-header";
import { TablePageSkeleton } from "@/components/ui/page-skeleton";
import { EmptyState } from "@/components/ui/empty-state";
import { createBillingPlanOptimistic, deleteBillingPlanOptimistic, updateBillingPlanOptimistic, useBillingPlans, useDashboardGroups } from "@/lib/swr";
import type { BillingPlan, BillingPlanInput } from "@/lib/api";

const NANO_PER_USD = 1_000_000_000n;
const SECONDS_PER_DAY = 86_400;
const WINDOWS = [
  ["limit_5h_nano_usd", "5h"], ["limit_24h_nano_usd", "24h"],
  ["limit_7d_nano_usd", "7d"], ["limit_30d_nano_usd", "30d"],
] as const;

function usdToNano(value: string): string | null {
  const match = /^(\d+)(?:\.(\d{0,9}))?$/.exec(value.trim());
  if (!match) return null;
  return (BigInt(match[1]) * NANO_PER_USD + BigInt((match[2] ?? "").padEnd(9, "0"))).toString();
}

/** Exact decimal USD text without trailing fractional zeros (PLN-UI2). */
function nanoToUsd(value: string | null): string {
  if (!value) return "";
  const nano = BigInt(value);
  const fraction = (nano % NANO_PER_USD).toString().padStart(9, "0").replace(/0+$/, "");
  return `${nano / NANO_PER_USD}${fraction ? `.${fraction}` : ""}`;
}

type WindowKey = (typeof WINDOWS)[number][0];
type FormState = {
  name: string;
  description: string;
  limits: Record<WindowKey, string>;
  groupIds: string[];
  multiplier: string;
  listed: boolean;
  prices: Array<{ priceUsd: string; durationDays: string }>;
};

const emptyForm = (): FormState => ({
  name: "",
  description: "",
  limits: { limit_5h_nano_usd: "", limit_24h_nano_usd: "", limit_7d_nano_usd: "", limit_30d_nano_usd: "" },
  groupIds: [],
  multiplier: "1",
  listed: false,
  prices: [],
});

function formFromPlan(plan: BillingPlan): FormState {
  return {
    name: plan.name,
    description: plan.description,
    limits: {
      limit_5h_nano_usd: nanoToUsd(plan.limit_5h_nano_usd),
      limit_24h_nano_usd: nanoToUsd(plan.limit_24h_nano_usd),
      limit_7d_nano_usd: nanoToUsd(plan.limit_7d_nano_usd),
      limit_30d_nano_usd: nanoToUsd(plan.limit_30d_nano_usd),
    },
    groupIds: plan.group_ids,
    multiplier: plan.multiplier,
    listed: plan.listed,
    prices: plan.prices.map((price) => ({
      priceUsd: price.price_usd,
      durationDays: String(price.duration_seconds / SECONDS_PER_DAY),
    })),
  };
}

export function BillingPlansPage() {
  const { t } = useTranslation();
  const { data, isLoading } = useBillingPlans();
  const { data: groups = [], isLoading: groupsLoading } = useDashboardGroups();
  const plans = useMemo(() => data ?? [], [data]);
  const [form, setForm] = useState<FormState>(emptyForm);
  const [open, setOpen] = useState(false);
  const [editing, setEditing] = useState<BillingPlan | null>(null);
  const [deleting, setDeleting] = useState<BillingPlan | null>(null);
  const [saving, setSaving] = useState(false);

  const startCreate = () => {
    setEditing(null);
    setForm(emptyForm());
    setOpen(true);
  };

  const startEdit = (plan: BillingPlan) => {
    setEditing(plan);
    setForm(formFromPlan(plan));
    setOpen(true);
  };

  const buildInput = (): BillingPlanInput | null => {
    const limits = {} as Record<WindowKey, string | null>;
    let invalidLimit = false;
    for (const [key] of WINDOWS) {
      limits[key] = form.limits[key] ? usdToNano(form.limits[key]) : null;
      if (form.limits[key] && limits[key] === null) invalidLimit = true;
    }
    if (
      !form.name.trim()
      || invalidLimit
      || !Object.values(limits).some(Boolean)
      || form.groupIds.length === 0
      || !/^\d+(?:\.\d+)?$/.test(form.multiplier)
      || Number(form.multiplier) <= 0
    ) {
      toast.error(t("billingPlans.invalidForm"));
      return null;
    }
    const prices = form.prices.map((price) => ({
      price_usd: price.priceUsd.trim(),
      duration_seconds: Number(price.durationDays) * SECONDS_PER_DAY,
    }));
    if (
      prices.some((price) => usdToNano(price.price_usd) === null || !Number.isSafeInteger(price.duration_seconds) || price.duration_seconds <= 0)
      || (form.listed && prices.length === 0)
    ) {
      toast.error(t("billingPlans.invalidPrices"));
      return null;
    }
    return {
      name: form.name.trim(),
      description: form.description.trim(),
      ...limits,
      group_ids: form.groupIds,
      multiplier: form.multiplier,
      listed: form.listed,
      prices,
    };
  };

  const save = async () => {
    const input = buildInput();
    if (!input) return;
    setSaving(true);
    try {
      if (editing) await updateBillingPlanOptimistic(editing.id, input, plans, (error) => toast.error(error.message));
      else await createBillingPlanOptimistic(input, plans, (error) => toast.error(error.message));
      toast.success(t(editing ? "billingPlans.updated" : "billingPlans.created"));
      setOpen(false);
    } catch {
      // optimistic helper already rolled back and toasted; keep the form open
    } finally {
      setSaving(false);
    }
  };

  const confirmDelete = async () => {
    if (!deleting) return;
    try {
      await deleteBillingPlanOptimistic(deleting.id, plans, (error) => toast.error(error.message));
    } catch {
      // optimistic helper already rolled back and toasted
    } finally {
      setDeleting(null);
    }
  };

  const createButton = (
    <Button onClick={startCreate}>
      <Plus aria-hidden="true" />
      {t("billingPlans.create")}
    </Button>
  );

  if (isLoading) {
    return (
      <PageWrapper>
        <TablePageSkeleton rows={3} columns={6} />
      </PageWrapper>
    );
  }

  return (
    <PageWrapper className="space-y-6">
      <PageHeader title={t("billingPlans.title")} description={t("billingPlans.description")} actions={createButton} />

      <DataTableShell
        toolbar={
          <p className="ml-auto text-sm tabular-nums text-muted-foreground">
            {t("billingPlans.count", { count: plans.length })}
          </p>
        }
        isEmpty={plans.length === 0}
        emptyState={
          <EmptyState
            icon={<CalendarClock className="size-10" aria-hidden="true" />}
            title={t("billingPlans.emptyTitle")}
            description={t("billingPlans.emptyDescription")}
            action={createButton}
          />
        }
      >
        <DataList columns="minmax(0,1.4fr) 9rem minmax(0,1fr) 9rem 4.5rem 6rem 5rem">
          <DataListHeader>
            <DataListHead>{t("billingPlans.name")}</DataListHead>
            <DataListHead>{t("billingPlans.limits")}</DataListHead>
            <DataListHead>{t("billingPlans.groups")}</DataListHead>
            <DataListHead>{t("billingPlans.prices")}</DataListHead>
            <DataListHead align="end">{t("billingPlans.multiplier")}</DataListHead>
            <DataListHead>{t("billingPlans.listedState")}</DataListHead>
            <DataListHead align="end">{t("common.actions")}</DataListHead>
          </DataListHeader>
          <DataListBody aria-label={t("billingPlans.title")}>
            {plans.map((plan) => (
              <DataListRow key={plan.id}>
                <DataListCell primary>
                  <p className="truncate font-medium" title={plan.name}>{plan.name}</p>
                  {plan.description ? (
                    <p className="line-clamp-2 text-muted-foreground" title={plan.description}>{plan.description}</p>
                  ) : null}
                </DataListCell>
                <DataListCell label={t("billingPlans.limits")}>
                  <dl className="inline-grid grid-cols-[auto_auto] gap-x-3 text-start">
                    {WINDOWS.filter(([key]) => plan[key]).map(([key, label]) => (
                      <Fragment key={key}>
                        <dt className="text-muted-foreground">{label}</dt>
                        <dd className="tabular-nums">${nanoToUsd(plan[key])}</dd>
                      </Fragment>
                    ))}
                  </dl>
                </DataListCell>
                <DataListCell label={t("billingPlans.groups")}>
                  {plan.group_ids.length > 0 ? (
                    <GroupsBadge groupIds={plan.group_ids} />
                  ) : (
                    <span className="text-muted-foreground">{t("billingPlans.groupsUnrestricted")}</span>
                  )}
                </DataListCell>
                <DataListCell label={t("billingPlans.prices")}>
                  {plan.prices.length > 0 ? (
                    <ul className="tabular-nums">
                      {plan.prices.map((price) => (
                        <li key={price.id}>
                          {t("billingPlans.priceOption", {
                            price: `$${price.price_usd}`,
                            count: price.duration_seconds / SECONDS_PER_DAY,
                          })}
                        </li>
                      ))}
                    </ul>
                  ) : (
                    <span className="text-muted-foreground">—</span>
                  )}
                </DataListCell>
                <DataListCell label={t("billingPlans.multiplier")} align="end">
                  <span className="tabular-nums">{plan.multiplier}×</span>
                </DataListCell>
                <DataListCell label={t("billingPlans.listedState")}>
                  {plan.listed ? (
                    <StatusBadge variant="success">{t("billingPlans.listed")}</StatusBadge>
                  ) : (
                    <span className="text-muted-foreground">{t("billingPlans.unlisted")}</span>
                  )}
                </DataListCell>
                <DataListActions>
                  <Button
                    variant="ghost"
                    size="icon"
                    className="size-11 touch-manipulation sm:size-9"
                    aria-label={t("common.editItem", { name: plan.name })}
                    onClick={() => startEdit(plan)}
                  >
                    <Pencil />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon"
                    className="size-11 touch-manipulation sm:size-9"
                    aria-label={t("common.deleteItem", { name: plan.name })}
                    onClick={() => setDeleting(plan)}
                  >
                    <Trash2 className="text-error-foreground" />
                  </Button>
                </DataListActions>
              </DataListRow>
            ))}
          </DataListBody>
        </DataList>
      </DataTableShell>

      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent className="max-h-[calc(100dvh-2rem)] max-w-2xl overflow-hidden p-0 sm:max-h-[calc(100dvh-3rem)]">
          <div className="flex min-h-0 flex-col p-6">
            <DialogHeader className="shrink-0">
              <DialogTitle>{t(editing ? "billingPlans.edit" : "billingPlans.create")}</DialogTitle>
              <DialogDescription>{t("billingPlans.formDescription")}</DialogDescription>
            </DialogHeader>
            <div className="grid min-h-0 flex-1 gap-5 overflow-y-auto py-2 pr-1">
              <div className="grid gap-2">
                <Label htmlFor="plan-name">{t("billingPlans.name")}</Label>
                <Input id="plan-name" value={form.name} onChange={(event) => setForm({ ...form, name: event.target.value })} />
              </div>
              <div className="grid gap-2">
                <Label htmlFor="plan-description">{t("billingPlans.planDescription")}</Label>
                <Textarea
                  id="plan-description"
                  value={form.description}
                  onChange={(event) => setForm({ ...form, description: event.target.value })}
                />
              </div>
              <div className="grid grid-cols-2 gap-3">
                {WINDOWS.map(([key, label]) => (
                  <div className="grid gap-2" key={key}>
                    <Label htmlFor={`plan-${key}`}>{label} {t("billingPlans.limitUsd")}</Label>
                    <Input
                      id={`plan-${key}`}
                      inputMode="decimal"
                      value={form.limits[key]}
                      onChange={(event) => setForm({ ...form, limits: { ...form.limits, [key]: event.target.value } })}
                      placeholder={t("billingPlans.optional")}
                    />
                  </div>
                ))}
              </div>
              <div role="group" aria-labelledby="plan-groups-label" className="grid gap-2">
                <Label id="plan-groups-label">{t("billingPlans.groups")}</Label>
                <GroupMultiSelect
                  value={form.groupIds}
                  groups={groups}
                  loading={groupsLoading}
                  onChange={(groupIds) => setForm({ ...form, groupIds })}
                />
              </div>
              <div className="grid gap-2">
                <Label htmlFor="plan-multiplier">{t("billingPlans.multiplier")}</Label>
                <Input
                  id="plan-multiplier"
                  inputMode="decimal"
                  value={form.multiplier}
                  onChange={(event) => setForm({ ...form, multiplier: event.target.value })}
                />
              </div>
              <div className="flex items-center justify-between gap-4 rounded-md border p-3">
                <div className="min-w-0">
                  <Label htmlFor="plan-listed">{t("billingPlans.listed")}</Label>
                  <p className="text-sm text-muted-foreground">{t("billingPlans.listedHelp")}</p>
                </div>
                <Switch id="plan-listed" checked={form.listed} onCheckedChange={(listed) => setForm({ ...form, listed })} />
              </div>
              <div role="group" aria-labelledby="plan-prices-label" className="grid gap-2">
                <div className="flex items-center justify-between">
                  <Label id="plan-prices-label">{t("billingPlans.prices")}</Label>
                  <Button
                    type="button"
                    size="sm"
                    variant="outline"
                    onClick={() => setForm({ ...form, prices: [...form.prices, { priceUsd: "", durationDays: "30" }] })}
                  >
                    <Plus aria-hidden="true" />
                    {t("billingPlans.addPrice")}
                  </Button>
                </div>
                {form.prices.map((price, index) => (
                  <div key={index} className="grid grid-cols-[1fr_1fr_auto] gap-2">
                    <Input
                      inputMode="decimal"
                      aria-label={`${t("billingPlans.priceUsd")} ${index + 1}`}
                      placeholder={t("billingPlans.priceUsd")}
                      value={price.priceUsd}
                      onChange={(event) =>
                        setForm({
                          ...form,
                          prices: form.prices.map((entry, i) => (i === index ? { ...entry, priceUsd: event.target.value } : entry)),
                        })
                      }
                    />
                    <Input
                      inputMode="numeric"
                      aria-label={`${t("billingPlans.durationDays")} ${index + 1}`}
                      placeholder={t("billingPlans.durationDays")}
                      value={price.durationDays}
                      onChange={(event) =>
                        setForm({
                          ...form,
                          prices: form.prices.map((entry, i) => (i === index ? { ...entry, durationDays: event.target.value } : entry)),
                        })
                      }
                    />
                    <Button
                      type="button"
                      size="icon"
                      variant="ghost"
                      aria-label={t("billingPlans.removePrice", { index: index + 1 })}
                      onClick={() => setForm({ ...form, prices: form.prices.filter((_, i) => i !== index) })}
                    >
                      <X />
                    </Button>
                  </div>
                ))}
              </div>
            </div>
            <DialogFooter className="shrink-0 pt-4">
              <Button variant="outline" onClick={() => setOpen(false)}>{t("common.cancel")}</Button>
              <Button disabled={saving} onClick={() => void save()}>{saving ? t("common.loading") : t("common.save")}</Button>
            </DialogFooter>
          </div>
        </DialogContent>
      </Dialog>

      <AlertDialog open={Boolean(deleting)} onOpenChange={(value) => !value && setDeleting(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("billingPlans.deleteTitle")}</AlertDialogTitle>
            <AlertDialogDescription>{t("billingPlans.deleteDescription", { name: deleting?.name })}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("common.cancel")}</AlertDialogCancel>
            <AlertDialogAction
              className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
              onClick={() => void confirmDelete()}
            >
              {t("common.delete")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </PageWrapper>
  );
}
