import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Pencil, Plus, Trash2, X } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from "@/components/ui/alert-dialog";
import { GroupsBadge } from "@/components/GroupsBadge";
import { GroupMultiSelect } from "@/components/groups/GroupPicker";
import { PageWrapper } from "@/components/ui/motion";
import { PageHeader } from "@/components/ui/page-header";
import { TablePageSkeleton } from "@/components/ui/page-skeleton";
import { EmptyState } from "@/components/ui/empty-state";
import { createBillingPlanOptimistic, deleteBillingPlanOptimistic, updateBillingPlanOptimistic, useBillingPlans, useDashboardGroups } from "@/lib/swr";
import { formatNanoUsd } from "@/lib/exact-decimal";
import type { BillingPlan, BillingPlanInput } from "@/lib/api";

const NANO_PER_USD = 1_000_000_000n;
const WINDOWS = [
  ["limit_5h_nano_usd", "5h"], ["limit_24h_nano_usd", "24h"],
  ["limit_7d_nano_usd", "7d"], ["limit_30d_nano_usd", "30d"],
] as const;

function usdToNano(value: string): string | null {
  const match = /^(\d+)(?:\.(\d{0,9}))?$/.exec(value.trim());
  if (!match) return null;
  return (BigInt(match[1]) * NANO_PER_USD + BigInt((match[2] ?? "").padEnd(9, "0"))).toString();
}

function nanoToUsd(value: string | null): string {
  if (!value) return "";
  const nano = BigInt(value);
  const fraction = (nano % NANO_PER_USD).toString().padStart(9, "0").replace(/0+$/, "");
  return `${nano / NANO_PER_USD}${fraction ? `.${fraction}` : ""}`;
}

type WindowKey = (typeof WINDOWS)[number][0];
type FormState = {
  name: string; description: string; limits: Record<WindowKey, string>;
  groupIds: string[]; multiplier: string; listed: boolean;
  prices: Array<{ priceUsd: string; durationDays: string }>;
};

const emptyForm = (): FormState => ({
  name: "", description: "",
  limits: { limit_5h_nano_usd: "", limit_24h_nano_usd: "", limit_7d_nano_usd: "", limit_30d_nano_usd: "" },
  groupIds: [], multiplier: "1", listed: false, prices: [],
});

