import { useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";

import { LayoutGroup, SharedTabIndicator } from "@/components/ui/motion";
import { useReducedMotionPreference } from "@/hooks/use-reduced-motion";
import { SETTINGS_CATEGORIES, type SettingsCategoryId } from "./settings-categories";

interface SettingsCategoryNavProps {
  activeId: SettingsCategoryId;
  /** Match counts per category while searching; `null` in browse mode. */
  matchCounts: ReadonlyMap<SettingsCategoryId, number> | null;
  onSelect: (id: SettingsCategoryId) => void;
}

/**
 * Category navigation (SSU-6). A vertical list inside a settings body of at least
 * 56rem; otherwise one horizontal row that scrolls inside its own container and keeps
 * the active category (or, while searching, the first matching one) in view.
 */
export function SettingsCategoryNav({ activeId, matchCounts, onSelect }: SettingsCategoryNavProps) {
  const { t } = useTranslation();
  const reduceMotion = useReducedMotionPreference();
  const listRef = useRef<HTMLUListElement>(null);
  const searching = matchCounts !== null;
  const firstMatchId = matchCounts
    ? SETTINGS_CATEGORIES.find((category) => matchCounts.has(category.id))?.id
    : undefined;
  const revealId = searching ? firstMatchId : activeId;

  useEffect(() => {
    const list = listRef.current;
    // Only the horizontal row scrolls; the vertical list is sticky and always visible.
    if (!list || !revealId || list.scrollWidth <= list.clientWidth) return;
    list.querySelector(`[data-category="${revealId}"]`)?.scrollIntoView({
      behavior: reduceMotion ? "auto" : "smooth",
      block: "nearest",
      inline: "nearest",
    });
  }, [revealId, reduceMotion]);

  return (
    <nav aria-label={t("settings.categoryRailLabel")} className="min-w-0">
      <LayoutGroup id="settings-category-nav">
        <ul
          ref={listRef}
          className="flex gap-1 overflow-x-auto [@container(min-width:56rem)]:flex-col [@container(min-width:56rem)]:overflow-visible"
        >
          {SETTINGS_CATEGORIES.map((category) => {
            const count = matchCounts?.get(category.id) ?? 0;
            const current = !searching && category.id === activeId;
            return (
              <li key={category.id} className="shrink-0">
                <button
                  type="button"
                  data-category={category.id}
                  aria-current={current ? "true" : undefined}
                  disabled={searching && count === 0}
                  onClick={() => onSelect(category.id)}
                  className="relative flex h-11 w-full items-center justify-between gap-3 whitespace-nowrap rounded-md px-3 text-left text-sm text-muted-foreground transition-colors hover:bg-muted/50 hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50 aria-[current=true]:font-medium aria-[current=true]:text-foreground sm:h-9"
                >
                  {current ? (
                    <SharedTabIndicator
                      layoutId="settings-category-indicator"
                      className="absolute inset-0 rounded-md bg-accent"
                    />
                  ) : null}
                  <span className="relative">{t(category.titleKey)}</span>
                  {searching ? (
                    <span className="relative tabular-nums text-muted-foreground">
                      <span aria-hidden="true">{count}</span>
                      <span className="sr-only">{t("settings.searchResultCount", { count })}</span>
                    </span>
                  ) : null}
                </button>
              </li>
            );
          })}
        </ul>
      </LayoutGroup>
    </nav>
  );
}
