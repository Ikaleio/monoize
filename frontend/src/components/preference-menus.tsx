import { useTranslation } from "react-i18next";
import { AnimatePresence, motion } from "framer-motion";
import { Languages, Moon, Sun } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { AnimatedButton } from "@/components/ui/motion";
import { useTheme } from "@/hooks/use-theme";
import { getCurrentLanguage, setLanguage, type SupportedLanguage } from "@/i18n";
import type { ThemePreference } from "@/lib/theme";

const LANGUAGES: SupportedLanguage[] = ["en", "zh", "zh-TW", "ja"];

const THEMES: ThemePreference[] = ["light", "dark", "system"];

const ICON_TRIGGER_CLASS = "size-11 touch-manipulation sm:size-9";

/** Radio items for every supported language; names are native in all locales. */
export function LanguageRadioGroup() {
  const { t } = useTranslation();
  return (
    <DropdownMenuRadioGroup
      value={getCurrentLanguage()}
      onValueChange={(value) => setLanguage(value as SupportedLanguage)}
    >
      {LANGUAGES.map((code) => (
        <DropdownMenuRadioItem key={code} value={code}>
          {t(`language.${code}`)}
        </DropdownMenuRadioItem>
      ))}
    </DropdownMenuRadioGroup>
  );
}

/**
 * Language dropdown. The default trigger is an outline button naming the current
 * language; `iconOnly` renders the compact ghost icon button used on the login page.
 */
export function LanguageDropdown({ iconOnly = false }: { iconOnly?: boolean }) {
  const { t } = useTranslation();
  const current = t(`language.${getCurrentLanguage()}`);
  const ariaLabel = `${t("language.switchLanguage")}: ${current}`;
  return (
    <DropdownMenu>
      {iconOnly ? (
        <AnimatedButton>
          <DropdownMenuTrigger asChild>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              className={ICON_TRIGGER_CLASS}
              title={t("language.switchLanguage")}
              aria-label={ariaLabel}
            >
              <Languages aria-hidden="true" />
            </Button>
          </DropdownMenuTrigger>
        </AnimatedButton>
      ) : (
        <DropdownMenuTrigger asChild>
          <Button type="button" variant="outline" aria-label={ariaLabel}>
            {current}
          </Button>
        </DropdownMenuTrigger>
      )}
      <DropdownMenuContent align="end">
        <LanguageRadioGroup />
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/**
 * Theme dropdown with light, dark and system options. The default trigger is an outline
 * button naming the preference; `iconOnly` renders the compact ghost icon button used on
 * the login page.
 */
export function ThemeDropdown({ iconOnly = false }: { iconOnly?: boolean }) {
  const { t } = useTranslation();
  const { theme, resolvedTheme, setTheme } = useTheme();
  const ariaLabel = `${t("theme.toggle")}: ${t(`theme.${theme}`)}`;
  return (
    <DropdownMenu>
      {iconOnly ? (
        <AnimatedButton>
          <DropdownMenuTrigger asChild>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              className={ICON_TRIGGER_CLASS}
              title={t("theme.toggle")}
              aria-label={ariaLabel}
            >
              <AnimatePresence mode="wait">
                <motion.div
                  key={resolvedTheme}
                  initial={{ rotate: -90, opacity: 0 }}
                  animate={{ rotate: 0, opacity: 1 }}
                  exit={{ rotate: 90, opacity: 0 }}
                  transition={{ duration: 0.15 }}
                >
                  {resolvedTheme === "dark" ? (
                    <Moon className="h-5 w-5" aria-hidden="true" />
                  ) : (
                    <Sun className="h-5 w-5" aria-hidden="true" />
                  )}
                </motion.div>
              </AnimatePresence>
            </Button>
          </DropdownMenuTrigger>
        </AnimatedButton>
      ) : (
        <DropdownMenuTrigger asChild>
          <Button type="button" variant="outline" aria-label={ariaLabel}>
            {t(`theme.${theme}`)}
          </Button>
        </DropdownMenuTrigger>
      )}
      <DropdownMenuContent align="end">
        <DropdownMenuRadioGroup
          value={theme}
          onValueChange={(value) => setTheme(value as ThemePreference)}
        >
          {THEMES.map((value) => (
            <DropdownMenuRadioItem key={value} value={value}>
              {t(`theme.${value}`)}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
