import { describe, expect, it } from "bun:test";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

import { KNOWN_ERROR_CODES } from "../src/lib/error-copy";
import { SSO_FAILURES, SSO_SHARED_NAMES } from "../src/lib/sso-failures";

// a single sign-on failure reaches the dashboard two ways: as `?sso_error=` on
// the redirect back to the login page and as `error.code` to a JSON caller
// (#3468). both word it through the one entry `errors.api.codes.<code>`
// (#3524), so this holds the dashboard's list to the control plane's
// `SsoFailure::code` and to every catalog. `ui/` builds on its own, but this
// is a development check, so it reads the rust source where it lives
const UI = join(import.meta.dir, "..");
const SSO_RS = join(UI, "..", "crates", "rolter-control", "src", "sso.rs");
const LOCALES = join(UI, "src", "lib", "i18n", "locales");

/** the codes the arms of `SsoFailure::code` return, in declaration order */
function sentCodes(source: string): string[] {
  const start = source.indexOf("const fn code(self) -> &'static str {");
  if (start < 0) return [];
  const end = source.indexOf("\n    }\n", start);
  const body = source.slice(start, end);
  return [...body.matchAll(/Self::\w+ => "([a-z_]+)"/g)].map((m) => m[1]);
}

type Catalog = {
  errors: { api: { codes: Record<string, unknown> } };
  auth: { sso: Record<string, unknown> };
};
const catalogs = readdirSync(LOCALES)
  .filter((file) => file.endsWith(".json"))
  .map((file) => ({
    locale: file.replace(/\.json$/, ""),
    catalog: JSON.parse(readFileSync(join(LOCALES, file), "utf8")) as Catalog,
  }));

describe("single sign-on failure codes", () => {
  it("reads the arms of the code match", () => {
    const source = [
      "    const fn code(self) -> &'static str {",
      "        match self {",
      '            Self::IdpError => "idp_error",',
      '            Self::InternalError => "internal_error",',
      "        }",
      "    }",
      "",
      "    const fn message(self) -> &'static str {",
      '            Self::IdpError => "the identity provider refused the sign-in",',
    ].join("\n");
    expect(sentCodes(source)).toEqual(["idp_error", "internal_error"]);
  });

  it("are every code the control plane sends, and no other", () => {
    const sent = sentCodes(readFileSync(SSO_RS, "utf8"));
    expect(sent.length).toBeGreaterThan(0);
    expect(
      [...(SSO_FAILURES as readonly string[])],
      "ui/src/lib/sso-failures.ts differs from SsoFailure::code in sso.rs",
    ).toEqual(sent);
  });

  it("are known error codes, so the JSON path words them too", () => {
    const known = new Set<string>(KNOWN_ERROR_CODES);
    const shared = new Set<string>(SSO_SHARED_NAMES);
    // every code but the names other endpoints share is translated wherever
    // it arrives; a shared name stays out, and stays an SSO code
    expect(SSO_FAILURES.filter((code) => !known.has(code) && !shared.has(code))).toEqual([]);
    expect(SSO_SHARED_NAMES.filter((code) => known.has(code))).toEqual([]);
    expect(
      SSO_SHARED_NAMES.filter((code) => !(SSO_FAILURES as readonly string[]).includes(code)),
    ).toEqual([]);
  });

  it("are worded once, under errors.api.codes, in every catalog", () => {
    expect(catalogs.length).toBeGreaterThan(1);
    for (const { locale, catalog } of catalogs) {
      const codes = catalog.errors.api.codes;
      const missing = SSO_FAILURES.filter((code) => {
        const text = codes[code];
        return typeof text !== "string" || text.trim() === "";
      });
      expect(missing, `${locale}: errors.api.codes lacks`).toEqual([]);
      // a second home for the same copy is how the two paths drifted
      expect(catalog.auth.sso.errors, `${locale}: auth.sso.errors is back`).toBeUndefined();
    }
  });
});
