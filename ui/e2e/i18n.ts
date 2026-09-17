import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

// the dashboard's own english catalog, so a spec asserts the copy the screen
// actually renders rather than a hand-copied string that drifts on the next
// rewording (#1504). read at runtime, not imported, so playwright's loader
// never has to agree with vite about json modules
const HERE = path.dirname(fileURLToPath(import.meta.url));
const CATALOG = JSON.parse(
  readFileSync(path.join(HERE, "../src/lib/i18n/locales/en.json"), "utf8"),
) as Record<string, unknown>;

/**
 * Resolve a dotted catalog key to its english string, interpolating
 * `{{placeholders}}` the way i18next does. Throws on a missing key so a renamed
 * key fails the spec loudly instead of matching nothing.
 */
export function t(key: string, vars: Record<string, string | number> = {}): string {
  let node: unknown = CATALOG;
  for (const part of key.split(".")) {
    node = node && typeof node === "object" ? (node as Record<string, unknown>)[part] : undefined;
  }
  if (typeof node !== "string") {
    throw new Error(`e2e i18n: no string at "${key}" in en.json`);
  }
  return node.replace(/\{\{\s*(\w+)\s*\}\}/g, (match, name: string) =>
    name in vars ? String(vars[name]) : match,
  );
}
