import { useEffect } from "react";

/**
 * The product's name as the browser tab carries it.
 *
 * Lowercase in every locale, including at the start of the tab title: the brand
 * guidelines (`docs/user-docs/community/brand.mdx`) fix the wordmark's case, and
 * a catalog is never asked to translate it.
 */
const BRAND = "rolter";

/** `{title} · rolter`, or the bare name while a screen has no title to give. */
export function documentTitle(title: string): string {
  return title ? `${title} · ${BRAND}` : BRAND;
}

/**
 * Name the browser tab after the screen on display (#2002).
 *
 * Every screen shared the one `<title>` in `index.html`, so tabs, history
 * entries and a screen reader's page announcement could not tell Keys from LLM
 * Logs (WCAG 2.4.2). `title` is the caller's already-translated screen title,
 * so a locale switch re-renders the caller with the new string and the tab
 * follows it without a reload.
 *
 * The shell's `Screen` calls this for every routed leaf, the forbidden state
 * included; `Login` and `AcceptInvite` render outside it and call it
 * themselves.
 */
export function useDocumentTitle(title: string): void {
  useEffect(() => {
    document.title = documentTitle(title);
  }, [title]);
}
