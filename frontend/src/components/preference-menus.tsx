import { useTranslation } from "react-i18next";
import { ChevronDown, Languages, Monitor, Moon, Sun } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useTheme } from "@/hooks/use-theme";
import { getCurrentLanguage, setLanguage, type SupportedLanguage } from "@/i18n";
import type { ThemePreference } from "@/lib/theme";

const LANGUAGES: SupportedLanguage[] = ["en", "zh", "zh-TW", "ja"];

const THEMES = [
  { value: "light", icon: Sun },
  { value: "dark", icon: Moon },
  { value: "system", icon: Monitor },
] as const;

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

/** Three-state theme radio items, reachable with the menu's arrow-key navigation. */
export function ThemeRadioGroup({ keepOpen = false }: { keepOpen?: boolean }) {
  const { t } = useTranslation();
  const { theme, setTheme } = useTheme();
  return (
    <DropdownMenuRadioGroup
      value={theme}
      onValueChange={(value) => setTheme(value as ThemePreference)}
    >
      {THEMES.map(({ value, icon: Icon }) => (
        <DropdownMenuRadioItem
          key={value}
          value={value}
          onSelect={keepOpen ? (event) => event.preventDefault() : undefined}
        >
          <Icon data-icon="inline-start" aria-hidden="true" />
          {t(`theme.${value}`)}
        </DropdownMenuRadioItem>
      ))}
    </DropdownMenuRadioGroup>
  );
}

/** Language dropdown whose trigger names the current language. */
export function LanguageDropdown({
  variant = "outline",
  className,
}: {
  variant?: "outline" | "ghost";
  className?: string;
}) {
  const { t } = useTranslation();
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          type="button"
          variant={variant}
          className={className}
          aria-label={`${t("language.switchLanguage")}: ${t(`language.${getCurrentLanguage()}`)}`}
        >
          <Languages data-icon="inline-start" aria-hidden="true" />
          {t(`language.${getCurrentLanguage()}`)}
          <ChevronDown data-icon="inline-end" aria-hidden="true" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <LanguageRadioGroup />
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** Theme dropdown whose trigger shows the current preference. */
export function ThemeDropdown({
  variant = "outline",
  className,
}: {
  variant?: "outline" | "ghost";
  className?: string;
}) {
  const { t } = useTranslation();
  const { theme } = useTheme();
  const Icon = THEMES.find((item) => item.value === theme)?.icon ?? Monitor;
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          type="button"
          variant={variant}
          className={className}
          aria-label={`${t("theme.toggle")}: ${t(`theme.${theme}`)}`}
        >
          <Icon data-icon="inline-start" aria-hidden="true" />
          {t(`theme.${theme}`)}
          <ChevronDown data-icon="inline-end" aria-hidden="true" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <ThemeRadioGroup />
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
