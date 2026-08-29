import { useMemo, useState } from "react";
import { useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import {
  Plus,
  Trash2,
  Pencil,
  Shield,
  ShieldCheck,
  User as UserIcon,
  Mail,
  PlusCircle,
  ScrollText,
  CalendarClock,
  AlertCircle,
} from "lucide-react";
import { GroupsBadge } from "@/components/GroupsBadge";
import { GroupSingleSelect } from "@/components/groups/GroupPicker";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Badge } from "@/components/ui/badge";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Skeleton } from "@/components/ui/skeleton";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Field,
  FieldDescription,
  FieldGroup,
  FieldLabel,
  FieldLegend,
  FieldSet,
} from "@/components/ui/field";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
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
import { TableVirtuoso } from "react-virtuoso";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useAuth } from "@/hooks/use-auth";
import {
  useUsers,
  useDashboardGroups,
  createUserOptimistic,
  updateUserOptimistic,
  deleteUserOptimistic,
  assignUserBillingPlanSubscriptionOptimistic,
  revokeUserBillingPlanSubscriptionOptimistic,
  useBillingPlans,
  useUserBillingPlanSubscription,
} from "@/lib/swr";
import { DashboardApiError, type User } from "@/lib/api";
import {
  formatNanoUsd,
  formatUsdDecimal,
  isSignedIntegerString,
} from "@/lib/exact-decimal";
import { Avatar, AvatarImage, AvatarFallback } from "@/components/ui/avatar";
import { getGravatarUrl } from "@/lib/utils";
import {
  AnimatedButton,
  PageWrapper,
  motion,
  transitions,
} from "@/components/ui/motion";
import { PageHeader } from "@/components/ui/page-header";
import { TablePageSkeleton } from "@/components/ui/page-skeleton";
import {
  DataTableShell,
  VirtualTableCell,
  VirtualTableHeaderCell,
} from "@/components/ui/data-table-shell";
import { EmptyState } from "@/components/ui/empty-state";
import { toast } from "sonner";

const NANO_PER_USD = 1_000_000_000n;
const DURATION_UNIT_SECONDS = {
  minutes: 60,
  hours: 3_600,
  days: 86_400,
} as const;

type DurationUnit = keyof typeof DURATION_UNIT_SECONDS;

function parseDurationSeconds(
  value: string,
  unit: DurationUnit,
): number | null {
  const trimmed = value.trim();
  if (!/^[1-9]\d*$/.test(trimmed)) return null;
  const seconds = BigInt(trimmed) * BigInt(DURATION_UNIT_SECONDS[unit]);
  if (seconds > BigInt(Number.MAX_SAFE_INTEGER)) return null;
  const numericSeconds = Number(seconds);
  const expiresAt = new Date(Date.now() + numericSeconds * 1000);
  return Number.isNaN(expiresAt.getTime()) ? null : numericSeconds;
}

function parseUsdToNanoBigInt(usd: string): bigint | null {
  const trimmed = usd.trim();
  if (!trimmed || trimmed === "-") return null;

  const negative = trimmed.startsWith("-");
  const abs = negative ? trimmed.slice(1) : trimmed;
  const parts = abs.split(".");
  if (parts.length > 2) return null;

  const intPart = parts[0] || "0";
  let fracPart = parts[1] || "";
  if (fracPart.length > 9) fracPart = fracPart.slice(0, 9);
  fracPart = fracPart.padEnd(9, "0");

  try {
    const nano = BigInt(intPart) * NANO_PER_USD + BigInt(fracPart);
    return negative ? -nano : nano;
  } catch {
    return null;
  }
}

function nanoToUsdString(nano: bigint): string {
  const negative = nano < 0n;
  const abs = negative ? -nano : nano;
  const intPart = abs / NANO_PER_USD;
  const fracPart = abs % NANO_PER_USD;
  const fracStr = fracPart.toString().padStart(9, "0").replace(/0+$/, "");
  const result = fracStr ? `${intPart}.${fracStr}` : `${intPart}`;
  return negative ? `-${result}` : result;
}

const roleIcons = {
  super_admin: ShieldCheck,
  admin: Shield,
  user: UserIcon,
};

const roleVariants = {
  super_admin: "destructive" as const,
  admin: "default" as const,
  user: "secondary" as const,
};

