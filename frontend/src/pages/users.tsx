import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Plus, Trash2, Pencil, Shield, ShieldCheck, User as UserIcon, Mail } from "lucide-react";
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

export function UsersPage() {
  const { t } = useTranslation();
  const { user: currentUser } = useAuth();
  const { data: users = [], isLoading } = useUsers();
  const [createOpen, setCreateOpen] = useState(false);
  const [editUser, setEditUser] = useState<User | null>(null);
  const [formData, setFormData] = useState({
    username: "",
    password: "",
    role: "user",
    balanceUsd: "0",
    balanceUnlimited: false,
    email: "",
  });
  const [saving, setSaving] = useState(false);

  const handleCreate = async () => {
    if (!formData.username.trim() || !formData.password) return;
    setSaving(true);
    try {
      await createUserOptimistic(
        formData.username.trim(),
        formData.password,
        formData.role,
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
      });
    } catch {
      // Error handled by optimistic update
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
      });
    } catch {
      // Error handled by optimistic update
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
    } catch {
      // Error handled by optimistic update
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
    } catch {
      // Error handled by optimistic update
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
          <DialogContent>
            <DialogHeader>
              <DialogTitle>{t("users.createUser")}</DialogTitle>
              <DialogDescription>{t("users.addNewUser")}</DialogDescription>
            </DialogHeader>
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
            </div>
            <DialogFooter>
              <Button variant="outline" onClick={() => setCreateOpen(false)}>
                {t("common.cancel")}
              </Button>
              <Button onClick={handleCreate} disabled={saving || !formData.username.trim() || !formData.password}>
                {saving ? t("common.creating") : t("common.create")}
              </Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>
      </motion.div>

      <Dialog open={!!editUser} onOpenChange={(open) => !open && setEditUser(null)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t("users.editUser")}</DialogTitle>
            <DialogDescription>{t("users.updateDetails")}</DialogDescription>
          </DialogHeader>
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
          <DialogFooter>
            <Button variant="outline" onClick={() => setEditUser(null)}>
              {t("common.cancel")}
            </Button>
            <Button onClick={handleUpdate} disabled={saving}>
              {saving ? t("common.saving") : t("common.save")}
            </Button>
          </DialogFooter>
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
              style={{ height: "calc(100vh - 280px)", minHeight: 400 }}
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
                        <span className="font-medium">{user.username}</span>
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
