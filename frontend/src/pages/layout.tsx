import { Navigate, Outlet, Link, useLocation, useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import {
  LayoutDashboard,
  Users,
  Key,
  Settings,
  Server,
  LogOut,
  Menu,
  Layers3,
  Sun,
  Moon,
  Monitor,
  Cog,
  MessageSquareCode,
  ScrollText,
  Database,
} from "lucide-react";
import { useAuth } from "@/hooks/use-auth";
import { useTheme } from "@/hooks/use-theme";
import { Button } from "@/components/ui/button";
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
  DropdownMenuLabel,
} from "@/components/ui/dropdown-menu";
import { Separator } from "@/components/ui/separator";
import { Sheet, SheetContent, SheetTrigger } from "@/components/ui/sheet";
import { useState } from "react";
import { motion } from "framer-motion";
import { getGravatarUrl } from "@/lib/utils";

const navTransition = {
  type: "spring",
  stiffness: 500,
  damping: 30,
} as const;

function NavLink({
  to,
  icon: Icon,
  label,
  onClick,
  layoutId = "nav-active",
  disableLayoutAnimation = false,
  exact = false,
}: {
  to: string;
  icon: React.ComponentType<{ className?: string }>;
  label: string;
  onClick?: () => void;
  layoutId?: string;
  disableLayoutAnimation?: boolean;
  exact?: boolean;
}) {
  const location = useLocation();
  const isActive = exact
    ? location.pathname === to
    : location.pathname === to || location.pathname.startsWith(to + "/");

  return (
    <Link
      to={to}
      onClick={onClick}
      className={`relative flex items-center gap-3 rounded-lg px-3 py-2 text-sm font-medium transition-colors duration-200 ${
        isActive
          ? "text-primary-foreground"
          : "text-muted-foreground hover:bg-secondary hover:text-foreground"
      }`}
    >
      {isActive && (
        disableLayoutAnimation ? (
          <div className="absolute inset-0 rounded-lg bg-primary" />
        ) : (
          <motion.div
            layoutId={layoutId}
            className="absolute inset-0 rounded-lg bg-primary"
            transition={navTransition}
          />
        )
      )}
      <span className="relative z-10 flex items-center gap-3">
        <Icon className="h-4 w-4" />
        {label}
      </span>
    </Link>
  );
}

function Sidebar({ onNavigate, layoutId = "nav-active", disableLayoutAnimation = false }: { onNavigate?: () => void; layoutId?: string; disableLayoutAnimation?: boolean }) {
  const { user, logout } = useAuth();
  const { t } = useTranslation();
  const navigate = useNavigate();
  const isAdmin = user?.role === "super_admin" || user?.role === "admin";

  const roleLabel = t(`roles.${user?.role || "user"}`);
  const navItems = [
    { to: "/dashboard", icon: LayoutDashboard, label: t("nav.dashboard"), exact: true },
    { to: "/dashboard/tokens", icon: Key, label: t("nav.apiKeys") },
    { to: "/dashboard/logs", icon: ScrollText, label: t("nav.logs") },
    { to: "/dashboard/playground", icon: MessageSquareCode, label: t("nav.playground") },
  ];

  const adminNavItems = [
    { to: "/dashboard/providers", icon: Server, label: t("nav.providers") },
    { to: "/dashboard/models", icon: Database, label: t("nav.models") },
    { to: "/dashboard/users", icon: Users, label: t("nav.users") },
    { to: "/dashboard/admin-settings", icon: Settings, label: t("nav.settings") },
  ];

  return (
    <motion.div
      initial={{ opacity: 0, x: -20 }}
      animate={{ opacity: 1, x: 0 }}
      transition={{ duration: 0.3, ease: [0.16, 1, 0.3, 1] }}
      className="flex h-full flex-col gap-4 p-4"
    >
      <Link to="/dashboard" className="flex items-center gap-3 px-3 py-2">
        <motion.div
          whileHover={{ scale: 1.05, rotate: 5 }}
          whileTap={{ scale: 0.95 }}
          transition={{ type: "spring", stiffness: 400, damping: 17 }}
          className="flex h-8 w-8 items-center justify-center rounded-xl bg-primary text-primary-foreground"
        >
          <Layers3 className="h-4 w-4" />
        </motion.div>
        <div className="leading-tight">
          <p className="text-sm font-semibold">Monoize</p>
          <p className="text-xs text-muted-foreground">{t("nav.dashboard")}</p>
        </div>
      </Link>

      <Separator />

      <nav className="flex flex-1 flex-col gap-1">
        {navItems.map((item, index) => (
          <motion.div
            key={item.to}
            initial={{ opacity: 0, x: -20 }}
            animate={{ opacity: 1, x: 0 }}
            transition={{ delay: index * 0.05, duration: 0.3, ease: [0.16, 1, 0.3, 1] }}
          >
            <NavLink {...item} onClick={onNavigate} layoutId={layoutId} disableLayoutAnimation={disableLayoutAnimation} />
          </motion.div>
        ))}

        {isAdmin && (
          <>
            <Separator className="my-2" />
            <motion.p
              initial={{ opacity: 0 }}
              animate={{ opacity: 1 }}
              transition={{ delay: 0.2 }}
              className="px-3 text-xs font-medium uppercase tracking-wider text-muted-foreground"
            >
              {t("nav.admin")}
            </motion.p>
            {adminNavItems.map((item, index) => (
              <motion.div
                key={item.to}
                initial={{ opacity: 0, x: -20 }}
                animate={{ opacity: 1, x: 0 }}
                transition={{ delay: 0.25 + index * 0.05, duration: 0.3, ease: [0.16, 1, 0.3, 1] }}
              >
                <NavLink {...item} onClick={onNavigate} layoutId={layoutId} disableLayoutAnimation={disableLayoutAnimation} />
              </motion.div>
            ))}
          </>
        )}
      </nav>

      {/* Account menu anchored to bottom of sidebar */}
      <div className="mt-auto">
        <Separator />
        <div className="pt-3">
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button variant="ghost" className="group w-full justify-start gap-3 px-3 py-2">
                <Avatar className="h-7 w-7">
                  {user?.email && (
                    <AvatarImage src={getGravatarUrl(user.email, 56) ?? undefined} alt={user?.username} />
                  )}
                  <AvatarFallback className="text-xs">
                    {user?.username?.[0]?.toUpperCase() || "U"}
                  </AvatarFallback>
                </Avatar>
                <div className="flex min-w-0 flex-1 flex-col items-start leading-tight">
                  <span className="truncate text-sm font-medium">{user?.username}</span>
                  <span className="text-xs text-muted-foreground group-hover:text-accent-foreground">{roleLabel}</span>
                </div>
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="start" className="w-64">
              <DropdownMenuItem
                onClick={() => {
                  onNavigate?.();
                  navigate("/settings");
                }}
              >
                <Cog className="mr-2 h-4 w-4" />
                {t("userSettings.title")}
              </DropdownMenuItem>
              <DropdownMenuSeparator />
              <DropdownMenuLabel className="font-normal p-0">
                <ThemeToggle />
              </DropdownMenuLabel>
              <DropdownMenuSeparator />
              <DropdownMenuItem
                onClick={() => {
                  onNavigate?.();
                  logout();
                }}
                className="text-destructive"
              >
                <LogOut className="mr-2 h-4 w-4" />
                {t("auth.signOut")}
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      </div>
    </motion.div>
  );
}

