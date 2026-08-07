/* eslint-disable import/no-extraneous-dependencies */
import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import LanguageDetector from "i18next-browser-languagedetector";

// Import translation resources
import enTranslation from "./locales/en/translation.json";
import zhTranslation from "./locales/zh/translation.json";

// Configure i18next instance
i18n
  // Detect user language
  .use(LanguageDetector)
  // Pass the i18n instance to react-i18next
  .use(initReactI18next)
  // Initialize i18next
  .init({
    // Default language when detection fails
    fallbackLng: "en",
    // Debug mode for development
    debug: process.env.NODE_ENV === "development",
    // Namespaces configuration
    defaultNS: "translation",
    ns: ["translation"],
    // Resources containing translations
    resources: {
      en: {
        translation: enTranslation,
      },
      zh: {
        translation: zhTranslation,
      },
    },
    // Language detection options
    detection: {
      // Read the legacy key, but persist only after an explicit settings Save.
      order: ["localStorage", "navigator"],
      caches: [],
      htmlTag: document.documentElement,
    },
    // Escaping special characters
    interpolation: {
      escapeValue: false, // React already safes from XSS
    },
    // React config
    react: {
      useSuspense: true,
    },
  });

let languagePreviewDepth = 0;

// Persist committed language changes and keep previews in memory only.
i18n.on("languageChanged", (lng) => {
  if (languagePreviewDepth === 0) localStorage.setItem("i18nextLng", lng);
  document.documentElement.lang = lng;
});

export async function previewLanguage(language: string): Promise<void> {
  if (language === i18n.language) return;
  languagePreviewDepth += 1;
  try {
    await i18n.changeLanguage(language);
  } finally {
    languagePreviewDepth -= 1;
  }
}

export async function commitLanguage(language: string): Promise<void> {
  localStorage.setItem("i18nextLng", language);
  document.documentElement.lang = language;
  if (language !== i18n.language) await i18n.changeLanguage(language);
}

export default i18n;
