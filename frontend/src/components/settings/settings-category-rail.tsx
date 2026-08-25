import { useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import { useReducedMotion } from "framer-motion";

import { TabsList, TabsTrigger } from "@/components/ui/tabs";
import { LayoutGroup, SharedTabIndicator } from "@/components/ui/motion";
import {
  SETTINGS_CATEGORIES,
  categoryIndexLabel,
  type SettingsCategoryId,
} from "./settings-categories";

interface SettingsCategoryRailProps {
  activeId: SettingsCategoryId;
}

/**
 * Horizontal, swipe/scrollable category rail for the system settings page.
 *
 * Renders inside a Radix `Tabs` root (owned by the page) so tablist/tab ARIA
 * roles, roving tabindex, and ArrowLeft/ArrowRight activation come from Radix.
 * The active-tab underline moves between chips through the shared layout
 * animation helper (`SharedTabIndicator`).
 */
export function SettingsCategoryRail({ activeId }: SettingsCategoryRailProps) {
  const { t } = useTranslation();
  const shouldReduceMotion = useReducedMotion();
  const scrollRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const activeChip = scrollRef.current?.querySelector<HTMLElement>(
      '[role="tab"][data-state="active"]'
    );
    activeChip?.scrollIntoView({
      behavior: shouldReduceMotion ? "auto" : "smooth",
      block: "nearest",
      inline: "nearest",
    });
  }, [activeId, shouldReduceMotion]);

  return (
    <div ref={scrollRef} className="min-w-0 overflow-x-auto overflow-y-hidden">
      <LayoutGroup id="settings-category-rail">
        <TabsList
          aria-label={t("settings.categoryRailLabel")}
          className="flex h-auto w-max min-w-full items-stretch justify-start gap-1 rounded-none border-b bg-transparent p-0"
        >
          {SETTINGS_CATEGORIES.map((category, index) => (
            <TabsTrigger
              key={category.id}
              value={category.id}
              className="group relative flex shrink-0 flex-col items-start gap-1 rounded-none px-3 pb-3 pt-2 text-left data-[state=active]:bg-transparent data-[state=active]:shadow-none"
            >
              <span
                aria-hidden="true"
                className="font-display text-xs leading-none tracking-widest text-muted-foreground/70 transition-colors group-data-[state=active]:text-primary"
              >
                {categoryIndexLabel(index)}
              </span>
              <span className="whitespace-nowrap text-sm font-medium leading-none">
                {t(category.titleKey)}
              </span>
              {category.id === activeId ? (
                <SharedTabIndicator
                  layoutId="settings-category-indicator"
                  className="absolute inset-x-3 bottom-0 h-0.5 bg-primary"
                />
              ) : null}
            </TabsTrigger>
          ))}
        </TabsList>
      </LayoutGroup>
    </div>
  );
}
