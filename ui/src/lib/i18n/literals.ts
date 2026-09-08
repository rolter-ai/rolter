// hardcoded-literal detector (#871).
//
// `check:i18n` validates that the catalogs agree with each other. It cannot see
// the failure that actually happens: a string that never reaches a catalog at
// all. `EditorSheet` shipped `window.confirm("Discard unsaved changes?")` and a
// default `"Cancel"` label, and the merge gate was green the whole time —
// catalog parity is a property of the catalogs, and a literal in JSX is not in
// them by definition.
//
// This scans source instead. It is deliberately a lexical scan rather than a
// real parse: the question is "does a user-visible English string appear where a
// `t()` call belongs", and every candidate is a string literal or a JSX text
// node, both of which a regex reads accurately enough. A TypeScript AST pass
// would cost a compiler dependency in a script that runs on every push, to
// answer the same question — and the `typescript` package the dashboard depends
// on is the Go port, which ships no in-process `createSourceFile` to walk.
//
// ## Why a tokenizer in front of the regexes
//
// The regexes alone read *formatting*, not content, and that broke the ratchet
// twice (#1143). `accept="image/*"` opened a block comment for the
// comment-stripping regex, blanking every literal between it and the next `*/`
// a hundred lines later, so `aria-label="Attach image"` and six of its
// neighbours in `Playground.tsx` were invisible; which literals a file lost
// depended on where the lines happened to break. Re-wrapping dense JSX moved
// strings in and out of the detected set with nothing added or removed from the
// source, which is exactly what `staleBaseline` exists to make impossible.
//
// So the source goes through `maskSource` first: a small TS/JSX tokenizer that
// knows strings, template literals, comments and regex literals apart, blanks
// what is not shipped, and collapses every whitespace run *in code position* to
// a single space. Two spellings of the same code — one line or twenty — mask to
// the same string, so the literal set is a property of the code and not of the
// line breaks. `maskSource` also returns an offset map, so a finding still
// reports the line it came from in the original file.
//
// ## Why a baseline
//
// The rule is repo-wide and the debt predates it: several hundred literals
// already exist across `pages/` and `components/`. Failing on all of them would
// mean either a several-hundred-string translation PR nobody asked for, or a
// gate that is permanently red and therefore ignored. So the existing set is
// recorded, and the gate fails on anything *new*. The baseline file is the debt,
// written down and countable, and it only ever shrinks.

/** One hardcoded user-facing string. */
export interface Literal {
  /** repo-relative source path */
  file: string;
  line: number;
  /** the offending text, normalized for stable baseline comparison */
  text: string;
  /** what matched, for the error message */
  kind: "dialog" | "prop" | "text" | "error";
}

/** file path -> the literal texts recorded as pre-existing */
export type Baseline = Record<string, string[]>;

/**
 * Props whose value is read by a person. Deliberately a closed list: the
 * alternative is flagging every string-valued prop, which would drown the real
 * findings in `className`, `type`, `id` and `data-*`.
 */
const USER_FACING_PROPS = [
  "title",
  "subtitle",
  "placeholder",
  "label",
  "description",
  "emptyText",
  "aria-label",
  "confirmLabel",
  "cancelLabel",
  "saveLabel",
];

