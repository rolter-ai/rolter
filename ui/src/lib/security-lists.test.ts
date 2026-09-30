import { describe, expect, it } from "bun:test";
import { readFileSync } from "node:fs";
import { fileURLToPath, URL } from "node:url";

import {
  ORIGIN_FORBIDDEN,
  ROUTE_FORBIDDEN,
  ROUTE_PREFIX,
  entriesOf,
  listToText,
  normalizeRequiredHeaders,
  parseBypassRoutes,
  parseHeaderNames,
  parseLists,
  parseOrigins,
  parseRequiredHeaders,
  problemCount,
  requiredHeadersPayload,
  requiredHeadersToText,
} from "@/lib/security-lists";

const SECURITY_RS = fileURLToPath(
  new URL("../../../crates/rolter-control/src/security.rs", import.meta.url),
);

// the codes of the problems, for a compact assertion
const codes = (parsed: { problems: { code: string }[] }) => parsed.problems.map((p) => p.code);

describe("the control plane's rules, as the dashboard applies them", () => {
  // the character sets are copied by hand, so the Rust source is read back: a
  // rule widened or narrowed there fails here rather than in a toast
  const source = readFileSync(SECURITY_RS, "utf8");
  const charSets = [...source.matchAll(/\.contains\(\[((?:'(?:\\.|[^'])',?\s*)+)\]\)/g)].map(
    (match) =>
      [...match[1].matchAll(/'(\\.|[^'])'/g)].map((m) =>
        m[1] === "\\r" ? "\r" : m[1] === "\\n" ? "\n" : m[1],
      ),
  );

  it("refuses the same characters in an origin", () => {
    expect(charSets).toContainEqual([...ORIGIN_FORBIDDEN]);
  });

  it("refuses the same characters in a bypass route", () => {
    expect(charSets).toContainEqual([...ROUTE_FORBIDDEN]);
  });

  it("requires the same bypass prefix", () => {
    expect(source).toContain(`route.starts_with("${ROUTE_PREFIX}")`);
  });

  it("names both origin schemes, in lower case", () => {
    expect(source).toContain('origin.starts_with("https://")');
    expect(source).toContain('origin.starts_with("http://")');
  });
});

describe("allowed origins", () => {
  it("accepts an exact origin, with or without a port or a trailing slash", () => {
    const parsed = parseOrigins(
      "https://console.example.com\nhttp://localhost:5173\nhttps://app.example.com/",
    );
    expect(parsed.problems).toEqual([]);
    expect(parsed.entries).toEqual([
      "https://console.example.com",
      "http://localhost:5173",
      "https://app.example.com/",
    ]);
  });

  it("refuses a wildcard", () => {
    expect(codes(parseOrigins("https://*.example.com"))).toEqual(["originWildcard"]);
  });

  it("refuses a scheme that is not http or https, and a bare host", () => {
    expect(codes(parseOrigins("ftp://example.com\nexample.com"))).toEqual([
      "originScheme",
      "originScheme",
    ]);
  });

  it("reads the scheme as the control plane does, in lower case only", () => {
    expect(codes(parseOrigins("HTTPS://example.com"))).toEqual(["originScheme"]);
  });

  it("refuses a query, a fragment and user info", () => {
    expect(codes(parseOrigins("https://a.com?x=1\nhttps://a.com#top\nhttps://me@a.com"))).toEqual([
      "originPart",
      "originPart",
      "originPart",
    ]);
  });

  it("refuses an origin with a path, and one with no host", () => {
    expect(codes(parseOrigins("https://a.com/app\nhttps://\nhttp:///"))).toEqual([
      "originPath",
      "originHost",
      "originHost",
    ]);
  });

  it("refuses a space in the host", () => {
    expect(codes(parseOrigins("https://a b.com"))).toEqual(["originSpace"]);
  });

  it("points at the line and keeps the entry as typed", () => {
    const parsed = parseOrigins("https://ok.example.com\n\n  https://*.bad.example.com  ");
    expect(parsed.entries).toEqual(["https://ok.example.com"]);
    expect(parsed.problems).toEqual([
      { line: 3, entry: "https://*.bad.example.com", code: "originWildcard" },
    ]);
  });

  it("says when a line holds several origins, which is what the old form wrote", () => {
    const parsed = parseOrigins("https://a.example.com, https://b.example.com");
    expect(codes(parsed)).toEqual(["commaList"]);
  });

  it("ignores blank lines and either kind of line break", () => {
    const parsed = parseOrigins("https://a.com\r\n\r\nhttps://b.com\rhttps://c.com\n");
    expect(parsed.problems).toEqual([]);
    expect(parsed.entries).toEqual(["https://a.com", "https://b.com", "https://c.com"]);
  });
});

describe("allowed headers", () => {
  it("accepts any header name the http crate parses, punctuation included", () => {
    const parsed = parseHeaderNames("X-Stainless-Timeout\nx_request.id\nx!#$%&'*+-.^_`|~9");
    expect(parsed.problems).toEqual([]);
    expect(parsed.entries).toHaveLength(3);
  });

  it("refuses a space, a colon, a slash and a non-ascii letter", () => {
    expect(codes(parseHeaderNames("x tenant\nx-tenant:\nx/tenant\nх-tenant"))).toEqual([
      "headerName",
      "headerName",
      "headerName",
      "headerName",
    ]);
  });

  it("says when a line holds several names", () => {
    expect(codes(parseHeaderNames("x-a, x-b"))).toEqual(["commaList"]);
  });

  it("refuses a name longer than the http crate accepts", () => {
    expect(codes(parseHeaderNames("a".repeat(65_536)))).toEqual(["headerName"]);
    expect(parseHeaderNames("a".repeat(65_535)).problems).toEqual([]);
  });
});

describe("required headers", () => {
  it("splits at the first colon, so a value may hold commas and colons", () => {
    const parsed = parseRequiredHeaders("x-trace: a,b\nx-callback: https://a.example.com:8443/x");
    expect(parsed.problems).toEqual([]);
    expect(parsed.entries).toEqual([
      { name: "x-trace", value: "a,b" },
      { name: "x-callback", value: "https://a.example.com:8443/x" },
    ]);
  });

  it("trims the space around the name and the value", () => {
    expect(parseRequiredHeaders("  x-tenant  :   acme  ").entries).toEqual([
      { name: "x-tenant", value: "acme" },
    ]);
  });

  it("refuses a pair with no colon instead of dropping it", () => {
    const parsed = parseRequiredHeaders("x-tenant=acme\nx-tenant acme");
    expect(parsed.entries).toEqual([]);
    expect(parsed.problems).toEqual([
      { line: 1, entry: "x-tenant=acme", code: "requiredFormat" },
      { line: 2, entry: "x-tenant acme", code: "requiredFormat" },
    ]);
  });

  it("names the bad part when the name is what is wrong", () => {
    const parsed = parseRequiredHeaders("x tenant: acme\n: acme");
    expect(parsed.problems).toEqual([
      { line: 1, entry: "x tenant", code: "headerName" },
      { line: 2, entry: ": acme", code: "headerName" },
    ]);
  });

  it("refuses an empty value", () => {
    expect(codes(parseRequiredHeaders("x-tenant:\nx-other:   "))).toEqual([
      "requiredValue",
      "requiredValue",
    ]);
  });

  it("refuses a header named twice, in any case, rather than keep one of them", () => {
    const parsed = parseRequiredHeaders("x-tenant: a\nX-Tenant: b\nx-other: c");
    expect(parsed.entries).toEqual([
      { name: "x-tenant", value: "a" },
      { name: "x-other", value: "c" },
    ]);
    expect(parsed.problems).toEqual([{ line: 2, entry: "X-Tenant", code: "requiredDuplicate" }]);
  });

  it("says when a line holds several names", () => {
    expect(codes(parseRequiredHeaders("x-a, x-b: 1"))).toEqual(["commaList"]);
  });

  it("builds the object the control plane expects", () => {
    const { entries } = parseRequiredHeaders("x-tenant: acme\nx-trace: a,b");
    expect(requiredHeadersPayload(entries)).toEqual({ "x-tenant": "acme", "x-trace": "a,b" });
  });
});

describe("bypass routes", () => {
  it("accepts an exact path under /v1/", () => {
    const parsed = parseBypassRoutes("/v1/models\n/v1/ping");
    expect(parsed.problems).toEqual([]);
    expect(parsed.entries).toEqual(["/v1/models", "/v1/ping"]);
  });

  it("refuses a path outside /v1/, the control plane's own routes included", () => {
    expect(codes(parseBypassRoutes("/admin/providers\n/healthz\n/v1\nv1/models"))).toEqual([
      "routePrefix",
      "routePrefix",
      "routePrefix",
      "routePrefix",
    ]);
  });

  it("refuses a wildcard, a placeholder, a query and a fragment", () => {
    expect(codes(parseBypassRoutes("/v1/*\n/v1/{id}\n/v1/models?x=1\n/v1/models#a"))).toEqual([
      "routePattern",
      "routePattern",
      "routePattern",
      "routePattern",
    ]);
  });

  it("refuses a space inside the path", () => {
    expect(codes(parseBypassRoutes("/v1/my models"))).toEqual(["routeSpace"]);
  });

  it("says when a line holds several paths", () => {
    expect(codes(parseBypassRoutes("/v1/models, /v1/ping"))).toEqual(["commaList"]);
  });
});

describe("loading what is stored", () => {
  it("shows every stored entry on its own line, nothing lost", () => {
    const stored = {
      origins: ["https://app.example.com", "https://console.example.com"],
      headers: ["x-request-id", "X-Stainless-Timeout"],
      required: { "x-tenant": "acme", "x-trace": "a,b,c" },
      routes: ["/v1/models", "/healthz"],
    };
    const lists = {
      allowedOrigins: listToText(stored.origins),
      allowedHeaders: listToText(stored.headers),
      requiredHeaders: requiredHeadersToText(stored.required),
      bypassRoutes: listToText(stored.routes),
    };
    expect(lists.requiredHeaders).toBe("x-tenant: acme\nx-trace: a,b,c");
    const parsed = parseLists(lists);
    expect(parsed.allowedOrigins.entries).toEqual(stored.origins);
    expect(parsed.allowedHeaders.entries).toEqual(stored.headers);
    expect(requiredHeadersPayload(parsed.requiredHeaders.entries)).toEqual(stored.required);
    // a stored route the rules now refuse is kept in the field and named, not dropped
    expect(parsed.bypassRoutes.entries).toEqual(["/v1/models"]);
    expect(parsed.bypassRoutes.problems).toEqual([
      { line: 2, entry: "/healthz", code: "routePrefix" },
    ]);
    expect(entriesOf(lists.bypassRoutes)).toEqual(stored.routes);
  });

  it("reads a missing list and a missing map as empty", () => {
    expect(listToText(undefined)).toBe("");
    expect(listToText(null)).toBe("");
    expect(requiredHeadersToText(undefined)).toBe("");
    expect(requiredHeadersToText(null)).toBe("");
  });
});

describe("comparing a field with what was loaded", () => {
  it("lists the non-blank lines, trimmed", () => {
    expect(entriesOf("  a \n\n b\r\n")).toEqual(["a", "b"]);
  });

  it("treats the space around a required header's colon as no change", () => {
    expect(normalizeRequiredHeaders("x-tenant:acme\n\n  x-trace :  a,b ")).toBe(
      "x-tenant: acme\nx-trace: a,b",
    );
    expect(normalizeRequiredHeaders("x-tenant=acme")).toBe("x-tenant=acme");
  });
});

describe("counting problems", () => {
  it("adds them up across the four lists", () => {
    const parsed = parseLists({
      allowedOrigins: "https://*.a.com",
      allowedHeaders: "x a\nx b",
      requiredHeaders: "no colon",
      bypassRoutes: "/v1/ok",
    });
    expect(problemCount(parsed)).toBe(4);
  });

  it("is zero for a clean form", () => {
    expect(
      problemCount(
        parseLists({
          allowedOrigins: "",
          allowedHeaders: "",
          requiredHeaders: "",
          bypassRoutes: "",
        }),
      ),
    ).toBe(0);
  });
});
