import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import { ChevronDown, KeyRound } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Skeleton } from "@/components/ui/skeleton";
import type { ApiKey } from "@/lib/api";
import { isEligibleKey } from "./auth";

interface ApiKeyDropdownProps {
  value: string;
  onChange: (apiKeyId: string) => void;
  apiKeys: ApiKey[];
  isLoading: boolean;
  resolvedKeyId: string | null;
}

export function ApiKeyDropdown({
  value,
  onChange,
  apiKeys,
  isLoading,
  resolvedKeyId,
}: ApiKeyDropdownProps) {
  const { t } = useTranslation();
  const eligibleKeys = useMemo(
    () => apiKeys.filter((key) => isEligibleKey(key)),
    [apiKeys],
  );
  const selectedKey = apiKeys.find((key) => key.id === value);
  const selectedLabel = value
    ? selectedKey?.name ?? t("playground.apiKey")
    : t("playground.apiKeyBuiltIn");

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          variant="ghost"
          size="sm"
          aria-label={`${t("playground.apiKey")}: ${selectedLabel}`}
          className="h-8 max-w-[10rem] gap-1.5 border border-transparent px-2 text-xs font-medium text-muted-foreground hover:border-border hover:text-foreground"
        >
          <KeyRound className="h-3.5 w-3.5 shrink-0" />
          <span className="min-w-0 truncate">{selectedLabel}</span>
          <ChevronDown className="h-3 w-3 shrink-0 opacity-60" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        side="top"
        sideOffset={8}
        align="start"
        collisionPadding={16}
        className="max-h-[min(18rem,var(--radix-dropdown-menu-content-available-height))] w-[min(20rem,calc(100vw-2rem))]"
      >
        <DropdownMenuLabel>{t("playground.apiKey")}</DropdownMenuLabel>
        <DropdownMenuSeparator />
        <DropdownMenuRadioGroup value={value} onValueChange={onChange}>
          <DropdownMenuRadioItem value="">
            <span className="min-w-0 flex-1 truncate">
              {t("playground.apiKeyBuiltIn")}
            </span>
          </DropdownMenuRadioItem>
          {eligibleKeys.map((key) => (
            <DropdownMenuRadioItem key={key.id} value={key.id}>
              <span className="min-w-0 flex-1 truncate">{key.name}</span>
              <span className="shrink-0 font-mono text-xs text-muted-foreground">
                {key.key_prefix}…
              </span>
              {key.id === resolvedKeyId && (
                <Badge variant="secondary">{t("playground.activeKey")}</Badge>
              )}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
        {isLoading && (
          <div className="flex flex-col gap-1 px-2 py-1" aria-hidden="true">
            <Skeleton className="h-6 w-full" />
            <Skeleton className="h-6 w-4/5" />
          </div>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
