/**
 * The four list fields on the Security screen, and the rules the control plane
 * applies to each.
 *
 * `PUT /api/v1/security-settings` refuses the whole save when one entry breaks
 * a rule (`validate_settings` in `crates/rolter-control/src/security.rs`), and
 * says so in a toast that names one entry. These functions run the same rules
 * in the field, so a bad entry is marked where it was typed and Save waits for
 * it. They accept what the control plane accepts and refuse what it refuses:
 * a stricter client would lock out an entry that is fine, a looser one is the
 * toast this exists to replace. `security-lists.test.ts` reads the Rust source
 * and fails when the character sets below drift from it.
 *
 * Every list is one entry per line. The old form split on commas, which cut a
 * required header value that held one in two, and dropped an entry it could
 * not read without a word. An entry that does not parse is reported here with
 * its line, never discarded.
 */

/** characters `validate_origin` refuses anywhere in an origin, besides `*` */
export const ORIGIN_FORBIDDEN = "?#@";
/** characters `validate_bypass_route` refuses anywhere in a path */
export const ROUTE_FORBIDDEN = "*{}?#";
/** the only prefix a bypass route may start with */
export const ROUTE_PREFIX = "/v1/";
/** the longest header name the `http` crate parses */
const HEADER_NAME_MAX = 65_535;
// a header name is an RFC 9110 token: what `http::HeaderName::from_bytes`
// accepts once it has lower-cased the letters
const HEADER_NAME = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/;

/**
 * Why an entry was refused. The page turns each code into a sentence under
 * `pages.security.errors.<code>`, so this module carries no copy.
 */
export type ProblemCode =
  | "originWildcard"
  | "originScheme"
  | "originPart"
  | "originHost"
  | "originPath"
  | "originSpace"
  | "headerName"
  | "requiredFormat"
  | "requiredValue"
  | "requiredDuplicate"
  | "routePrefix"
  | "routePattern"
  | "routeSpace"
  // the entry failed and holds a comma: several entries typed on one line
  | "commaList";

export interface EntryProblem {
  /** the line in the field, counted from 1 over blank lines too */
  line: number;
  /** what to name in the message: the entry, or the bad part of it */
  entry: string;
  code: ProblemCode;
}

export interface ParsedList<T> {
  /** the entries that parsed, in field order */
  entries: T[];
  problems: EntryProblem[];
}

export interface RequiredHeader {
  name: string;
  value: string;
}

export interface SecurityLists {
  allowedOrigins: string;
  allowedHeaders: string;
  requiredHeaders: string;
  bypassRoutes: string;
}

export interface ParsedLists {
  allowedOrigins: ParsedList<string>;
  allowedHeaders: ParsedList<string>;
  requiredHeaders: ParsedList<RequiredHeader>;
  bypassRoutes: ParsedList<string>;
}

interface Line {
  line: number;
  text: string;
}

// the non-blank lines of a field, trimmed, keeping the number each had in the
// field so a message can point at it
function linesOf(text: string): Line[] {
  const out: Line[] = [];
  text.split(/\r\n|\r|\n/).forEach((raw, index) => {
    const trimmed = raw.trim();
    if (trimmed) out.push({ line: index + 1, text: trimmed });
  });
  return out;
}

/** The entries of a field: its non-blank lines, trimmed, valid or not. */
export function entriesOf(text: string): string[] {
  return linesOf(text).map((l) => l.text);
}

/** A list as the field shows it, one entry per line. */
export function listToText(values: readonly string[] | null | undefined): string {
  return (values ?? []).join("\n");
}

/** Required headers as the field shows them, one `name: value` per line. */
export function requiredHeadersToText(
  headers: Readonly<Record<string, string>> | null | undefined,
): string {
  return Object.entries(headers ?? {})
    .map(([name, value]) => `${name}: ${value}`)
    .join("\n");
}

/**
 * A required-headers field with the space around each colon and each line
 * removed, for comparing two of them: `x-tenant:acme` and `x-tenant: acme`
 * are the same header.
 */
export function normalizeRequiredHeaders(text: string): string {
  return entriesOf(text)
    .map((entry) => {
      const colon = entry.indexOf(":");
      return colon < 0
        ? entry
        : `${entry.slice(0, colon).trim()}: ${entry.slice(colon + 1).trim()}`;
    })
    .join("\n");
}

function isHeaderName(name: string): boolean {
  return name.length <= HEADER_NAME_MAX && HEADER_NAME.test(name);
}

