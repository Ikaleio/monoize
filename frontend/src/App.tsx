/* eslint-disable react-refresh/only-export-components */
import { lazy, Suspense, useEffect, type ReactNode } from "react";
import {
  createBrowserRouter,
  Navigate,
  Outlet,
  useMatches,
  type UIMatch,
} from "react-router-dom";
import { SWRConfig } from "swr";
import { MotionConfig } from "framer-motion";
import { useTranslation } from "react-i18next";
import { AuthProvider, useAuth } from "@/hooks/use-auth";
import { ThemeProvider } from "@/hooks/use-theme";
import { Toaster } from "@/components/ui/sonner";
import { RouteErrorBoundary } from "@/components/ui/error-boundary";
import { NoPermissionState } from "@/components/ui/no-permission";
import { RouteSkeleton, type RouteSkeletonKind } from "@/components/ui/route-skeleton";
import { usePublicSettings } from "@/lib/swr";
import { DashboardLayout } from "@/pages/layout";
import "@/i18n";

const LoginPage = lazy(() => import("@/pages/login").then((m) => ({ default: m.LoginPage })));
const DashboardPage = lazy(() => import("@/pages/dashboard").then((m) => ({ default: m.DashboardPage })));
const AdminDashboardPage = lazy(() => import("@/pages/admin-dashboard").then((m) => ({ default: m.AdminDashboardPage })));
const ProvidersPage = lazy(() => import("@/pages/providers").then((m) => ({ default: m.ProvidersPage })));
const ApiKeysPage = lazy(() => import("@/pages/api-keys").then((m) => ({ default: m.ApiKeysPage })));
const UsersPage = lazy(() => import("@/pages/users").then((m) => ({ default: m.UsersPage })));
const GroupsPage = lazy(() => import("@/pages/groups").then((m) => ({ default: m.GroupsPage })));
const BillingPlansPage = lazy(() => import("@/pages/billing-plans").then((m) => ({ default: m.BillingPlansPage })));
const SettingsPage = lazy(() => import("@/pages/settings").then((m) => ({ default: m.SettingsPage })));
const UserSettingsPage = lazy(() => import("@/pages/user-settings").then((m) => ({ default: m.UserSettingsPage })));
const PlaygroundPage = lazy(() => import("@/pages/playground").then((m) => ({ default: m.PlaygroundPage })));
const RequestLogsPage = lazy(() => import("@/pages/request-logs").then((m) => ({ default: m.RequestLogsPage })));
const ModelPricingPage = lazy(() => import("@/pages/model-pricing").then((m) => ({ default: m.ModelPricingPage })));
const ModelMarketplacePage = lazy(() => import("@/pages/model-marketplace").then((m) => ({ default: m.ModelMarketplacePage })));
const CustomTransformsPage = lazy(() => import("@/pages/custom-transforms").then((m) => ({ default: m.CustomTransformsPage })));
const WalletPage = lazy(() => import("@/pages/wallet").then((m) => ({ default: m.WalletPage })));
const PaymentsPage = lazy(() => import("@/pages/payments").then((m) => ({ default: m.PaymentsPage })));
const NotFoundPage = lazy(() => import("@/pages/not-found").then((m) => ({ default: m.NotFoundPage })));

/** Route `handle`: the i18n key of the page title, shared by the nav label and `document.title`. */
interface RouteHandle {
  titleKey: string;
}

const DEFAULT_SITE_NAME = "Monoize";

function routeTitleKey(matches: UIMatch[]): string | undefined {
  for (let index = matches.length - 1; index >= 0; index -= 1) {
    const handle = matches[index].handle as RouteHandle | undefined;
    if (handle?.titleKey) return handle.titleKey;
  }
  return undefined;
}

/** Keeps `document.title` at `{page title} · {site_name}` (dashboard-ui-layout.spec.md DL8). */
function DocumentTitle() {
  const { t } = useTranslation();
  const matches = useMatches();
  const { data: publicSettings } = usePublicSettings();
  const siteName = publicSettings?.site_name.trim() || DEFAULT_SITE_NAME;
  const titleKey = routeTitleKey(matches);
  const title = titleKey ? `${t(titleKey)} · ${siteName}` : siteName;

  useEffect(() => {
    document.title = title;
  }, [title]);

  return null;
}

