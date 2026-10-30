import { Fragment, useId, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { CalendarClock, Pencil, Plus, Trash2, X } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { FieldError } from "@/components/ui/field";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
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
import { AnimatedButton, PageWrapper } from "@/components/ui/motion";
import { QueryError } from "@/components/ui/query-error";
import { PageHeader } from "@/components/ui/page-header";
import { TablePageSkeleton } from "@/components/ui/page-skeleton";
import { EmptyState } from "@/components/ui/empty-state";
import { createBillingPlanOptimistic, deleteBillingPlanOptimistic, updateBillingPlanOptimistic, useBillingPlans, useDashboardGroups } from "@/lib/swr";
import { DashboardApiError, type BillingPlan, type BillingPlanInput } from "@/lib/api";
import { toastApiError } from "@/lib/api-error";

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

/** Field errors keyed by form area; price rows use `price-<index>` / `days-<index>`. */
type FormErrors = Partial<Record<string, string>>;

// Server codes (billing-plan-subscriptions.spec.md BP-A3) mapped to the form area they concern.
const SERVER_ERROR_FIELDS: Record<string, string> = {
  plan_name_exists: "name",
  invalid_plan_name: "name",
  invalid_plan_description: "description",
  invalid_plan_limits: "limits",
  invalid_plan_groups: "groups",
  invalid_plan_multiplier: "multiplier",
  invalid_plan_prices: "prices",
  invalid_plan_price_duration: "prices",
};

function RequiredMark() {
  return (
    <span aria-hidden="true" className="text-error-foreground">
      *
    </span>
  );
}

export function BillingPlansPage() {
  const { t } = useTranslation();
  const formId = useId();
  const { data, error, isLoading, isValidating, mutate } = useBillingPlans();
  const { data: groups = [], isLoading: groupsLoading } = useDashboardGroups();
  const plans = useMemo(() => data ?? [], [data]);
  const [form, setForm] = useState<FormState>(emptyForm);
  const [initialForm, setInitialForm] = useState<FormState>(emptyForm);
  const [errors, setErrors] = useState<FormErrors>({});
  const [open, setOpen] = useState(false);
  const [editing, setEditing] = useState<BillingPlan | null>(null);
  const [deleting, setDeleting] = useState<BillingPlan | null>(null);
  const [saving, setSaving] = useState(false);
  const dirty = open && JSON.stringify(form) !== JSON.stringify(initialForm);
  const fieldId = (key: string) => `${formId}-${key}`;

  const openForm = (plan: BillingPlan | null) => {
    const next = plan ? formFromPlan(plan) : emptyForm();
    setEditing(plan);
    setForm(next);
    setInitialForm(next);
    setErrors({});
    setOpen(true);
  };

  const patchForm = (patch: Partial<FormState>, clear: string[]) => {
    setForm((previous) => ({ ...previous, ...patch }));
    setErrors((previous) => {
      const next = { ...previous };
      for (const key of clear) delete next[key];
      return next;
    });
  };

  const focusFirstError = (next: FormErrors) => {
    const order = [
      "name",
      "description",
      "limits",
      "groups",
      "multiplier",
      ...form.prices.flatMap((_, index) => [`price-${index}`, `days-${index}`]),
      "prices",
    ];
    const first = order.find((key) => next[key]);
    if (!first) return;
    const target =
      first === "limits"
        ? fieldId(WINDOWS[0][0])
        : first === "groups"
          ? `${fieldId("groups")}-label`
          : first === "prices"
            ? fieldId("add-price")
            : fieldId(first);
    const element = document.getElementById(target);
    element?.scrollIntoView({ block: "center" });
    element?.focus();
  };

  const buildInput = (): BillingPlanInput | null => {
    const next: FormErrors = {};
    if (!form.name.trim()) next.name = t("billingPlans.nameRequired");
    const limits = {} as Record<WindowKey, string | null>;
    for (const [key] of WINDOWS) {
      const raw = form.limits[key].trim();
      limits[key] = raw ? usdToNano(raw) : null;
      if (raw && (limits[key] === null || limits[key] === "0")) next.limits = t("billingPlans.errorLimitInvalid");
    }
    if (!next.limits && !Object.values(limits).some(Boolean)) next.limits = t("billingPlans.errorLimitRequired");
    if (form.groupIds.length === 0) next.groups = t("billingPlans.errorGroupsRequired");
    if (!/^\d+(?:\.\d{1,9})?$/.test(form.multiplier.trim()) || !/[1-9]/.test(form.multiplier)) {
      next.multiplier = t("billingPlans.errorMultiplier");
    }
    const seenDays = new Set<string>();
    form.prices.forEach((price, index) => {
      const nano = usdToNano(price.priceUsd);
      if (nano === null || nano === "0") next[`price-${index}`] = t("billingPlans.errorPrice");
      // BP-D4a: whole days only.
      const days = price.durationDays.trim();
      if (!/^[1-9]\d*$/.test(days) || !Number.isSafeInteger(Number(days) * SECONDS_PER_DAY)) {
        next[`days-${index}`] = t("billingPlans.errorDurationDays");
      } else if (seenDays.has(days)) {
        next[`days-${index}`] = t("billingPlans.errorDurationDuplicate");
      }
      seenDays.add(days);
    });
    if (form.listed && form.prices.length === 0) next.prices = t("billingPlans.errorListedNeedsPrice");
    setErrors(next);
    if (Object.keys(next).length > 0) {
      focusFirstError(next);
      return null;
    }
    return {
      name: form.name.trim(),
      description: form.description.trim(),
      ...limits,
      group_ids: form.groupIds,
      multiplier: form.multiplier.trim(),
      listed: form.listed,
      prices: form.prices.map((price) => ({
        price_usd: price.priceUsd.trim(),
        duration_seconds: Number(price.durationDays.trim()) * SECONDS_PER_DAY,
      })),
    };
  };

  const handleServerError = (saveError: Error) => {
    const field = saveError instanceof DashboardApiError ? SERVER_ERROR_FIELDS[saveError.code] : undefined;
    if (field && saveError instanceof DashboardApiError) {
      const next = { [field]: t(`apiErrors.${saveError.code}`) };
      setErrors(next);
      focusFirstError(next);
      return;
    }
    toastApiError(saveError, t, t(editing ? "billingPlans.updateFailed" : "billingPlans.createFailed"));
  };

  const save = async () => {
    if (saving) return;
    const input = buildInput();
    if (!input) return;
    setSaving(true);
    try {
      if (editing) await updateBillingPlanOptimistic(editing.id, input, plans, handleServerError);
      else await createBillingPlanOptimistic(input, plans, handleServerError);
      toast.success(t(editing ? "billingPlans.updated" : "billingPlans.created"));
      setOpen(false);
    } catch {
      // optimistic helper already rolled back; the error is shown on its field or as a toast
    } finally {
      setSaving(false);
    }
  };

  const confirmDelete = async () => {
    if (!deleting) return;
    await deleteBillingPlanOptimistic(deleting.id, plans, (deleteError) =>
      toastApiError(deleteError, t, t("billingPlans.deleteFailed"))
    ).catch(() => undefined);
  };

  const errorText = (key: string) =>
    errors[key] ? (
      <FieldError id={`${fieldId(key)}-error`}>{errors[key]}</FieldError>
    ) : null;

  const invalidProps = (key: string, helpId?: string) => ({
    "aria-invalid": errors[key] ? true : undefined,
    "aria-describedby":
      [errors[key] ? `${fieldId(key)}-error` : null, helpId].filter(Boolean).join(" ") || undefined,
  });

  const createButton = (
    <Button onClick={() => openForm(null)}>
      <Plus data-icon="inline-start" aria-hidden="true" />
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
      <PageHeader title={t("billingPlans.title")} description={t("billingPlans.description")} actions={<AnimatedButton>{createButton}</AnimatedButton>} />

      {error ? <QueryError onRetry={mutate} retrying={isValidating} stale={data !== undefined} /> : null}

      {data !== undefined ? (
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
                    onClick={() => openForm(plan)}
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
      ) : null}

      <Dialog open={open} onOpenChange={(next) => !saving && setOpen(next)}>
        <DialogContent
          dirty={dirty}
          className="max-h-[calc(100dvh-2rem)] max-w-2xl overflow-hidden p-0 sm:max-h-[calc(100dvh-3rem)]"
        >
          <form
            noValidate
            className="flex max-h-[inherit] min-h-0 flex-col p-6"
            onSubmit={(event) => {
              event.preventDefault();
              void save();
            }}
          >
            <DialogHeader className="shrink-0">
              <DialogTitle>{t(editing ? "billingPlans.edit" : "billingPlans.create")}</DialogTitle>
              <DialogDescription>{t("billingPlans.formDescription")}</DialogDescription>
            </DialogHeader>
            <div className="grid min-h-0 flex-1 gap-5 overflow-y-auto py-2 pr-1">
              <div className="grid gap-2">
                <Label htmlFor={fieldId("name")}>
                  {t("billingPlans.name")}
                  <RequiredMark />
                </Label>
                <Input
                  id={fieldId("name")}
                  value={form.name}
                  aria-required
                  {...invalidProps("name")}
                  onChange={(event) => patchForm({ name: event.target.value }, ["name"])}
                />
                {errorText("name")}
              </div>
              <div className="grid gap-2">
                <Label htmlFor={fieldId("description")}>{t("billingPlans.planDescription")}</Label>
                <Textarea
                  id={fieldId("description")}
                  value={form.description}
                  {...invalidProps("description")}
                  onChange={(event) => patchForm({ description: event.target.value }, ["description"])}
                />
                {errorText("description")}
              </div>
              <fieldset className="grid gap-2" aria-describedby={errors.limits ? `${fieldId("limits")}-error` : undefined}>
                <legend className="mb-2 text-sm font-medium leading-none">
                  {t("billingPlans.limitsLegend")}
                  <RequiredMark />
                </legend>
                <div className="grid grid-cols-2 gap-3">
                  {WINDOWS.map(([key, label]) => (
                    <div className="grid gap-2" key={key}>
                      <Label htmlFor={fieldId(key)}>{t("billingPlans.limitWindow", { window: label })}</Label>
                      <Input
                        id={fieldId(key)}
                        inputMode="decimal"
                        value={form.limits[key]}
                        aria-invalid={errors.limits ? true : undefined}
                        onChange={(event) =>
                          patchForm({ limits: { ...form.limits, [key]: event.target.value } }, ["limits"])
                        }
                      />
                    </div>
                  ))}
                </div>
                {errorText("limits")}
              </fieldset>
              <div
                role="group"
                aria-labelledby={`${fieldId("groups")}-label`}
                aria-describedby={errors.groups ? `${fieldId("groups")}-error` : undefined}
                className="grid gap-2"
              >
                <Label id={`${fieldId("groups")}-label`} tabIndex={-1}>
                  {t("billingPlans.groups")}
                  <RequiredMark />
                </Label>
                <GroupMultiSelect
                  value={form.groupIds}
                  groups={groups}
                  loading={groupsLoading}
                  onChange={(groupIds) => patchForm({ groupIds }, ["groups"])}
                />
                {errorText("groups")}
              </div>
              <div className="grid gap-2">
                <Label htmlFor={fieldId("multiplier")}>
                  {t("billingPlans.multiplier")}
                  <RequiredMark />
                </Label>
                <Input
                  id={fieldId("multiplier")}
                  inputMode="decimal"
                  value={form.multiplier}
                  aria-required
                  {...invalidProps("multiplier", `${fieldId("multiplier")}-help`)}
                  onChange={(event) => patchForm({ multiplier: event.target.value }, ["multiplier"])}
                />
                <p id={`${fieldId("multiplier")}-help`} className="text-xs text-muted-foreground">
                  {t("billingPlans.multiplierHelp")}
                </p>
                {errorText("multiplier")}
              </div>
              <div className="flex items-center justify-between gap-4 rounded-md border p-3">
                <div className="min-w-0">
                  <Label htmlFor={fieldId("listed")}>{t("billingPlans.listed")}</Label>
                  <p className="text-sm text-muted-foreground">{t("billingPlans.listedHelp")}</p>
                </div>
                <Switch
                  id={fieldId("listed")}
                  checked={form.listed}
                  onCheckedChange={(listed) => patchForm({ listed }, ["prices"])}
                />
              </div>
              <div role="group" aria-labelledby={`${fieldId("prices")}-label`} className="grid gap-2">
                <div className="flex items-center justify-between">
                  <Label id={`${fieldId("prices")}-label`}>{t("billingPlans.prices")}</Label>
                  <Button
                    id={fieldId("add-price")}
                    type="button"
                    size="sm"
                    variant="outline"
                    onClick={() =>
                      patchForm({ prices: [...form.prices, { priceUsd: "", durationDays: "30" }] }, ["prices"])
                    }
                  >
                    <Plus aria-hidden="true" />
                    {t("billingPlans.addPrice")}
                  </Button>
                </div>
                {form.prices.length > 0 ? (
                  <div aria-hidden="true" className="grid grid-cols-[1fr_1fr_2.25rem] gap-2 text-sm font-medium leading-none">
                    <span>{t("billingPlans.priceUsd")}</span>
                    <span>{t("billingPlans.durationDays")}</span>
                  </div>
                ) : null}
                {form.prices.map((price, index) => (
                  <div key={index} className="grid grid-cols-[1fr_1fr_auto] items-start gap-2">
                    <div className="grid gap-1">
                      <Input
                        id={fieldId(`price-${index}`)}
                        inputMode="decimal"
                        aria-label={t("billingPlans.priceUsdRow", { index: index + 1 })}
                        placeholder={t("billingPlans.priceUsd")}
                        value={price.priceUsd}
                        {...invalidProps(`price-${index}`)}
                        onChange={(event) =>
                          patchForm(
                            {
                              prices: form.prices.map((entry, i) =>
                                i === index ? { ...entry, priceUsd: event.target.value } : entry
                              ),
                            },
                            [`price-${index}`]
                          )
                        }
                      />
                      {errorText(`price-${index}`)}
                    </div>
                    <div className="grid gap-1">
                      <Input
                        id={fieldId(`days-${index}`)}
                        inputMode="numeric"
                        aria-label={t("billingPlans.durationDaysRow", { index: index + 1 })}
                        placeholder={t("billingPlans.durationDays")}
                        value={price.durationDays}
                        {...invalidProps(`days-${index}`)}
                        onChange={(event) =>
                          patchForm(
                            {
                              prices: form.prices.map((entry, i) =>
                                i === index ? { ...entry, durationDays: event.target.value } : entry
                              ),
                            },
                            [`days-${index}`]
                          )
                        }
                      />
                      {errorText(`days-${index}`)}
                    </div>
                    <Button
                      type="button"
                      size="icon"
                      variant="ghost"
                      aria-label={t("billingPlans.removePrice", { index: index + 1 })}
                      onClick={() => {
                        setForm((previous) => ({
                          ...previous,
                          prices: previous.prices.filter((_, i) => i !== index),
                        }));
                        setErrors({});
                      }}
                    >
                      <X />
                    </Button>
                  </div>
                ))}
                {errorText("prices")}
              </div>
            </div>
            <DialogFooter className="shrink-0 pt-4">
              <Button type="button" variant="outline" disabled={saving} onClick={() => setOpen(false)}>
                {t("common.cancel")}
              </Button>
              <Button type="submit" disabled={saving}>
                {saving ? t("common.saving") : t("common.save")}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>

      <ConfirmDeleteDialog
        open={deleting !== null}
        onOpenChange={(value) => {
          if (!value) setDeleting(null);
        }}
        title={t("billingPlans.deleteTitle")}
        description={t("billingPlans.deleteDescription", { name: deleting?.name })}
        onConfirm={confirmDelete}
      />
    </PageWrapper>
  );
}
