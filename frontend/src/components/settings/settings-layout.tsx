import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";

import { Card } from "@/components/ui/card";
import { Label } from "@/components/ui/label";
import { cn } from "@/lib/utils";
import {
  SETTINGS_GROUPS,
  getSettingEntry,
  type SettingEntryId,
  type SettingsCategory,
  type SettingsGroup as SettingsGroupModel,
  type SettingsGroupId,
} from "./settings-categories";
import { useEntryVisible, useGroupVisible } from "./settings-search";

interface SettingsBodyProps {
  /** Search input and category navigation. */
  aside: ReactNode;
  children: ReactNode;
}

/**
 * Settings body geometry (SSU-5): a sticky 13rem left column beside the content from
 * a 56rem container width; one stacked column below it.
 */
export function SettingsBody({ aside, children }: SettingsBodyProps) {
  return (
    <div className="min-w-0 [container-type:inline-size]">
      <div className="flex flex-col gap-6 [@container(min-width:56rem)]:grid [@container(min-width:56rem)]:grid-cols-[13rem_minmax(0,1fr)] [@container(min-width:56rem)]:items-start [@container(min-width:56rem)]:gap-10">
        <div className="flex min-w-0 flex-col gap-3 [@container(min-width:56rem)]:sticky [@container(min-width:56rem)]:top-0">
          {aside}
        </div>
        <div className="flex min-w-0 flex-col gap-6">{children}</div>
      </div>
    </div>
  );
}

interface SettingsCategorySectionProps {
  category: SettingsCategory;
  children: ReactNode;
}

/** One category: plain `h2` title and description above its groups (SSU-12). */
export function SettingsCategorySection({ category, children }: SettingsCategorySectionProps) {
  const { t } = useTranslation();
  const headingId = `settings-category-${category.id}-title`;

  return (
    <section
      id={`settings-category-${category.id}`}
      aria-labelledby={headingId}
      className="flex min-w-0 flex-col gap-5"
    >
      <header className="flex flex-col gap-1">
        <h2 id={headingId} className="text-lg font-semibold tracking-tight">
          {t(category.titleKey)}
        </h2>
        <p className="text-pretty text-sm text-muted-foreground">{t(category.descriptionKey)}</p>
      </header>
      {children}
    </section>
  );
}

interface SettingsGroupProps {
  id: SettingsGroupId;
  /** Help paragraph rendered below the group body. */
  footer?: ReactNode;
  children: ReactNode;
}

/**
 * Row groups put their rows on one `Card`; editor groups render the editor bare
 * because every editor already draws its own border (SSU-13).
 */
export function SettingsGroup({ id, footer, children }: SettingsGroupProps) {
  const { t } = useTranslation();
  const visible = useGroupVisible(id);
  if (!visible) return null;

  const group: SettingsGroupModel = SETTINGS_GROUPS[id];
  return (
    <div className="flex min-w-0 flex-col gap-3">
      {group.titleKey ? <h3 className="text-sm font-semibold">{t(group.titleKey)}</h3> : null}
      {group.editor ? (
        children
      ) : (
        <Card className="divide-y overflow-clip [container-type:inline-size]">{children}</Card>
      )}
      {footer ? <p className="text-pretty text-sm text-muted-foreground">{footer}</p> : null}
    </div>
  );
}

interface SettingRowProps {
  id: SettingEntryId;
  /** Switch rows keep the switch beside the label at every width. */
  switchControl?: boolean;
  children: ReactNode;
}

/**
 * One labelled setting inside a row group. The control must use `id` so the label
 * binds to it. Label and description come from the entry model (SSU-1b, SSU-14).
 */
export function SettingRow({ id, switchControl = false, children }: SettingRowProps) {
  const { t } = useTranslation();
  const visible = useEntryVisible(id);
  if (!visible) return null;

  const entry = getSettingEntry(id);
  return (
    <div
      id={`setting-${id}`}
      className={cn(
        "px-4 py-4 sm:px-6",
        switchControl
          ? "flex items-start justify-between gap-6"
          : "grid gap-3 [@container(min-width:40rem)]:grid-cols-[minmax(0,1fr)_20rem] [@container(min-width:40rem)]:gap-x-8"
      )}
    >
      {/* In two-column rows, pt-2 centers the 20px label line on the 36px control. */}
      <div
        className={cn(
          "flex min-w-0 flex-col gap-1.5",
          !switchControl && "[@container(min-width:40rem)]:pt-2"
        )}
      >
        <Label htmlFor={id} className={cn("leading-5", entry.rawLabel && "font-mono font-normal")}>
          {entry.labelKey ? t(entry.labelKey) : entry.rawLabel}
        </Label>
        {entry.descriptionKey ? (
          <p className="text-pretty text-sm text-muted-foreground">{t(entry.descriptionKey)}</p>
        ) : null}
      </div>
      <div className={cn("min-w-0", switchControl && "shrink-0 pt-0.5")}>{children}</div>
    </div>
  );
}

interface SettingBlockProps {
  id: SettingEntryId;
  children: ReactNode;
}

/** The single entry of an editor group. */
export function SettingBlock({ id, children }: SettingBlockProps) {
  const visible = useEntryVisible(id);
  if (!visible) return null;

  return (
    <div id={`setting-${id}`} className="min-w-0">
      {children}
    </div>
  );
}