function AppRoot() {
  return (
    <MotionConfig reducedMotion="user">
      <ThemeProvider>
        <SWRConfig
          value={{
            revalidateOnFocus: true,
            revalidateOnReconnect: true,
            dedupingInterval: 2000,
          }}
        >
          <AuthProvider>
            <DocumentTitle />
            <Outlet />
          </AuthProvider>
        </SWRConfig>
        <Toaster />
      </ThemeProvider>
    </MotionConfig>
  );
}

/**
 * Admin-only routes (DL6a). Non-admins get the shared no-permission state; the page
 * component never mounts, so it never fetches.
 */
function RequireAdmin() {
  const { user } = useAuth();
  const { t } = useTranslation();
  const titleKey = routeTitleKey(useMatches());
  if (user?.role === "admin" || user?.role === "super_admin") {
    return <Outlet />;
  }
  return <NoPermissionState title={titleKey ? t(titleKey) : undefined} />;
}

// The Suspense key is per route: without it, navigating to a page that is still
// downloading keeps the previous page on screen instead of showing the skeleton.
const page = (
  titleKey: string,
  element: ReactNode,
  skeleton: RouteSkeletonKind = "table",
  headerActions = 0,
) => ({
  handle: { titleKey } satisfies RouteHandle,
  element: (
    <Suspense key={titleKey} fallback={<RouteSkeleton kind={skeleton} headerActions={headerActions} />}>
      {element}
    </Suspense>
  ),
});

export const router = createBrowserRouter([
  {
    path: "/",
    element: <AppRoot />,
    errorElement: <RouteErrorBoundary />,
    children: [
      { index: true, element: <Navigate to="/dashboard" replace /> },
      { path: "login", ...page("auth.signIn", <LoginPage />, "login") },
      {
        element: <DashboardLayout />,
        children: [
          {
            // Errors render inside the shell, so the sidebar stays usable (DL9).
            errorElement: <RouteErrorBoundary />,
            children: [
              {
                path: "dashboard",
                children: [
                  { index: true, ...page("nav.dashboard", <DashboardPage />, "header") },
                  { path: "tokens", ...page("nav.apiKeys", <ApiKeysPage />, "table", 1) },
                  { path: "wallet", ...page("nav.wallet", <WalletPage />, "header") },
                  { path: "logs", ...page("nav.logs", <RequestLogsPage />) },
                  { path: "playground", ...page("nav.playground", <PlaygroundPage />, "none") },
                  { path: "marketplace", ...page("nav.marketplace", <ModelMarketplacePage />) },
                  {
                    element: <RequireAdmin />,
                    children: [
                      {
                        path: "admin",
                        ...page("nav.adminDashboard", <AdminDashboardPage />, "header", 1),
                      },
                      { path: "providers", ...page("nav.providers", <ProvidersPage />, "cards", 1) },
                      { path: "groups", ...page("nav.groups", <GroupsPage />, "table", 1) },
                      {
                        path: "custom-transforms",
                        ...page("nav.customTransforms", <CustomTransformsPage />, "cards", 2),
                      },
                      { path: "models", ...page("nav.models", <ModelPricingPage />) },
                      { path: "plans", ...page("nav.billingPlans", <BillingPlansPage />, "table", 1) },
                      { path: "payments", ...page("nav.payments", <PaymentsPage />) },
                      { path: "users", ...page("nav.users", <UsersPage />, "table", 1) },
                      {
                        path: "admin-settings",
                        ...page("nav.systemSettings", <SettingsPage />, "settings"),
                      },
                    ],
                  },
                ],
              },
              { path: "settings", ...page("nav.accountSettings", <UserSettingsPage />, "header") },
              { path: "*", ...page("errors.notFoundTitle", <NotFoundPage />, "none") },
            ],
          },
        ],
      },
    ],
  },
]);
