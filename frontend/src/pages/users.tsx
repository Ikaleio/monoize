import { useContext, useMemo, useState, type FormEvent } from "react";
import { useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import {
  AlertCircle,
  CalendarClock,
  KeyRound,
  Mail,
  MoreHorizontal,
  Pencil,
  Plus,
  PlusCircle,
  ScrollText,
  SearchX,
  Shield,
  ShieldCheck,
  Trash2,
  User as UserIcon,
} from "lucide-react";
import { Virtuoso } from "react-virtuoso";
import { toast } from "sonner";
import { GroupsBadge } from "@/components/GroupsBadge";
import { GroupSingleSelect } from "@/components/groups/GroupPicker";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
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
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";
import { Badge } from "@/components/ui/badge";
import { Button, buttonVariants } from "@/components/ui/button";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import {
  DataList,
  DataListActions,
  DataListCell,
  DataListHead,
  DataListHeader,
} from "@/components/ui/data-list";
import { virtualDataListComponents } from "@/components/ui/data-list-virtual";
import { DataTableShell, TableToolbarSearch } from "@/components/ui/data-table-shell";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { EmptyState } from "@/components/ui/empty-state";
import {
  Field,
  FieldDescription,
  FieldError,
  FieldGroup,
  FieldLabel,
} from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { AnimatedButton, PageWrapper } from "@/components/ui/motion";
import { PageHeader } from "@/components/ui/page-header";
import { TablePageSkeleton } from "@/components/ui/page-skeleton";
import { QueryError } from "@/components/ui/query-error";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import { Skeleton } from "@/components/ui/skeleton";
import { StatusBadge } from "@/components/ui/status";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import { useAuth } from "@/hooks/use-auth";
import { useSearchParamState } from "@/hooks/use-search-param-state";
import { DashboardApiError, type UpdateUserInput, type User } from "@/lib/api";
import { apiErrorMessage, apiErrorParam, toastApiError } from "@/lib/api-error";
import { DashboardScrollParentContext } from "@/lib/dashboard-scroll";
import {
  formatNanoUsd,
  formatUsdDecimal,
  isSignedIntegerString,
  usdDecimalToNano,
} from "@/lib/exact-decimal";
import { formatDate, formatDateTime } from "@/lib/format-time";
import {
  assignUserBillingPlanSubscriptionOptimistic,
  createUserOptimistic,
  deleteUserOptimistic,
  revokeUserBillingPlanSubscriptionOptimistic,
  updateUserOptimistic,
  useBillingPlans,
  useDashboardGroups,
  useUserApiKeys,
  useUserBillingPlanSubscription,
  useUsers,
} from "@/lib/swr";
import { cn, getGravatarUrl } from "@/lib/utils";

const DURATION_UNIT_SECONDS = {
  minutes: 60,
  hours: 3_600,
  days: 86_400,
} as const;

type DurationUnit = keyof typeof DURATION_UNIT_SECONDS;

const USERNAME_PATTERN = /^[A-Za-z0-9_]{3,22}$/;
const MIN_PASSWORD_LENGTH = 8;
/** Mirrors the server's `balance_note` limit (A8). */
const MAX_BALANCE_NOTE_CHARS = 200;

function parseDurationSeconds(value: string, unit: DurationUnit): number | null {
  const trimmed = value.trim();
  if (!/^[1-9]\d*$/.test(trimmed)) return null;
  const seconds = BigInt(trimmed) * BigInt(DURATION_UNIT_SECONDS[unit]);
  if (seconds > BigInt(Number.MAX_SAFE_INTEGER)) return null;
  const numericSeconds = Number(seconds);
  const expiresAt = new Date(Date.now() + numericSeconds * 1000);
  return Number.isNaN(expiresAt.getTime()) ? null : numericSeconds;
}

const roleIcons = {
  super_admin: ShieldCheck,
  admin: Shield,
  user: UserIcon,
};

// Role is an attribute, not a status: neutral badge variants (design.md §4.2).
const roleVariants = {
  super_admin: "default" as const,
  admin: "secondary" as const,
  user: "outline" as const,
};

interface UserForm {
  username: string;
  password: string;
  role: User["role"];
  email: string;
  groupId: string;
  balanceMode: "set" | "add";
  balanceUsd: string;
  balanceAddUsd: string;
  balanceUnlimited: boolean;
  balanceNote: string;
}

const EMPTY_USER_FORM: UserForm = {
  username: "",
  password: "",
  role: "user",
  email: "",
  groupId: "",
  balanceMode: "set",
  balanceUsd: "0",
  balanceAddUsd: "",
  balanceUnlimited: false,
  balanceNote: "",
};

type UserFormField = "username" | "password" | "email" | "balance" | "balanceNote";
type UserFormErrors = Partial<Record<UserFormField, string>>;

const USER_FIELD_ORDER: readonly UserFormField[] = [
  "username",
  "password",
  "email",
  "balance",
  "balanceNote",
];
const USER_FIELD_BY_PARAM: Record<string, UserFormField> = {
  username: "username",
  password: "password",
  email: "email",
  balance: "balance",
  balance_note: "balanceNote",
};

function userFieldElementId(field: UserFormField, form: UserForm): string {
  if (field === "balance") return form.balanceMode === "set" ? "user-balance" : "user-balance-add";
  return {
    username: "user-username",
    password: "user-password",
    email: "user-email",
    balanceNote: "user-balance-note",
  }[field];
}

type UserDialogTarget = { mode: "create" } | { mode: "edit"; user: User };

interface UserFormDialogProps {
  target: UserDialogTarget | null;
  onClose: () => void;
  users: User[];
  reloadUsers: () => Promise<User[] | undefined>;
}

/** UP12/UP13/UP17/UP18: create and edit with deferred submit only. */
function UserFormDialog({ target, onClose, users, reloadUsers }: UserFormDialogProps) {
  const { t } = useTranslation();
  const { user: currentUser } = useAuth();
  const { data: groups = [], isLoading: groupsLoading } = useDashboardGroups();
  const defaultGroupId = useMemo(
    () => groups.find((group) => group.is_default)?.id ?? "",
    [groups],
  );
  const [form, setForm] = useState<UserForm>(EMPTY_USER_FORM);
  const [initialForm, setInitialForm] = useState<UserForm>(EMPTY_USER_FORM);
  // UP17: the balance the dialog showed; the "Set" guard and the "Add" preview base.
  const [expectedNano, setExpectedNano] = useState("0");
  const [errors, setErrors] = useState<UserFormErrors>({});
  const [saving, setSaving] = useState(false);
  const [openedFor, setOpenedFor] = useState<UserDialogTarget | null>(null);

  // Reset the draft whenever the dialog opens for a new target.
  if (target !== openedFor) {
    setOpenedFor(target);
    const next: UserForm =
      target?.mode === "edit"
        ? {
            ...EMPTY_USER_FORM,
            username: target.user.username,
            role: target.user.role,
            email: target.user.email ?? "",
            groupId: target.user.group_id,
            balanceUsd: target.user.balance_usd,
            balanceUnlimited: target.user.balance_unlimited,
          }
        : EMPTY_USER_FORM;
    setForm(next);
    setInitialForm(next);
    setExpectedNano(target?.mode === "edit" ? target.user.balance_nano_usd : "0");
    setErrors({});
  }

  const isEdit = target?.mode === "edit";
  const editUser = target?.mode === "edit" ? target.user : null;
  const canChooseRole =
    !isEdit || (currentUser?.role === "super_admin" && editUser?.role !== "super_admin");
  const dirty = JSON.stringify(form) !== JSON.stringify(initialForm);
  const currentNano = BigInt(expectedNano);
  const addNano = usdDecimalToNano(form.balanceAddUsd, true);
  const setNano = usdDecimalToNano(form.balanceUsd, true);

  const patch = (next: Partial<UserForm>, clear: UserFormField[] = []) => {
    setForm((prev) => ({ ...prev, ...next }));
    if (clear.length > 0) {
      setErrors((prev) => {
        const copy = { ...prev };
        for (const field of clear) delete copy[field];
        return copy;
      });
    }
  };

  const showErrors = (next: UserFormErrors) => {
    setErrors(next);
    const first = USER_FIELD_ORDER.find((field) => next[field]);
    if (first) document.getElementById(userFieldElementId(first, form))?.focus();
  };

  const validate = (): UserFormErrors => {
    const next: UserFormErrors = {};
    const username = form.username.trim();
    if ((!isEdit || username !== editUser?.username) && !USERNAME_PATTERN.test(username)) {
      next.username = t("apiErrors.invalid_username");
    }
    if ((!isEdit || form.password) && form.password.length < MIN_PASSWORD_LENGTH) {
      next.password = t("apiErrors.invalid_password");
    }
    if (isEdit) {
      if (form.balanceMode === "set" && setNano === null) {
        next.balance = t("users.balanceInvalid");
      }
      if (form.balanceMode === "add" && form.balanceAddUsd.trim() && addNano === null) {
        next.balance = t("users.balanceInvalid");
      }
      if (form.balanceNote.trim().length > MAX_BALANCE_NOTE_CHARS) {
        next.balanceNote = t("users.balanceNoteTooLong", { max: MAX_BALANCE_NOTE_CHARS });
      }
    }
    return next;
  };

  const buildUpdate = (user: User): UpdateUserInput => {
    const updates: UpdateUserInput = {};
    const username = form.username.trim();
    if (username !== user.username) updates.username = username;
    if (form.password) updates.password = form.password;
    if (form.role !== user.role) updates.role = form.role;
    const email = form.email.trim();
    if (email !== (user.email ?? "")) updates.email = email || null;
    if (form.groupId && form.groupId !== user.group_id) updates.group_id = form.groupId;
    if (form.balanceMode === "add") {
      if (addNano !== null && addNano !== 0n) {
        updates.balance_delta_nano_usd = addNano.toString();
      }
    } else if (setNano !== null && setNano !== currentNano) {
      updates.balance_nano_usd = setNano.toString();
      updates.expected_balance_nano_usd = expectedNano;
    }
    if (form.balanceUnlimited !== user.balance_unlimited) {
      updates.balance_unlimited = form.balanceUnlimited;
    }
    const note = form.balanceNote.trim();
    const changesBalance =
      updates.balance_delta_nano_usd !== undefined
      || updates.balance_nano_usd !== undefined
      || updates.balance_unlimited !== undefined;
    if (note && changesBalance) updates.balance_note = note;
    return updates;
  };

  const handleSubmit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!target) return;
    const next = validate();
    if (Object.keys(next).length > 0) {
      showErrors(next);
      return;
    }
    setErrors({});
    setSaving(true);
    try {
      if (target.mode === "create") {
        await createUserOptimistic(
          form.username.trim(),
          form.password,
          form.role,
          form.groupId || undefined,
          users,
        );
        toast.success(t("users.createSuccess"));
      } else {
        await updateUserOptimistic(target.user.id, buildUpdate(target.user), users);
        toast.success(t("users.updateSuccess"));
      }
      onClose();
    } catch (error) {
      if (error instanceof DashboardApiError && error.code === "balance_changed" && editUser) {
        const fresh = (await reloadUsers())?.find((user) => user.id === editUser.id);
        if (fresh) setExpectedNano(fresh.balance_nano_usd);
        showErrors({
          balance: t("users.balanceChanged", {
            amount: fresh
              ? formatUsdDecimal(fresh.balance_usd, 2)
              : formatNanoUsd(expectedNano, 2),
          }),
        });
        return;
      }
      const field = USER_FIELD_BY_PARAM[apiErrorParam(error) ?? ""];
      if (field) {
        showErrors({ [field]: apiErrorMessage(error, t) });
      } else {
        toastApiError(error, t, t(isEdit ? "users.failedUpdate" : "users.failedCreate"));
      }
    } finally {
      setSaving(false);
    }
  };

  return (
    <Dialog open={target !== null} onOpenChange={(open) => { if (!open) onClose(); }}>
      <DialogContent
        dirty={dirty}
        className="max-h-[calc(100dvh-2rem)] overflow-hidden p-0 sm:max-h-[calc(100dvh-3rem)]"
      >
        <form noValidate onSubmit={(event) => void handleSubmit(event)} className="flex min-h-0 flex-col p-6">
          <DialogHeader className="shrink-0">
            <DialogTitle>{t(isEdit ? "users.editUser" : "users.createUser")}</DialogTitle>
            <DialogDescription>{t(isEdit ? "users.updateDetails" : "users.addNewUser")}</DialogDescription>
          </DialogHeader>
          <div className="mt-2 min-h-0 flex-1 overflow-y-auto pr-1">
            <FieldGroup className="gap-4 py-4">
              <Field data-invalid={errors.username ? true : undefined}>
                <FieldLabel htmlFor="user-username">{t("auth.username")}</FieldLabel>
                <Input
                  id="user-username"
                  value={form.username}
                  autoComplete="off"
                  aria-invalid={errors.username ? true : undefined}
                  onChange={(event) => patch({ username: event.target.value }, ["username"])}
                  placeholder="johndoe"
                />
                {errors.username && <FieldError>{errors.username}</FieldError>}
              </Field>
              <Field data-invalid={errors.password ? true : undefined}>
                <FieldLabel htmlFor="user-password">
                  {t(isEdit ? "users.newPassword" : "auth.password")}
                </FieldLabel>
                <Input
                  id="user-password"
                  type="password"
                  autoComplete="new-password"
                  value={form.password}
                  aria-invalid={errors.password ? true : undefined}
                  onChange={(event) => patch({ password: event.target.value }, ["password"])}
                  placeholder="••••••••"
                />
                {errors.password && <FieldError>{errors.password}</FieldError>}
              </Field>
              {isEdit && (
                <Field data-invalid={errors.email ? true : undefined}>
                  <FieldLabel htmlFor="user-email">{t("userSettings.email")}</FieldLabel>
                  <div className="relative">
                    <Mail
                      className="absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground"
                      aria-hidden="true"
                    />
                    <Input
                      id="user-email"
                      type="email"
                      value={form.email}
                      aria-invalid={errors.email ? true : undefined}
                      onChange={(event) => patch({ email: event.target.value }, ["email"])}
                      placeholder="user@example.com"
                      className="pl-9"
                    />
                  </div>
                  <FieldDescription>{t("userSettings.emailDescription")}</FieldDescription>
                  {errors.email && <FieldError>{errors.email}</FieldError>}
                </Field>
              )}
              {canChooseRole && (
                <Field>
                  <FieldLabel htmlFor="user-role">{t("users.role")}</FieldLabel>
                  <Select
                    value={form.role}
                    onValueChange={(role) => patch({ role: role as User["role"] })}
                  >
                    <SelectTrigger id="user-role">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {currentUser?.role === "super_admin" && (
                        <SelectItem value="admin">{t("roles.admin")}</SelectItem>
                      )}
                      <SelectItem value="user">{t("roles.user")}</SelectItem>
                    </SelectContent>
                  </Select>
                </Field>
              )}
              <Field>
                <FieldLabel htmlFor="user-group">{t("users.group")}</FieldLabel>
                <GroupSingleSelect
                  id="user-group"
                  value={form.groupId || defaultGroupId}
                  groups={groups}
                  loading={groupsLoading}
                  onChange={(groupId) => patch({ groupId })}
                />
                <FieldDescription>{t("users.groupHelp")}</FieldDescription>
              </Field>
              {isEdit && (
                <Field data-invalid={errors.balance ? true : undefined}>
                  <div className="flex items-center justify-between gap-2">
                    <FieldLabel
                      htmlFor={form.balanceMode === "set" ? "user-balance" : "user-balance-add"}
                    >
                      {t("users.balance")}
                    </FieldLabel>
                    <Tabs
                      value={form.balanceMode}
                      onValueChange={(mode) =>
                        patch({ balanceMode: mode as UserForm["balanceMode"], balanceAddUsd: "" }, ["balance"])
                      }
                    >
                      <TabsList className="h-7">
                        <TabsTrigger value="set" className="px-2.5 py-0.5 text-xs">
                          {t("users.balanceSet")}
                        </TabsTrigger>
                        <TabsTrigger value="add" className="px-2.5 py-0.5 text-xs">
                          <PlusCircle className="mr-1 h-3 w-3" aria-hidden="true" />
                          {t("users.balanceAdd")}
                        </TabsTrigger>
                      </TabsList>
                    </Tabs>
                  </div>
                  {form.balanceMode === "set" ? (
                    <Input
                      id="user-balance"
                      inputMode="decimal"
                      value={form.balanceUsd}
                      aria-invalid={errors.balance ? true : undefined}
                      onChange={(event) => patch({ balanceUsd: event.target.value }, ["balance"])}
                      placeholder="0"
                    />
                  ) : (
                    <Input
                      id="user-balance-add"
                      inputMode="decimal"
                      value={form.balanceAddUsd}
                      aria-invalid={errors.balance ? true : undefined}
                      onChange={(event) => patch({ balanceAddUsd: event.target.value }, ["balance"])}
                      placeholder={t("users.balanceAddPlaceholder")}
                    />
                  )}
                  <FieldDescription className="tabular-nums">
                    {t("users.balanceCurrentHint", { amount: formatNanoUsd(currentNano, 2) })}
                    {form.balanceMode === "add" && addNano !== null && addNano !== 0n && (
                      <>
                        {" → "}
                        <span className="font-medium text-foreground">
                          {formatNanoUsd(currentNano + addNano, 2)}
                        </span>
                      </>
                    )}
                    {form.balanceMode === "set" && setNano !== null && setNano !== currentNano && (
                      <>
                        {" → "}
                        <span className="font-medium text-foreground">{formatNanoUsd(setNano, 2)}</span>
                      </>
                    )}
                  </FieldDescription>
                  {errors.balance && <FieldError>{errors.balance}</FieldError>}
                  <div className="flex items-center gap-2">
                    <Switch
                      id="user-balance-unlimited"
                      checked={form.balanceUnlimited}
                      onCheckedChange={(balanceUnlimited) => patch({ balanceUnlimited })}
                    />
                    <FieldLabel htmlFor="user-balance-unlimited" className="font-normal text-muted-foreground">
                      {t("users.unlimited")}
                    </FieldLabel>
                  </div>
                </Field>
              )}
              {isEdit && (
                <Field data-invalid={errors.balanceNote ? true : undefined}>
                  <FieldLabel htmlFor="user-balance-note">{t("users.balanceNote")}</FieldLabel>
                  <Textarea
                    id="user-balance-note"
                    rows={2}
                    value={form.balanceNote}
                    aria-invalid={errors.balanceNote ? true : undefined}
                    onChange={(event) => patch({ balanceNote: event.target.value }, ["balanceNote"])}
                    placeholder={t("users.balanceNotePlaceholder")}
                  />
                  <FieldDescription>{t("users.balanceNoteHelp")}</FieldDescription>
                  {errors.balanceNote && <FieldError>{errors.balanceNote}</FieldError>}
                </Field>
              )}
            </FieldGroup>
          </div>
          <DialogFooter className="shrink-0 pt-4">
            <Button type="button" variant="outline" onClick={onClose}>
              {t("common.cancel")}
            </Button>
            <Button type="submit" disabled={saving}>
              {isEdit
                ? t(saving ? "common.saving" : "common.save")
                : t(saving ? "common.creating" : "common.create")}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

interface PlanDialogProps {
  user: User | null;
  onClose: () => void;
  onChanged: () => void;
}

/** BP-UI6..BP-UI8: immediate-effect subscription management, separate from the edit form. */
function PlanDialog({ user, onClose, onChanged }: PlanDialogProps) {
  const { t } = useTranslation();
  const { user: currentUser } = useAuth();
  const {
    data: billingPlans = [],
    isLoading: billingPlansLoading,
    error: billingPlansError,
    mutate: revalidateBillingPlans,
  } = useBillingPlans();
  const {
    data: subscription,
    isLoading: subscriptionLoading,
    error: subscriptionError,
    mutate: revalidateSubscription,
  } = useUserBillingPlanSubscription(user?.id ?? null);
  const [selectedPlanId, setSelectedPlanId] = useState("");
  const [durationValue, setDurationValue] = useState("30");
  const [durationUnit, setDurationUnit] = useState<DurationUnit>("days");
  const [saving, setSaving] = useState(false);
  const [confirmation, setConfirmation] = useState<"replace" | "revoke" | null>(null);
  const [openedFor, setOpenedFor] = useState<User | null>(null);
  if (user !== openedFor) {
    setOpenedFor(user);
    setSelectedPlanId("");
    setDurationValue("30");
    setDurationUnit("days");
    setConfirmation(null);
  }
  const selectedPlan = billingPlans.find((plan) => plan.id === selectedPlanId) ?? null;
  const durationSeconds = parseDurationSeconds(durationValue, durationUnit);
  const isSelf = user?.id === currentUser?.id;

  const errorMessage = (error: unknown) => {
    if (error instanceof DashboardApiError) {
      if (error.code === "plan_not_available") return t("users.planUnavailable");
      if (error.code === "forbidden") return t("users.planForbidden");
      if (error.code === "not_found") return t("users.planUserMissing");
    }
    return t("users.planUpdateFailed");
  };

  const assign = async () => {
    if (!user || !selectedPlan || durationSeconds === null) return;
    setSaving(true);
    try {
      await assignUserBillingPlanSubscriptionOptimistic(
        user.id,
        selectedPlan,
        durationSeconds,
        subscription,
        isSelf,
      );
      toast.success(t(subscription ? "users.planReplaced" : "users.planAssigned"));
      setSelectedPlanId("");
      setDurationValue("30");
      setDurationUnit("days");
      setConfirmation(null);
      onChanged();
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      setSaving(false);
    }
  };

  const revoke = async () => {
    if (!user) return;
    setSaving(true);
    try {
      await revokeUserBillingPlanSubscriptionOptimistic(user.id, subscription, isSelf);
      toast.success(t("users.planRemoved"));
      setConfirmation(null);
      onChanged();
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      setSaving(false);
    }
  };

  return (
    <>
      <Dialog open={user !== null} onOpenChange={(open) => { if (!open) onClose(); }}>
        <DialogContent className="sm:max-w-lg">
          <DialogHeader>
            <DialogTitle>{t("users.planDialogTitle", { name: user?.username })}</DialogTitle>
            <DialogDescription>{t("users.subscriptionHelp")}</DialogDescription>
          </DialogHeader>
          {subscriptionLoading || billingPlansLoading ? (
            <div className="flex flex-col gap-3" aria-label={t("common.loading")}>
              <Skeleton className="h-16 w-full" />
              <div className="grid gap-3 sm:grid-cols-2">
                <Skeleton className="h-9 w-full" />
                <Skeleton className="h-9 w-full" />
              </div>
            </div>
          ) : subscriptionError || billingPlansError ? (
            <Alert variant="destructive">
              <AlertCircle aria-hidden="true" />
              <AlertTitle>{t("users.planLoadFailed")}</AlertTitle>
              <AlertDescription className="flex flex-col items-start gap-3">
                <p>{t("users.planLoadFailedHelp")}</p>
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  onClick={() => void Promise.all([revalidateSubscription(), revalidateBillingPlans()])}
                >
                  {t("common.retry")}
                </Button>
              </AlertDescription>
            </Alert>
          ) : (
            <div className="flex flex-col gap-4">
              <div className="flex min-w-0 items-start gap-3 rounded-md border p-3">
                <CalendarClock className="mt-0.5 size-5 shrink-0 text-muted-foreground" aria-hidden="true" />
                <div className="min-w-0 flex-1">
                  <div className="flex flex-wrap items-center gap-2">
                    <span className="text-sm font-medium">
                      {subscription?.plan_name ?? t("users.noActivePlan")}
                    </span>
                    {subscription && <StatusBadge variant="success">{t("users.planActive")}</StatusBadge>}
                  </div>
                  <p className="mt-1 text-sm text-muted-foreground">
                    {subscription
                      ? t("users.planExpires", { date: formatDateTime(subscription.expires_at) })
                      : t("users.noActivePlanHelp")}
                  </p>
                </div>
              </div>
              <FieldGroup className="gap-3 sm:grid sm:grid-cols-2">
                <Field>
                  <FieldLabel htmlFor="plan-select">{t("users.choosePlan")}</FieldLabel>
                  <Select
                    value={selectedPlanId}
                    onValueChange={setSelectedPlanId}
                    disabled={billingPlans.length === 0 || saving}
                  >
                    <SelectTrigger id="plan-select">
                      <SelectValue placeholder={t("users.choosePlanPlaceholder")} />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectGroup>
                        {billingPlans.map((plan) => (
                          <SelectItem key={plan.id} value={plan.id}>
                            {plan.name}
                            {plan.listed ? "" : ` · ${t("users.planUnlisted")}`}
                          </SelectItem>
                        ))}
                      </SelectGroup>
                    </SelectContent>
                  </Select>
                </Field>
                <Field data-invalid={durationSeconds === null ? true : undefined}>
                  <FieldLabel htmlFor="plan-duration">{t("users.chooseDuration")}</FieldLabel>
                  <div className="grid grid-cols-[minmax(0,1fr)_7.5rem] gap-2">
                    <Input
                      id="plan-duration"
                      inputMode="numeric"
                      value={durationValue}
                      onChange={(event) => setDurationValue(event.target.value)}
                      disabled={saving}
                      aria-invalid={durationSeconds === null ? true : undefined}
                    />
                    <Select
                      value={durationUnit}
                      onValueChange={(value) => setDurationUnit(value as DurationUnit)}
                      disabled={saving}
                    >
                      <SelectTrigger aria-label={t("users.durationUnit")}>
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value="minutes">{t("users.durationMinutes")}</SelectItem>
                        <SelectItem value="hours">{t("users.durationHours")}</SelectItem>
                        <SelectItem value="days">{t("users.durationDays")}</SelectItem>
                      </SelectContent>
                    </Select>
                  </div>
                  {durationSeconds === null ? (
                    <FieldError>{t("users.durationInvalid")}</FieldError>
                  ) : (
                    <FieldDescription>{t("users.durationHelp")}</FieldDescription>
                  )}
                </Field>
              </FieldGroup>
              {billingPlans.length === 0 && (
                <FieldDescription>{t("users.noPlansAvailable")}</FieldDescription>
              )}
              <div className="flex flex-col-reverse gap-2 sm:flex-row sm:justify-end">
                {subscription && (
                  <Button
                    type="button"
                    variant="outline"
                    className="text-error-foreground hover:text-error-foreground"
                    disabled={saving}
                    onClick={() => setConfirmation("revoke")}
                  >
                    {t("users.removePlan")}
                  </Button>
                )}
                <Button
                  type="button"
                  variant="secondary"
                  disabled={!selectedPlan || durationSeconds === null || saving}
                  onClick={() => {
                    if (subscription) setConfirmation("replace");
                    else void assign();
                  }}
                >
                  {saving
                    ? t("common.saving")
                    : t(subscription ? "users.replacePlan" : "users.assignPlan")}
                </Button>
              </div>
            </div>
          )}
          <DialogFooter>
            <Button type="button" variant="outline" onClick={onClose}>
              {t("common.close")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <AlertDialog open={confirmation !== null} onOpenChange={(open) => !open && setConfirmation(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {t(confirmation === "revoke" ? "users.removePlanTitle" : "users.replacePlanTitle")}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {t(
                confirmation === "revoke"
                  ? "users.removePlanDescription"
                  : "users.replacePlanDescription",
                { current: subscription?.plan_name, replacement: selectedPlan?.name },
              )}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={saving}>{t("common.cancel")}</AlertDialogCancel>
            <AlertDialogAction
              disabled={saving}
              className={confirmation === "revoke" ? buttonVariants({ variant: "destructive" }) : undefined}
              onClick={(event) => {
                event.preventDefault();
                if (confirmation === "revoke") void revoke();
                else void assign();
              }}
            >
              {saving
                ? t("common.saving")
                : t(confirmation === "revoke" ? "users.removePlan" : "users.replacePlan")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}

/** UP15: read-only list of another user's API keys. */
function UserKeysSheet({
  user,
  openedAt,
  onClose,
}: {
  user: User | null;
  /** Time the sheet was opened; expiry is judged against it (render stays pure). */
  openedAt: number;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const { data, error, isLoading, isValidating, mutate } = useUserApiKeys(user?.id ?? null);

  return (
    <Sheet open={user !== null} onOpenChange={(open) => { if (!open) onClose(); }}>
      <SheetContent side="right" className="flex w-full flex-col gap-4 overflow-y-auto sm:max-w-md">
        <SheetHeader>
          <SheetTitle>{t("users.keysTitle", { name: user?.username })}</SheetTitle>
          <SheetDescription>{t("users.keysDescription")}</SheetDescription>
        </SheetHeader>
        {error ? (
          <QueryError onRetry={() => void mutate()} retrying={isValidating} stale={data !== undefined} />
        ) : null}
        {isLoading ? (
          <div className="flex flex-col gap-2" aria-label={t("common.loading")}>
            <Skeleton className="h-14 w-full" />
            <Skeleton className="h-14 w-full" />
            <Skeleton className="h-14 w-full" />
          </div>
        ) : data && data.length === 0 ? (
          <EmptyState
            variant="inline"
            icon={<KeyRound className="size-8" aria-hidden="true" />}
            title={t("users.keysEmpty")}
          />
        ) : data ? (
          <ul className="flex flex-col divide-y rounded-lg border">
            {data.map((key) => {
              const expired = key.expires_at != null && new Date(key.expires_at).getTime() < openedAt;
              return (
                <li key={key.id} className="flex min-w-0 items-start justify-between gap-3 p-3">
                  <div className="min-w-0">
                    <p className={cn("truncate font-medium", (!key.enabled || expired) && "text-muted-foreground")}>
                      {key.name}
                    </p>
                    <code className="text-xs text-muted-foreground">{key.key_prefix}…</code>
                  </div>
                  <div className="flex shrink-0 flex-col items-end gap-1 text-sm">
                    {expired ? (
                      <StatusBadge variant="warning">{t("apiKeys.expired")}</StatusBadge>
                    ) : key.enabled ? (
                      <StatusBadge variant="success">{t("common.enabled")}</StatusBadge>
                    ) : (
                      <Badge variant="outline">{t("common.disabled")}</Badge>
                    )}
                    <span className="tabular-nums text-muted-foreground">
                      {key.sub_account_enabled
                        ? formatUsdDecimal(key.sub_account_balance_usd, 2)
                        : t("apiKeys.usesAccountBalance")}
                    </span>
                  </div>
                </li>
              );
            })}
          </ul>
        ) : null}
      </SheetContent>
    </Sheet>
  );
}

/** UP16: disabling another user states how many keys stop working. */
function DisableUserDialog({
  user,
  onCancel,
  onConfirm,
}: {
  user: User | null;
  onCancel: () => void;
  onConfirm: (user: User) => void;
}) {
  const { t } = useTranslation();
  const { data: keys } = useUserApiKeys(user?.id ?? null);

  return (
    <AlertDialog open={user !== null} onOpenChange={(open) => { if (!open) onCancel(); }}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{t("users.disableTitle", { name: user?.username })}</AlertDialogTitle>
          <AlertDialogDescription>
            {keys
              ? t("users.disableDescription", { name: user?.username, count: keys.length })
              : t("users.disableDescriptionNoCount", { name: user?.username })}
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>{t("common.cancel")}</AlertDialogCancel>
          <AlertDialogAction
            className={buttonVariants({ variant: "destructive" })}
            onClick={() => {
              if (user) onConfirm(user);
            }}
          >
            {t("users.disableConfirm")}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}

export function UsersPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const { user: currentUser } = useAuth();
  const { data, error, isLoading, isValidating, mutate } = useUsers();
  const users = useMemo(() => data ?? [], [data]);
  const scrollParent = useContext(DashboardScrollParentContext);
  const [urlQuery, setUrlQuery] = useSearchParamState<string>("q", "");
  // The router applies search-param writes asynchronously; binding the input to the URL
  // drops keystrokes typed before the previous write lands, so the input owns a draft.
  const [query, setQueryDraft] = useState(urlQuery);
  const setQuery = (next: string) => {
    setQueryDraft(next);
    setUrlQuery(next);
  };
  const needle = query.trim().toLowerCase();
  const visibleUsers = useMemo(
    () =>
      users.filter(
        (user) =>
          needle === ""
          || user.username.toLowerCase().includes(needle)
          || (user.email ?? "").toLowerCase().includes(needle),
      ),
    [users, needle],
  );
  const todayTotals = useMemo(() => {
    let calls = 0;
    let cost = 0n;
    for (const user of visibleUsers) {
      calls += user.today_calls ?? 0;
      const raw = user.today_cost_nano_usd ?? "0";
      if (isSignedIntegerString(raw)) cost += BigInt(raw);
    }
    return { calls, cost };
  }, [visibleUsers]);
  const [dialogTarget, setDialogTarget] = useState<UserDialogTarget | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<User | null>(null);
  const [planUser, setPlanUser] = useState<User | null>(null);
  const [keysUser, setKeysUser] = useState<User | null>(null);
  const [keysOpenedAt, setKeysOpenedAt] = useState(0);
  const [disableTarget, setDisableTarget] = useState<User | null>(null);

  const setEnabled = async (user: User, enabled: boolean) => {
    try {
      await updateUserOptimistic(user.id, { enabled }, users);
    } catch (error) {
      toastApiError(error, t, t("users.failedUpdate"));
    }
  };

  const confirmDelete = async () => {
    if (!deleteTarget) return;
    try {
      await deleteUserOptimistic(deleteTarget.id, users);
    } catch (error) {
      toastApiError(error, t, t("users.failedDelete"));
    }
  };

  const canEdit = (user: User) => {
    if (currentUser?.role === "super_admin") return true;
    if (user.role === "super_admin") return false;
    return currentUser?.role === "admin";
  };

  const canDelete = (user: User) =>
    user.id !== currentUser?.id && user.role !== "super_admin" && canEdit(user);

  if (isLoading) {
    return (
      <PageWrapper className="space-y-6">
        <TablePageSkeleton />
      </PageWrapper>
    );
  }

  return (
    <PageWrapper className="space-y-6">
      <PageHeader
        title={t("users.title")}
        description={t("users.description")}
        actions={
          <AnimatedButton>
            <Button onClick={() => setDialogTarget({ mode: "create" })}>
              <Plus data-icon="inline-start" aria-hidden="true" />
              {t("users.addUser")}
            </Button>
          </AnimatedButton>
        }
      />

      {error ? <QueryError onRetry={mutate} retrying={isValidating} stale={data !== undefined} /> : null}

      {data !== undefined ? (
        <DataTableShell
          toolbar={
            users.length > 0 ? (
              <>
                <TableToolbarSearch
                  value={query}
                  onChange={(event) => setQuery(event.target.value)}
                  placeholder={t("users.searchPlaceholder")}
                  aria-label={t("users.searchPlaceholder")}
                  containerClassName="sm:flex-none"
                />
                <p className="text-sm tabular-nums text-muted-foreground">
                  {t("users.usersTotal", { count: visibleUsers.length })}
                  {" · "}
                  {t("users.todaySummary", {
                    spend: formatNanoUsd(todayTotals.cost, 2),
                    calls: todayTotals.calls.toLocaleString(),
                  })}
                </p>
              </>
            ) : undefined
          }
          isEmpty={users.length === 0}
          emptyState={
            <EmptyState
              icon={<UserIcon className="size-10" aria-hidden="true" />}
              title={t("users.allUsers")}
              description={t("users.noUsers")}
            />
          }
        >
          {visibleUsers.length === 0 ? (
            <EmptyState
              variant="inline"
              icon={<SearchX className="size-8" aria-hidden="true" />}
              title={t("users.noMatchesTitle")}
              description={t("users.noMatchesDescription", { query: query.trim() })}
            />
          ) : (
            <DataList columns="minmax(12rem,1.6fr) 6.5rem 6.5rem 8rem 7rem 3.5rem 5rem">
              <DataListHeader>
                <DataListHead>{t("users.user")}</DataListHead>
                <DataListHead align="end">{t("users.balance")}</DataListHead>
                <DataListHead align="end">{t("users.today")}</DataListHead>
                <DataListHead>{t("users.plan")}</DataListHead>
                <DataListHead>{t("users.dates")}</DataListHead>
                <DataListHead>{t("common.status")}</DataListHead>
                <DataListHead align="end">{t("common.actions")}</DataListHead>
              </DataListHeader>
              {scrollParent && (
                <Virtuoso
                  customScrollParent={scrollParent}
                  data={visibleUsers}
                  context={{ label: t("users.title") }}
                  computeItemKey={(_index, user) => user.id}
                  components={virtualDataListComponents}
                  itemContent={(_index, user) => {
                    const RoleIcon = roleIcons[user.role];
                    const isSelf = user.id === currentUser?.id;
                    const editable = canEdit(user);
                    return (
                      <>
                        <DataListCell primary>
                          <div className="flex min-w-0 items-center gap-2">
                            <Avatar className="size-8 shrink-0">
                              {user.email && (
                                <AvatarImage
                                  src={getGravatarUrl(user.email, 64) ?? undefined}
                                  alt={user.username}
                                />
                              )}
                              <AvatarFallback>{user.username[0].toUpperCase()}</AvatarFallback>
                            </Avatar>
                            <div className="min-w-0 flex-1">
                              <p
                                className={cn(
                                  "truncate font-medium",
                                  !user.enabled && "text-muted-foreground",
                                )}
                                title={user.username}
                              >
                                {user.username}
                              </p>
                              <div className="mt-0.5 flex min-w-0 items-center gap-1.5">
                                <Badge
                                  variant={roleVariants[user.role]}
                                  className="shrink-0 gap-1 whitespace-nowrap px-1.5 text-xs"
                                >
                                  <RoleIcon className="size-3 shrink-0" aria-hidden="true" />
                                  {t(`roles.${user.role}`)}
                                </Badge>
                                {user.group_id && (
                                  <GroupsBadge groupIds={[user.group_id]} className="min-w-0" />
                                )}
                              </div>
                            </div>
                          </div>
                        </DataListCell>
                        <DataListCell label={t("users.balance")} align="end">
                          <span className="tabular-nums">
                            {user.balance_unlimited
                              ? t("users.unlimited")
                              : formatUsdDecimal(user.balance_usd, 2)}
                          </span>
                        </DataListCell>
                        <DataListCell label={t("users.today")} align="end">
                          <span className="block tabular-nums">
                            {formatNanoUsd(user.today_cost_nano_usd, 2)}
                          </span>
                          <span className="block text-xs tabular-nums text-muted-foreground">
                            {t("users.callsCount", { count: user.today_calls ?? 0 })}
                          </span>
                        </DataListCell>
                        <DataListCell label={t("users.plan")}>
                          {user.active_plan_name ? (
                            <>
                              <span className="block truncate" title={user.active_plan_name}>
                                {user.active_plan_name}
                              </span>
                              <span className="block text-xs tabular-nums text-muted-foreground">
                                {user.active_plan_expires_at
                                  ? t("users.planUntil", { date: formatDate(user.active_plan_expires_at) })
                                  : null}
                              </span>
                            </>
                          ) : (
                            <span className="text-muted-foreground">—</span>
                          )}
                        </DataListCell>
                        <DataListCell label={t("users.dates")}>
                          <span className="block tabular-nums text-muted-foreground" title={t("common.created")}>
                            {formatDate(user.created_at)}
                          </span>
                          <span className="block text-xs tabular-nums text-muted-foreground" title={t("users.lastLogin")}>
                            {user.last_login_at
                              ? t("users.lastLoginAt", { date: formatDate(user.last_login_at) })
                              : t("users.neverLoggedIn")}
                          </span>
                        </DataListCell>
                        <DataListCell label={t("common.status")}>
                          <Switch
                            className="align-middle"
                            checked={user.enabled}
                            aria-label={t("common.enableItem", { name: user.username })}
                            title={isSelf ? t("users.cannotDisableSelf") : undefined}
                            aria-describedby={isSelf ? `user-self-disable-${user.id}` : undefined}
                            onCheckedChange={(enabled) => {
                              if (enabled) void setEnabled(user, true);
                              else setDisableTarget(user);
                            }}
                            disabled={!editable || isSelf}
                          />
                          {isSelf && (
                            <span id={`user-self-disable-${user.id}`} className="sr-only">
                              {t("users.cannotDisableSelf")}
                            </span>
                          )}
                        </DataListCell>
                        <DataListActions>
                          {editable && (
                            <Button
                              variant="ghost"
                              size="icon"
                              className="size-11 touch-manipulation sm:size-9"
                              aria-label={t("common.editItem", { name: user.username })}
                              onClick={() => setDialogTarget({ mode: "edit", user })}
                            >
                              <Pencil />
                            </Button>
                          )}
                          <DropdownMenu>
                            <DropdownMenuTrigger asChild>
                              <Button
                                variant="ghost"
                                size="icon"
                                className="size-11 touch-manipulation sm:size-9"
                                aria-label={t("users.moreActions", { name: user.username })}
                              >
                                <MoreHorizontal />
                              </Button>
                            </DropdownMenuTrigger>
                            <DropdownMenuContent align="end">
                              <DropdownMenuItem
                                onSelect={() =>
                                  navigate(`/dashboard/logs?username=${encodeURIComponent(user.username)}`)
                                }
                              >
                                <ScrollText aria-hidden="true" />
                                {t("users.viewLogs")}
                              </DropdownMenuItem>
                              <DropdownMenuItem
                                onSelect={() => {
                                  setKeysOpenedAt(Date.now());
                                  setKeysUser(user);
                                }}
                              >
                                <KeyRound aria-hidden="true" />
                                {t("users.viewKeys")}
                              </DropdownMenuItem>
                              {editable && (
                                <DropdownMenuItem onSelect={() => setPlanUser(user)}>
                                  <CalendarClock aria-hidden="true" />
                                  {t("users.managePlan")}
                                </DropdownMenuItem>
                              )}
                              {canDelete(user) && (
                                <>
                                  <DropdownMenuSeparator />
                                  <DropdownMenuItem
                                    className="text-error-foreground focus:text-error-foreground"
                                    onSelect={() => setDeleteTarget(user)}
                                  >
                                    <Trash2 aria-hidden="true" />
                                    {t("common.delete")}
                                  </DropdownMenuItem>
                                </>
                              )}
                            </DropdownMenuContent>
                          </DropdownMenu>
                        </DataListActions>
                      </>
                    );
                  }}
                />
              )}
            </DataList>
          )}
        </DataTableShell>
      ) : null}

      <UserFormDialog
        target={dialogTarget}
        onClose={() => setDialogTarget(null)}
        users={users}
        reloadUsers={() => mutate()}
      />

      <PlanDialog user={planUser} onClose={() => setPlanUser(null)} onChanged={() => void mutate()} />

      <UserKeysSheet user={keysUser} openedAt={keysOpenedAt} onClose={() => setKeysUser(null)} />

      <DisableUserDialog
        user={disableTarget}
        onCancel={() => setDisableTarget(null)}
        onConfirm={(user) => {
          setDisableTarget(null);
          void setEnabled(user, false);
        }}
      />

      <ConfirmDeleteDialog
        open={deleteTarget !== null}
        onOpenChange={(open) => {
          if (!open) setDeleteTarget(null);
        }}
        title={t("users.deleteTitle")}
        description={t("users.confirmDelete", { name: deleteTarget?.username })}
        onConfirm={confirmDelete}
      />
    </PageWrapper>
  );
}
