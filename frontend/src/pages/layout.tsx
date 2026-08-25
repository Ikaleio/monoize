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
  Sun,
  Moon,
  Monitor,
  Cog,
  MessageSquareCode,
  ScrollText,
  Database,
  Store,
  CalendarClock,
  Gauge,
  Boxes,
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
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { useState } from "react";
import { motion } from "framer-motion";
import { formatUsdDecimal } from "@/lib/exact-decimal";
import { cn, getGravatarUrl } from "@/lib/utils";
import { MonoizeLogo } from "@/components/MonoizeLogo";
import { springs } from "@/components/ui/motion";

const navTransition = springs.snappy;

function NavLink({
  to,
  icon: Icon,
  label,
  onClick,
  layoutId = "nav-active",
  disableLayoutAnimation = false,
  collapsed = false,
  exact = false,
}: {
  to: string;
  icon: React.ComponentType<{ className?: string }>;
  label: string;
  onClick?: () => void;
  layoutId?: string;
  disableLayoutAnimation?: boolean;
  collapsed?: boolean;
  exact?: boolean;
}) {
  const location = useLocation();
  const isActive = exact
    ? location.pathname === to
    : location.pathname === to || location.pathname.startsWith(to + "/");

  const link = (
    <Link
      to={to}
      onClick={onClick}
      className={cn(
        "relative flex items-center rounded-md text-sm font-medium transition-colors duration-150",
        collapsed ? "justify-center px-2 py-2" : "gap-3 px-2.5 py-1.5",
        isActive
          ? "text-foreground"
          : "text-muted-foreground hover:bg-accent/60 hover:text-foreground"
      )}
    >
      {isActive && (
        disableLayoutAnimation ? (
          <div className="absolute inset-0 rounded-md bg-accent" />
        ) : (
          <motion.div
            layoutId={layoutId}
            className="absolute inset-0 rounded-md bg-accent"
            transition={navTransition}
          />
        )
      )}
      <span className={cn("relative z-10 flex items-center", collapsed ? "" : "gap-3")}>
        <Icon className={cn("h-4 w-4 shrink-0", isActive && "text-primary")} />
        {!collapsed && label}
      </span>
    </Link>
  );

  if (collapsed) {
    return (
      <Tooltip>
        <TooltipTrigger asChild>{link}</TooltipTrigger>
        <TooltipContent side="right" sideOffset={8}>
          {label}
        </TooltipContent>
      </Tooltip>
    );
  }

  return link;
}

