import { Globe } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { Menu, MenuItemRadio } from "@/components/ui/menu";
import {
  LOCALES,
  LOCALE_NAMES,
  LOCALE_SHORT,
  currentLocale,
  setLocale,
  type Locale,
} from "@/lib/i18n";
import { useOptionalAuth } from "@/lib/auth";
import { useOptionalPreferences } from "@/lib/preferences";
import { errorDetail, useToast } from "@/lib/toast";
import { cn } from "@/lib/utils";

// language switcher pinned to the sidebar footer next to the version, so it is
// reachable from every screen without opening a menu (#489). switching swaps
// the catalog in place — react-i18next re-renders the tree, nothing reloads.
//
// the list is a `Menu` of `menuitemradio` entries, so its panel, its keyboard
// and its dismissal are the shared ones: on the folded rail it opens beside the
// strip like the account menu does, anywhere else above the button
export function LocalePicker({ collapsed = false }: { collapsed?: boolean }) {
  // subscribes the component to language changes, so `currentLocale()` below
  // is re-read and the label repaints the moment the catalog swaps
  const { t } = useTranslation();
  const [open, setOpen] = React.useState(false);
  const trigger = React.useRef<HTMLButtonElement>(null);

  const active = currentLocale();

  // a signed-in account keeps its language on the server (#2448), so the pick
  // is saved there too; the page switches at once either way
  const toast = useToast();
  const preferences = useOptionalPreferences();
  const signedIn = !!useOptionalAuth()?.token && !!preferences?.preferences;
  const choose = (locale: Locale) => {
    setOpen(false);
    trigger.current?.focus();
    if (locale === active) return;
    void setLocale(locale);
    if (!signedIn) return;
    preferences?.save({ language: locale }).catch((error: unknown) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: t("screens.preferences.title") }),
        detail: errorDetail(error),
      });
    });
  };

  return (
    <>
      <button
        ref={trigger}
        type="button"
        onClick={() => setOpen((v) => !v)}
        title={t("locale.change")}
        aria-label={t("locale.change")}
        aria-haspopup="menu"
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
      {open && trigger.current && (
        <Menu
          anchor={trigger.current}
          label={t("locale.label")}
          side={collapsed ? "right" : "above"}
          align={collapsed ? "end" : "start"}
          onClose={(restoreFocus) => {
            setOpen(false);
            if (restoreFocus) trigger.current?.focus();
          }}
        >
          {LOCALES.map((locale) => (
            <MenuItemRadio
              key={locale}
              lang={locale}
              checked={locale === active}
              onSelect={() => choose(locale)}
            >
              {LOCALE_NAMES[locale]}
            </MenuItemRadio>
          ))}
        </Menu>
      )}
    </>
  );
}
