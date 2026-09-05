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
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarGroupContent,
  SidebarGroupLabel,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarProvider,
  SidebarSeparator,
  SidebarTrigger,
  useSidebar,
} from "@/components/ui/sidebar";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
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
  exact = false,
}: {
  to: string;
  icon: React.ComponentType<{ className?: string }>;
  label: string;
  exact?: boolean;
}) {
  const location = useLocation();
  const { isMobile, state, setOpenMobile } = useSidebar();
  const isActive = exact
    ? location.pathname === to
    : location.pathname === to || location.pathname.startsWith(to + "/");

  return (
    <SidebarMenuItem>
      <SidebarMenuButton
        asChild
        isActive={isActive}
        tooltip={label}
        className="relative"
      >
        <Link
          to={to}
          onClick={() => setOpenMobile(false)}
          aria-current={isActive ? "page" : undefined}
          aria-label={label}
        >
          {isActive && !isMobile && (
            <motion.div
              layoutId={`nav-active-${state}`}
              className="absolute inset-0 rounded-md bg-sidebar-accent"
              transition={navTransition}
            />
          )}
          <Icon className={cn("relative", isActive && "text-primary")} />
          <span className="relative">{label}</span>
        </Link>
      </SidebarMenuButton>
    </SidebarMenuItem>
  );
}

function AppSidebar() {
  const { user } = useAuth();
  const { t } = useTranslation();
  const { isMobile, state, setOpenMobile, toggleSidebar } = useSidebar();
  const collapsed = !isMobile && state === "collapsed";
  const onNavigate = () => setOpenMobile(false);
  const isAdmin = user?.role === "super_admin" || user?.role === "admin";

  const navItems = [
    {
      to: "/dashboard",
      icon: LayoutDashboard,
      label: t("nav.dashboard"),
      exact: true,
    },
    { to: "/dashboard/tokens", icon: Key, label: t("nav.apiKeys") },
    { to: "/dashboard/wallet", icon: Wallet, label: t("nav.wallet") },
    { to: "/dashboard/logs", icon: ScrollText, label: t("nav.logs") },
    {
      to: "/dashboard/playground",
      icon: MessageSquareCode,
      label: t("nav.playground"),
    },
    { to: "/dashboard/marketplace", icon: Store, label: t("nav.marketplace") },
  ];

  const adminNavItems = [
    { to: "/dashboard/admin", icon: Gauge, label: t("nav.adminDashboard") },
    { to: "/dashboard/providers", icon: Server, label: t("nav.providers") },
    { to: "/dashboard/models", icon: Database, label: t("nav.models") },
    {
      to: "/dashboard/plans",
      icon: CalendarClock,
      label: t("nav.billingPlans"),
    },
    { to: "/dashboard/payments", icon: CreditCard, label: t("nav.payments") },
    { to: "/dashboard/users", icon: Users, label: t("nav.users") },
    { to: "/dashboard/groups", icon: Boxes, label: t("nav.groups") },
    {
      to: "/dashboard/custom-transforms",
      icon: Code2,
      label: t("nav.customTransforms"),
    },
    {
      to: "/dashboard/admin-settings",
      icon: Settings,
      label: t("nav.settings"),
    },
  ];

  return (
    <Sidebar variant="sidebar" collapsible="icon">
      <SidebarHeader className="p-3 group-data-[collapsible=icon]:items-center">
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
                onClick={toggleSidebar}
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
              onClick={onNavigate}
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
                <span className="truncate font-display text-sm font-semibold tracking-tight">
                  Monoize
                </span>
                <span className="mt-0.5 truncate text-xs text-muted-foreground">
                  Console
                </span>
              </div>
            </Link>
            {!isMobile && (
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    className="shrink-0"
                    aria-label={t("nav.collapseSidebar")}
                    aria-expanded={true}
                    onClick={toggleSidebar}
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
      </SidebarHeader>
      <SidebarSeparator className="mx-3" />
      <SidebarContent>
        <nav aria-label={t("nav.sidebarTitle")}>
          <SidebarGroup className="p-3 group-data-[collapsible=icon]:px-4">
            <SidebarGroupContent>
              <SidebarMenu>
                {navItems.map((item) => (
                  <NavLink key={item.to} {...item} />
                ))}
              </SidebarMenu>
            </SidebarGroupContent>
          </SidebarGroup>
          {isAdmin && (
            <>
              <SidebarSeparator className="mx-3" />
              <SidebarGroup className="p-3 group-data-[collapsible=icon]:px-4">
                <SidebarGroupLabel>{t("nav.admin")}</SidebarGroupLabel>
                <SidebarGroupContent>
                  <SidebarMenu>
                    {adminNavItems.map((item) => (
                      <NavLink key={item.to} {...item} />
                    ))}
                  </SidebarMenu>
                </SidebarGroupContent>
              </SidebarGroup>
            </>
          )}
        </nav>
      </SidebarContent>
      <SidebarFooter className="gap-3 p-3">
        <SidebarSeparator className="mx-0" />
        <UserCenterMenu collapsed={collapsed} onNavigate={onNavigate} />
      </SidebarFooter>
    </Sidebar>
  );
}

export function DashboardLayout() {
  const { user, loading } = useAuth();
  const { t } = useTranslation();

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
    <SidebarProvider className="h-dvh overflow-hidden bg-background">
      <AppSidebar />
      <SidebarTrigger
        variant="outline"
        className="fixed left-4 top-4 z-10 size-11 lg:hidden"
        aria-label={t("nav.openSidebar")}
      >
        <Menu aria-hidden="true" />
      </SidebarTrigger>

      <div className="min-h-0 min-w-0 flex flex-1 flex-col overflow-y-auto px-6 py-6 pt-16 lg:px-8 lg:pt-6">
        <main className="mx-auto min-w-0 w-full max-w-6xl flex-1">
          <Outlet />
        </main>
      </div>
    </SidebarProvider>
  );
}
