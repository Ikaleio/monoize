import { useTranslation } from "react-i18next";
import { Settings2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import type { PlaygroundPrefs } from "./prefs";

interface SettingsPopoverProps {
  prefs: PlaygroundPrefs;
  setPref: (name: keyof PlaygroundPrefs, value: string) => void;
}

export function SettingsPopover({
  prefs,
  setPref,
}: SettingsPopoverProps) {
  const { t } = useTranslation();

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
        <div className="flex flex-col gap-4">
          <div className="flex flex-col gap-2">
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
            <div className="flex flex-col gap-2">
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
            <div className="flex flex-col gap-2">
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
        </div>
      </PopoverContent>
    </Popover>
  );
}
