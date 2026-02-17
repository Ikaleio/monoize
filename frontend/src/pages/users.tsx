import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Plus, Trash2, Pencil, Shield, ShieldCheck, User as UserIcon } from "lucide-react";
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
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
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
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>{t("users.user")}</TableHead>
                  <TableHead>{t("users.role")}</TableHead>
                  <TableHead>{t("common.created")}</TableHead>
                  <TableHead>{t("users.lastLogin")}</TableHead>
                  <TableHead>Balance</TableHead>
                  <TableHead>{t("common.status")}</TableHead>
                  <TableHead className="w-[100px]">{t("common.actions")}</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {users.map((user, index) => {
                  const RoleIcon = roleIcons[user.role];
                  return (
                    <motion.tr
                      key={user.id}
                      initial={{ opacity: 0, x: -20 }}
                      animate={{ opacity: 1, x: 0 }}
                      transition={{ delay: index * 0.05, ...transitions.normal }}
                      className="border-b transition-colors hover:bg-muted/50"
                    >
                      <TableCell>
                        <div className="flex items-center gap-2">
                          <motion.div
                            whileHover={{ scale: 1.1 }}
                            className="flex h-8 w-8 items-center justify-center rounded-full bg-secondary"
                          >
                            {user.username[0].toUpperCase()}
                          </motion.div>
                          <span className="font-medium">{user.username}</span>
                        </div>
                      </TableCell>
                      <TableCell>
                        <Badge variant={roleVariants[user.role]} className="gap-1">
                          <RoleIcon className="h-3 w-3" />
                          {t(`roles.${user.role}`)}
                        </Badge>
                      </TableCell>
                      <TableCell>{formatDate(user.created_at)}</TableCell>
                      <TableCell>{user.last_login_at ? formatDate(user.last_login_at) : t("common.never")}</TableCell>
                      <TableCell>
                        {user.balance_unlimited ? "Unlimited" : `$${user.balance_usd}`}
                      </TableCell>
                      <TableCell>
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
                      </TableCell>
                      <TableCell>
                        <div className="flex items-center gap-1">
                          {canEdit(user) && (
                            <motion.div whileHover={{ scale: 1.1 }} whileTap={{ scale: 0.9 }}>
                              <Button variant="ghost" size="icon" onClick={() => openEdit(user)}>
                                <Pencil className="h-4 w-4" />
                              </Button>
                            </motion.div>
                          )}
                          {canDelete(user) && (
                            <motion.div whileHover={{ scale: 1.1 }} whileTap={{ scale: 0.9 }}>
                              <Button
                                variant="ghost"
                                size="icon"
                                onClick={() => handleDelete(user.id)}
                                className="text-destructive hover:text-destructive"
                              >
                                <Trash2 className="h-4 w-4" />
                              </Button>
                            </motion.div>
                          )}
                        </div>
                      </TableCell>
                    </motion.tr>
                  );
                })}
              </TableBody>
            </Table>
          </CardContent>
        </Card>
      </motion.div>
    </PageWrapper>
  );
}
