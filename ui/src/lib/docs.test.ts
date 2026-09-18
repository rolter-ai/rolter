import { describe, expect, it } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { DOCS_PAGES, docsBaseUrl, docsUrl, normalizeDocsBase } from "./docs";

describe("normalizeDocsBase", () => {
  it("returns nothing for an unset or blank base", () => {
    expect(normalizeDocsBase(undefined)).toBe("");
    expect(normalizeDocsBase(null)).toBe("");
    expect(normalizeDocsBase("   ")).toBe("");
  });

  it("drops the trailing slash so the join is unambiguous", () => {
    expect(normalizeDocsBase("https://docs.example.com/")).toBe("https://docs.example.com");
    expect(normalizeDocsBase("https://example.com/docs///")).toBe("https://example.com/docs");
  });

  it("keeps a path prefix, for a site mounted under a subdirectory", () => {
    expect(normalizeDocsBase("https://intra.example.com/rolter/docs")).toBe(
      "https://intra.example.com/rolter/docs",
    );
  });

  it("accepts plain http, which is what an internal mirror usually serves", () => {
    expect(normalizeDocsBase("http://docs.internal:8080")).toBe("http://docs.internal:8080");
  });

  // an operator-supplied value lands in an href; a scheme that executes is a
  // click away from script running in the dashboard's own origin
  it("refuses a scheme that is not http(s)", () => {
    expect(normalizeDocsBase("javascript:alert(1)")).toBe("");
    expect(normalizeDocsBase("  JavaScript:alert(1)")).toBe("");
    expect(normalizeDocsBase("data:text/html,<script>alert(1)</script>")).toBe("");
    expect(normalizeDocsBase("file:///etc/passwd")).toBe("");
  });

  it("refuses something that is not a URL at all", () => {
    expect(normalizeDocsBase("docs.example.com")).toBe("");
    expect(normalizeDocsBase("/docs")).toBe("");
  });
});

describe("docsBaseUrl", () => {
  it("is empty when neither layer configured one", () => {
    expect(docsBaseUrl(undefined, "")).toBe("");
    expect(docsBaseUrl({}, "")).toBe("");
  });

  it("falls back to the build-time value when the runtime block is silent", () => {
    expect(docsBaseUrl({}, "https://built.example.com")).toBe("https://built.example.com");
  });

  it("lets the runtime block override the build", () => {
    expect(
      docsBaseUrl({ docsBaseUrl: "https://mirror.internal" }, "https://built.example.com"),
    ).toBe("https://mirror.internal");
  });

  // a deployment that set a broken override meant to replace the build-time
  // host; silently linking to the host it replaced would be worse than nothing
  it("does not fall back to the build when the override is unusable", () => {
    expect(docsBaseUrl({ docsBaseUrl: "javascript:alert(1)" }, "https://built.example.com")).toBe(
      "",
    );
  });
});

describe("docsUrl", () => {
  it("is null when docs are not configured, so no link is rendered", () => {
    expect(docsUrl(DOCS_PAGES.whichKey, {}, "")).toBeNull();
  });

  it("joins the base and the page with exactly one slash", () => {
    expect(docsUrl("security/which-key", {}, "https://docs.example.com")).toBe(
      "https://docs.example.com/security/which-key",
    );
    expect(docsUrl("/security/which-key", {}, "https://docs.example.com/")).toBe(
      "https://docs.example.com/security/which-key",
    );
  });

  it("keeps an anchor on the page path", () => {
    expect(docsUrl("security/which-key#virtual-keys", {}, "https://docs.example.com")).toBe(
      "https://docs.example.com/security/which-key#virtual-keys",
    );
  });

  it("returns the base itself for an empty page", () => {
    expect(docsUrl("", {}, "https://docs.example.com/")).toBe("https://docs.example.com");
  });
});

describe("DOCS_PAGES", () => {
  // these are site-relative paths out of docs/user-docs/docs.json, never URLs:
  // a hostname here would defeat the point of the whole module
  it("holds bare site-relative paths", () => {
    for (const path of Object.values(DOCS_PAGES)) {
      expect(path).not.toContain("://");
      expect(path.startsWith("/")).toBe(false);
      expect(path.endsWith(".mdx")).toBe(false);
    }
  });
});

// a path that no longer exists on the documentation site is a dead link in the
// dashboard, and nothing else would notice: the site and the SPA build apart
describe("documentation pages the dashboard links to", () => {
  /** every `pages` string anywhere in the Mintlify nav tree */
  function collectPages(node: unknown, out: Set<string> = new Set()): Set<string> {
    if (typeof node === "string") out.add(node);
    else if (Array.isArray(node)) for (const child of node) collectPages(child, out);
    else if (node && typeof node === "object")
      for (const [key, value] of Object.entries(node)) {
        if (key === "pages" || key === "groups" || key === "tabs" || key === "navigation")
          collectPages(value, out);
      }
    return out;
  }

  const docsJson = join(import.meta.dir, "..", "..", "..", "docs", "user-docs", "docs.json");
  const listed = collectPages(JSON.parse(readFileSync(docsJson, "utf8")));

  for (const [name, path] of Object.entries(DOCS_PAGES)) {
    it(`${name} points at a page the documentation site lists`, () => {
      expect(listed.has(path)).toBe(true);
    });
  }
});
