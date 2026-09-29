import { useTranslation } from "react-i18next";
import { AnimatePresence } from "framer-motion";
import { Save } from "lucide-react";

import { Button } from "@/components/ui/button";
import { AnimatedButton, motion, transitions } from "@/components/ui/motion";
import { useReducedMotionPreference } from "@/hooks/use-reduced-motion";

interface SettingsSaveBarProps {
  open: boolean;
  saving: boolean;
  onDiscard: () => void;
  onSave: () => void;
}

/** Sticky unsaved-changes bar; the only primary action of the page (SSU-21). */
export function SettingsSaveBar({ open, saving, onDiscard, onSave }: SettingsSaveBarProps) {
  const { t } = useTranslation();
  const reduceMotion = useReducedMotionPreference();
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
            <p role="status" className="text-sm font-medium">
              {t("settings.unsavedChanges")}
            </p>
            <div className="flex items-center gap-2">
              <Button type="button" variant="outline" disabled={saving} onClick={onDiscard}>
                {t("settings.discardChanges")}
              </Button>
              <AnimatedButton>
                <Button type="button" disabled={saving} onClick={onSave}>
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
