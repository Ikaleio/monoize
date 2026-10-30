import { Fragment, useState } from "react";
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
import { DashboardScrollParentContext } from "@/lib/dashboard-scroll";
import { usePublicSettings } from "@/lib/swr";

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
  const { data: publicSettings } = usePublicSettings();
  const siteName = publicSettings?.site_name.trim() || "Monoize";

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

  // Task blocks separated by SidebarSeparator (DL6): overview / routing / billing /
  // accounts / system.
  const adminNavBlocks = [
    [{ to: "/dashboard/admin", icon: Gauge, label: t("nav.adminDashboard") }],
    [
      { to: "/dashboard/providers", icon: Server, label: t("nav.providers") },
      { to: "/dashboard/groups", icon: Boxes, label: t("nav.groups") },
      {
        to: "/dashboard/custom-transforms",
        icon: Code2,
        label: t("nav.customTransforms"),
      },
    ],
    [
      { to: "/dashboard/models", icon: Database, label: t("nav.models") },
      {
        to: "/dashboard/plans",
        icon: CalendarClock,
        label: t("nav.billingPlans"),
      },
      { to: "/dashboard/payments", icon: CreditCard, label: t("nav.payments") },
    ],
    [{ to: "/dashboard/users", icon: Users, label: t("nav.users") }],
    [
      {
        to: "/dashboard/admin-settings",
        icon: Settings,
        label: t("nav.systemSettings"),
      },
    ],
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
                // The tap gesture would otherwise add tabindex="0" inside the link (DS34c).
                tabIndex={-1}
                whileHover={{ scale: 1.05 }}
                whileTap={{ scale: 0.95 }}
                transition={springs.snappy}
                className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-foreground text-background shadow-sm"
              >
                <MonoizeLogo className="size-full" />
              </motion.div>
              {/* Admin-set names can outgrow the rail: wrap instead of cutting a word mid-way. */}
              <span
                title={siteName}
                className="line-clamp-2 min-w-0 break-words font-display text-sm font-semibold leading-4 tracking-tight"
              >
                {siteName}
              </span>
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
          <SidebarGroup className="px-3 py-2 group-data-[collapsible=icon]:px-4">
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
              <SidebarGroup className="px-3 py-2 group-data-[collapsible=icon]:px-4">
                <SidebarGroupLabel>{t("nav.admin")}</SidebarGroupLabel>
                <SidebarGroupContent className="flex flex-col gap-1">
                  {adminNavBlocks.map((block, index) => (
                    <Fragment key={block[0].to}>
                      {index > 0 && <SidebarSeparator className="mx-2" />}
                      <SidebarMenu>
                        {block.map((item) => (
                          <NavLink key={item.to} {...item} />
                        ))}
                      </SidebarMenu>
                    </Fragment>
                  ))}
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

const SIDEBAR_STORAGE_KEY = "monoize-sidebar";

export function DashboardLayout() {
  const { user, loading } = useAuth();
  const { t } = useTranslation();
  const location = useLocation();
  const [mainPane, setMainPane] = useState<HTMLElement | null>(null);
  // Desktop collapse state survives reloads (DL1f); the mobile sheet is independent.
  const [sidebarOpen, setSidebarOpen] = useState(
    () => localStorage.getItem(SIDEBAR_STORAGE_KEY) !== "collapsed",
  );
  const changeSidebarOpen = (open: boolean) => {
    setSidebarOpen(open);
    localStorage.setItem(SIDEBAR_STORAGE_KEY, open ? "expanded" : "collapsed");
  };

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
    return <Navigate to="/login" replace state={{ from: location }} />;
  }

  return (
    <SidebarProvider
      open={sidebarOpen}
      onOpenChange={changeSidebarOpen}
      className="h-dvh overflow-hidden bg-background"
    >
      <AppSidebar />
      <SidebarTrigger
        variant="outline"
        className="fixed left-4 top-4 z-10 size-11 lg:hidden"
        aria-label={t("nav.openSidebar")}
      >
        <Menu aria-hidden="true" />
      </SidebarTrigger>

      <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden">
        {/* The pane spans the full width and height so its scrollbar and scrolled content
            reach the viewport edges; page padding lives inside it (DL7c). */}
        <main
          ref={setMainPane}
          className="flex min-h-0 min-w-0 w-full flex-1 flex-col overflow-y-auto px-6 pb-6 pt-16 lg:px-8 lg:pt-6"
        >
          <div className="mx-auto flex w-full min-w-0 max-w-6xl flex-1 flex-col">
            <DashboardScrollParentContext.Provider value={mainPane}>
              <Outlet />
            </DashboardScrollParentContext.Provider>
          </div>
        </main>
      </div>
    </SidebarProvider>
  );
}
