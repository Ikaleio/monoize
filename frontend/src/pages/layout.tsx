import { Navigate, Outlet, Link, useLocation } from "react-router-dom";
import { useTranslation } from "react-i18next";
import {
  LayoutDashboard,
  Users,
  Key,
  Settings,
  Server,
  Menu,
  MessageSquareCode,
  ScrollText,
  Database,
  Store,
  CalendarClock,
  Gauge,
  Boxes,
  Code2,
  PanelLeftClose,
  PanelLeftOpen,
  Wallet,
  CreditCard,
} from "lucide-react";
import { useAuth } from "@/hooks/use-auth";
import { Button } from "@/components/ui/button";
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
import { cn } from "@/lib/utils";
import { MonoizeLogo } from "@/components/MonoizeLogo";
import { UserCenterMenu } from "@/components/user-center-menu";
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
  onCollapse,
  onExpand,
  layoutId = "nav-active",
  disableLayoutAnimation = false,
  collapsed = false,
}: {
  onNavigate?: () => void;
  onCollapse?: () => void;
  onExpand?: () => void;
  layoutId?: string;
  disableLayoutAnimation?: boolean;
  collapsed?: boolean;
}) {
  const { user } = useAuth();
  const { t } = useTranslation();
  const isAdmin = user?.role === "super_admin" || user?.role === "admin";

  const navItems = [
    { to: "/dashboard", icon: LayoutDashboard, label: t("nav.dashboard"), exact: true },
    { to: "/dashboard/tokens", icon: Key, label: t("nav.apiKeys") },
    { to: "/dashboard/wallet", icon: Wallet, label: t("nav.wallet") },
    { to: "/dashboard/logs", icon: ScrollText, label: t("nav.logs") },
    { to: "/dashboard/playground", icon: MessageSquareCode, label: t("nav.playground") },
    { to: "/dashboard/marketplace", icon: Store, label: t("nav.marketplace") },
  ];

  const adminNavItems = [
    { to: "/dashboard/admin", icon: Gauge, label: t("nav.adminDashboard") },
    { to: "/dashboard/providers", icon: Server, label: t("nav.providers") },
    { to: "/dashboard/models", icon: Database, label: t("nav.models") },
    { to: "/dashboard/plans", icon: CalendarClock, label: t("nav.billingPlans") },
    { to: "/dashboard/payments", icon: CreditCard, label: t("nav.payments") },
    { to: "/dashboard/users", icon: Users, label: t("nav.users") },
    { to: "/dashboard/groups", icon: Boxes, label: t("nav.groups") },
    { to: "/dashboard/custom-transforms", icon: Code2, label: t("nav.customTransforms") },
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
        {collapsed ? (
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                type="button"
                variant="ghost"
                size="icon"
                className="group size-10 p-1"
                aria-label={t("nav.expandSidebar")}
                aria-expanded={false}
                onClick={onExpand}
              >
                <span className="relative flex size-8 items-center justify-center rounded-lg bg-foreground text-background shadow-sm">
                  <MonoizeLogo className="absolute inset-0 !size-full transition-opacity duration-150 group-hover:opacity-0 group-focus-visible:opacity-0" />
                  <PanelLeftOpen
                    data-icon="inline-start"
                    className="opacity-0 transition-opacity duration-150 group-hover:opacity-100 group-focus-visible:opacity-100"
                  />
                </span>
              </Button>
            </TooltipTrigger>
            <TooltipContent side="right" sideOffset={8}>
              {t("nav.expandSidebar")}
            </TooltipContent>
          </Tooltip>
        ) : (
          <div className="flex items-center gap-2">
            <Link
              to="/dashboard"
              className="group flex min-w-0 flex-1 items-center gap-3 rounded-lg px-2.5 py-2.5 transition-colors hover:bg-accent/50"
            >
              <motion.div
                whileHover={{ scale: 1.05 }}
                whileTap={{ scale: 0.95 }}
                transition={springs.snappy}
                className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-foreground text-background shadow-sm"
              >
                <MonoizeLogo className="size-full" />
              </motion.div>
              <div className="flex min-w-0 flex-col leading-none">
                <span className="truncate font-display text-sm font-semibold tracking-tight">Monoize</span>
                <span className="mt-0.5 truncate text-xs text-muted-foreground">Console</span>
              </div>
            </Link>
            {onCollapse && (
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    className="shrink-0"
                    aria-label={t("nav.collapseSidebar")}
                    aria-expanded={true}
                    onClick={onCollapse}
                  >
                    <PanelLeftClose data-icon="inline-start" />
                  </Button>
                </TooltipTrigger>
                <TooltipContent side="right" sideOffset={8}>
                  {t("nav.collapseSidebar")}
                </TooltipContent>
              </Tooltip>
            )}
          </div>
        )}

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

        {/* Account menu (dashboard-ui-layout.spec.md DL3a-DL3g) */}
        <div className="mt-auto pt-3">
          <Separator className="mb-3" />
          <UserCenterMenu collapsed={collapsed} onNavigate={onNavigate} />
        </div>
      </motion.div>
    </TooltipProvider>
  );
}

export function DashboardLayout() {
  const { user, loading } = useAuth();
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [desktopSidebarCollapsed, setDesktopSidebarCollapsed] = useState(false);

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

      {/* Desktop sidebar: full-bleed, user-controlled collapse */}
      <aside
        className={cn(
          "hidden h-dvh shrink-0 overflow-hidden border-r transition-[width] duration-200 ease-out lg:block",
          desktopSidebarCollapsed ? "w-16" : "w-64"
        )}
      >
        <div className="h-full">
          <Sidebar
            collapsed={desktopSidebarCollapsed}
            onCollapse={() => setDesktopSidebarCollapsed(true)}
            onExpand={() => setDesktopSidebarCollapsed(false)}
            layoutId={desktopSidebarCollapsed ? "nav-active-collapsed" : "nav-active"}
          />
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
