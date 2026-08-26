import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { EmptyState } from "@/components/ui/empty-state";
import { motion, transitions } from "@/components/ui/motion";
import type { PublicSystemSettings } from "@/lib/api";

const ENDPOINTS = [
  { labelKey: "dashboard.api.chatCompletions", fallback: "Chat Completions", path: "/v1/chat/completions" },
  { labelKey: "dashboard.api.responses", fallback: "Responses", path: "/v1/responses" },
  { labelKey: "dashboard.api.models", fallback: "Models", path: "/v1/models" },
  { labelKey: "dashboard.api.messages", fallback: "Messages", path: "/v1/messages" },
] as const;

interface ApiInfoPanelProps {
  settings: PublicSystemSettings | undefined;
  loading?: boolean;
}

export function ApiInfoPanel({ settings, loading }: ApiInfoPanelProps) {
  const { t } = useTranslation();
  const baseUrl = settings?.api_base_url?.trim() ?? "";

  const copy = async (value: string) => {
    try {
      await navigator.clipboard.writeText(value);
      toast.success(t("common.copied", "Copied"));
    } catch {
      toast.error(t("common.error", "Error"));
    }
  };

  return (
    <motion.div
      initial={{ opacity: 0, y: 18 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ delay: 0.22, ...transitions.normal }}
      className="h-full min-h-0"
    >
      <Card className="flex h-full min-h-0 flex-col">
        <CardHeader className="p-4 pb-2">
          <CardTitle className="text-balance text-base font-semibold leading-none tracking-tight">
            {t("dashboard.apiInformation", "API Information")}
          </CardTitle>
        </CardHeader>
        <CardContent className="flex min-h-0 flex-1 flex-col gap-2 p-4 pt-2">
          {loading ? (
            <div className="flex flex-col gap-2">
              {Array.from({ length: 4 }).map((_, i) => (
                <Skeleton key={i} className="h-14 w-full" />
              ))}
            </div>
          ) : !baseUrl ? (
            <EmptyState
              title={t("dashboard.noApiInfo", "No API Information")}
              description={t(
                "dashboard.noApiInfoDescription",
                "Please configure the API base URL in system settings."
              )}
              className="flex-1 py-6"
            />
          ) : (
            <div className="flex min-h-0 flex-col gap-2 overflow-auto">
              <button
                type="button"
                className="w-full rounded-lg border bg-muted/30 p-2.5 text-left transition-colors hover:bg-muted/50 active:bg-muted/70"
                onClick={() => void copy(baseUrl)}
              >
                <p className="text-xs text-muted-foreground">
                  {t("dashboard.apiBaseUrl", "API Base URL")}
                </p>
                <p className="mt-0.5 truncate font-mono text-xs font-semibold">{baseUrl}</p>
              </button>
              {ENDPOINTS.map((endpoint, index) => {
                const fullUrl = `${baseUrl.replace(/\/+$/, "")}${endpoint.path}`;
                return (
                  <motion.button
                    key={endpoint.path}
                    type="button"
                    initial={{ opacity: 0, x: 10 }}
                    animate={{ opacity: 1, x: 0 }}
                    transition={{ delay: 0.04 * (index + 1), ...transitions.normal }}
                    className="w-full rounded-lg border bg-muted/30 p-2.5 text-left transition-colors hover:bg-muted/50 active:bg-muted/70"
                    onClick={() => void copy(fullUrl)}
                  >
                    <p className="text-xs text-muted-foreground">
                      {t(endpoint.labelKey, endpoint.fallback)}
                    </p>
                    <p className="mt-0.5 font-mono text-xs text-muted-foreground">
                      {endpoint.path}
                    </p>
                  </motion.button>
                );
              })}
            </div>
          )}
        </CardContent>
      </Card>
    </motion.div>
  );
}
