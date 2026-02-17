import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import en from "./locales/en.json";
import zh from "./locales/zh.json";
import ja from "./locales/ja.json";
import zhTW from "./locales/zh-TW.json";

export type SupportedLanguage = "en" | "zh" | "ja" | "zh-TW";

const LANGUAGE_KEY = "urp_language";
const SUPPORTED_LANGUAGES: SupportedLanguage[] = ["en", "zh", "ja", "zh-TW"];

function getInitialLanguage(): SupportedLanguage {
  const saved = localStorage.getItem(LANGUAGE_KEY);
  if (saved && SUPPORTED_LANGUAGES.includes(saved as SupportedLanguage)) {
    return saved as SupportedLanguage;
  }

  const browserLang = navigator.language;
  // zh-TW, zh-HK → zh-TW; zh, zh-CN → zh
  if (browserLang.startsWith("zh")) {
    return browserLang === "zh-TW" || browserLang === "zh-HK" ? "zh-TW" : "zh";
  }
  if (browserLang.startsWith("ja")) {
    return "ja";
  }
  return "en";
}

i18n.use(initReactI18next).init({
  resources: {
    en: { translation: en },
    zh: { translation: zh },
    ja: { translation: ja },
    "zh-TW": { translation: zhTW },
  },
  lng: getInitialLanguage(),
  fallbackLng: "en",
  interpolation: {
    escapeValue: false,
  },
});

i18n.on("languageChanged", (lng) => {
  localStorage.setItem(LANGUAGE_KEY, lng);
});

export default i18n;

export function setLanguage(lang: SupportedLanguage) {
  i18n.changeLanguage(lang);
}

export function getCurrentLanguage(): SupportedLanguage {
  return i18n.language as SupportedLanguage;
}

export function toggleLanguage() {
  const current = getCurrentLanguage();
  const idx = SUPPORTED_LANGUAGES.indexOf(current);
  const next = SUPPORTED_LANGUAGES[(idx + 1) % SUPPORTED_LANGUAGES.length];
  setLanguage(next);
}