const DIALOG = /window\.(?:confirm|alert|prompt)\(\s*(["'`])([^"'`]{2,})\1/g;
// `\s*=\s*` rather than a bare `=`: the literal that motivated this rule was a
// destructured default (`cancelLabel = "Cancel"`), not a JSX attribute, and a
// pattern that only saw attributes would have missed the very bug it exists for
const PROP = new RegExp(
  `\\b(?:${USER_FACING_PROPS.map((p) => p.replace("-", "\\-")).join("|")})\\s*=\\s*\\{?["']([^"']{2,})["']`,
  "g",
);
/**
 * a user-facing prop whose value is an expression: `title={open ? "Edit" : "Add"}`.
 * the braces are captured so the string literals inside can be checked one by
 * one (#1200)
 */
const PROP_EXPR = new RegExp(
  `\\b(?:${USER_FACING_PROPS.map((p) => p.replace("-", "\\-")).join("|")})\\s*=\\s*\\{([^{}]*(?:\\{[^{}]*\\}[^{}]*)*)\\}`,
  "g",
);
/**
 * a JSX text node: prose sitting directly between tags, on one line or
 * several — `<p>` blocks wrap, and a scan that needed `>`, the prose and `<` on
 * one line reported every wrapped paragraph as clean (#1200).
 *
 * The `>` must not be the tail of an arrow, or `=> Promise<Response>` reads as
 * the text node "Promise" sitting between two tags. Every `.tsx` file with a
 * generic return type on an arrow function hits that, so the lookbehind is
 * load-bearing rather than defensive.
 */
const TEXT = /(?<!=)>\s*([A-Za-z][^<>{}]{2,}?)\s*</g;
/**
 * an expression sitting where text would: `>{pending ? "Saving…" : "Save"}<`.
 * the string literals inside are the copy; the rest of the expression is not
 */
const TEXT_EXPR = /(?<!=)>\s*\{([^{}]*(?:\{[^{}]*\}[^{}]*)*)\}\s*</g;
/**
 * one `{…}` expression in text position, with one level of nesting allowed
 * (`{fmt({ x })}`). `<` and `>` are excluded on purpose: a `{cond && <Badge/>}`
 * carries its own tags, and letting the group span them would read a tag's
 * attributes as prose
 */
const EXPR_SOURCE = "\\{[^{}<>]*(?:\\{[^{}<>]*\\}[^{}<>]*)*\\}";
/**
 * children that mix prose with interpolation: `<p>{name} owns enforcement</p>`
 * (#1355). `TEXT` cannot see these — its text run may not contain a brace — so
 * a whole grammatical class of copy, the class that most needs a catalog entry
 * because interpolation order differs by language, never entered the ratchet.
 *
 * The region has to hold at least one expression; children without one are
 * `TEXT`'s job and are left to it, so this widening only ever adds findings.
 */
const TEXT_MIXED = new RegExp(`(?<!=)>((?:[^<>{}]*${EXPR_SOURCE})+[^<>{}]*)<`, "g");
/** the expressions inside such a region, to cut it into prose runs */
const EXPR_IN_TEXT = new RegExp(EXPR_SOURCE, "g");
/**
 * what an interpolation collapses to in a reported literal. The runs of one
 * element are reported as a single candidate rather than one finding each: the
 * sentence is what a catalog entry holds, and `Charged to {name} monthly` split
 * in two would record `Charged to` and `monthly`, neither of which is copy
 * anybody would translate.
 */
const PLACEHOLDER = "{…}";
/** `{" "}`, the explicit JSX space */
const JSX_SPACE = /^\{\s*(["'])\s+\1\s*\}$/;
/**
 * an error thrown with a literal message: LoadError prints `error.message`
 * under its heading, so this is copy the operator reads (#1200)
 */
const THROWN = /new (?:Api)?Error\(\s*(["'`])([A-Za-z][^"'`]{2,})\1/g;
/** a string literal inside an expression that reads as copy rather than code */
const STRING_IN_EXPR = /(["'])((?:[A-Z][^"'\n]*|[A-Za-z][^"'\n]* [^"'\n]*))\1/g;

/**
 * Strings that look like prose to a regex but are not copy. Kept narrow — a
 * false negative here is a string that silently stays untranslated, so the bar
 * for adding one is that it could never be shown to a person as a sentence.
 */
function isNotCopy(text: string): boolean {
  const t = text.trim();
  if (t.length < 3) return true;
  // no lowercase letter at all: an acronym, a code, a unit (USD, RPM, TPM, ID)
  if (!/[a-z]/.test(t)) return true;
  // an identifier or a wire value rather than a sentence: no spaces and it
  // looks like code (dots, slashes, underscores, camelCase runs of digits)
  if (!t.includes(" ") && /^[a-z0-9._/-]+$/.test(t)) return true;
  // a keyboard key, a header name or a mime type: a single capitalised word
  // that is a well-known code rather than a label
  if (CODE_WORDS.has(t)) return true;
  // a model name, a URL, a header, a mime type
  if (/^https?:\/\//.test(t) || t.includes("://")) return true;
  // a numeric placeholder like "0.00" or "1024"
  if (/^[\d.,\s%-]+$/.test(t)) return true;
  // a tailwind class list or a css value never starts with a capital and
  // never carries sentence punctuation; a capitalised class-looking token is
  // still checked by the caller
  if (/^[a-z0-9:[\]()\-./%]+( [a-z0-9:[\]()\-./%]+)+$/.test(t)) return true;
  return false;
}

// values that read as capitalised words but are wire codes the browser or the
// api defines, not copy. keyboard keys and http header names are the bulk
const CODE_WORDS = new Set([
  // "Delete" and "Home" are left out on purpose: they are also button labels,
  // and a missed key check costs less than a missed destructive-button label
  "Escape", "Enter", "Tab", "Backspace", "Space", "End",
  "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "PageUp", "PageDown",
  "Authorization", "Bearer", "Content-Type", "Accept", "Retry-After",
]);

/** Normalize for baseline comparison: collapse whitespace so a reflow of the
 * same string is not a new violation. */
function normalize(text: string): string {
  return text.trim().replace(/\s+/g, " ");
}

/**
 * A `t("…")` call, so a translated string can be removed from the source rather
 * than costing the whole line its scan. Skipping the entire line is what let a
 * translated header hide every other literal beside it (#1092): these screens
 * are written one dense statement per component, so "the rest of this line" is
 * routinely the rest of the component. Multi-line calls are blanked too.
 */
const T_CALL = /\bt\(\s*["'`][^"'`]*["'`][^)]*\)/g;

/** blank a match in place, keeping its length so the offset map stays aligned */
function blank(match: string): string {
  return " ".repeat(match.length);
}

/**
 * The masked source, plus `map[i]` = the offset in the original source that
 * masked character `i` came from. Whitespace runs collapse, so the two are not
 * the same length and a finding's line has to be looked up through the map.
 */
interface Masked {
  text: string;
  map: number[];
}

/** characters after which a `/` opens a regex literal rather than dividing.
 *
 * `<` and `}` are deliberately absent even though a real tokenizer would allow
 * a regex after both: in `.tsx` they are overwhelmingly `</Foo>` and
 * `{x}</Foo>`, and reading a closing tag as a regex would swallow the rest of
 * the component. A division misread as a regex costs real findings; a regex
 * misread as division costs nothing here, because a regex body is never copy. */
const REGEX_CONTEXT = new Set(["", "(", ",", "=", ":", "[", "!", "&", "|", "?", "+", "-", "*", "%", "^", "~", ";"]);

const WHITESPACE = /\s/;

/**
 * Blank what is never shipped and canonicalise the rest.
 *
 * Comments and regex literals become spaces; string and template literals are
 * copied through verbatim, because they are the candidates. Everything else is
 * code, and every whitespace run in code position collapses to one space — the
 * step that makes the scan independent of where the lines break. Newlines
 * inside a template literal survive, since they are part of the value.
 */
export function maskSource(source: string): Masked {
  const out: string[] = [];
  const map: number[] = [];
  const emit = (ch: string, at: number) => {
    out.push(ch);
    map.push(at);
  };
  // the `${` depths of the template literals we are currently nested inside, so
  // the `}` that closes an interpolation returns to template text rather than
  // reading as a block end
  const templates: number[] = [];
  let braces = 0;
  let last = "";
  let i = 0;

  /** is there a closing `quote` before the line ends? an apostrophe in JSX prose
   * (`Don't`) is not a string, and reading it as one would eat the copy after it */
  const closes = (quote: string) => {
    for (let j = i + 1; j < source.length && source[j] !== "\n"; j++) {
      if (source[j] === "\\") j++;
      else if (source[j] === quote) return true;
    }
    return false;
  };

  /** copy a `'…'` or `"…"` literal verbatim, quotes included */
  const copyString = (quote: string) => {
    emit(quote, i++);
    while (i < source.length && source[i] !== quote && source[i] !== "\n") {
      if (source[i] === "\\") emit(source[i], i++);
      if (i < source.length) emit(source[i], i++);
    }
    if (i < source.length && source[i] === quote) emit(source[i], i++);
    last = quote;
  };

  /** copy template text up to the closing backtick or the next `${` */
  const copyTemplate = () => {
    while (i < source.length) {
      if (source[i] === "\\") {
        emit(source[i], i++);
        if (i < source.length) emit(source[i], i++);
        continue;
      }
      if (source[i] === "`") {
        emit(source[i], i++);
        templates.pop();
        last = "`";
        return;
      }
      if (source[i] === "$" && source[i + 1] === "{") {
        emit(source[i], i++);
        emit(source[i], i++);
        braces++;
        last = "{";
        return;
      }
      emit(source[i], i++);
    }
  };

  while (i < source.length) {
    const ch = source[i];

    if (templates.length && templates[templates.length - 1] === braces) {
      copyTemplate();
      continue;
    }

    if (WHITESPACE.test(ch)) {
      const at = i;
      while (i < source.length && WHITESPACE.test(source[i])) i++;
      emit(" ", at);
      continue;
    }

    // `https://…` sitting in JSX text is a URL, not a comment and not an empty
    // regex. no line comment ever starts flush against a colon
    if (ch === "/" && source[i + 1] === "/" && source[i - 1] === ":") {
      emit(source[i], i++);
      emit(source[i], i++);
      last = "/";
      continue;
    }
    if (ch === "/" && source[i + 1] === "/") {
      while (i < source.length && source[i] !== "\n") i++;
      continue;
    }
    if (ch === "/" && source[i + 1] === "*") {
      const end = source.indexOf("*/", i + 2);
      i = end === -1 ? source.length : end + 2;
      continue;
    }
    if (ch === "/" && REGEX_CONTEXT.has(last)) {
      const at = i++;
      let inClass = false;
      while (i < source.length && source[i] !== "\n") {
        if (source[i] === "\\") i += 2;
        else if (source[i] === "[") (inClass = true), i++;
        else if (source[i] === "]") (inClass = false), i++;
        else if (source[i] === "/" && !inClass) break;
        else i++;
      }
      if (i < source.length && source[i] === "/") {
        i++;
        while (i < source.length && /[dgimsuvy]/.test(source[i])) i++;
        emit(" ", at);
        last = "/";
        continue;
      }
      // no terminator on this line: it was a division after all
      i = at;
    }

    if ((ch === '"' || ch === "'") && closes(ch)) {
      copyString(ch);
      continue;
    }
    if (ch === "`") {
      emit(ch, i++);
      templates.push(braces);
      continue;
    }
    if (ch === "{") braces++;
    if (ch === "}") braces = Math.max(0, braces - 1);

    emit(ch, i++);
    last = ch;
  }

  return { text: out.join(""), map };
}

/**
 * The sentence a mixed children region carries, with every interpolation
 * replaced by `{…}` — or `null` when the region holds no prose at all, which is
 * the common case (`{a} · {b}`, `{rows.map(…)}`, a lone `(`).
 *
 * Requiring a prose run with a letter in it is what keeps the baseline free of
 * separator noise; the usual `isNotCopy` thresholds still apply to the result.
 */
export function mixedText(region: string): string | null {
  const parts: string[] = [];
  let prose = false;
  let at = 0;
  EXPR_IN_TEXT.lastIndex = 0;
  for (const m of region.matchAll(EXPR_IN_TEXT)) {
    const run = region.slice(at, m.index);
    if (/[A-Za-z]/.test(run)) prose = true;
    // `{" "}` is JSX's way of writing a space the formatter cannot eat; it is
    // whitespace, not a value, and reporting it as a placeholder would put a
    // second `{…}` in the middle of every wrapped sentence
    parts.push(run, JSX_SPACE.test(m[0]) ? " " : PLACEHOLDER);
    at = m.index + m[0].length;
  }
  const tail = region.slice(at);
  if (/[A-Za-z]/.test(tail)) prose = true;
  parts.push(tail);
  if (!prose) return null;
  return parts.join("");
}

/**
 * Is the `>` at `gt` the end of a JSX tag, rather than the end of a generic
 * argument list?
 *
 * `TEXT` never had to ask: its run may not contain a brace, so the worst a
 * `React.useState<Row[]>(…)` could yield was a short token. `TEXT_MIXED` spans
 * braces, so a misread `>` swallows whole statements — `{…} const ARROWS:
 * Record` and `{…} async function getText(url: string): Promise` both showed up
 * as copy before this check existed.
 *
 * The tell is the character in front of the matching `<`: a generic opens flush
 * against the identifier it parameterises (`Promise<`, `Record<`), and JSX never
 * does — a tag opens after whitespace, `(`, `{`, `,` or another tag.
 */
function endsJsxTag(masked: string, gt: number): boolean {
  const lt = masked.lastIndexOf("<", gt);
  if (lt === -1) return false;
  const after = masked[lt + 1] ?? "";
  // `</Foo>` closes an element and its parent's children run on; `<>` opens a
  // fragment. neither can be a generic
  if (after === "/" || after === ">") return true;
  if (!/[A-Za-z]/.test(after)) return false;
  return !/[A-Za-z0-9_$.)\]]/.test(masked[lt - 1] ?? "");
}

function lineOf(source: string, index: number): number {
  let line = 1;
  for (let i = 0; i < index; i++) if (source.charCodeAt(i) === 10) line++;
  return line;
}

/**
 * Every hardcoded user-facing literal in one source file.
 *
 * The source is masked first (`maskSource`), then `t("pages.x.y")` calls are
 * blanked — that is exactly what this rule wants — and whatever remains is
 * scanned as a whole, so prose that wraps across lines, a second literal on a
 * dense line, and the strings inside `{cond ? "A" : "B"}` are all seen (#1200),
 * regardless of how the file happens to be wrapped (#1143).
 */
export function findLiterals(source: string, file: string): Literal[] {
  const out: Literal[] = [];
  const seen = new Set<string>();
  const masked = maskSource(source);
  const scanned = masked.text.replace(T_CALL, blank);

  const push = (index: number, raw: string, kind: Literal["kind"]) => {
    const text = normalize(raw);
    if (isNotCopy(text)) return;
    const line = lineOf(source, masked.map[index] ?? 0);
    const key = `${line}:${text}`;
    if (seen.has(key)) return;
    seen.add(key);
    out.push({ file, line, text, kind });
  };

  for (const m of scanned.matchAll(DIALOG)) push(m.index, m[2], "dialog");
  for (const m of scanned.matchAll(THROWN)) push(m.index, m[2], "error");
  for (const m of scanned.matchAll(PROP)) push(m.index, m[1], "prop");
  for (const m of scanned.matchAll(PROP_EXPR)) {
    for (const inner of m[1].matchAll(STRING_IN_EXPR)) push(m.index, inner[2], "prop");
  }
  for (const m of scanned.matchAll(TEXT)) push(m.index, m[1], "text");
  for (const m of scanned.matchAll(TEXT_EXPR)) {
    for (const inner of m[1].matchAll(STRING_IN_EXPR)) push(m.index, inner[2], "text");
  }
  for (const m of scanned.matchAll(TEXT_MIXED)) {
    if (!endsJsxTag(scanned, m.index)) continue;
    const text = mixedText(m[1]);
    if (text !== null) push(m.index, text, "text");
  }
  out.sort((a, b) => a.line - b.line);
  return out;
}

/** Findings not present in the baseline — the ones that should fail the build. */
export function newViolations(found: Literal[], baseline: Baseline): Literal[] {
  return found.filter((l) => !(baseline[l.file] ?? []).includes(l.text));
}

/**
 * Baseline entries no longer found in the source. These are debt that was paid
 * off; leaving them recorded would let the same literal come back unnoticed.
 */
export function staleBaseline(found: Literal[], baseline: Baseline): string[] {
  const live = new Set(found.map((l) => `${l.file} ${l.text}`));
  const stale: string[] = [];
  for (const [file, texts] of Object.entries(baseline)) {
    for (const text of texts) {
      if (!live.has(`${file} ${text}`)) stale.push(`${file}: ${text}`);
    }
  }
  return stale;
}

/** Build a baseline from findings, in a stable order so the file diffs cleanly. */
export function toBaseline(found: Literal[]): Baseline {
  const out: Baseline = {};
  for (const l of found) {
    (out[l.file] ??= []).push(l.text);
  }
  const sorted: Baseline = {};
  for (const file of Object.keys(out).sort()) {
    sorted[file] = [...new Set(out[file])].sort();
  }
  return sorted;
}