const hasAny = (value: string, chars: string) => [...chars].some((c) => value.includes(c));

function originProblem(origin: string): ProblemCode | null {
  if (origin.includes("*")) return "originWildcard";
  if (!origin.startsWith("https://") && !origin.startsWith("http://")) return "originScheme";
  if (hasAny(origin, ORIGIN_FORBIDDEN)) return "originPart";
  let authority = origin.slice(origin.startsWith("https://") ? 8 : 7);
  if (authority.endsWith("/")) authority = authority.slice(0, -1);
  if (authority === "") return "originHost";
  if (authority.includes("/")) return "originPath";
  if (/\s/.test(authority)) return "originSpace";
  return null;
}

function routeProblem(route: string): ProblemCode | null {
  if (!route.startsWith(ROUTE_PREFIX)) return "routePrefix";
  if (hasAny(route, ROUTE_FORBIDDEN)) return "routePattern";
  if (/\s/.test(route)) return "routeSpace";
  return null;
}

// a problem on an entry that holds a comma is most often a line the old form
// would have split: say that, it is the fix
const explain = (entry: string, code: ProblemCode): ProblemCode =>
  entry.includes(",") ? "commaList" : code;

function parseEach(text: string, check: (entry: string) => ProblemCode | null): ParsedList<string> {
  const parsed: ParsedList<string> = { entries: [], problems: [] };
  for (const { line, text: entry } of linesOf(text)) {
    const code = check(entry);
    if (code) parsed.problems.push({ line, entry, code: explain(entry, code) });
    else parsed.entries.push(entry);
  }
  return parsed;
}

/** `allowed_origins`: exact http(s) origins, no wildcard, no path. */
export function parseOrigins(text: string): ParsedList<string> {
  return parseEach(text, originProblem);
}

/** `allowed_headers`: header names. */
export function parseHeaderNames(text: string): ParsedList<string> {
  return parseEach(text, (entry) => (isHeaderName(entry) ? null : "headerName"));
}

/** `auth_bypass_routes`: exact `/v1/` paths. */
export function parseBypassRoutes(text: string): ParsedList<string> {
  return parseEach(text, routeProblem);
}

/**
 * `required_headers`: one `name: value` per line.
 *
 * The name is what comes before the first colon, so a value may hold colons
 * and commas. The control plane stores the pairs as a map, so two lines naming
 * one header (the gateway compares names without regard to case) would leave
 * one of them silently gone: the later line is refused instead.
 */
export function parseRequiredHeaders(text: string): ParsedList<RequiredHeader> {
  const parsed: ParsedList<RequiredHeader> = { entries: [], problems: [] };
  const seen = new Set<string>();
  for (const { line, text: entry } of linesOf(text)) {
    const colon = entry.indexOf(":");
    if (colon < 0) {
      parsed.problems.push({ line, entry, code: "requiredFormat" });
      continue;
    }
    const name = entry.slice(0, colon).trim();
    const value = entry.slice(colon + 1).trim();
    if (!isHeaderName(name)) {
      const bad = name || entry;
      parsed.problems.push({ line, entry: bad, code: explain(bad, "headerName") });
    } else if (!value) {
      parsed.problems.push({ line, entry, code: "requiredValue" });
    } else if (seen.has(name.toLowerCase())) {
      parsed.problems.push({ line, entry: name, code: "requiredDuplicate" });
    } else {
      seen.add(name.toLowerCase());
      parsed.entries.push({ name, value });
    }
  }
  return parsed;
}

/** Every list field of the form, parsed. */
export function parseLists(lists: SecurityLists): ParsedLists {
  return {
    allowedOrigins: parseOrigins(lists.allowedOrigins),
    allowedHeaders: parseHeaderNames(lists.allowedHeaders),
    requiredHeaders: parseRequiredHeaders(lists.requiredHeaders),
    bypassRoutes: parseBypassRoutes(lists.bypassRoutes),
  };
}

/** How many entries across all four lists do not parse. */
export function problemCount(parsed: ParsedLists): number {
  return (
    parsed.allowedOrigins.problems.length +
    parsed.allowedHeaders.problems.length +
    parsed.requiredHeaders.problems.length +
    parsed.bypassRoutes.problems.length
  );
}

/** The `required_headers` object the control plane expects. */
export function requiredHeadersPayload(entries: readonly RequiredHeader[]): Record<string, string> {
  return Object.fromEntries(entries.map(({ name, value }) => [name, value]));
}
