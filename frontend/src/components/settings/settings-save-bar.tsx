import { useState } from "react";
import { useTranslation } from "react-i18next";
import { AnimatePresence } from "framer-motion";
import { Save } from "lucide-react";

import { Button } from "@/components/ui/button";
import { AnimatedButton, motion, transitions } from "@/components/ui/motion";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { useReducedMotionPreference } from "@/hooks/use-reduced-motion";
import type { SettingEntryId } from "./settings-categories";
import type { SettingChange } from "./settings-diff";

interface SettingsSaveBarProps {
  open: boolean;
  saving: boolean;
  /** Per-entry diff for the "view changes" popover; omitted by editors that have no setting entries. */
  changes?: SettingChange[];
  /** Why saving is blocked; `null` when the draft can be saved (SSU-21b). */
  blockedReason?: string | null;
  onDiscard: () => void;
  onSave: () => void;
  onJump?: (entryId: SettingEntryId) => void;
}

/** Sticky unsaved-changes bar; the only primary action of the page (SSU-21). */
export function SettingsSaveBar({
  open,
  saving,
  changes,
  blockedReason = null,
  onDiscard,
  onSave,
  onJump,
}: SettingsSaveBarProps) {
  const { t } = useTranslation();
  const reduceMotion = useReducedMotionPreference();
  const [changesOpen, setChangesOpen] = useState(false);
  const hidden = reduceMotion ? { opacity: 0 } : { opacity: 0, y: 8 };

  return (
    <AnimatePresence initial={false}>
      {open ? (
        <motion.div
          initial={hidden}
          animate={{ opacity: 1, y: 0 }}
          exit={hidden}
          transition={transitions.normal}
          className="sticky bottom-0 z-20 pt-2"
        >
          <div className="flex flex-wrap items-center justify-between gap-3 rounded-lg border bg-card px-4 py-3 shadow-sm">
            <div className="flex min-w-0 flex-col gap-1">
              <div className="flex flex-wrap items-center gap-2">
                <p role="status" className="text-sm font-medium">
                  {saving
                    ? t("common.saving")
                    : changes === undefined
                      ? t("settings.unsavedChanges")
                      : t("settings.unsavedChangesCount", { count: changes.length })}
                </p>
                {changes && changes.length > 0 && onJump ? (
                  <Popover open={changesOpen} onOpenChange={setChangesOpen}>
                    <PopoverTrigger asChild>
                      <Button type="button" variant="ghost" size="sm">
                        {t("settings.viewChanges")}
                      </Button>
                    </PopoverTrigger>
                    <PopoverContent
                      side="top"
                      align="start"
                      className="max-h-80 w-96 max-w-[calc(100vw-2rem)] overflow-y-auto p-1"
                    >
                      <ul className="flex flex-col">
                        {changes.map((change) => (
                          <li key={change.entryId}>
                            <button
                              type="button"
                              className="flex w-full flex-col gap-0.5 rounded-md px-3 py-2 text-left text-sm hover:bg-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
                              onClick={() => {
                                setChangesOpen(false);
                                onJump(change.entryId);
                              }}
                            >
                              <span className="font-medium">{change.label}</span>
                              <span className="break-words text-muted-foreground">
                                {change.oldValue} → {change.newValue}
                              </span>
                            </button>
                          </li>
                        ))}
                      </ul>
                    </PopoverContent>
                  </Popover>
                ) : null}
              </div>
              {blockedReason ? (
                <p className="text-sm text-error-foreground">{blockedReason}</p>
              ) : null}
            </div>
            <div className="flex items-center gap-2">
              <Button type="button" variant="outline" disabled={saving} onClick={onDiscard}>
                {t("settings.discardChanges")}
              </Button>
              <AnimatedButton>
                <Button
                  type="button"
                  disabled={saving || blockedReason !== null}
                  title={blockedReason ?? undefined}
                  onClick={onSave}
                >
                  <Save aria-hidden="true" />
                  {saving ? t("common.saving") : t("common.saveChanges")}
                </Button>
              </AnimatedButton>
            </div>
          </div>
        </motion.div>
      ) : null}
    </AnimatePresence>
  );
}
