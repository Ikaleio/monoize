import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { useReducedMotion } from "framer-motion";

import { TabsContent } from "@/components/ui/tabs";
import { motion, transitions } from "@/components/ui/motion";
import { categoryIndexLabel, type SettingsCategory } from "./settings-categories";

interface SettingsCategoryPanelProps {
  category: SettingsCategory;
  index: number;
  children: ReactNode;
}

/**
 * Shell for one settings category: asymmetric header band (index numeral +
 * oversized serif title on the left, description offset right on `lg`) above
 * the dense field-cluster content. No Card chrome per SSU-14.
 */
export function SettingsCategoryPanel({
  category,
  index,
  children,
}: SettingsCategoryPanelProps) {
  const { t } = useTranslation();
  const shouldReduceMotion = useReducedMotion();

  return (
    <TabsContent value={category.id} className="mt-0 min-w-0">
      <motion.div
        initial={shouldReduceMotion ? { opacity: 0 } : { opacity: 0, x: 8 }}
        animate={shouldReduceMotion ? { opacity: 1 } : { opacity: 1, x: 0 }}
        transition={transitions.normal}
        className="flex min-w-0 flex-col gap-8"
      >
        <header className="grid gap-3 lg:grid-cols-[minmax(0,7fr)_minmax(0,5fr)] lg:gap-12">
          <div className="flex min-w-0 items-baseline gap-3">
            <span
              aria-hidden="true"
              className="font-display text-lg leading-none text-muted-foreground/50"
            >
              {categoryIndexLabel(index)}
            </span>
            <h2 className="font-display text-3xl font-semibold tracking-tight text-balance sm:text-4xl">
              {t(category.titleKey)}
            </h2>
          </div>
          <p className="text-pretty text-sm leading-6 text-muted-foreground lg:pt-4">
            {t(category.descriptionKey)}
          </p>
        </header>
        <div className="flex min-w-0 flex-col gap-6">{children}</div>
      </motion.div>
    </TabsContent>
  );
}

interface SettingsGroupProps {
  label?: ReactNode;
  children: ReactNode;
}

/** Kicker-labeled subgroup inside a category panel. */
export function SettingsGroup({ label, children }: SettingsGroupProps) {
  return (
    <section className="flex min-w-0 flex-col gap-4">
      {label ? (
        <h3 className="text-xs font-medium uppercase tracking-wider text-muted-foreground">
          {label}
        </h3>
      ) : null}
      {children}
    </section>
  );
}
