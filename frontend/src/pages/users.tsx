import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Plus, Trash2, Pencil, Shield, ShieldCheck, User as UserIcon, Mail, X } from "lucide-react";
import { GroupsBadge } from "@/components/GroupsBadge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Badge } from "@/components/ui/badge";
import { Skeleton } from "@/components/ui/skeleton";
import { Switch } from "@/components/ui/switch";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
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
} from "@/lib/swr";
import type { User } from "@/lib/api";
import { Avatar, AvatarImage, AvatarFallback } from "@/components/ui/avatar";
import { getGravatarUrl } from "@/lib/utils";
import { PageWrapper, motion, transitions } from "@/components/ui/motion";

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

type Translator = (key: string) => string;

function logOptimisticFlowRejection(flow: string, error: unknown) {
  console.debug(`[UsersPage] ${flow} rejected after optimistic helper handling`, error);
}

function groupKey(value: string): string {
  return value.trim().toLowerCase();
}

function dedupeAllowedGroups(values: string[]): string[] {
  const seen = new Set<string>();
  const next: string[] = [];

  for (const value of values) {
    const trimmed = value.trim();
    const key = groupKey(trimmed);
    if (!key || seen.has(key)) {
      continue;
    }
    seen.add(key);
    next.push(trimmed);
  }

  return next;
}

function allowedGroupsEqual(left: string[], right: string[]): boolean {
  const nextLeft = dedupeAllowedGroups(left);
  const nextRight = dedupeAllowedGroups(right);

  return (
    nextLeft.length === nextRight.length &&
    nextLeft.every((value, index) => groupKey(value) === groupKey(nextRight[index]))
  );
}

interface AllowedGroupsInputProps {
  inputId: string;
  value: string[];
  suggestions: string[];
  suggestionsLoading: boolean;
  t: Translator;
  onChange: (next: string[]) => void;
}

function AllowedGroupsInput({
  inputId,
  value,
  suggestions,
  suggestionsLoading,
  t,
  onChange,
}: AllowedGroupsInputProps) {
  const [draft, setDraft] = useState("");
  const groups = useMemo(() => dedupeAllowedGroups(value), [value]);
  const draftKey = groupKey(draft);
  const filteredSuggestions = useMemo(
    () =>
      suggestions.filter((suggestion) => {
        const suggestionKey = groupKey(suggestion);
        if (!suggestionKey) {
          return false;
        }
        if (groups.some((group) => groupKey(group) === suggestionKey)) {
          return false;
        }
        return !draftKey || suggestionKey.includes(draftKey);
      }),
    [draftKey, groups, suggestions]
  );

  const commitGroups = (nextValues: string[]) => {
    onChange(dedupeAllowedGroups(nextValues));
  };

  const flushDraft = () => {
    const parts = draft
      .split(",")
      .map((part) => part.trim())
      .filter(Boolean);
    if (parts.length > 0) {
      commitGroups([...groups, ...parts]);
    }
    setDraft("");
  };

  const removeGroup = (group: string) => {
    commitGroups(groups.filter((entry) => groupKey(entry) !== groupKey(group)));
  };

  const addSuggestion = (group: string) => {
    commitGroups([...groups, group]);
    setDraft("");
  };

  return (
    <div className="space-y-2">
      <div className="flex items-center justify-between gap-2">
        <Label htmlFor={inputId}>{t("users.allowedGroups")}</Label>
        <span className="text-xs text-muted-foreground">{t("providers.optional")}</span>
      </div>
      <Input
        id={inputId}
        value={draft}
        placeholder={t("providers.groupsPlaceholder")}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={flushDraft}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === ",") {
            e.preventDefault();
            flushDraft();
          }
        }}
      />
      <p className="text-xs text-muted-foreground">
        {groups.length === 0
          ? t("users.allowedGroupsEmptyHelp")
          : t("users.allowedGroupsSelectedHelp")}
      </p>
      {groups.length > 0 && (
        <div className="flex flex-wrap gap-2">
          {groups.map((group) => (
            <Badge
              key={groupKey(group)}
              variant="secondary"
              className="flex items-center gap-1 font-mono"
            >
              <span>{group}</span>
              <Button
                type="button"
                variant="ghost"
                size="icon"
                className="h-4 w-4"
                onClick={() => removeGroup(group)}
              >
                <X className="h-3 w-3" />
              </Button>
            </Badge>
          ))}
        </div>
      )}
      {suggestionsLoading ? (
        <div className="flex flex-wrap gap-2">
          <Skeleton className="h-7 w-20 rounded-full" />
          <Skeleton className="h-7 w-24 rounded-full" />
          <Skeleton className="h-7 w-16 rounded-full" />
        </div>
      ) : filteredSuggestions.length > 0 ? (
        <div className="flex flex-wrap gap-2">
          {filteredSuggestions.slice(0, 8).map((group) => (
            <Button
              key={group}
              type="button"
              variant="outline"
              size="sm"
              className="h-7 rounded-full px-3 font-mono text-xs"
              onClick={() => addSuggestion(group)}
            >
              {group}
            </Button>
          ))}
        </div>
      ) : null}
    </div>
  );
}

