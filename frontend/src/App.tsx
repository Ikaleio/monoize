import { Routes, Route, Navigate } from "react-router-dom";
import { SWRConfig } from "swr";
import { AuthProvider } from "@/hooks/use-auth";
import { ThemeProvider } from "@/hooks/use-theme";
import { Toaster } from "@/components/ui/sonner";
import { LoginPage } from "@/pages/login";
import { DashboardLayout } from "@/pages/layout";
import { DashboardPage } from "@/pages/dashboard";
import { ProvidersPage } from "@/pages/providers";
import { ApiKeysPage } from "@/pages/api-keys";
import { UsersPage } from "@/pages/users";
import { SettingsPage } from "@/pages/settings";
import { UserSettingsPage } from "@/pages/user-settings";
import { PlaygroundPage } from "@/pages/playground";
import { RequestLogsPage } from "@/pages/request-logs";
import { ModelMetadataPage } from "@/pages/model-metadata";
import "@/i18n";

function App() {
  return (
    <ThemeProvider>
      <SWRConfig
        value={{
          revalidateOnFocus: true,
          revalidateOnReconnect: true,
          dedupingInterval: 2000,
        }}
      >
        <AuthProvider>
        <Routes>
          <Route path="/login" element={<LoginPage />} />
          {/* Dashboard routes - admin panel */}
          <Route path="/dashboard" element={<DashboardLayout />}>
            <Route index element={<DashboardPage />} />
            <Route path="providers" element={<ProvidersPage />} />
            <Route path="tokens" element={<ApiKeysPage />} />
            <Route path="logs" element={<RequestLogsPage />} />
            <Route path="playground" element={<PlaygroundPage />} />
            <Route path="models" element={<ModelMetadataPage />} />
            <Route path="users" element={<UsersPage />} />
            <Route path="admin-settings" element={<SettingsPage />} />
          </Route>
          {/* User settings routes */}
          <Route path="/settings" element={<DashboardLayout />}>
            <Route index element={<UserSettingsPage />} />
          </Route>
          {/* Redirect root to dashboard */}
          <Route path="/" element={<Navigate to="/dashboard" replace />} />
          <Route path="*" element={<Navigate to="/dashboard" replace />} />
        </Routes>
        </AuthProvider>
      </SWRConfig>
      <Toaster />
    </ThemeProvider>
  );
}

export default App;