export function UsersPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const { user: currentUser } = useAuth();
  const { data: users = [], isLoading } = useUsers();
  const { data: groups = [], isLoading: groupsLoading } = useDashboardGroups();
  const {
    data: billingPlans = [],
    isLoading: billingPlansLoading,
    error: billingPlansError,
    mutate: revalidateBillingPlans,
  } = useBillingPlans();
  const defaultGroupId = useMemo(
    () => groups.find((group) => group.is_default)?.id ?? "",
    [groups],
  );
  const todayTotals = useMemo(() => {
    let calls = 0;
    let cost = 0n;
    for (const user of users) {
      calls += user.today_calls ?? 0;
      const raw = user.today_cost_nano_usd ?? "0";
      if (isSignedIntegerString(raw)) {
        cost += BigInt(raw);
      }
    }
    return { calls, cost };
  }, [users]);
  const [createOpen, setCreateOpen] = useState(false);
  const [editUser, setEditUser] = useState<User | null>(null);
  const [formData, setFormData] = useState({
    username: "",
    password: "",
    role: "user",
    balanceUsd: "0",
    balanceUnlimited: false,
    email: "",
    groupId: "",
  });
  const [balanceMode, setBalanceMode] = useState<"set" | "add">("set");
  const [balanceAddAmount, setBalanceAddAmount] = useState("");
  const [saving, setSaving] = useState(false);
  const [deleteTargetId, setDeleteTargetId] = useState<string | null>(null);
  const [selectedPlanId, setSelectedPlanId] = useState("");
  const [durationValue, setDurationValue] = useState("30");
  const [durationUnit, setDurationUnit] = useState<DurationUnit>("days");
  const [subscriptionSaving, setSubscriptionSaving] = useState(false);
  const [subscriptionConfirmation, setSubscriptionConfirmation] = useState<
    "replace" | "revoke" | null
  >(null);
  const {
    data: editSubscription,
    isLoading: editSubscriptionLoading,
    error: editSubscriptionError,
    mutate: revalidateEditSubscription,
  } = useUserBillingPlanSubscription(editUser?.id ?? null);
  const selectedPlan = useMemo(
    () => billingPlans.find((plan) => plan.id === selectedPlanId) ?? null,
    [billingPlans, selectedPlanId],
  );
  const selectedDurationSeconds = useMemo(
    () => parseDurationSeconds(durationValue, durationUnit),
    [durationUnit, durationValue],
  );

  const handleCreate = async () => {
    if (!formData.username.trim() || !formData.password) return;
    setSaving(true);
    try {
      await createUserOptimistic(
        formData.username.trim(),
        formData.password,
        formData.role,
        formData.groupId || undefined,
        users,
      );
      setCreateOpen(false);
      setFormData({
        username: "",
        password: "",
        role: "user",
        balanceUsd: "0",
        balanceUnlimited: false,
        email: "",
        groupId: "",
      });
    } catch (error) {
      toast.error(
        error instanceof Error ? error.message : t("users.failedCreate"),
      );
    } finally {
      setSaving(false);
    }
  };

  const handleUpdate = async () => {
    if (!editUser) return;
    setSaving(true);
    try {
      const updates: {
        username?: string;
        password?: string;
        role?: User["role"];
        balance_usd?: string;
        balance_nano_usd?: string;
        balance_unlimited?: boolean;
        email?: string | null;
        group_id?: string;
      } = {};
      if (formData.username.trim() && formData.username !== editUser.username) {
        updates.username = formData.username.trim();
      }
      if (formData.password) {
        updates.password = formData.password;
      }
      if (formData.role !== editUser.role) {
        updates.role = formData.role as User["role"];
      }
      if (balanceMode === "add") {
        const addNano = parseUsdToNanoBigInt(balanceAddAmount);
        if (addNano !== null && addNano !== 0n) {
          const newNano = BigInt(editUser.balance_nano_usd) + addNano;
          updates.balance_nano_usd = newNano.toString();
        }
      } else if (formData.balanceUsd !== editUser.balance_usd) {
        updates.balance_usd = formData.balanceUsd.trim();
      }
      if (formData.balanceUnlimited !== editUser.balance_unlimited) {
        updates.balance_unlimited = formData.balanceUnlimited;
      }
      const trimmedEmail = formData.email.trim();
      const currentEmail = editUser.email ?? "";
      if (trimmedEmail !== currentEmail) {
        updates.email = trimmedEmail || null;
      }
      if (formData.groupId && formData.groupId !== editUser.group_id) {
        updates.group_id = formData.groupId;
      }
      await updateUserOptimistic(editUser.id, updates, users);
      setEditUser(null);
      setBalanceMode("set");
      setBalanceAddAmount("");
      setFormData({
        username: "",
        password: "",
        role: "user",
        balanceUsd: "0",
        balanceUnlimited: false,
        email: "",
        groupId: "",
      });
    } catch (error) {
      toast.error(
        error instanceof Error ? error.message : t("users.failedUpdate"),
      );
    } finally {
      setSaving(false);
    }
  };

  const handleToggleEnabled = async (user: User) => {
    try {
      await updateUserOptimistic(user.id, { enabled: !user.enabled }, users);
    } catch (error) {
      toast.error(
        error instanceof Error ? error.message : t("users.failedUpdate"),
      );
    }
  };

  const handleDelete = async (id: string) => {
    setDeleteTargetId(id);
  };

  const confirmDelete = async () => {
    if (!deleteTargetId) return;
    try {
      await deleteUserOptimistic(deleteTargetId, users);
    } catch (error) {
      toast.error(
        error instanceof Error ? error.message : t("users.failedDelete"),
      );
    } finally {
      setDeleteTargetId(null);
    }
  };

  const openEdit = (user: User) => {
    setEditUser(user);
    setBalanceMode("set");
    setBalanceAddAmount("");
    setSelectedPlanId("");
    setDurationValue("30");
    setDurationUnit("days");
    setSubscriptionConfirmation(null);
    setFormData({
      username: user.username,
      password: "",
      role: user.role,
      balanceUsd: user.balance_usd,
      balanceUnlimited: user.balance_unlimited,
      email: user.email ?? "",
      groupId: user.group_id,
    });
  };

  const subscriptionErrorMessage = (error: unknown) => {
    if (error instanceof DashboardApiError) {
      if (error.code === "plan_not_available")
        return t("users.planUnavailable");
      if (error.code === "forbidden") return t("users.planForbidden");
      if (error.code === "not_found") return t("users.planUserMissing");
    }
    return t("users.planUpdateFailed");
  };

  const assignSubscription = async () => {
    if (!editUser || !selectedPlan || selectedDurationSeconds === null) return;
    setSubscriptionSaving(true);
    try {
      await assignUserBillingPlanSubscriptionOptimistic(
        editUser.id,
        selectedPlan,
        selectedDurationSeconds,
        editSubscription,
        editUser.id === currentUser?.id,
      );
      toast.success(
        t(editSubscription ? "users.planReplaced" : "users.planAssigned"),
      );
      setSelectedPlanId("");
      setDurationValue("30");
      setDurationUnit("days");
      setSubscriptionConfirmation(null);
    } catch (error) {
      toast.error(subscriptionErrorMessage(error));
    } finally {
      setSubscriptionSaving(false);
    }
  };

  const revokeSubscription = async () => {
    if (!editUser) return;
    setSubscriptionSaving(true);
    try {
      await revokeUserBillingPlanSubscriptionOptimistic(
        editUser.id,
        editSubscription,
        editUser.id === currentUser?.id,
      );
      toast.success(t("users.planRemoved"));
      setSubscriptionConfirmation(null);
    } catch (error) {
      toast.error(subscriptionErrorMessage(error));
    } finally {
      setSubscriptionSaving(false);
    }
  };

  const formatDate = (date: string) => {
    return new Date(date).toLocaleDateString(undefined, {
      year: "numeric",
      month: "short",
      day: "numeric",
    });
  };

  const canEdit = (user: User) => {
    if (currentUser?.role === "super_admin") return true;
    if (user.role === "super_admin") return false;
    if (currentUser?.role === "admin") return true;
    return false;
  };

  const canDelete = (user: User) => {
    if (user.id === currentUser?.id) return false;
    if (user.role === "super_admin") return false;
    return canEdit(user);
  };

  if (isLoading) {
    return (
      <PageWrapper className="space-y-6">
        <TablePageSkeleton />
      </PageWrapper>
    );
  }

  return (
    <PageWrapper className="space-y-6">
      <motion.div
        initial={{ opacity: 0, y: -10 }}
        animate={{ opacity: 1, y: 0 }}
        transition={transitions.normal}
      >
        <PageHeader
          title={t("users.title")}
          description={t("users.description")}
          actions={
            <Dialog open={createOpen} onOpenChange={setCreateOpen}>
              <DialogTrigger asChild>
                <AnimatedButton>
                  <Button>
                    <Plus className="mr-2 h-4 w-4" />
                    {t("users.addUser")}
                  </Button>
                </AnimatedButton>
              </DialogTrigger>
              <DialogContent className="max-h-[calc(100dvh-2rem)] overflow-hidden p-0 sm:max-h-[calc(100dvh-3rem)]">
                <div className="flex min-h-0 flex-col p-6">
                  <DialogHeader className="shrink-0">
                    <DialogTitle>{t("users.createUser")}</DialogTitle>
                    <DialogDescription>
                      {t("users.addNewUser")}
                    </DialogDescription>
                  </DialogHeader>
                  <div
                    className="min-h-0 flex-1 overflow-y-auto pr-1"
                    style={{ WebkitOverflowScrolling: "touch" }}
                  >
                    <div className="space-y-4 py-4">
                      <div className="space-y-2">
                        <Label htmlFor="username">{t("auth.username")}</Label>
                        <Input
                          id="username"
                          value={formData.username}
                          onChange={(e) =>
                            setFormData({
                              ...formData,
                              username: e.target.value,
                            })
                          }
                          placeholder="johndoe"
                          minLength={3}
                          maxLength={22}
                          pattern="[a-zA-Z0-9_]+"
                        />
                      </div>
                      <div className="space-y-2">
                        <Label htmlFor="password">{t("auth.password")}</Label>
                        <Input
                          id="password"
                          type="password"
                          value={formData.password}
                          onChange={(e) =>
                            setFormData({
                              ...formData,
                              password: e.target.value,
                            })
                          }
                          placeholder="••••••••"
                        />
                      </div>
                      <div className="space-y-2">
                        <Label>{t("users.role")}</Label>
                        <DropdownMenu>
                          <DropdownMenuTrigger asChild>
                            <Button
                              variant="outline"
                              className="w-full justify-start"
                            >
                              {t(`roles.${formData.role}`)}
                            </Button>
                          </DropdownMenuTrigger>
                          <DropdownMenuContent className="w-full">
                            {currentUser?.role === "super_admin" && (
                              <DropdownMenuItem
                                onClick={() =>
                                  setFormData({ ...formData, role: "admin" })
                                }
                              >
                                {t("roles.admin")}
                              </DropdownMenuItem>
                            )}
                            <DropdownMenuItem
                              onClick={() =>
                                setFormData({ ...formData, role: "user" })
                              }
                            >
                              {t("roles.user")}
                            </DropdownMenuItem>
                          </DropdownMenuContent>
                        </DropdownMenu>
                      </div>
                      <div className="space-y-2">
                        <Label htmlFor="user-group">{t("users.group")}</Label>
                        <GroupSingleSelect
                          id="user-group"
                          value={formData.groupId || defaultGroupId}
                          groups={groups}
                          loading={groupsLoading}
                          onChange={(groupId) =>
                            setFormData({ ...formData, groupId })
                          }
                        />
                        <p className="text-xs text-muted-foreground">
                          {t("users.groupHelp")}
                        </p>
                      </div>
                    </div>
                  </div>
                  <DialogFooter className="shrink-0 pt-4">
                    <Button
                      variant="outline"
                      onClick={() => setCreateOpen(false)}
                    >
                      {t("common.cancel")}
                    </Button>
                    <Button
                      onClick={handleCreate}
                      disabled={
                        saving ||
                        !formData.username.trim() ||
                        !formData.password
                      }
                    >
                      {saving ? t("common.creating") : t("common.create")}
                    </Button>
                  </DialogFooter>
                </div>
              </DialogContent>
            </Dialog>
          }
        />
      </motion.div>

      <AlertDialog
        open={!!deleteTargetId}
        onOpenChange={(open) => {
          if (!open) setDeleteTargetId(null);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("users.confirmDelete")}</AlertDialogTitle>
            <AlertDialogDescription>
              {t("users.confirmDelete")}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("common.cancel")}</AlertDialogCancel>
            <AlertDialogAction
              className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
              onClick={confirmDelete}
            >
              {t("common.delete")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <Dialog
        open={!!editUser}
        onOpenChange={(open) => !open && setEditUser(null)}
      >
        <DialogContent className="max-h-[calc(100dvh-2rem)] overflow-hidden p-0 sm:max-h-[calc(100dvh-3rem)]">
          <div className="flex min-h-0 flex-col p-6">
            <DialogHeader className="shrink-0">
              <DialogTitle>{t("users.editUser")}</DialogTitle>
              <DialogDescription>{t("users.updateDetails")}</DialogDescription>
            </DialogHeader>
            <div
              className="mt-2 min-h-0 flex-1 overflow-y-auto pr-1"
              style={{ WebkitOverflowScrolling: "touch" }}
            >
              <div className="space-y-4 py-4">
                <div className="space-y-2">
                  <Label htmlFor="edit-username">{t("auth.username")}</Label>
                  <Input
                    id="edit-username"
                    value={formData.username}
                    onChange={(e) =>
                      setFormData({ ...formData, username: e.target.value })
                    }
                    minLength={3}
                    maxLength={22}
                    pattern="[a-zA-Z0-9_]+"
                  />
                </div>
                <div className="space-y-2">
                  <Label htmlFor="edit-password">
                    {t("users.newPassword")}
                  </Label>
                  <Input
                    id="edit-password"
                    type="password"
                    value={formData.password}
                    onChange={(e) =>
                      setFormData({ ...formData, password: e.target.value })
                    }
                    placeholder="••••••••"
                  />
                </div>
                <div className="space-y-2">
                  <Label htmlFor="edit-email">{t("userSettings.email")}</Label>
                  <div className="relative">
                    <Mail className="absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
                    <Input
                      id="edit-email"
                      type="email"
                      value={formData.email}
                      onChange={(e) =>
                        setFormData({ ...formData, email: e.target.value })
                      }
                      placeholder="user@example.com"
                      className="pl-9"
                    />
                  </div>
                  <p className="text-xs text-muted-foreground">
                    {t("userSettings.emailDescription")}
                  </p>
                </div>
                {currentUser?.role === "super_admin" &&
                  editUser?.role !== "super_admin" && (
                    <div className="space-y-2">
                      <Label>{t("users.role")}</Label>
                      <DropdownMenu>
                        <DropdownMenuTrigger asChild>
                          <Button
                            variant="outline"
                            className="w-full justify-start"
                          >
                            {t(`roles.${formData.role}`)}
                          </Button>
                        </DropdownMenuTrigger>
                        <DropdownMenuContent className="w-full">
                          <DropdownMenuItem
                            onClick={() =>
                              setFormData({ ...formData, role: "admin" })
                            }
                          >
                            {t("roles.admin")}
                          </DropdownMenuItem>
                          <DropdownMenuItem
                            onClick={() =>
                              setFormData({ ...formData, role: "user" })
                            }
                          >
                            {t("roles.user")}
                          </DropdownMenuItem>
                        </DropdownMenuContent>
                      </DropdownMenu>
                    </div>
                  )}
                <div className="space-y-2">
                  <Label htmlFor="edit-user-group">{t("users.group")}</Label>
                  <GroupSingleSelect
                    id="edit-user-group"
                    value={formData.groupId}
                    groups={groups}
                    loading={groupsLoading}
                    onChange={(groupId) =>
                      setFormData({ ...formData, groupId })
                    }
                  />
                  <p className="text-xs text-muted-foreground">
                    {t("users.groupHelp")}
                  </p>
                </div>
                {currentUser?.role && (
                  <div className="space-y-2">
                    <div className="flex items-center justify-between gap-2">
                      <Label>{t("users.balance")}</Label>
                      <Tabs
                        value={balanceMode}
                        onValueChange={(v) => {
                          setBalanceMode(v as "set" | "add");
                          setBalanceAddAmount("");
                        }}
                      >
                        <TabsList className="h-7">
                          <TabsTrigger
                            value="set"
                            className="px-2.5 py-0.5 text-xs"
                          >
                            {t("users.balanceSet")}
                          </TabsTrigger>
                          <TabsTrigger
                            value="add"
                            className="px-2.5 py-0.5 text-xs"
                          >
                            <PlusCircle className="mr-1 h-3 w-3" />
                            {t("users.balanceAdd")}
                          </TabsTrigger>
                        </TabsList>
                      </Tabs>
                    </div>
                    {balanceMode === "set" ? (
                      <Input
                        value={formData.balanceUsd}
                        onChange={(e) =>
                          setFormData({
                            ...formData,
                            balanceUsd: e.target.value,
                          })
                        }
                        placeholder="0"
                      />
                    ) : (
                      <>
                        <Input
                          value={balanceAddAmount}
                          onChange={(e) => setBalanceAddAmount(e.target.value)}
                          placeholder={t("users.balanceAddPlaceholder")}
                        />
                        <p className="text-xs text-muted-foreground">
                          {t("users.balanceCurrentHint", {
                            amount: editUser?.balance_usd ?? "0",
                          })}
                          {balanceAddAmount.trim() &&
                            parseUsdToNanoBigInt(balanceAddAmount) !== null &&
                            editUser && (
                              <>
                                {" → "}
                                <span className="font-medium text-foreground">
                                  $
                                  {nanoToUsdString(
                                    BigInt(editUser.balance_nano_usd) +
                                      (parseUsdToNanoBigInt(balanceAddAmount) ??
                                        0n),
                                  )}
                                </span>
                              </>
                            )}
                        </p>
                      </>
                    )}
                    <div className="flex items-center gap-2">
                      <Switch
                        checked={formData.balanceUnlimited}
                        onCheckedChange={(checked) =>
                          setFormData({
                            ...formData,
                            balanceUnlimited: checked,
                          })
                        }
                      />
                      <span className="text-sm text-muted-foreground">
                        {t("users.unlimited")}
                      </span>
                    </div>
                  </div>
                )}
                <FieldSet className="gap-4 rounded-lg border p-4">
                  <FieldLegend className="mb-0">
                    {t("users.subscriptionPlan")}
                  </FieldLegend>
                  <FieldDescription>
                    {t("users.subscriptionHelp")}
                  </FieldDescription>
                  {editSubscriptionLoading || billingPlansLoading ? (
                    <div
                      className="flex flex-col gap-3"
                      aria-label={t("common.loading")}
                    >
                      <Skeleton className="h-16 w-full" />
                      <div className="grid gap-3 sm:grid-cols-2">
                        <Skeleton className="h-9 w-full" />
                        <Skeleton className="h-9 w-full" />
                      </div>
                    </div>
                  ) : editSubscriptionError || billingPlansError ? (
                    <Alert variant="destructive">
                      <AlertCircle aria-hidden="true" />
                      <AlertTitle>{t("users.planLoadFailed")}</AlertTitle>
                      <AlertDescription className="flex flex-col items-start gap-3">
                        <p>{t("users.planLoadFailedHelp")}</p>
                        <Button
                          type="button"
                          size="sm"
                          variant="outline"
                          onClick={() => {
                            void Promise.all([
                              revalidateEditSubscription(),
                              revalidateBillingPlans(),
                            ]);
                          }}
                        >
                          {t("common.retry")}
                        </Button>
                      </AlertDescription>
                    </Alert>
                  ) : (
                    <>
                      <div className="flex min-w-0 items-start gap-3 rounded-md border p-3">
                        <CalendarClock
                          className="mt-0.5 size-5 shrink-0 text-muted-foreground"
                          aria-hidden="true"
                        />
                        <div className="min-w-0 flex-1">
                          <div className="flex flex-wrap items-center gap-2">
                            <span className="text-sm font-medium">
                              {editSubscription?.plan_name ??
                                t("users.noActivePlan")}
                            </span>
                            <Badge
                              variant={
                                editSubscription ? "default" : "secondary"
                              }
                            >
                              {editSubscription
                                ? t("users.planActive")
                                : t("users.noPlan")}
                            </Badge>
                          </div>
                          <p className="mt-1 text-sm text-muted-foreground">
                            {editSubscription
                              ? t("users.planExpires", {
                                  date: new Date(
                                    editSubscription.expires_at,
                                  ).toLocaleString(),
                                })
                              : t("users.noActivePlanHelp")}
                          </p>
                        </div>
                      </div>
                      <FieldGroup className="gap-3 sm:grid sm:grid-cols-2">
                        <Field>
                          <FieldLabel htmlFor="edit-subscription-plan">
                            {t("users.choosePlan")}
                          </FieldLabel>
                          <Select
                            value={selectedPlanId}
                            onValueChange={setSelectedPlanId}
                            disabled={
                              billingPlans.length === 0 || subscriptionSaving
                            }
                          >
                            <SelectTrigger id="edit-subscription-plan">
                              <SelectValue
                                placeholder={t("users.choosePlanPlaceholder")}
                              />
                            </SelectTrigger>
                            <SelectContent>
                              <SelectGroup>
                                {billingPlans.map((plan) => (
                                  <SelectItem key={plan.id} value={plan.id}>
                                    {plan.name}
                                    {plan.listed
                                      ? ""
                                      : ` · ${t("users.planUnlisted")}`}
                                  </SelectItem>
                                ))}
                              </SelectGroup>
                            </SelectContent>
                          </Select>
                        </Field>
                        <Field>
                          <FieldLabel htmlFor="edit-subscription-duration">
                            {t("users.chooseDuration")}
                          </FieldLabel>
                          <div className="grid grid-cols-[minmax(0,1fr)_7.5rem] gap-2">
                            <Input
                              id="edit-subscription-duration"
                              type="number"
                              inputMode="numeric"
                              min="1"
                              step="1"
                              value={durationValue}
                              onChange={(event) =>
                                setDurationValue(event.target.value)
                              }
                              disabled={subscriptionSaving}
                              aria-invalid={
                                durationValue.length > 0 &&
                                selectedDurationSeconds === null
                              }
                            />
                            <Select
                              value={durationUnit}
                              onValueChange={(value) =>
                                setDurationUnit(value as DurationUnit)
                              }
                              disabled={subscriptionSaving}
                            >
                              <SelectTrigger
                                aria-label={t("users.durationUnit")}
                              >
                                <SelectValue />
                              </SelectTrigger>
                              <SelectContent>
                                <SelectItem value="minutes">
                                  {t("users.durationMinutes")}
                                </SelectItem>
                                <SelectItem value="hours">
                                  {t("users.durationHours")}
                                </SelectItem>
                                <SelectItem value="days">
                                  {t("users.durationDays")}
                                </SelectItem>
                              </SelectContent>
                            </Select>
                          </div>
                          <FieldDescription>
                            {selectedDurationSeconds === null
                              ? t("users.durationInvalid")
                              : t("users.durationHelp")}
                          </FieldDescription>
                        </Field>
                      </FieldGroup>
                      {billingPlans.length === 0 && (
                        <FieldDescription>
                          {t("users.noPlansAvailable")}
                        </FieldDescription>
                      )}
                      <div className="flex flex-col-reverse gap-2 sm:flex-row sm:justify-end">
                        {editSubscription && (
                          <Button
                            type="button"
                            variant="outline"
                            className="text-destructive hover:text-destructive"
                            disabled={subscriptionSaving}
                            onClick={() =>
                              setSubscriptionConfirmation("revoke")
                            }
                          >
                            {t("users.removePlan")}
                          </Button>
                        )}
                        <Button
                          type="button"
                          variant="secondary"
                          disabled={
                            !selectedPlan ||
                            selectedDurationSeconds === null ||
                            subscriptionSaving
                          }
                          onClick={() => {
                            if (editSubscription)
                              setSubscriptionConfirmation("replace");
                            else void assignSubscription();
                          }}
                        >
                          {subscriptionSaving
                            ? t("common.saving")
                            : t(
                                editSubscription
                                  ? "users.replacePlan"
                                  : "users.assignPlan",
                              )}
                        </Button>
                      </div>
                    </>
                  )}
                </FieldSet>
              </div>
            </div>
            <DialogFooter className="shrink-0 pt-4">
              <Button variant="outline" onClick={() => setEditUser(null)}>
                {t("common.cancel")}
              </Button>
              <Button onClick={handleUpdate} disabled={saving}>
                {saving ? t("common.saving") : t("common.save")}
              </Button>
            </DialogFooter>
          </div>
        </DialogContent>
      </Dialog>

      <AlertDialog
        open={subscriptionConfirmation !== null}
        onOpenChange={(open) => !open && setSubscriptionConfirmation(null)}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {t(
                subscriptionConfirmation === "revoke"
                  ? "users.removePlanTitle"
                  : "users.replacePlanTitle",
              )}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {t(
                subscriptionConfirmation === "revoke"
                  ? "users.removePlanDescription"
                  : "users.replacePlanDescription",
                {
                  current: editSubscription?.plan_name,
                  replacement: selectedPlan?.name,
                },
              )}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={subscriptionSaving}>
              {t("common.cancel")}
            </AlertDialogCancel>
            <AlertDialogAction
              disabled={subscriptionSaving}
              className={
                subscriptionConfirmation === "revoke"
                  ? "bg-destructive text-destructive-foreground hover:bg-destructive/90"
                  : undefined
              }
              onClick={(event) => {
                event.preventDefault();
                if (subscriptionConfirmation === "revoke")
                  void revokeSubscription();
                else void assignSubscription();
              }}
            >
              {subscriptionSaving
                ? t("common.saving")
                : t(
                    subscriptionConfirmation === "revoke"
                      ? "users.removePlan"
                      : "users.replacePlan",
                  )}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <motion.div
        initial={{ opacity: 0, y: 20 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ delay: 0.1, ...transitions.normal }}
      >
        <DataTableShell
          toolbar={
            <div>
              <h2 className="text-base font-semibold">{t("users.allUsers")}</h2>
              <p className="text-sm text-muted-foreground">
                {t("users.usersTotal", { count: users.length })}
                {" · "}
                {t("users.todaySummary", {
                  spend: formatNanoUsd(todayTotals.cost, 2),
                  calls: todayTotals.calls.toLocaleString(),
                })}
              </p>
            </div>
          }
          isEmpty={users.length === 0}
          emptyState={
            <EmptyState
              icon={<UserIcon className="h-12 w-12" />}
              title={t("users.allUsers")}
              description={t("users.noUsers")}
            />
          }
        >
          <TableVirtuoso
            style={{
              height: "calc(100dvh - 280px)",
              minHeight: 400,
              overflowX: "auto",
            }}
            data={users}
            components={{
              Table: (props) => (
                <table
                  {...props}
                  className="w-full caption-bottom text-sm"
                  style={{ minWidth: "80rem" }}
                />
              ),
              TableHead: (props) => (
                <thead {...props} className="[&_tr]:border-b" />
              ),
              TableRow: (props) => (
                <tr
                  {...props}
                  className="border-b transition-colors hover:bg-muted/50"
                />
              ),
              TableBody: (props) => (
                <tbody {...props} className="[&_tr:last-child]:border-0" />
              ),
            }}
            fixedHeaderContent={() => (
              <tr className="border-b bg-background">
                <VirtualTableHeaderCell className="min-w-[14rem]">
                  {t("users.user")}
                </VirtualTableHeaderCell>
                <VirtualTableHeaderCell className="w-[8.5rem] whitespace-nowrap">
                  {t("users.role")}
                </VirtualTableHeaderCell>
                <VirtualTableHeaderCell>
                  {t("users.balance")}
                </VirtualTableHeaderCell>
                <VirtualTableHeaderCell>
                  {t("users.todaySpend")}
                </VirtualTableHeaderCell>
                <VirtualTableHeaderCell>
                  {t("users.todayCalls")}
                </VirtualTableHeaderCell>
                <VirtualTableHeaderCell>
                  {t("common.created")}
                </VirtualTableHeaderCell>
                <VirtualTableHeaderCell>
                  {t("users.lastLogin")}
                </VirtualTableHeaderCell>
                <VirtualTableHeaderCell>
                  {t("common.status")}
                </VirtualTableHeaderCell>
                <VirtualTableHeaderCell className="w-[100px]">
                  {t("common.actions")}
                </VirtualTableHeaderCell>
              </tr>
            )}
            itemContent={(_index, user) => {
              const RoleIcon = roleIcons[user.role];
              return (
                <>
                  <VirtualTableCell className="whitespace-nowrap">
                    <div className="flex min-w-0 items-center gap-2 overflow-hidden whitespace-nowrap">
                      <Avatar className="size-8 shrink-0">
                        {user.email && (
                          <AvatarImage
                            src={getGravatarUrl(user.email, 64) ?? undefined}
                            alt={user.username}
                          />
                        )}
                        <AvatarFallback>
                          {user.username[0].toUpperCase()}
                        </AvatarFallback>
                      </Avatar>
                      <div className="flex min-w-0 items-center gap-2 overflow-hidden whitespace-nowrap">
                        <span className="min-w-0 truncate font-medium">
                          {user.username}
                        </span>
                        {user.group_id && (
                          <GroupsBadge
                            groupIds={[user.group_id]}
                            className="shrink-0 whitespace-nowrap"
                          />
                        )}
                      </div>
                    </div>
                  </VirtualTableCell>
                  <VirtualTableCell className="w-[8.5rem] whitespace-nowrap">
                    <div className="flex h-8 max-w-full items-center overflow-x-auto overflow-y-hidden whitespace-nowrap">
                      <Badge
                        variant={roleVariants[user.role]}
                        className="h-7 min-w-max shrink-0 flex-nowrap gap-1 whitespace-nowrap"
                      >
                        <RoleIcon className="h-3 w-3 shrink-0" />
                        {t(`roles.${user.role}`)}
                      </Badge>
                    </div>
                  </VirtualTableCell>
                  <VirtualTableCell className="tabular-nums">
                    {user.balance_unlimited
                      ? t("users.unlimited")
                      : formatUsdDecimal(user.balance_usd, 2)}
                  </VirtualTableCell>
                  <VirtualTableCell className="tabular-nums">
                    {formatNanoUsd(user.today_cost_nano_usd, 2)}
                  </VirtualTableCell>
                  <VirtualTableCell className="tabular-nums">
                    {(user.today_calls ?? 0).toLocaleString()}
                  </VirtualTableCell>
                  <VirtualTableCell>
                    {formatDate(user.created_at)}
                  </VirtualTableCell>
                  <VirtualTableCell>
                    {user.last_login_at
                      ? formatDate(user.last_login_at)
                      : t("common.never")}
                  </VirtualTableCell>
                  <VirtualTableCell>
                    <div className="flex items-center gap-2">
                      <Switch
                        checked={user.enabled}
                        onCheckedChange={() => handleToggleEnabled(user)}
                        disabled={!canEdit(user)}
                      />
                      <span className="text-sm text-muted-foreground">
                        {user.enabled
                          ? t("common.enabled")
                          : t("common.disabled")}
                      </span>
                    </div>
                  </VirtualTableCell>
                  <VirtualTableCell>
                    <div className="flex items-center gap-1">
                      <Button
                        variant="ghost"
                        size="icon"
                        className="size-11 touch-manipulation sm:size-9"
                        title={t("users.viewLogs")}
                        aria-label={t("users.viewLogs")}
                        onClick={() =>
                          navigate(
                            `/dashboard/logs?username=${encodeURIComponent(user.username)}`,
                          )
                        }
                      >
                        <ScrollText className="h-4 w-4" />
                      </Button>
                      {canEdit(user) && (
                        <Button
                          variant="ghost"
                          size="icon"
                          className="size-11 touch-manipulation sm:size-9"
                          aria-label={t("common.edit")}
                          onClick={() => openEdit(user)}
                        >
                          <Pencil className="h-4 w-4" />
                        </Button>
                      )}
                      {canDelete(user) && (
                        <Button
                          variant="ghost"
                          size="icon"
                          aria-label={t("common.delete")}
                          onClick={() => handleDelete(user.id)}
                          className="size-11 touch-manipulation sm:size-9 text-destructive hover:text-destructive"
                        >
                          <Trash2 className="h-4 w-4" />
                        </Button>
                      )}
                    </div>
                  </VirtualTableCell>
                </>
              );
            }}
          />
        </DataTableShell>
      </motion.div>
    </PageWrapper>
  );
}
