import type { ReactNode } from "react";
import { Link } from "react-router-dom";
import { Check, Copy } from "lucide-react";
import { useAuth } from "@/hooks/use-auth";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";
import { Button } from "@/components/ui/button";
import { QueryError, type QueryErrorProps } from "@/components/ui/query-error";
import { useTranslation } from "react-i18next";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { EmptyState } from "@/components/ui/empty-state";
import { motion, transitions } from "@/components/ui/motion";
import type { PublicSystemSettings } from "@/lib/api";

const ENDPOINTS = [
  {
    labelKey: "dashboard.api.chatCompletions",
    fallback: "Chat Completions",
    path: "/v1/chat/completions",
  },
  {
    labelKey: "dashboard.api.responses",
    fallback: "Responses",
    path: "/v1/responses",
  },
  {
    labelKey: "dashboard.api.messages",
    fallback: "Messages",
    path: "/v1/messages",
  },
  { labelKey: "dashboard.api.models", fallback: "Models", path: "/v1/models" },
] as const;

/** DH-8c: one row-sized copy button with the shared copy feedback. */
function CopyRow({ value, children }: { value: string; children: ReactNode }) {
  const { t } = useTranslation();
  const { copied, copy } = useCopyToClipboard();
  return (
    <button
      type="button"
      aria-label={`${t("common.copy")}: ${value}`}
      className="flex w-full items-center gap-3 rounded-lg border bg-muted/30 p-2.5 text-left transition-colors hover:bg-muted/50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
      onClick={() => void copy(value)}
    >
      <span className="min-w-0 flex-1">{children}</span>
      {copied ? (
        <Check className="size-4 shrink-0 text-success" aria-hidden="true" />
      ) : (
        <Copy className="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
      )}
    </button>
  );
}

interface ApiInfoPanelProps extends QueryErrorProps {
  failed?: boolean;
  settings: PublicSystemSettings | undefined;
  loading?: boolean;
}

export function ApiInfoPanel({
  settings,
  loading,
  failed,
  onRetry,
  retrying,
}: ApiInfoPanelProps) {
  const { t } = useTranslation();
  const { user } = useAuth();
  const isAdmin = user?.role === "admin" || user?.role === "super_admin";
  const baseUrl = settings?.api_base_url?.trim() ?? "";

  return (
    <motion.div
      initial={{ opacity: 0, y: 18 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ delay: 0.22, ...transitions.normal }}
      className="h-full min-h-0"
    >
      <Card className="flex h-full max-h-[28rem] min-h-0 flex-col">
        <CardHeader className="p-4 pb-2">
          <CardTitle className="text-balance text-base font-semibold leading-none tracking-tight">
            {t("dashboard.apiInformation", "API Information")}
          </CardTitle>
        </CardHeader>
        <CardContent className="flex min-h-0 flex-1 flex-col gap-2 p-4 pt-2">
          {failed && (
            <QueryError
              onRetry={onRetry}
              retrying={retrying}
              stale={settings !== undefined}
            />
          )}
          {failed && settings === undefined ? null : loading &&
            settings === undefined ? (
            <div className="flex flex-col gap-2">
              {Array.from({ length: 4 }).map((_, i) => (
                <Skeleton key={i} className="h-14 w-full" />
              ))}
            </div>
          ) : !baseUrl ? (
            <EmptyState
              title={t("dashboard.noApiInfo", "No API Information")}
              description={t(
                isAdmin
                  ? "dashboard.noApiInfoDescription"
                  : "dashboard.apiNotConfigured",
              )}
              variant="inline"
              action={
                isAdmin ? (
                  <Button asChild variant="outline">
                    <Link to="/dashboard/admin-settings">
                      {t("dashboard.configureApi")}
                    </Link>
                  </Button>
                ) : undefined
              }
              className="py-6"
            />
          ) : (
            <div className="flex min-h-0 flex-col gap-2 overflow-auto">
              <CopyRow value={baseUrl}>
                <span className="block text-sm text-muted-foreground">
                  {t("dashboard.apiBaseUrl", "API Base URL")}
                </span>
                <span className="mt-0.5 block truncate font-mono text-sm font-semibold">
                  {baseUrl}
                </span>
              </CopyRow>
              {ENDPOINTS.map((endpoint, index) => {
                const fullUrl = `${baseUrl.replace(/\/+$/, "")}${endpoint.path}`;
                return (
                  <motion.div
                    key={endpoint.path}
                    initial={{ opacity: 0, x: 10 }}
                    animate={{ opacity: 1, x: 0 }}
                    transition={{
                      delay: 0.04 * (index + 1),
                      ...transitions.normal,
                    }}
                  >
                    <CopyRow value={fullUrl}>
                      <span className="block text-sm text-muted-foreground">
                        {t(endpoint.labelKey, endpoint.fallback)}
                      </span>
                      <span className="mt-0.5 block font-mono text-sm text-muted-foreground">
                        {endpoint.path}
                      </span>
                    </CopyRow>
                  </motion.div>
                );
              })}
            </div>
          )}
        </CardContent>
      </Card>
    </motion.div>
  );
}
