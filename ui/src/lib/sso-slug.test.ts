import { describe, expect, it } from "bun:test";
import { readFileSync } from "node:fs";
import { fileURLToPath, URL } from "node:url";

import { SSO_SLUG_MAX, SSO_SLUG_PATTERN, ssoSlugProblem, suggestSsoSlug } from "./sso-slug";

const MIGRATION = fileURLToPath(
  new URL("../../../crates/rolter-store/migrations/0047_sso_providers.sql", import.meta.url),
);

// the same table `sso_slug_outside_the_charset_is_a_400_that_states_the_rule` runs
// against the store and the endpoint in crates/rolter-control/tests, so the
// dashboard, the handler and the constraint are all checked on the same values
const ACCEPTED = [
  "okta",
  "a",
  "0",
  "9lives",
  "entra-staging",
  "a--b",
  "okta-",
  "a".repeat(SSO_SLUG_MAX),
];

const REFUSED: [slug: string, problem: "empty" | "charset" | "length"][] = [
  ["", "empty"],
  ["Okta", "charset"],
  ["acme okta", "charset"],
  ["-okta", "charset"],
  ["okta_prod", "charset"],
  ["okta.prod", "charset"],
  [" okta", "charset"],
  ["okta ", "charset"],
  ["okta\n", "charset"],
  // a Cyrillic "о", which looks like the Latin one
  ["оkta", "charset"],
  ["résumé", "charset"],
  ["a".repeat(SSO_SLUG_MAX + 1), "length"],
];

describe("ssoSlugProblem", () => {
  it("accepts what the store accepts", () => {
    for (const slug of ACCEPTED) expect(ssoSlugProblem(slug)).toBeNull();
  });

  it("names why a slug is refused", () => {
    for (const [slug, problem] of REFUSED) {
      expect({ slug, problem: ssoSlugProblem(slug) }).toEqual({ slug, problem });
    }
  });

  it("reads the slug exactly as typed, without trimming or folding case", () => {
    expect(ssoSlugProblem("okta ")).toBe("charset");
    expect(ssoSlugProblem("OKTA")).toBe("charset");
  });

  it("puts a bad character ahead of the length", () => {
    // the suggestion fixes a charset problem; a length problem needs a decision
    expect(ssoSlugProblem("A".repeat(SSO_SLUG_MAX + 1))).toBe("charset");
  });

  it("agrees with the constraint in the migration", () => {
    const sql = readFileSync(MIGRATION, "utf8");
    const rule = /constraint sso_providers_slug_charset check \(slug ~ '([^']+)'\)/.exec(sql)?.[1];
    expect(rule).toBeDefined();
    expect(SSO_SLUG_PATTERN.source).toBe(rule ?? "");
    // and the longest slug it lets through is the one the limit names
    expect(rule).toContain(`{0,${SSO_SLUG_MAX - 1}}`);
  });
});

describe("suggestSsoSlug", () => {
  it("lowers case and joins words with a hyphen", () => {
    expect(suggestSsoSlug("Okta")).toBe("okta");
    expect(suggestSsoSlug("Acme Okta")).toBe("acme-okta");
    expect(suggestSsoSlug("okta_prod")).toBe("okta-prod");
    expect(suggestSsoSlug("okta.prod")).toBe("okta-prod");
  });

  it("folds accents to the base letter", () => {
    expect(suggestSsoSlug("Café")).toBe("cafe");
  });

  it("collapses runs and trims the ends", () => {
    expect(suggestSsoSlug("  Acme   --  Okta  ")).toBe("acme-okta");
    expect(suggestSsoSlug("-okta")).toBe("okta");
  });

  it("keeps a long value inside the limit without ending on a hyphen", () => {
    const suggestion = suggestSsoSlug(`${"a".repeat(SSO_SLUG_MAX - 1)} b`);
    expect(suggestion).toBe("a".repeat(SSO_SLUG_MAX - 1));
    expect(ssoSlugProblem(suggestion ?? "")).toBeNull();
  });

  it("offers nothing when nothing is left to offer", () => {
    expect(suggestSsoSlug("日本語")).toBeNull();
    expect(suggestSsoSlug("---")).toBeNull();
    expect(suggestSsoSlug("")).toBeNull();
  });

  it("offers nothing for a slug that is already fine", () => {
    expect(suggestSsoSlug("okta")).toBeNull();
  });

  it("only ever offers a slug the server accepts", () => {
    for (const [slug] of REFUSED) {
      const suggestion = suggestSsoSlug(slug);
      if (suggestion !== null) expect(ssoSlugProblem(suggestion)).toBeNull();
    }
  });
});