export function UsersPage() {
  const { t } = useTranslation();
  const { user: currentUser } = useAuth();
  const { data: users = [], isLoading } = useUsers();
  const { data: groupSuggestions = [], isLoading: groupsLoading } = useDashboardGroups();
  const [createOpen, setCreateOpen] = useState(false);
  const [editUser, setEditUser] = useState<User | null>(null);
  const [formData, setFormData] = useState({
    username: "",
    password: "",
    role: "user",
    balanceUsd: "0",
    balanceUnlimited: false,
    email: "",
    allowedGroups: [] as string[],
  });
  const [saving, setSaving] = useState(false);

  const handleCreate = async () => {
    if (!formData.username.trim() || !formData.password) return;
    setSaving(true);
    try {
      const allowedGroups = dedupeAllowedGroups(formData.allowedGroups);
      await createUserOptimistic(
        formData.username.trim(),
        formData.password,
        formData.role,
        allowedGroups,
        users,
        (error) => console.error(t("users.failedCreate"), error)
      );
      setCreateOpen(false);
      setFormData({
        username: "",
        password: "",
        role: "user",
        balanceUsd: "0",
        balanceUnlimited: false,
        email: "",
        allowedGroups: [],
      });
    } catch (error) {
      logOptimisticFlowRejection("create user", error);
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
        balance_unlimited?: boolean;
        email?: string | null;
        allowed_groups?: string[];
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
      if (formData.balanceUsd !== editUser.balance_usd) {
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
      const nextAllowedGroups = dedupeAllowedGroups(formData.allowedGroups);
      if (!allowedGroupsEqual(nextAllowedGroups, editUser.allowed_groups)) {
        updates.allowed_groups = nextAllowedGroups;
      }
      await updateUserOptimistic(
        editUser.id,
        updates,
        users,
        (error) => console.error(t("users.failedUpdate"), error)
      );
      setEditUser(null);
      setFormData({
        username: "",
        password: "",
        role: "user",
        balanceUsd: "0",
        balanceUnlimited: false,
        email: "",
        allowedGroups: [],
      });
    } catch (error) {
      logOptimisticFlowRejection("update user", error);
    } finally {
      setSaving(false);
    }
  };

  const handleToggleEnabled = async (user: User) => {
    try {
      await updateUserOptimistic(
        user.id,
        { enabled: !user.enabled },
        users,
        (error) => console.error(t("users.failedUpdate"), error)
      );
    } catch (error) {
      logOptimisticFlowRejection("toggle user enabled", error);
    }
  };

  const handleDelete = async (id: string) => {
    if (!confirm(t("users.confirmDelete"))) return;
    try {
      await deleteUserOptimistic(
        id,
        users,
        (error) => console.error(t("users.failedDelete"), error)
      );
    } catch (error) {
      logOptimisticFlowRejection("delete user", error);
    }
  };

  const openEdit = (user: User) => {
    setEditUser(user);
    setFormData({
      username: user.username,
      password: "",
      role: user.role,
      balanceUsd: user.balance_usd,
      balanceUnlimited: user.balance_unlimited,
      email: user.email ?? "",
      allowedGroups: user.allowed_groups,
    });
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
      <div className="space-y-6">
        <Skeleton className="h-8 w-48" />
        <Skeleton className="h-64" />
      </div>
    );
  }

  return (
    <PageWrapper className="space-y-6">
      <motion.div
        initial={{ opacity: 0, y: -10 }}
        animate={{ opacity: 1, y: 0 }}
        transition={transitions.normal}
        className="flex items-center justify-between"
      >
        <div>
          <h1 className="text-3xl font-bold tracking-tight">{t("users.title")}</h1>
          <p className="text-muted-foreground">{t("users.description")}</p>
        </div>
        <Dialog open={createOpen} onOpenChange={setCreateOpen}>
          <DialogTrigger asChild>
            <motion.div whileHover={{ scale: 1.02 }} whileTap={{ scale: 0.98 }}>
              <Button>
                <Plus className="mr-2 h-4 w-4" />
                {t("users.addUser")}
              </Button>
            </motion.div>
          </DialogTrigger>
          <DialogContent className="max-h-[calc(100dvh-2rem)] overflow-hidden p-0 sm:max-h-[calc(100dvh-3rem)]">
            <div className="flex min-h-0 flex-col p-6">
              <DialogHeader className="shrink-0">
                <DialogTitle>{t("users.createUser")}</DialogTitle>
                <DialogDescription>{t("users.addNewUser")}</DialogDescription>
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
                      onChange={(e) => setFormData({ ...formData, username: e.target.value })}
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
                      onChange={(e) => setFormData({ ...formData, password: e.target.value })}
                      placeholder="••••••••"
                    />
                  </div>
                  <div className="space-y-2">
                    <Label>{t("users.role")}</Label>
                    <DropdownMenu>
                      <DropdownMenuTrigger asChild>
                        <Button variant="outline" className="w-full justify-start">
                          {t(`roles.${formData.role}`)}
                        </Button>
                      </DropdownMenuTrigger>
                      <DropdownMenuContent className="w-full">
                        {currentUser?.role === "super_admin" && (
                          <DropdownMenuItem onClick={() => setFormData({ ...formData, role: "admin" })}>
                            {t("roles.admin")}
                          </DropdownMenuItem>
                        )}
                        <DropdownMenuItem onClick={() => setFormData({ ...formData, role: "user" })}>
                          {t("roles.user")}
                        </DropdownMenuItem>
                      </DropdownMenuContent>
                    </DropdownMenu>
                  </div>
                  <AllowedGroupsInput
                    inputId="allowed-groups"
                    value={formData.allowedGroups}
                    suggestions={groupSuggestions}
                    suggestionsLoading={groupsLoading}
                    t={t}
                    onChange={(allowedGroups) => setFormData({ ...formData, allowedGroups })}
                  />
                </div>
              </div>
              <DialogFooter className="shrink-0 pt-4">
                <Button variant="outline" onClick={() => setCreateOpen(false)}>
                  {t("common.cancel")}
                </Button>
                <Button onClick={handleCreate} disabled={saving || !formData.username.trim() || !formData.password}>
                  {saving ? t("common.creating") : t("common.create")}
                </Button>
              </DialogFooter>
            </div>
          </DialogContent>
        </Dialog>
      </motion.div>

      <Dialog open={!!editUser} onOpenChange={(open) => !open && setEditUser(null)}>
        <DialogContent className="max-h-[calc(100dvh-2rem)] overflow-hidden p-0 sm:max-h-[calc(100dvh-3rem)]">
          <div className="flex min-h-0 flex-col p-6">
            <DialogHeader className="shrink-0">
              <DialogTitle>{t("users.editUser")}</DialogTitle>
              <DialogDescription>{t("users.updateDetails")}</DialogDescription>
            </DialogHeader>
            <div
              className="min-h-0 flex-1 overflow-y-auto pr-1"
              style={{ WebkitOverflowScrolling: "touch" }}
            >
              <div className="space-y-4 py-4">
                <div className="space-y-2">
                  <Label htmlFor="edit-username">{t("auth.username")}</Label>
                  <Input
                    id="edit-username"
                    value={formData.username}
                    onChange={(e) => setFormData({ ...formData, username: e.target.value })}
                    minLength={3}
                    maxLength={22}
                    pattern="[a-zA-Z0-9_]+"
                  />
                </div>
                <div className="space-y-2">
                  <Label htmlFor="edit-password">{t("users.newPassword")}</Label>
                  <Input
                    id="edit-password"
                    type="password"
                    value={formData.password}
                    onChange={(e) => setFormData({ ...formData, password: e.target.value })}
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
                      onChange={(e) => setFormData({ ...formData, email: e.target.value })}
                      placeholder="user@example.com"
                      className="pl-9"
                    />
                  </div>
                  <p className="text-xs text-muted-foreground">{t("userSettings.emailDescription")}</p>
                </div>
                {currentUser?.role === "super_admin" && editUser?.role !== "super_admin" && (
                  <div className="space-y-2">
                    <Label>{t("users.role")}</Label>
                    <DropdownMenu>
                      <DropdownMenuTrigger asChild>
                        <Button variant="outline" className="w-full justify-start">
                          {t(`roles.${formData.role}`)}
                        </Button>
                      </DropdownMenuTrigger>
                      <DropdownMenuContent className="w-full">
                        <DropdownMenuItem onClick={() => setFormData({ ...formData, role: "admin" })}>
                          {t("roles.admin")}
                        </DropdownMenuItem>
                        <DropdownMenuItem onClick={() => setFormData({ ...formData, role: "user" })}>
                          {t("roles.user")}
                        </DropdownMenuItem>
                      </DropdownMenuContent>
                    </DropdownMenu>
                  </div>
                )}
                <AllowedGroupsInput
                  inputId="edit-allowed-groups"
                  value={formData.allowedGroups}
                  suggestions={groupSuggestions}
                  suggestionsLoading={groupsLoading}
                  t={t}
                  onChange={(allowedGroups) => setFormData({ ...formData, allowedGroups })}
                />
                {currentUser?.role && (
                  <div className="space-y-2">
                    <Label>Balance (USD)</Label>
                    <Input
                      value={formData.balanceUsd}
                      onChange={(e) => setFormData({ ...formData, balanceUsd: e.target.value })}
                      placeholder="0"
                    />
                    <div className="flex items-center gap-2">
                      <Switch
                        checked={formData.balanceUnlimited}
                        onCheckedChange={(checked) =>
                          setFormData({ ...formData, balanceUnlimited: checked })
                        }
                      />
                      <span className="text-sm text-muted-foreground">Unlimited</span>
                    </div>
                  </div>
                )}
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

      <motion.div
        initial={{ opacity: 0, y: 20 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ delay: 0.1, ...transitions.normal }}
      >
        <Card>
          <CardHeader>
            <CardTitle>{t("users.allUsers")}</CardTitle>
            <CardDescription>
              {t("users.usersTotal", { count: users.length })}
            </CardDescription>
          </CardHeader>
          <CardContent>
            <TableVirtuoso
              style={{ height: "calc(100dvh - 280px)", minHeight: 400 }}
              data={users}
              components={{
                Table: (props) => (
                  <table
                    {...props}
                    className="w-full caption-bottom text-sm"
                    style={{ minWidth: "56rem" }}
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
                  <th className="h-10 px-4 text-left align-middle font-medium text-muted-foreground">
                    {t("users.user")}
                  </th>
                  <th className="h-10 px-4 text-left align-middle font-medium text-muted-foreground w-[8.5rem] whitespace-nowrap">
                    {t("users.role")}
                  </th>
                  <th className="h-10 px-4 text-left align-middle font-medium text-muted-foreground">
                    {t("common.created")}
                  </th>
                  <th className="h-10 px-4 text-left align-middle font-medium text-muted-foreground">
                    {t("users.lastLogin")}
                  </th>
                  <th className="h-10 px-4 text-left align-middle font-medium text-muted-foreground">
                    Balance
                  </th>
                  <th className="h-10 px-4 text-left align-middle font-medium text-muted-foreground">
                    {t("common.status")}
                  </th>
                  <th className="h-10 px-4 text-left align-middle font-medium text-muted-foreground w-[100px]">
                    {t("common.actions")}
                  </th>
                </tr>
              )}
              itemContent={(_index, user) => {
                const RoleIcon = roleIcons[user.role];
                return (
                  <>
                    <td className="p-4 align-middle">
                      <div className="flex items-center gap-2">
                        <Avatar className="h-8 w-8">
                          {user.email && <AvatarImage src={getGravatarUrl(user.email, 64) ?? undefined} alt={user.username} />}
                          <AvatarFallback>{user.username[0].toUpperCase()}</AvatarFallback>
                        </Avatar>
                        <div className="flex flex-col gap-1">
                          <span className="font-medium">{user.username}</span>
                          {user.allowed_groups.length > 0 && (
                            <GroupsBadge groups={user.allowed_groups} />
                          )}
                        </div>
                      </div>
                    </td>
                    <td className="p-4 align-middle">
                      <div className="max-h-8 overflow-x-auto overflow-y-hidden">
                        <Badge
                          variant={roleVariants[user.role]}
                          className="inline-flex h-7 min-w-max flex-nowrap items-center gap-1 whitespace-nowrap"
                        >
                          <RoleIcon className="h-3 w-3 shrink-0" />
                          {t(`roles.${user.role}`)}
                        </Badge>
                      </div>
                    </td>
                    <td className="p-4 align-middle">{formatDate(user.created_at)}</td>
                    <td className="p-4 align-middle">
                      {user.last_login_at ? formatDate(user.last_login_at) : t("common.never")}
                    </td>
                    <td className="p-4 align-middle">
                      {user.balance_unlimited ? "Unlimited" : `$${user.balance_usd}`}
                    </td>
                    <td className="p-4 align-middle">
                      <div className="flex items-center gap-2">
                        <Switch
                          checked={user.enabled}
                          onCheckedChange={() => handleToggleEnabled(user)}
                          disabled={!canEdit(user)}
                        />
                        <span className="text-sm text-muted-foreground">
                          {user.enabled ? t("common.enabled") : t("common.disabled")}
                        </span>
                      </div>
                    </td>
                    <td className="p-4 align-middle">
                      <div className="flex items-center gap-1">
                        {canEdit(user) && (
                          <Button variant="ghost" size="icon" onClick={() => openEdit(user)}>
                            <Pencil className="h-4 w-4" />
                          </Button>
                        )}
                        {canDelete(user) && (
                          <Button
                            variant="ghost"
                            size="icon"
                            onClick={() => handleDelete(user.id)}
                            className="text-destructive hover:text-destructive"
                          >
                            <Trash2 className="h-4 w-4" />
                          </Button>
                        )}
                      </div>
                    </td>
                  </>
                );
              }}
            />
          </CardContent>
        </Card>
      </motion.div>
    </PageWrapper>
  );
}
