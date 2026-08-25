import { useTranslation } from "react-i18next";
import { Check, KeyRound, Settings2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Separator } from "@/components/ui/separator";
import { Skeleton } from "@/components/ui/skeleton";
import { Textarea } from "@/components/ui/textarea";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import type { ApiKey } from "@/lib/api";
import { cn } from "@/lib/utils";
import { isEligibleKey } from "./auth";
import type { PlaygroundPrefs } from "./prefs";

interface SettingsPopoverProps {
  prefs: PlaygroundPrefs;
  setPref: (name: keyof PlaygroundPrefs, value: string) => void;
  apiKeys: ApiKey[];
  keysLoading: boolean;
  resolvedKeyId: string | null;
}

export function SettingsPopover({
  prefs,
  setPref,
  apiKeys,
  keysLoading,
  resolvedKeyId,
}: SettingsPopoverProps) {
  const { t } = useTranslation();
  const eligibleKeys = apiKeys.filter((key) => isEligibleKey(key));

  return (
    <Popover>
      <PopoverTrigger asChild>
        <Button
          variant="ghost"
          size="icon"
          aria-label={t("playground.settings")}
          className="size-11 shrink-0 touch-manipulation text-muted-foreground hover:text-foreground sm:size-8"
        >
          <Settings2 className="h-4 w-4" />
        </Button>
      </PopoverTrigger>
      <PopoverContent
        side="top"
        sideOffset={8}
        align="end"
        collisionPadding={16}
        className="max-h-[var(--radix-popover-content-available-height)] w-[min(20rem,calc(100vw-2rem))] overflow-y-auto overscroll-contain"
      >
        <div className="space-y-4">
          <div className="space-y-2">
            <Label htmlFor="playground-system-prompt" className="text-xs">
              {t("playground.systemPrompt")}
            </Label>
            <Textarea
              id="playground-system-prompt"
              value={prefs.systemPrompt}
              onChange={(e) => setPref("systemPrompt", e.target.value)}
              placeholder={t("playground.systemPromptPlaceholder")}
              className="min-h-[64px] resize-y text-sm"
            />
          </div>

          <div className="grid grid-cols-2 gap-3">
            <div className="space-y-2">
              <Label htmlFor="playground-temperature" className="text-xs">
                {t("playground.temperature")}
              </Label>
              <Input
                id="playground-temperature"
                type="number"
                min="0"
                max="2"
                step="0.1"
                value={prefs.temperature}
                onChange={(e) => setPref("temperature", e.target.value)}
                placeholder={t("playground.defaultValue")}
                className="h-8 text-sm"
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="playground-max-tokens" className="text-xs">
                {t("playground.maxTokens")}
              </Label>
              <Input
                id="playground-max-tokens"
                type="number"
                min="1"
                value={prefs.maxTokens}
                onChange={(e) => setPref("maxTokens", e.target.value)}
                placeholder={t("playground.defaultValue")}
                className="h-8 text-sm"
              />
            </div>
          </div>

          <Separator />

          <div className="space-y-2">
            <Label className="flex items-center gap-1.5 text-xs">
              <KeyRound className="h-3.5 w-3.5" />
              {t("playground.apiKey")}
            </Label>
            <div className="max-h-44 space-y-1 overflow-y-auto">
              <button
                type="button"
                onClick={() => setPref("apiKeyId", "")}
                className={cn(
                  "flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-xs transition-colors hover:bg-accent",
                  prefs.apiKeyId === "" && "bg-accent/60",
                )}
              >
                <span className="min-w-0 flex-1 truncate">
                  {t("playground.apiKeyBuiltIn")}
                </span>
                <Check
                  className={cn(
                    "h-3.5 w-3.5 shrink-0",
                    prefs.apiKeyId === "" ? "opacity-100" : "opacity-0",
                  )}
                />
              </button>
              {keysLoading ? (
                <div className="flex flex-col gap-1 px-2 py-1">
                  <Skeleton className="h-6 w-full" />
                  <Skeleton className="h-6 w-4/5" />
                </div>
              ) : (
                eligibleKeys.map((key) => (
                  <button
                    key={key.id}
                    type="button"
                    onClick={() => setPref("apiKeyId", key.id)}
                    className={cn(
                      "flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-xs transition-colors hover:bg-accent",
                      prefs.apiKeyId === key.id && "bg-accent/60",
                    )}
                  >
                    <span className="min-w-0 flex-1 truncate">{key.name}</span>
                    <span className="shrink-0 font-mono text-[11px] text-muted-foreground">
                      {key.key_prefix}…
                    </span>
                    {key.id === resolvedKeyId && (
                      <span className="shrink-0 rounded-md bg-accent px-1.5 py-0.5 text-[10px] font-medium text-accent-foreground">
                        {t("playground.activeKey")}
                      </span>
                    )}
                    <Check
                      className={cn(
                        "h-3.5 w-3.5 shrink-0",
                        prefs.apiKeyId === key.id ? "opacity-100" : "opacity-0",
                      )}
                    />
                  </button>
                ))
              )}
            </div>
            <p className="text-[11px] leading-relaxed text-muted-foreground">
              {prefs.apiKeyId
                ? t("playground.apiKeyScopeHint")
                : t("playground.apiKeyBuiltInHint")}
            </p>
          </div>
        </div>
      </PopoverContent>
    </Popover>
  );
}