function ThemeToggle() {
  const { theme, setTheme } = useTheme();
  const { t } = useTranslation();

  const themes = [
    { value: "light", icon: Sun, label: t("theme.light") },
    { value: "dark", icon: Moon, label: t("theme.dark") },
    { value: "system", icon: Monitor, label: t("theme.system") },
  ] as const;

  return (
    <div className="flex items-center justify-between gap-2 px-2 py-1.5">
      <span className="text-sm text-muted-foreground">{t("theme.toggle")}</span>
      <div className="relative flex h-8 items-center rounded-full bg-muted p-1">
        {themes.map((item) => {
          const Icon = item.icon;
          const isActive = theme === item.value;
          return (
            <button
              key={item.value}
              onClick={(e) => {
                e.preventDefault();
                e.stopPropagation();
                setTheme(item.value);
              }}
              className={`relative z-10 flex h-6 w-8 items-center justify-center rounded-full transition-colors ${
                isActive ? "text-foreground" : "text-muted-foreground hover:text-foreground"
              }`}
              title={item.label}
            >
              {isActive && (
                <motion.div
                  layoutId="theme-toggle-indicator"
                  className="absolute inset-0 rounded-full bg-background shadow-sm"
                  transition={{ type: "spring", stiffness: 500, damping: 30 }}
                />
              )}
              <Icon className="relative z-10 h-3.5 w-3.5" />
            </button>
          );
        })}
      </div>
    </div>
  );
}

export function DashboardLayout() {
  const { user, loading } = useAuth();
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);

  if (loading) {
    return (
      <div className="flex min-h-screen items-center justify-center">
        <motion.div
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          className="text-muted-foreground"
        >
          {t("common.loading")}
        </motion.div>
      </div>
    );
  }

  if (!user) {
    return <Navigate to="/login" replace />;
  }

  return (
    <div className="flex h-screen overflow-hidden">
      {/* Mobile: floating menu button + sheet */}
      <Sheet open={open} onOpenChange={setOpen}>
        <SheetTrigger asChild>
          <Button
            variant="outline"
            size="icon"
            className="fixed left-4 top-4 z-50 shadow-md lg:hidden"
          >
            <Menu className="h-5 w-5" />
            <span className="sr-only">Toggle menu</span>
          </Button>
        </SheetTrigger>
        <SheetContent side="left" className="w-64 p-0">
          <Sidebar onNavigate={() => setOpen(false)} disableLayoutAnimation />
        </SheetContent>
      </Sheet>

      <motion.aside
        initial={{ opacity: 0, x: -20 }}
        animate={{ opacity: 1, x: 0 }}
        transition={{ duration: 0.4, ease: [0.16, 1, 0.3, 1] }}
        className="m-4 mr-0 hidden h-[calc(100vh-2rem)] w-64 shrink-0 rounded-xl border bg-card shadow-md lg:block"
      >
        <Sidebar />
      </motion.aside>
      <div className="min-h-0 min-w-0 flex flex-1 flex-col overflow-y-auto p-4 pt-16 lg:pt-4">
        <motion.main
          initial={{ opacity: 0, y: 10 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ duration: 0.3, delay: 0.1, ease: [0.16, 1, 0.3, 1] }}
          className="min-w-0 flex-1"
        >
          <Outlet />
        </motion.main>
      </div>
    </div>
  );
}
