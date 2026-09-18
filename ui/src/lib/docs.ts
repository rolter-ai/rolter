/**
 * Where the user documentation lives, and how a screen links into it (#1164).
 *
 * Until now no screen could say "read more" and mean it: there is no canonical
 * documentation domain recorded anywhere in the repository (#1360), so a
 * hyperlink would have had to hard-code an invented hostname. #943 hit this
 * head-on — `security/which-key` exists as a page, and the provider, key and
 * playground screens could only inline the explanation.
 *
 * The base URL therefore comes from the deployment, in two layers:
 *
 *   1. build time — `VITE_DOCS_BASE_URL`, baked into the bundle. This is what
 *      a distribution build sets once a canonical domain exists.
 *   2. run time — `window.__ROLTER_CONFIG__.docsBaseUrl`, injected into the
 *      served HTML by the control plane (`--ui-docs-base-url`). Wins over the
 *      build-time value, because the SPA is built ahead of time and shipped as
 *      static assets: an operator mirroring the Mintlify site internally cannot
 *      rebuild it.
 *
 * Neither is required, and the default is neither. An air-gapped deployment
 * with no documentation host must degrade to *no link* rather than to a link
 * that 404s or reaches for a host the network will not resolve — so every
 * accessor below returns `null`/`""` when nothing is configured, and `DocsLink`
 * renders nothing at all.
 */
import type { RolterRuntimeConfig } from "@/lib/telemetry";

/**
 * The schemes a documentation base may use.
 *
 * The base URL is operator-supplied and ends up in an `href`, so it is checked
 * rather than trusted: `javascript:` in a config value that nothing validates
 * is a click away from script execution in the dashboard's own origin.
 */
const ALLOWED_PROTOCOLS = ["http:", "https:"];

/**
 * Normalise a configured base, or reject it.
 *
 * Returns `""` for anything unusable — unset, blank, not a URL, or a scheme
 * outside {@link ALLOWED_PROTOCOLS} — so a misconfiguration suppresses links
 * exactly the way an unconfigured deployment does. A trailing slash is dropped
 * here so `docsUrl` can join with exactly one.
 */
export function normalizeDocsBase(raw: string | undefined | null): string {
  const trimmed = (raw ?? "").trim();
  if (!trimmed) return "";
  let parsed: URL;
  try {
    parsed = new URL(trimmed);
  } catch {
    return "";
  }
  if (!ALLOWED_PROTOCOLS.includes(parsed.protocol)) return "";
  return parsed.href.replace(/\/+$/, "");
}

/** The build-time default, or `""` when the build set none. */
function buildTimeBase(): string {
  // typed loosely: `VITE_DOCS_BASE_URL` is optional, and a build that never
  // set it leaves `import.meta.env` without the key rather than with `""`
  const env = import.meta.env as unknown as Record<string, string | undefined>;
  return env.VITE_DOCS_BASE_URL ?? "";
}

/**
 * The documentation base URL this deployment should link to, or `""`.
 *
 * Both arguments are injectable so the unit tests and the stories can drive
 * every combination without touching globals.
 */
export function docsBaseUrl(
  config: RolterRuntimeConfig | undefined = typeof window === "undefined"
    ? undefined
    : window.__ROLTER_CONFIG__,
  fallback: string = buildTimeBase(),
): string {
  // the runtime value wins, but only when it survives normalisation: a
  // deployment that sets a broken override should not silently fall back to a
  // build-time host it explicitly meant to replace
  const injected = (config?.docsBaseUrl ?? "").trim();
  if (injected) return normalizeDocsBase(injected);
  return normalizeDocsBase(fallback);
}

/**
 * The absolute URL of a documentation page, or `null` when docs are not
 * configured.
 *
 * `page` is a site-relative path as it appears in `docs/user-docs/docs.json`
 * — `security/which-key`, with or without a leading slash, optionally with a
 * `#anchor`. It is never a hostname: that is the whole point of writing the
 * path once here instead of a URL at each call site.
 */
export function docsUrl(
  page: string,
  config?: RolterRuntimeConfig,
  fallback?: string,
): string | null {
  const base = docsBaseUrl(config, fallback);
  if (!base) return null;
  const path = page.trim().replace(/^\/+/, "");
  if (!path) return base;
  return `${base}/${path}`;
}

/**
 * Documentation pages the dashboard links to, by name.
 *
 * A path is written once, here, and referenced by key from the screens. A page
 * that is renamed in `docs.json` is then one edit away from being fixed
 * everywhere, instead of a grep for a string that reads like prose.
 */
export const DOCS_PAGES = {
  /** which of the three credentials a screen means (#943) */
  whichKey: "security/which-key",
  /** virtual keys: what they are, budgets, rotation */
  virtualKeys: "concepts/virtual-keys",
} as const;

export type DocsPage = keyof typeof DOCS_PAGES;
