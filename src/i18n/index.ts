import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import LanguageDetector from "i18next-browser-languagedetector";

import en from "./locales/en.json";
import zh from "./locales/zh.json";

export const LANGS = [
  { code: "en", label: "English" },
  { code: "zh", label: "中文" },
] as const;

i18n
  .use(LanguageDetector)
  .use(initReactI18next)
  .init({
    resources: {
      en: { translation: en },
      zh: { translation: zh },
    },
    fallbackLng: "en",
    supportedLngs: ["en", "zh"],
    interpolation: { escapeValue: false },
    detection: {
      order: ["localStorage", "navigator"],
      caches: ["localStorage"],
      lookupLocalStorage: "lanbeam.lang",
    },
  });

// The page's language follows the app's (L27): <html lang> stayed "zh-CN"
// in English, so a screen reader read English text with Chinese rules.
const htmlLang = (lng: string) => (lng.startsWith("zh") ? "zh-CN" : "en");
document.documentElement.lang = htmlLang(i18n.language ?? "en");
i18n.on("languageChanged", (lng) => {
  document.documentElement.lang = htmlLang(lng);
});

export default i18n;
