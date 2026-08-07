import { Check, Globe } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import {
  LOCALES,
  LOCALE_NAMES,
  LOCALE_SHORT,
  currentLocale,
  setLocale,
  type Locale,
} from "@/lib/i18n";
import { cn } from "@/lib/utils";

// language switcher pinned to the sidebar footer next to the version, so it is
// reachable from every screen without opening a menu (#489). switching swaps
// the catalog in place — react-i18next re-renders the tree, nothing reloads.
export function LocalePicker({ collapsed = false }: { collapsed?: boolean }) {
  // subscribes the component to language changes, so `currentLocale()` below
  // is re-read and the label repaints the moment the catalog swaps
  const { t } = useTranslation();
  const [open, setOpen] = React.useState(false);
  const ref = React.useRef<HTMLDivElement>(null);

  const active = currentLocale();

  // same dismiss contract as the user menu above it: outside click or escape
  React.useEffect(() => {
    if (!open) return;
    const onDoc = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    document.addEventListener("mousedown", onDoc);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDoc);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  const choose = (locale: Locale) => {
    setOpen(false);
    if (locale !== active) void setLocale(locale);
  };

  return (
    <div ref={ref} className="relative">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        title={t("locale.change")}
        aria-label={t("locale.change")}
        aria-haspopup="listbox"
        aria-expanded={open}
        className={cn(
          "flex items-center gap-1 rounded-md p-1.5 text-[color:var(--text-subtle)] transition-colors hover:bg-muted hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
          open && "bg-muted text-foreground",
        )}
      >
        <Globe aria-hidden className="h-4 w-4 flex-none" />
        {!collapsed && (
          <span className="font-mono text-[0.6875rem] leading-none">{LOCALE_SHORT[active]}</span>
        )}
      </button>
      {open && (
        <ul
          role="listbox"
          aria-label={t("locale.label")}
          className="absolute bottom-[calc(100%+6px)] left-0 z-40 min-w-[150px] rounded-lg border border-[color:var(--border-default)] bg-[color:var(--surface-elevated)] py-1 shadow-lg"
        >
          {LOCALES.map((locale) => {
            const selected = locale === active;
            return (
              <li key={locale}>
                <button
                  type="button"
                  role="option"
                  aria-selected={selected}
                  lang={locale}
                  onClick={() => choose(locale)}
                  className={cn(
                    "flex w-full items-center gap-2 px-3 py-1.5 text-left text-sm transition-colors hover:bg-[color:var(--surface-hover)]",
                    selected ? "text-foreground" : "text-[color:var(--text-secondary)]",
                  )}
                >
                  <span className="min-w-0 flex-1 truncate">{LOCALE_NAMES[locale]}</span>
                  {selected && (
                    <Check aria-hidden className="h-3.5 w-3.5 flex-none text-[color:var(--red-folk)]" />
                  )}
                </button>
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}
