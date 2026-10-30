/* eslint-disable react-refresh/only-export-components */
import { useEffect } from "react";
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
import { usePublicSettings } from "@/lib/swr";
import { LoginPage } from "@/pages/login";
import { DashboardLayout } from "@/pages/layout";
import { DashboardPage } from "@/pages/dashboard";
import { AdminDashboardPage } from "@/pages/admin-dashboard";
import { ProvidersPage } from "@/pages/providers";
import { ApiKeysPage } from "@/pages/api-keys";
import { UsersPage } from "@/pages/users";
import { GroupsPage } from "@/pages/groups";
import { BillingPlansPage } from "@/pages/billing-plans";
import { SettingsPage } from "@/pages/settings";
import { UserSettingsPage } from "@/pages/user-settings";
import { PlaygroundPage } from "@/pages/playground";
import { RequestLogsPage } from "@/pages/request-logs";
import { ModelPricingPage } from "@/pages/model-pricing";
import { ModelMarketplacePage } from "@/pages/model-marketplace";
import { CustomTransformsPage } from "@/pages/custom-transforms";
import { WalletPage } from "@/pages/wallet";
import { PaymentsPage } from "@/pages/payments";
import { NotFoundPage } from "@/pages/not-found";
import "@/i18n";

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

const page = (titleKey: string) => ({ handle: { titleKey } satisfies RouteHandle });

export const router = createBrowserRouter([
  {
    path: "/",
    element: <AppRoot />,
    errorElement: <RouteErrorBoundary />,
    children: [
      { index: true, element: <Navigate to="/dashboard" replace /> },
      { path: "login", element: <LoginPage />, ...page("auth.signIn") },
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
                  { index: true, element: <DashboardPage />, ...page("nav.dashboard") },
                  { path: "tokens", element: <ApiKeysPage />, ...page("nav.apiKeys") },
                  { path: "wallet", element: <WalletPage />, ...page("nav.wallet") },
                  { path: "logs", element: <RequestLogsPage />, ...page("nav.logs") },
                  {
                    path: "playground",
                    element: <PlaygroundPage />,
                    ...page("nav.playground"),
                  },
                  {
                    path: "marketplace",
                    element: <ModelMarketplacePage />,
                    ...page("nav.marketplace"),
                  },
                  {
                    element: <RequireAdmin />,
                    children: [
                      {
                        path: "admin",
                        element: <AdminDashboardPage />,
                        ...page("nav.adminDashboard"),
                      },
                      {
                        path: "providers",
                        element: <ProvidersPage />,
                        ...page("nav.providers"),
                      },
                      { path: "groups", element: <GroupsPage />, ...page("nav.groups") },
                      {
                        path: "custom-transforms",
                        element: <CustomTransformsPage />,
                        ...page("nav.customTransforms"),
                      },
                      { path: "models", element: <ModelPricingPage />, ...page("nav.models") },
                      {
                        path: "plans",
                        element: <BillingPlansPage />,
                        ...page("nav.billingPlans"),
                      },
                      {
                        path: "payments",
                        element: <PaymentsPage />,
                        ...page("nav.payments"),
                      },
                      { path: "users", element: <UsersPage />, ...page("nav.users") },
                      {
                        path: "admin-settings",
                        element: <SettingsPage />,
                        ...page("nav.systemSettings"),
                      },
                    ],
                  },
                ],
              },
              {
                path: "settings",
                element: <UserSettingsPage />,
                ...page("nav.accountSettings"),
              },
              { path: "*", element: <NotFoundPage />, ...page("errors.notFoundTitle") },
            ],
          },
        ],
      },
    ],
  },
]);
