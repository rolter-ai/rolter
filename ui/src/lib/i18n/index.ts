import i18n from "i18next";
import { initReactI18next } from "react-i18next";

import en from "./locales/en.json";

// dashboard localization (#489). everything ships inside the bundle — no CDN,
// no runtime fetch to a translation host — so the panel stays air-gapped.
// `en` is linked statically because it is the fallback and must always be
// resolvable; every other locale is a dynamic import, which vite emits as its
// own chunk and we only pay for once the user picks it.

export const LOCALES = ["en", "ru"] as const;
export type Locale = (typeof LOCALES)[number];

export const DEFAULT_LOCALE = "en" satisfies Locale;

// native names, deliberately identical in every catalog: a language is always
// listed in its own language so a lost user can find their way back
export const LOCALE_NAMES: Record<Locale, string> = {
  en: "English",
  ru: "Русский",
};

// short label for the compact sidebar switcher
export const LOCALE_SHORT: Record<Locale, string> = {
  en: "EN",
  ru: "RU",
};

const STORAGE_KEY = "rolter.locale";

const loaders: Record<Exclude<Locale, "en">, () => Promise<{ default: object }>> = {
  ru: () => import("./locales/ru.json"),
};

function isLocale(value: string | null | undefined): value is Locale {
  return !!value && (LOCALES as readonly string[]).includes(value);
}

// stored choice wins, then the browser's ordered preferences (matching on the
// primary subtag so `ru-RU` resolves to `ru`), then english
export function detectLocale(): Locale {
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (isLocale(stored)) return stored;
  } catch {
    // private-mode / disabled storage — fall through to browser detection
  }
  const preferences =
    typeof navigator === "undefined"
      ? []
      : (navigator.languages ?? [navigator.language]).filter(Boolean);
  for (const preference of preferences) {
    const primary = preference.toLowerCase().split("-")[0];
    if (isLocale(primary)) return primary;
  }
  return DEFAULT_LOCALE;
}

async function ensureLoaded(locale: Locale): Promise<void> {
  if (locale === DEFAULT_LOCALE || i18n.hasResourceBundle(locale, "translation")) return;
  const module = await loaders[locale]();
  i18n.addResourceBundle(locale, "translation", module.default, true, true);
}

// keeps `<html lang>` in step so screen readers switch voice and css
// `:lang()` rules resolve — cheap, and easy to forget
function syncDocumentLang(locale: Locale): void {
  if (typeof document !== "undefined") document.documentElement.lang = locale;
}

/** switch locale at runtime — loads the catalog on demand, no page reload */
export async function setLocale(locale: Locale): Promise<void> {
  await ensureLoaded(locale);
  await i18n.changeLanguage(locale);
  try {
    localStorage.setItem(STORAGE_KEY, locale);
  } catch {
    // choice simply does not survive the session when storage is unavailable
  }
  syncDocumentLang(locale);
}

export function currentLocale(): Locale {
  return isLocale(i18n.resolvedLanguage) ? i18n.resolvedLanguage : DEFAULT_LOCALE;
}

const initial = detectLocale();

void i18n.use(initReactI18next).init({
  resources: { en: { translation: en } },
  lng: initial,
  fallbackLng: DEFAULT_LOCALE,
  supportedLngs: LOCALES as unknown as string[],
  interpolation: {
    // react escapes for us; double-escaping mangles names with & or quotes
    escapeValue: false,
  },
  // catalogs are added synchronously before `changeLanguage` resolves, so no
  // component ever needs to suspend on a missing bundle
  react: { useSuspense: false },
});

syncDocumentLang(initial);

// the detected locale may itself be lazy (a ru-RU browser on first visit), so
// pull its catalog in right away — until it lands, `en` renders as fallback
if (initial !== DEFAULT_LOCALE) void setLocale(initial);

export default i18n;
