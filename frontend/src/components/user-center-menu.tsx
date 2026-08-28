import { useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { Activity, Cog, LogOut, Monitor, Moon, RefreshCw, Sun } from "lucide-react";
import { motion } from "framer-motion";
import { useAuth } from "@/hooks/use-auth";
import { useTheme } from "@/hooks/use-theme";
import { useLiveUsage } from "@/lib/swr";
import { formatUsdDecimal } from "@/lib/exact-decimal";
import { formatCacheHitRate } from "@/lib/live-usage";
import { cn, getGravatarUrl } from "@/lib/utils";
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Skeleton } from "@/components/ui/skeleton";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { springs } from "@/components/ui/motion";

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

function QuotaRow({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <div className="flex items-baseline justify-between gap-2">
      <span className="shrink-0 text-xs text-muted-foreground">{label}</span>
      {children}
    </div>
  );
}

function LiveMetric({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex flex-col items-center gap-0.5 rounded-md bg-muted/60 px-1 py-1.5">
      <span className="font-mono text-xs font-medium tabular-nums">{value}</span>
      <span className="text-[10px] uppercase tracking-wide text-muted-foreground">
        {label}
      </span>
    </div>
  );
}

// Mounted only while the dropdown content is open, so the useLiveUsage 10s
// poll runs only while the menu is visible (user-live-usage.spec.md LU-10).
function LiveUsageSection() {
  const { t } = useTranslation();
  const { data, error, mutate } = useLiveUsage();

  return (
    <div className="px-2 py-1.5">
      <div className="flex items-center gap-1.5 pb-1">
        <Activity className="h-3 w-3 text-primary" aria-hidden="true" />
        <span className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
          {t("userMenu.liveUsage", "Last 60s")}
        </span>
      </div>
      {error ? (
        <div className="flex items-center justify-between gap-2">
          <span className="text-xs text-destructive">
            {t("userMenu.liveUsageError", "Failed to load live usage")}
          </span>
          <button
            type="button"
            onClick={(e) => {
              e.preventDefault();
              e.stopPropagation();
              void mutate();
            }}
            className="inline-flex shrink-0 items-center gap-1 rounded-md px-1.5 py-0.5 text-xs text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
          >
            <RefreshCw className="h-3 w-3" aria-hidden="true" />
            {t("common.retry")}
          </button>
        </div>
      ) : data ? (
        <div className="grid grid-cols-3 gap-1">
          <LiveMetric label={t("userMenu.rpm", "RPM")} value={data.rpm.toLocaleString()} />
          <LiveMetric label={t("userMenu.tpm", "TPM")} value={data.tpm.toLocaleString()} />
          <LiveMetric
            label={t("userMenu.cacheHit", "Cache hit")}
            value={formatCacheHitRate(data.cache_hit_rate)}
          />
        </div>
      ) : (
        <div className="grid grid-cols-3 gap-1">
          <Skeleton className="h-10 rounded-md" />
          <Skeleton className="h-10 rounded-md" />
          <Skeleton className="h-10 rounded-md" />
        </div>
      )}
    </div>
  );
}

/**
 * Sidebar-bottom user-center dropdown (dashboard-ui-layout.spec.md DL3a-DL3g).
 *
 * Renders the account trigger (expanded row or collapsed avatar with tooltip)
 * and a compact dropdown containing identity, quota/plan facts from the
 * session user, the user's own rolling 60-second usage, and account actions.
 */
export function UserCenterMenu({
  collapsed = false,
  onNavigate,
}: {
  collapsed?: boolean;
  onNavigate?: () => void;
}) {
  const { user, logout } = useAuth();
  const { t } = useTranslation();
  const navigate = useNavigate();

  const roleLabel = t(`roles.${user?.role || "user"}`);
  const balanceLabel = user?.balance_unlimited
    ? t("users.unlimited")
    : formatUsdDecimal(user?.balance_usd, 2);
  const accountSummary = balanceLabel;

  return (
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
      <DropdownMenuContent
        align={collapsed ? "center" : "start"}
        side={collapsed ? "right" : "top"}
        className="w-72"
      >
        {/* Identity header (DL3c) */}
        <div className="flex items-center gap-2.5 px-2 py-1.5">
          <Avatar className="h-8 w-8 shrink-0">
            {user?.email && (
              <AvatarImage src={getGravatarUrl(user.email, 64) ?? undefined} alt={user?.username} />
            )}
            <AvatarFallback className="text-sm">
              {user?.username?.[0]?.toUpperCase() || "U"}
            </AvatarFallback>
          </Avatar>
          <div className="flex min-w-0 flex-1 flex-col">
            <div className="flex items-center justify-between gap-2">
              <span className="truncate text-sm font-medium">{user?.username}</span>
              <span className="shrink-0 rounded-md border px-1.5 text-[10px] leading-4 text-muted-foreground">
                {roleLabel}
              </span>
            </div>
            {user?.email && (
              <span className="truncate text-xs text-muted-foreground">{user.email}</span>
            )}
          </div>
        </div>
        <DropdownMenuSeparator />
        {/* Prepaid balance from the session user only (DL3d). */}
        <div className="flex flex-col gap-1 px-2 py-1.5">
          {user ? (
            <>
              <QuotaRow label={t("userMenu.balance", "Balance")}>
                <span className="font-mono text-xs font-medium tabular-nums">{balanceLabel}</span>
              </QuotaRow>
            </>
          ) : (
            <>
              <Skeleton className="h-4 w-full" />
              <Skeleton className="h-4 w-2/3" />
            </>
          )}
        </div>
        <DropdownMenuSeparator />
        {/* Own last-60s usage (DL3e) */}
        <LiveUsageSection />
        <DropdownMenuSeparator />
        {/* Actions (DL3f) */}
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
  );
}