function Sidebar({
  onNavigate,
  layoutId = "nav-active",
  disableLayoutAnimation = false,
  collapsed = false,
}: {
  onNavigate?: () => void;
  layoutId?: string;
  disableLayoutAnimation?: boolean;
  collapsed?: boolean;
}) {
  const { user, logout } = useAuth();
  const { t } = useTranslation();
  const navigate = useNavigate();
  const isAdmin = user?.role === "super_admin" || user?.role === "admin";

  const roleLabel = t(`roles.${user?.role || "user"}`);
  const balanceLabel = user?.balance_unlimited
    ? t("users.unlimited")
    : formatUsdDecimal(user?.balance_usd, 2);
  const accountSummary = user?.billing_plan?.name
    ? `${user.billing_plan.name} · ${balanceLabel}`
    : balanceLabel;
  const navItems = [
    { to: "/dashboard", icon: LayoutDashboard, label: t("nav.dashboard"), exact: true },
    { to: "/dashboard/tokens", icon: Key, label: t("nav.apiKeys") },
    { to: "/dashboard/logs", icon: ScrollText, label: t("nav.logs") },
    { to: "/dashboard/playground", icon: MessageSquareCode, label: t("nav.playground") },
    { to: "/dashboard/marketplace", icon: Store, label: t("nav.marketplace") },
  ];

  const adminNavItems = [
    { to: "/dashboard/admin", icon: Gauge, label: t("nav.adminDashboard") },
    { to: "/dashboard/providers", icon: Server, label: t("nav.providers") },
    { to: "/dashboard/models", icon: Database, label: t("nav.models") },
    { to: "/dashboard/plans", icon: CalendarClock, label: t("nav.billingPlans") },
    { to: "/dashboard/users", icon: Users, label: t("nav.users") },
    { to: "/dashboard/groups", icon: Boxes, label: t("nav.groups") },
    { to: "/dashboard/admin-settings", icon: Settings, label: t("nav.settings") },
  ];

  return (
    <TooltipProvider delayDuration={0}>
      <motion.div
        initial={{ opacity: 0 }}
        animate={{ opacity: 1 }}
        transition={{ duration: 0.2 }}
        className={cn("flex h-full flex-col p-3", collapsed ? "items-center" : "")}
      >
        <Link
          to="/dashboard"
          className={cn(
            "group flex items-center rounded-lg transition-colors hover:bg-accent/50",
            collapsed ? "justify-center p-2" : "gap-3 px-2.5 py-2.5"
          )}
        >
          <motion.div
            whileHover={{ scale: 1.05 }}
            whileTap={{ scale: 0.95 }}
            transition={springs.snappy}
            className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-foreground p-1.5 text-background shadow-sm"
          >
            <MonoizeLogo className="h-full w-full" />
          </motion.div>
          {!collapsed && (
            <div className="flex flex-col leading-none">
              <span className="font-display text-sm font-semibold tracking-tight">Monoize</span>
              <span className="mt-0.5 text-xs text-muted-foreground">Console</span>
            </div>
          )}
        </Link>

        <Separator className="my-3" />

        <nav className="flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto">
          {navItems.map((item) => (
            <NavLink
              key={item.to}
              {...item}
              onClick={onNavigate}
              layoutId={layoutId}
              disableLayoutAnimation={disableLayoutAnimation}
              collapsed={collapsed}
            />
          ))}

          {isAdmin && (
            <>
              <Separator className="my-2" />
              {!collapsed && (
                <p className="px-2.5 pb-1 text-xs font-medium uppercase tracking-wider text-muted-foreground">
                  {t("nav.admin")}
                </p>
              )}
              {adminNavItems.map((item) => (
                <NavLink
                  key={item.to}
                  {...item}
                  onClick={onNavigate}
                  layoutId={layoutId}
                  disableLayoutAnimation={disableLayoutAnimation}
                  collapsed={collapsed}
                />
              ))}
            </>
          )}
        </nav>

        {/* Account menu */}
        <div className="mt-auto pt-3">
          <Separator className="mb-3" />
          <DropdownMenu>
            <Tooltip>
              <TooltipTrigger asChild>
                <DropdownMenuTrigger asChild>
                  <Button
                    variant="ghost"
                    className={cn(
                      "group w-full",
                      collapsed ? "justify-center px-2" : "justify-start gap-3 px-2.5"
                    )}
                    size="sm"
                  >
                    <Avatar className="h-6 w-6 shrink-0">
                      {user?.email && (
                        <AvatarImage src={getGravatarUrl(user.email, 48) ?? undefined} alt={user?.username} />
                      )}
                      <AvatarFallback className="text-xs">
                        {user?.username?.[0]?.toUpperCase() || "U"}
                      </AvatarFallback>
                    </Avatar>
                    {!collapsed && (
                      <div className="flex min-w-0 flex-1 flex-col items-start leading-tight">
                        <span className="truncate text-sm font-medium">{user?.username}</span>
                        <span className="truncate text-xs text-muted-foreground">{accountSummary}</span>
                      </div>
                    )}
                  </Button>
                </DropdownMenuTrigger>
              </TooltipTrigger>
              {collapsed && (
                <TooltipContent side="right" sideOffset={8}>
                  <div className="space-y-0.5">
                    <p className="font-medium">{user?.username}</p>
                    <p className="text-xs text-muted-foreground">{accountSummary}</p>
                    <p className="text-xs text-muted-foreground">{roleLabel}</p>
                  </div>
                </TooltipContent>
              )}
            </Tooltip>
            <DropdownMenuContent align={collapsed ? "center" : "start"} side={collapsed ? "right" : "top"} className="w-56">
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
              <DropdownMenuLabel className="p-0 font-normal">
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
      </motion.div>
    </TooltipProvider>
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
                  transition={springs.snappy}
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
      <div className="flex min-h-dvh items-center justify-center bg-background">
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
    <div className="flex h-dvh overflow-hidden bg-background">
      {/* Mobile: floating menu button + sheet */}
      <Sheet open={open} onOpenChange={setOpen}>
        <SheetTrigger asChild>
          <Button
            variant="outline"
            size="icon"
            className="fixed left-4 top-4 z-50 lg:hidden"
          >
            <Menu className="h-5 w-5" />
            <span className="sr-only">Toggle menu</span>
          </Button>
        </SheetTrigger>
        <SheetContent side="left" className="w-64 border-r bg-background p-0 shadow-none">
          <Sidebar onNavigate={() => setOpen(false)} disableLayoutAnimation />
        </SheetContent>
      </Sheet>

      {/* Desktop sidebar: full-bleed, responsive collapse */}
      <aside className="hidden h-dvh shrink-0 border-r lg:block lg:w-16 xl:w-64">
        {/* Collapsed sidebar at lg, expanded at xl */}
        <div className="hidden h-full lg:block xl:hidden">
          <Sidebar collapsed layoutId="nav-active-collapsed" />
        </div>
        <div className="hidden h-full xl:block">
          <Sidebar />
        </div>
      </aside>

      {/* Main content area */}
      <div className="min-h-0 min-w-0 flex flex-1 flex-col overflow-y-auto px-6 py-6 pt-16 lg:px-8 lg:pt-6">
        <main className="mx-auto min-w-0 w-full max-w-6xl flex-1">
          <Outlet />
        </main>
      </div>
    </div>
  );
}