function formFromPlan(plan: BillingPlan): FormState {
  return {
    name: plan.name, description: plan.description,
    limits: {
      limit_5h_nano_usd: nanoToUsd(plan.limit_5h_nano_usd), limit_24h_nano_usd: nanoToUsd(plan.limit_24h_nano_usd),
      limit_7d_nano_usd: nanoToUsd(plan.limit_7d_nano_usd), limit_30d_nano_usd: nanoToUsd(plan.limit_30d_nano_usd),
    },
    groupIds: plan.group_ids, multiplier: plan.multiplier, listed: plan.listed,
    prices: plan.prices.map((price) => ({ priceUsd: price.price_usd, durationDays: String(price.duration_seconds / 86400) })),
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

  const startCreate = () => { setEditing(null); setForm(emptyForm()); setOpen(true); };
  const startEdit = (plan: BillingPlan) => { setEditing(plan); setForm(formFromPlan(plan)); setOpen(true); };

  const buildInput = (): BillingPlanInput | null => {
    const limits = {} as Record<WindowKey, string | null>;
    let invalidLimit = false;
    for (const [key] of WINDOWS) {
      limits[key] = form.limits[key] ? usdToNano(form.limits[key]) : null;
      if (form.limits[key] && limits[key] === null) invalidLimit = true;
    }
    if (!form.name.trim() || invalidLimit || !Object.values(limits).some(Boolean) || form.groupIds.length === 0 || !/^\d+(?:\.\d+)?$/.test(form.multiplier) || Number(form.multiplier) <= 0) {
      toast.error(t("billingPlans.invalidForm")); return null;
    }
    const prices = form.prices.map((price) => ({ price_usd: price.priceUsd.trim(), duration_seconds: Number(price.durationDays) * 86400 }));
    if (prices.some((price) => usdToNano(price.price_usd) === null || !Number.isSafeInteger(price.duration_seconds) || price.duration_seconds <= 0) || (form.listed && prices.length === 0)) {
      toast.error(t("billingPlans.invalidPrices")); return null;
    }
    return { name: form.name.trim(), description: form.description.trim(), ...limits, group_ids: form.groupIds, multiplier: form.multiplier, listed: form.listed, prices };
  };

  const save = async () => {
    const input = buildInput(); if (!input) return;
    setSaving(true);
    try {
      if (editing) await updateBillingPlanOptimistic(editing.id, input, plans, (error) => toast.error(error.message));
      else await createBillingPlanOptimistic(input, plans, (error) => toast.error(error.message));
      toast.success(t(editing ? "billingPlans.updated" : "billingPlans.created")); setOpen(false);
    } finally { setSaving(false); }
  };

  if (isLoading) return <TablePageSkeleton />;
  return <PageWrapper className="space-y-6">
    <PageHeader title={t("billingPlans.title")} description={t("billingPlans.description")} actions={<Button onClick={startCreate}><Plus className="mr-2 h-4 w-4" />{t("billingPlans.create")}</Button>} />
    {plans.length === 0 ? <EmptyState title={t("billingPlans.emptyTitle")} description={t("billingPlans.emptyDescription")} /> : <div className="overflow-x-auto rounded-lg border"><table className="w-full text-left text-sm">
      <thead className="bg-muted/50 text-muted-foreground"><tr><th className="px-4 py-3">{t("billingPlans.name")}</th><th className="px-4 py-3">{t("billingPlans.limits")}</th><th className="px-4 py-3">{t("billingPlans.groups")}</th><th className="px-4 py-3">{t("billingPlans.prices")}</th><th className="px-4 py-3">{t("billingPlans.multiplier")}</th><th className="px-4 py-3 text-right">{t("common.actions")}</th></tr></thead>
      <tbody>{plans.map((plan) => <tr key={plan.id} className="border-t align-top">
        <td className="px-4 py-3"><div className="font-medium">{plan.name}</div><div className="max-w-64 text-xs text-muted-foreground">{plan.description}</div>{plan.listed && <div className="mt-1 text-xs text-primary">{t("billingPlans.listed")}</div>}</td>
        <td className="px-4 py-3 text-xs">{WINDOWS.map(([key, label]) => plan[key] && <div key={key}>{label}: {formatNanoUsd(plan[key]!)}</div>)}</td>
        <td className="px-4 py-3"><GroupsBadge groupIds={plan.group_ids} /></td>
        <td className="px-4 py-3 text-xs">{plan.prices.map((price) => <div key={price.id}>{price.price_usd} USD / {price.duration_seconds / 86400}d</div>)}</td>
        <td className="px-4 py-3 font-mono">{plan.multiplier}×</td>
        <td className="px-4 py-3"><div className="flex justify-end gap-1"><Button size="icon" variant="ghost" onClick={() => startEdit(plan)}><Pencil className="h-4 w-4" /></Button><Button size="icon" variant="ghost" onClick={() => setDeleting(plan)}><Trash2 className="h-4 w-4" /></Button></div></td>
      </tr>)}</tbody></table></div>}

    <Dialog open={open} onOpenChange={setOpen}><DialogContent className="max-h-[calc(100dvh-2rem)] max-w-2xl overflow-hidden p-0 sm:max-h-[calc(100dvh-3rem)]"><div className="flex min-h-0 flex-col p-6"><DialogHeader className="shrink-0"><DialogTitle>{t(editing ? "billingPlans.edit" : "billingPlans.create")}</DialogTitle><DialogDescription>{t("billingPlans.formDescription")}</DialogDescription></DialogHeader>
      <div className="grid min-h-0 flex-1 gap-5 overflow-y-auto py-2 pr-1">
        <div className="grid gap-2"><Label>{t("billingPlans.name")}</Label><Input value={form.name} onChange={(event) => setForm({ ...form, name: event.target.value })} /></div>
        <div className="grid gap-2"><Label>{t("billingPlans.planDescription")}</Label><Textarea value={form.description} onChange={(event) => setForm({ ...form, description: event.target.value })} /></div>
        <div className="grid grid-cols-2 gap-3">{WINDOWS.map(([key, label]) => <div className="grid gap-2" key={key}><Label>{label} {t("billingPlans.limitUsd")}</Label><Input inputMode="decimal" value={form.limits[key]} onChange={(event) => setForm({ ...form, limits: { ...form.limits, [key]: event.target.value } })} placeholder={t("billingPlans.optional")} /></div>)}</div>
        <div className="grid gap-2"><Label>{t("billingPlans.groups")}</Label><GroupMultiSelect value={form.groupIds} groups={groups} loading={groupsLoading} onChange={(groupIds) => setForm({ ...form, groupIds })} /></div>
        <div className="grid gap-2"><Label>{t("billingPlans.multiplier")}</Label><Input inputMode="decimal" value={form.multiplier} onChange={(event) => setForm({ ...form, multiplier: event.target.value })} /></div>
        <div className="flex items-center justify-between rounded-md border p-3"><div><Label>{t("billingPlans.listed")}</Label><p className="text-xs text-muted-foreground">{t("billingPlans.listedHelp")}</p></div><Switch checked={form.listed} onCheckedChange={(listed) => setForm({ ...form, listed })} /></div>
        <div className="grid gap-2"><div className="flex items-center justify-between"><Label>{t("billingPlans.prices")}</Label><Button type="button" size="sm" variant="outline" onClick={() => setForm({ ...form, prices: [...form.prices, { priceUsd: "", durationDays: "30" }] })}><Plus className="mr-1 h-3.5 w-3.5" />{t("billingPlans.addPrice")}</Button></div>
          {form.prices.map((price, index) => <div key={index} className="grid grid-cols-[1fr_1fr_auto] gap-2"><Input inputMode="decimal" placeholder={t("billingPlans.priceUsd")} value={price.priceUsd} onChange={(event) => setForm({ ...form, prices: form.prices.map((entry, i) => i === index ? { ...entry, priceUsd: event.target.value } : entry) })} /><Input inputMode="numeric" placeholder={t("billingPlans.durationDays")} value={price.durationDays} onChange={(event) => setForm({ ...form, prices: form.prices.map((entry, i) => i === index ? { ...entry, durationDays: event.target.value } : entry) })} /><Button type="button" size="icon" variant="ghost" onClick={() => setForm({ ...form, prices: form.prices.filter((_, i) => i !== index) })}><X className="h-4 w-4" /></Button></div>)}
        </div>
      </div><DialogFooter className="shrink-0 pt-4"><Button variant="outline" onClick={() => setOpen(false)}>{t("common.cancel")}</Button><Button disabled={saving} onClick={save}>{saving ? t("common.loading") : t("common.save")}</Button></DialogFooter>
    </div></DialogContent></Dialog>
    <AlertDialog open={Boolean(deleting)} onOpenChange={(value) => !value && setDeleting(null)}><AlertDialogContent><AlertDialogHeader><AlertDialogTitle>{t("billingPlans.deleteTitle")}</AlertDialogTitle><AlertDialogDescription>{t("billingPlans.deleteDescription", { name: deleting?.name })}</AlertDialogDescription></AlertDialogHeader><AlertDialogFooter><AlertDialogCancel>{t("common.cancel")}</AlertDialogCancel><AlertDialogAction onClick={async () => { if (!deleting) return; await deleteBillingPlanOptimistic(deleting.id, plans, (error) => toast.error(error.message)); setDeleting(null); }}>{t("common.delete")}</AlertDialogAction></AlertDialogFooter></AlertDialogContent></AlertDialog>
  </PageWrapper>;
}
