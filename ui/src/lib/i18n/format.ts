import { useTranslation } from "react-i18next";

import { DEFAULT_LOCALE, type Locale } from "./index";

// number/date/currency formatting bound to the active locale (#489). screens
// used bare `toLocaleString()` before, which silently follows the *browser*
// locale — so a ru-RU browser rendered russian separators inside an english
// panel. going through here keeps formatting and copy in the same language.

// Intl formatters are expensive to construct and immutable once built, so one
// per (locale, options) pair is cached for the life of the tab
const cache = new Map<string, Intl.NumberFormat | Intl.DateTimeFormat>();

function numberFormat(locale: string, options?: Intl.NumberFormatOptions): Intl.NumberFormat {
  const key = `n:${locale}:${JSON.stringify(options ?? {})}`;
  let formatter = cache.get(key) as Intl.NumberFormat | undefined;
  if (!formatter) {
    formatter = new Intl.NumberFormat(locale, options);
    cache.set(key, formatter);
  }
  return formatter;
}

function dateFormat(locale: string, options?: Intl.DateTimeFormatOptions): Intl.DateTimeFormat {
  const key = `d:${locale}:${JSON.stringify(options ?? {})}`;
  let formatter = cache.get(key) as Intl.DateTimeFormat | undefined;
  if (!formatter) {
    formatter = new Intl.DateTimeFormat(locale, options);
    cache.set(key, formatter);
  }
  return formatter;
}

export interface Formatters {
  locale: Locale;
  number: (value: number, options?: Intl.NumberFormatOptions) => string;
  /** money — defaults to USD, the currency the gateway prices in */
  currency: (value: number, currency?: string) => string;
  percent: (value: number, fractionDigits?: number) => string;
  date: (value: Date | string | number, options?: Intl.DateTimeFormatOptions) => string;
  /** compact date+time for log rows and audit trails */
  dateTime: (value: Date | string | number) => string;
}

function toDate(value: Date | string | number): Date {
  return value instanceof Date ? value : new Date(value);
}

export function formattersFor(locale: Locale): Formatters {
  return {
    locale,
    number: (value, options) => numberFormat(locale, options).format(value),
    currency: (value, currency = "USD") =>
      numberFormat(locale, {
        style: "currency",
        currency,
        // sub-cent gateway costs read as $0.00 without this
        maximumFractionDigits: Math.abs(value) < 1 ? 4 : 2,
      }).format(value),
    percent: (value, fractionDigits = 1) =>
      numberFormat(locale, {
        style: "percent",
        minimumFractionDigits: fractionDigits,
        maximumFractionDigits: fractionDigits,
      }).format(value),
    date: (value, options) => dateFormat(locale, options).format(toDate(value)),
    dateTime: (value) =>
      dateFormat(locale, {
        dateStyle: "short",
        timeStyle: "medium",
      }).format(toDate(value)),
  };
}

/** formatters for the locale currently rendered — re-derives on every switch */
export function useFormat(): Formatters {
  const { i18n } = useTranslation();
  const locale = (i18n.resolvedLanguage ?? DEFAULT_LOCALE) as Locale;
  return formattersFor(locale);
}
