export type ThemePreference = "light" | "dark" | "system";
export type ResolvedTheme = "light" | "dark";

export const THEME_STORAGE_KEY = "monoize-theme";

export function getStoredThemePreference(storage: Storage | null | undefined): ThemePreference {
  const stored = storage?.getItem(THEME_STORAGE_KEY);
  return stored === "light" || stored === "dark" || stored === "system" ? stored : "system";
}

export function getSystemTheme(windowObject: Window | null | undefined): ResolvedTheme {
  if (!windowObject) {
    return "light";
  }

  return windowObject.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

export function resolveThemePreference(
  theme: ThemePreference,
  windowObject: Window | null | undefined,
): ResolvedTheme {
  return theme === "system" ? getSystemTheme(windowObject) : theme;
}

export function applyResolvedTheme(
  root: HTMLElement,
  resolvedTheme: ResolvedTheme,
): ResolvedTheme {
  root.classList.remove("light", "dark");
  root.classList.add(resolvedTheme);
  root.style.colorScheme = resolvedTheme;
  return resolvedTheme;
}

export function applyInitialTheme(windowObject: Window | null | undefined = window): ResolvedTheme {
  if (!windowObject) {
    return "light";
  }

  const theme = getStoredThemePreference(windowObject.localStorage);
  const resolvedTheme = resolveThemePreference(theme, windowObject);
  return applyResolvedTheme(windowObject.document.documentElement, resolvedTheme);
}
