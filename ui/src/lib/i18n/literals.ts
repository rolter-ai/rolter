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
// source, which is exactly what `staleAllowed` exists to make impossible.
//
// So the source goes through `maskSource` first: a small TS/JSX tokenizer that
// knows strings, template literals, comments and regex literals apart, blanks
// what is not shipped, and collapses every whitespace run *in code position* to
// a single space. Two spellings of the same code — one line or twenty — mask to
// the same string, so the literal set is a property of the code and not of the
// line breaks. `maskSource` also returns an offset map, so a finding still
// reports the line it came from in the original file.
//
// ## Why an allow-list, and no baseline
//
// The rule is repo-wide and the debt predated it, so it shipped with a recorded
// baseline of several hundred literals that the gate tolerated and that could
// only shrink. That baseline read as clean on every run while the `ru` locale
// was mostly English (#958), and it has now been paid off: every piece of copy
// the scan finds is in the catalogs.
//
// What is left is notation the scan cannot tell from prose — `n=1`, `v{…}`, a
// `{…} rpm` unit, a thrown invariant no operator can reach. Those live in
// `literals-allowlist.ts`, one entry per string, and every entry states why it
// is not copy. There is no command that records findings into it: an exception
// is written by hand and reviewed, so it never grows by accident.

/** One hardcoded user-facing string. */
export interface Literal {
  /** repo-relative source path */
  file: string;
  line: number;
  /** the offending text, normalized for stable allow-list comparison */
  text: string;
  /** what matched, for the error message */
  kind: "dialog" | "prop" | "text" | "error";
}

/** file path -> literal text -> why it is not copy */
export type AllowList = Record<string, Record<string, string>>;

/**
 * Props whose value is read by a person. Deliberately a closed list: the
 * alternative is flagging every string-valued prop, which would drown the real
 * findings in `className`, `type`, `id` and `data-*`.
 *
 * Closed does not mean frozen: the list has to name what the dashboard's own
 * components render as copy. It once stopped at `saveLabel`, and six English
 * `desc:` strings in `FeatureFlags.tsx` sat beside the `title:` it did report
 * (#1545). A component that takes copy under a new name adds the name here.
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
  // RelatedLink, FeatureFlags and the settings screens' rows
  "desc",
  // Field and SwitchRow: the line under the control, and the InfoHint beside it
  "hint",
  "info",
  // InfoHint's own prop, and the hover explanation other rows spell out
  "text",
  "tooltip",
  // a toast's second line, and a notice's message or body
  "detail",
  "message",
  "body",
  // ScopeNote and the cost attribution notes
  "note",
  // Field's validation line and the create dialogs' failure line
  "error",
  "errorMessage",
  // an image's accessible name
  "alt",
  // a table column's heading, which is data and never a JSX text node
  "header",
  // the header a ComboboxOption sits under; the migration off `<select>` (#968)
  // moved a dropdown's whole vocabulary — label, description, group — out of
  // JSX text and into properties, so the keys are where that copy is read
  "group",
  // the names the dashboard's own components declare for copy and that the
  // list above did not reach (#1745): PageLead and SettingsPanel's kicker, the
  // charts' axis titles and units, the donut's centre, a chip's remove button,
  // a Segmented group's name, a screen's empty slot and a flag's refusal
  "eyebrow",
  "xLabel",
  "yLabel",
  "unit",
  "xUnit",
  "yUnit",
  "centerLabel",
  "centerSub",
  "removeLabel",
  "ariaLabel",
  "empty",
  "experimentalNote",
  "unavailableReason",
  // the rest of the ARIA attributes a screen reader speaks, and the html
  // elements' own captions
  "aria-description",
  "aria-roledescription",
  "aria-valuetext",
  "aria-placeholder",
  "heading",
  "caption",
  "legend",
];

/**
 * A state setter whose value is rendered as a message: `setError("enter at
 * least two texts")` in a submit handler (#1745). The value never touches JSX
 * or a prop, so nothing above sees it, and a validation line is exactly the copy
 * an operator reads. `setStatus` is left out: its values are state codes
 */
const COPY_SETTER =
  /\bset(?:[A-Z]\w*?)?(?:Error|Message|Notice|Hint|Warning|Note|Detail|Text|Label|Title|Summary|Feedback)\(/g;

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
 * load-bearing rather than defensive. A comparison (`a > b`) survives the
 * lookbehind, so every match is confirmed against `jsxTagEnds` too (#1370).
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
const THROWN = /new (?:Api)?Error\(\s*(["'])([A-Za-z][^"'`]{2,})\1/g;
/**
 * the same, when the message is a template literal. `THROWN` used to take
 * backticks too and read up to the first quote character, so `duplicate param
 * "${key}"` ended its match on the embedded `"` and never matched at all
 * (#1390). the template is read whole instead
 */
const THROWN_TEMPLATE = /new (?:Api)?Error\(\s*(?=`)/g;
/**
 * a string literal inside an expression that reads as copy rather than code:
 * capitalised, or a lowercase phrase with a space in it. `stringsIn` cuts the
 * literal out first — a regex over the whole expression read the quotes inside
 * a template literal as string boundaries
 */
const READS_AS_COPY = /^(?:[A-Z].*|[A-Za-z].* .*)$/s;
/**
 * the template variant: once the values are taken out, a message such as
 * `"${key}": not a valid number` starts on punctuation, so a letter anywhere
 * plus a space is enough
 */
const TEMPLATE_READS_AS_COPY = /^(?:[A-Z].*|.*[A-Za-z].* .*|.* .*[A-Za-z].*)$/s;
/**
 * a user-facing key in an object literal: `{ label: `Other (${n})` }` (#1537).
 * the `{` or `,` in front is what tells a key from a ternary's `: ` — `open ?
 * label : fallback` names a variable, not a key — and from a type member, whose
 * value is a type rather than a literal and so never yields a candidate anyway
 */
const OBJECT_KEY = new RegExp(
  `[{,]\\s*(["']?)(?:${USER_FACING_PROPS.map((p) => p.replace("-", "\\-")).join("|")})\\1\\s*:\\s*`,
  "g",
);
/**
 * an object literal with nothing nested inside it — the shape a lookup table
 * takes. matched innermost-first, so `{ verbs: { retry: "Try again" } }` yields
 * the inner table rather than nothing (#1599)
 */
const FLAT_OBJECT = /\{[^{}]*\}/g;
/** one `code: "label"` member of such a table, quoted key or not */
const MAP_ENTRY = /(["']?)([A-Za-z_$][\w$-]*)\1\s*:\s*(["'])((?:[^"'\\]|\\.)*)\3/g;
/**
 * A value that reads as a label rather than as another code: it is a phrase, or
 * a word the author capitalised, and either way it carries a run of letters
 * long enough to be a word — `r .1s`, a css transition, is a phrase otherwise.
 * Stricter than `isNotCopy` on purpose: a table of wire values is the normal
 * case here, and `cache-first`, `provider_name` and `/api/v1/keys` all pass
 * `isNotCopy` while being nobody's copy.
 */
const READS_AS_LABEL = /^(?=.*[A-Za-z]{2})(?:.*\s|[A-Z])/s;
/**
 * Keys that name a *prop* rather than a wire value. One of them anywhere in the
 * object says this is a bag of settings being passed somewhere, not a lookup
 * table, and its strings are whatever that prop means — `id`, `className` and
 * `kind` all carry capitalised values that are nobody's copy. The copy-carrying
 * props are read by name through `OBJECT_KEY` instead, which is the precise
 * rule; this one only has to know when to stand down.
 */
const NOT_A_TABLE_KEY = new Set([
  "id",
  "key",
  "name",
  "kind",
  "type",
  "value",
  "className",
  "class",
  "style",
  "href",
  "src",
  "url",
  "path",
  "to",
  "role",
  "variant",
  "size",
  "color",
  "icon",
  "testId",
  "method",
  "slug",
  "field",
  "column",
]);

/**
 * The labels in a map from wire codes to copy, or `null` when the object is not
 * such a map.
 *
 * `USER_FACING_PROPS` is a list of *names*, so it only ever reaches a string
 * whose key is one of them. A lookup table inverts that — the key is the wire
 * value and the copy is on the right — and no name on that list can catch it
 * (#1599). Every member has to be a `code: "string"` pair, and there has to be
 * more than one: a single pair is as often an options bag as a table.
 */
function codeMapLabels(inner: string, base: number): Candidate[] | null {
  let at = 0;
  let members = 0;
  const out: Candidate[] = [];
  MAP_ENTRY.lastIndex = 0;
  for (const m of inner.matchAll(MAP_ENTRY)) {
    members += 1;
    if (NOT_A_TABLE_KEY.has(m[2])) return null;
    // anything but whitespace and the separating comma between two members
    // means this is not a table of pairs
    if (!/^[\s,]*$/.test(inner.slice(at, m.index))) return null;
    at = m.index + m[0].length;
    const text = normalize(m[4]);
    if (READS_AS_LABEL.test(text))
      out.push({ index: base + m.index + m[0].lastIndexOf(m[4]), text });
  }
  if (!/^[\s,]*$/.test(inner.slice(at))) return null;
  return members > 1 ? out : null;
}

/** a bare identifier rendered where copy goes: `{cta}` as children, `title={title}` */
const RENDERED_IDENT = /^\s*([A-Za-z_$][\w$]*)\s*$/;
/** one entry of a table rendered where copy goes: `{HINTS[mode]}`, `title={COPY.save}` */
const RENDERED_ENTRY = /^\s*([A-Za-z_$][\w$]*)(?:\.[A-Za-z_$][\w$]*|\[[^\]]+\])\s*$/;
/** a local binding whose value might be copy held for later */
const BINDING = /\b(?:const|let)\s+([A-Za-z_$][\w$]*)\s*(?::[^=;]*)?=\s*/g;

/** a string, template or interpolation found by `stringsIn` */
interface Candidate {
  /** offset into the scanned text */
  index: number;
  text: string;
  /** read in a position that renders it as it stands — see `isNotCopy` */
  strict?: boolean;
}

/**
 * A single lowercase word. Outside a rendered position it is far more often a
 * code (`chat`, `inherit`, `idle`) than a label, which is why `isNotCopy` lets
 * it go; in one, the dashboard's lowercase style makes it copy — `aria-label=
 * "close"`, `<span>optional</span>`, `{on ? "enabled" : "disabled"}` (#1745)
 */
const LOWERCASE_WORD = /^[a-z]{3,}[.…:!?]?$/;

/**
 * Is the literal spanning `[start, end)` of `expr` a value the expression can
 * evaluate to, rather than an operand? `open ? "enabled" : "disabled"`, `name
 * ?? "unnamed"` and `ready && "live"` are results; `kind === "chat"`,
 * `cn("block")`, `variant="ghost"` and `"a".length` are code
 */
function isResult(expr: string, start: number, end: number): boolean {
  const before = expr.slice(0, start).trimEnd();
  const after = expr.slice(end).trimStart();
  if (before && !/(?:\?\?|\|\||&&|[?:+])$/.test(before)) return false;
  // `{ dateStyle: "medium" }`, `scope_type?: "org"`: a property's value or a
  // type, not a branch. a copy key is read by name through `OBJECT_KEY`, which
  // starts past the colon, so this never hides one
  if (PROPERTY_COLON.test(before)) return false;
  return !/^(?:[=!]=|\.|\[|\?(?!\?)|in\b)/.test(after);
}

/** the colon of an object property or a type member, as opposed to a ternary's */
const PROPERTY_COLON = /(?:^|[{,;])\s*(["']?)[A-Za-z_$][\w$-]*\1\??\s*:$/;

/**
 * A fragment glued to something else with `+`: `name + " settings"`. The space
 * at its edge is what makes it a piece of a sentence rather than a code, and it
 * is also what hid it — `READS_AS_COPY` wants the literal to start on a letter
 */
function isFragment(expr: string, start: number, end: number, text: string): boolean {
  if (!/^\s|\s$/.test(text) || !/[A-Za-z]{2}/.test(text)) return false;
  return /\+$/.test(expr.slice(0, start).trimEnd()) || /^\+/.test(expr.slice(end).trimStart());
}

/** the index just past the `'…'` / `"…"` literal opening at `i` */
function skipString(s: string, i: number): number {
  const quote = s[i];
  let j = i + 1;
  while (j < s.length && s[j] !== quote) j += s[j] === "\\" ? 2 : 1;
  return j + 1;
}

/**
 * Read the template literal opening at `i`. Every `${…}` collapses to `{…}`, the
 * same placeholder a mixed text node gets, so `Other (${rest.length})` and
 * `Other ({rest.length})` in JSX record the same way. The interpolations are
 * returned too: a `${n === 1 ? "key" : "keys"}` holds copy of its own.
 */
function readTemplate(
  s: string,
  i: number,
): { end: number; text: string; holes: [number, number][] } {
  let text = "";
  const holes: [number, number][] = [];
  let j = i + 1;
  while (j < s.length && s[j] !== "`") {
    if (s[j] === "\\") {
      text += s.slice(j, j + 2);
      j += 2;
    } else if (s[j] === "$" && s[j + 1] === "{") {
      const start = j + 2;
      j = skipExpression(s, start, "}");
      holes.push([start, j]);
      text += PLACEHOLDER;
      j++;
    } else {
      text += s[j++];
    }
  }
  return { end: j + 1, text, holes };
}

/**
 * The index where the expression starting at `i` ends: the first of `stops` at
 * bracket depth zero, or the bracket that closes the region it sits in. Strings
 * and templates are stepped over whole, so a `,` or `}` inside one is text.
 */
function skipExpression(s: string, i: number, stops: string): number {
  let depth = 0;
  let j = i;
  while (j < s.length) {
    const ch = s[j];
    if (ch === '"' || ch === "'") j = skipString(s, j);
    else if (ch === "`") j = readTemplate(s, j).end;
    else if ("([{".includes(ch)) (depth++, j++);
    else if (")]}".includes(ch)) {
      if (depth === 0) return j;
      (depth--, j++);
    } else if (depth === 0 && stops.includes(ch)) return j;
    else j++;
  }
  return j;
}

/**
 * Every string and template literal inside `expr` that reads as copy. `base`
 * is where `expr` starts in the scanned text, so findings keep their line.
 *
 * A template counts only when its prose does: `${a}/${b}` and `/v1/${id}` are
 * paths with no sentence in them, so the text is judged with the placeholders
 * taken out — the placeholder itself carries no lowercase letter to trip the
 * usual thresholds on.
 *
 * `strict` says the expression is rendered as it stands — a copy prop, a
 * child, a message setter — so a single lowercase word that the expression can
 * evaluate to counts as copy too (#1745). A fragment glued on with `+` counts
 * wherever it is read.
 */
function stringsIn(expr: string, base: number, strict = false): Candidate[] {
  const out: Candidate[] = [];
  let j = 0;
  while (j < expr.length) {
    const ch = expr[j];
    if (ch === '"' || ch === "'") {
      const end = skipString(expr, j);
      const text = expr.slice(j + 1, end - 1);
      if (text.includes("\n")) {
        // not a single-line literal; leave it
      } else if (isFragment(expr, j, end, text)) {
        out.push({ index: base + j, text, strict: true });
      } else if (READS_AS_COPY.test(text)) {
        out.push({ index: base + j, text });
      } else if (strict && LOWERCASE_WORD.test(text) && isResult(expr, j, end)) {
        out.push({ index: base + j, text, strict: true });
      }
      j = end;
    } else if (ch === "`") {
      const { end, text, holes } = readTemplate(expr, j);
      const prose = normalize(text.split(PLACEHOLDER).join(" "));
      if (TEMPLATE_READS_AS_COPY.test(prose) && !isNotCopy(prose))
        out.push({ index: base + j, text });
      // `${n} tokens`: one word beside a value, which is a unit and a plural
      else if (strict && holes.length && LOWERCASE_WORD.test(prose) && isResult(expr, j, end))
        out.push({ index: base + j, text, strict: true });
      for (const [from, to] of holes)
        out.push(...stringsIn(expr.slice(from, to), base + from, strict));
      j = end;
    } else {
      j++;
    }
  }
  return out;
}

/**
 * Strings that look like prose to a regex but are not copy. Kept narrow — a
 * false negative here is a string that silently stays untranslated, so the bar
 * for adding one is that it could never be shown to a person as a sentence.
 *
 * `strict` is set where the string is rendered as it stands (`LOWERCASE_WORD`):
 * there a bare lowercase word is a label, not a wire value.
 */
function isNotCopy(text: string, strict = false): boolean {
  const t = text.trim();
  if (t.length < 3) return true;
  // no lowercase letter at all: an acronym, a code, a unit (USD, RPM, TPM, ID)
  if (!/[a-z]/.test(t)) return true;
  if (strict && LOWERCASE_WORD.test(t)) return false;
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
  // svg path data: a move command and then nothing but commands and numbers.
  // `M12 19V5M5 12l7-7 7 7` is an arrow, not a sentence (#1599)
  if (/^[Mm][\d\s.,-]/.test(t) && /^[A-Za-z\d\s.,-]+$/.test(t) && !/[A-Za-z]{2}/.test(t))
    return true;
  // a tailwind class list or a css value: lowercase, no sentence punctuation,
  // and every token a utility. the character set alone is not enough —
  // `request failed: {…}` and `no events yet` fit it too, and were dropped
  // while their capitalised spellings were reported (#1546). one plain word
  // among the tokens makes it prose. `_`, `+` and `,` belong to arbitrary
  // values (`grid-cols-[1fr_2fr]`, `bottom-[calc(100%+6px)]`) and to css
  // functions (`color-mix(in srgb, …)`), which sat in the baseline as copy (#958)
  if (
    /^[a-z0-9:[\]()\-./%_+,]+( [a-z0-9:[\]()\-./%_+,]+)+$/.test(t) &&
    t.split(" ").every(isUtilityToken)
  ) {
    return true;
  }
  return false;
}

/**
 * A token that reads as a Tailwind utility or a css value rather than a word:
 * it carries a dash, a bracket, a slash, a percent or a digit (`px-3.5`,
 * `w-[9px]`, `w-1/2`, `1px`), a variant colon with something after it
 * (`sm:block`, where `failed:` ends a clause), or it is one of the utilities
 * and css keywords that are a bare word. The parentheses and commas of a css
 * function's argument list are not part of the word: `srgb,` and
 * `transparent)` are still the keyword.
 */
function isUtilityToken(token: string): boolean {
  return /[-[\]/%\d_]|:./.test(token) || BARE_UTILITIES.has(token.replace(/^\(+|[),]+$/g, ""));
}

// utilities and css keywords spelled as a plain word. only a class list made
// entirely of these and dashed tokens is skipped, so a word that doubles as
// English (`block`, `none`) costs nothing beside real prose
const BARE_UTILITIES = new Set([
  // display, position and visibility
  "flex",
  "grid",
  "block",
  "inline",
  "hidden",
  "contents",
  "table",
  "relative",
  "absolute",
  "fixed",
  "sticky",
  "static",
  "isolate",
  "visible",
  "invisible",
  "collapse",
  // type
  "truncate",
  "italic",
  "uppercase",
  "lowercase",
  "capitalize",
  "underline",
  "antialiased",
  "ordinal",
  "grow",
  "shrink",
  // borders, effects and state markers
  "border",
  "rounded",
  "shadow",
  "ring",
  "outline",
  "transition",
  "transform",
  "filter",
  "blur",
  "resize",
  "container",
  "group",
  "peer",
  "prose",
  "dark",
  // css values
  "auto",
  "none",
  "solid",
  "dashed",
  "dotted",
  "transparent",
  "inherit",
  "currentcolor",
  "normal",
  "bold",
  "nowrap",
  "pointer",
  "center",
  "ease",
  "linear",
  "infinite",
  "srgb",
]);

// values that read as capitalised words but are wire codes the browser or the
// api defines, not copy. keyboard keys and http header names are the bulk
const CODE_WORDS = new Set([
  // "Delete" and "Home" are left out on purpose: they are also button labels,
  // and a missed key check costs less than a missed destructive-button label
  "Escape",
  "Enter",
  "Tab",
  "Backspace",
  "Space",
  "End",
  "ArrowUp",
  "ArrowDown",
  "ArrowLeft",
  "ArrowRight",
  "PageUp",
  "PageDown",
  "Authorization",
  "Bearer",
  "Content-Type",
  "Accept",
  "Retry-After",
]);

/** Normalize for allow-list comparison: collapse whitespace so a reflow of the
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
const T_CALL = /\bt\(\s*["'`]/g;

/**
 * Blank every `t(…)` call through its own closing paren. The call used to be
 * read as far as the first `)`, so `t("k", { count: m.get(id) ?? 0 })` left `??
 * 0, })` behind with its brackets unbalanced, and the expression walker that
 * reads children lost track of where the expression holding it ended (#1745).
 */
function blankTCalls(text: string): string {
  let out = text;
  for (const m of text.matchAll(T_CALL)) {
    const open = m.index + m[0].indexOf("(");
    const close = skipExpression(text, open + 1, "");
    out = out.slice(0, m.index) + " ".repeat(close + 1 - m.index) + out.slice(close + 1);
  }
  return out;
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
const REGEX_CONTEXT = new Set([
  "",
  "(",
  ",",
  "=",
  ":",
  "[",
  "!",
  "&",
  "|",
  "?",
  "+",
  "-",
  "*",
  "%",
  "^",
  "~",
  ";",
]);

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
        else if (source[i] === "[") ((inClass = true), i++);
        else if (source[i] === "]") ((inClass = false), i++);
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
 * Requiring a prose run with a letter in it is what keeps the findings free of
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
 * Every JSX tag in `masked`, as the offset of its `<` mapped to its `>`.
 *
 * A `>` is otherwise ambiguous, and both misreads cost findings: a generic
 * (`Promise<Response>`) makes the type name read as a text node, and a
 * comparison (`a > b && c < d`) makes the code between the operator and the
 * next `<` read as one (#1370). `TEXT_MIXED` spans braces, so there a misread
 * `>` swallows whole statements — `{...} const ARROWS: Record` and `{...} async
 * function getText(url: string): Promise` both showed up as copy before this
 * check existed (#1355).
 *
 * Asking the question from the `<` end is what makes it answerable. A tag opens
 * as `<Name`, `</Name`, or `<>` for a fragment, and never flush against an
 * identifier — that is a generic (`Promise<`, `Record<`). From there the tag
 * runs to its own `>`, skipping attribute strings and everything inside `{…}`,
 * so the `=>` of an `onClick={() => …}` handler stays an arrow instead of
 * ending the tag it sits in.
 */
function jsxTags(masked: string): Map<number, number> {
  const tags = new Map<number, number>();
  for (let i = 0; i < masked.length; i++) {
    if (masked[i] !== "<") continue;
    const after = masked[i + 1] ?? "";
    const opensTag =
      after === ">" ||
      after === "/" ||
      (/[A-Za-z]/.test(after) && !/[A-Za-z0-9_$.)\]]/.test(masked[i - 1] ?? ""));
    if (!opensTag) continue;
    let depth = 0;
    for (let j = i + 1; j < masked.length; j++) {
      const ch = masked[j];
      if (ch === '"' || ch === "'") {
        const quote = ch;
        while (++j < masked.length && masked[j] !== quote);
        continue;
      }
      if (ch === "{") depth++;
      else if (ch === "}") depth = Math.max(0, depth - 1);
      else if (depth > 0) continue;
      // a second `<` before this one closed: not a tag after all
      else if (ch === "<") break;
      else if (ch === ">") {
        tags.set(i, j);
        break;
      }
    }
  }
  return tags;
}

/**
 * The same text with every tag blanked, so only the expressions and text
 * between them are left. A child expression is read whole (`childExpressions`),
 * and a tag nested inside one carries `className` and the other props the scan
 * deliberately ignores — reading those as children reported ten class lists as
 * copy. The props inside are `PROP`'s job either way.
 */
function blankTags(masked: string, tags: Map<number, number>): string {
  const out = masked.split("");
  for (const [start, end] of tags) {
    for (let i = start; i <= end; i++) out[i] = " ";
  }
  return out.join("");
}

/**
 * The tag ends that open onto children: the ones after which some element is
 * still open. The rest close the outermost element of an expression — `icon:
 * <Gauge />, children: [` in a nav table, `</div> ); }` at the end of a
 * component — and what follows them is code.
 *
 * A closing tag pops back to the element it names, so a tag this scan misread
 * cannot leave the rest of the file looking like children.
 */
function childPositions(masked: string, tags: Map<number, number>): Set<number> {
  const inside = new Set<number>();
  const open: string[] = [];
  for (const [start, end] of [...tags].sort((a, b) => a[0] - b[0])) {
    const name = /^<\/?([\w.$-]*)/.exec(masked.slice(start, end))?.[1] ?? "";
    if (masked[start + 1] === "/") {
      const at = open.lastIndexOf(name);
      if (at !== -1) open.length = at;
    } else if (masked[end - 1] !== "/") {
      open.push(name);
    }
    if (open.length) inside.add(end);
  }
  return inside;
}

/** one run of children after a tag: its expressions, and its prose if any */
interface ChildRun {
  /** the tag end the run follows */
  start: number;
  /** the sentence, `{…}` for each expression, or `null` when there is none */
  text: string | null;
  /** every `{…}` in the run, as `[start, end)` inside the braces */
  exprs: [number, number][];
}

/**
 * Every run of children after a tag: text and `{…}` expressions up to the next
 * tag.
 *
 * `TEXT_EXPR` reads the shape `>{…}<`: one expression alone between two tags.
 * An element that renders a spinner beside its label writes two in a row —
 * `>{pending && <Loader2/>}{draining ? "Return to service" : "Drain"}<` — and
 * the second one is preceded by `}`, so the pattern never reaches it, while
 * `TEXT_MIXED` gives up on the tag inside the first (#1594). Walking forward
 * from each tag end instead reads the whole run, and the braces carry the
 * nested elements with them — their tags are blanked before the strings inside
 * are read, since a prop is not children.
 *
 * The text between the expressions is read on the same walk. `TEXT_MIXED`
 * cannot take an expression holding a `>`, so `{rows.length} nodes · {live}
 * live {lagging > 0 && …}` hid its whole sentence from the gate (#1745).
 */
function childRuns(masked: string, tags: Map<number, number>, inside: Set<number>): ChildRun[] {
  const out: ChildRun[] = [];
  for (const end of tags.values()) {
    const run: ChildRun = { start: end, text: null, exprs: [] };
    // after the outermost element closes the text is code again: read only
    // the expressions written flush against the tag, as before (#1594)
    const children = inside.has(end);
    const parts: string[] = [];
    let prose = false;
    let i = end + 1;
    let reached = false;
    while (i < masked.length) {
      if (masked[i] === "<") {
        reached = true;
        break;
      }
      if (masked[i] !== "{") {
        const from = i;
        while (i < masked.length && masked[i] !== "{" && masked[i] !== "<") i++;
        const text = masked.slice(from, i);
        if (!children && text.trim()) break;
        // a `}`, `>` or `;` in the run, or a `(` that opens onto the next tag,
        // is code — the run left the children it started in
        if (/[}>;]/.test(text) || /\(\s*$/.test(text)) break;
        if (/[A-Za-z]/.test(text)) prose = true;
        parts.push(text);
        continue;
      }
      const close = skipExpression(masked, i + 1, "");
      if (masked[close] !== "}") break;
      run.exprs.push([i + 1, close]);
      parts.push(JSX_SPACE.test(masked.slice(i, close + 1)) ? " " : PLACEHOLDER);
      i = close + 1;
    }
    // prose only counts when the run reached the next tag; one that ran into
    // code has no sentence in it
    if (prose && run.exprs.length && reached) run.text = parts.join("");
    out.push(run);
  }
  return out;
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
  const scanned = blankTCalls(masked.text);
  const tags = jsxTags(scanned);
  // the tag ends that open onto children; text after any other one is code
  const inside = childPositions(scanned, tags);

  const push = (index: number, raw: string, kind: Literal["kind"], strict = false) => {
    const text = normalize(raw);
    if (isNotCopy(text, strict)) return;
    const line = lineOf(source, masked.map[index] ?? 0);
    const key = `${line}:${text}`;
    if (seen.has(key)) return;
    seen.add(key);
    out.push({ file, line, text, kind });
  };
  /** every candidate in an expression that renders as it stands */
  const pushAll = (cs: Candidate[], kind: Literal["kind"]) => {
    for (const c of cs) push(c.index, c.text, kind, c.strict);
  };

  // identifiers rendered bare where copy goes, with the kind they render as.
  // their bindings are read once every position is known (#1537)
  const rendered = new Map<string, Literal["kind"]>();
  // tables with an entry rendered where copy goes, read value by value
  const tables = new Map<string, Literal["kind"]>();
  /** records a rendered identifier, or a table indexed where copy goes */
  const noteRendered = (expr: string, kind: Literal["kind"]) => {
    const ident = RENDERED_IDENT.exec(expr);
    if (ident) rendered.set(ident[1], rendered.get(ident[1]) ?? kind);
    const entry = RENDERED_ENTRY.exec(expr);
    if (entry) tables.set(entry[1], tables.get(entry[1]) ?? kind);
  };
  /** the start of capture group `n`'s text inside match `m` */
  const groupAt = (m: RegExpMatchArray, n: number) => (m.index ?? 0) + m[0].lastIndexOf(m[n]);
  const readExpr = (m: RegExpMatchArray, n: number, kind: Literal["kind"]) => {
    noteRendered(m[n], kind);
    pushAll(stringsIn(m[n], groupAt(m, n), true), kind);
  };

  for (const m of scanned.matchAll(DIALOG)) push(m.index, m[2], "dialog");
  for (const m of scanned.matchAll(THROWN)) push(m.index, m[2], "error");
  for (const m of scanned.matchAll(THROWN_TEMPLATE)) {
    const at = m.index + m[0].length;
    const { end } = readTemplate(scanned, at);
    pushAll(stringsIn(scanned.slice(at, end), at), "error");
  }
  // a message set from a handler (#1745)
  for (const m of scanned.matchAll(COPY_SETTER)) {
    const at = m.index + m[0].length;
    pushAll(stringsIn(scanned.slice(at, skipExpression(scanned, at, ",")), at, true), "error");
  }
  for (const m of scanned.matchAll(PROP)) push(m.index, m[1], "prop", true);
  for (const m of scanned.matchAll(PROP_EXPR)) readExpr(m, 1, "prop");
  // not strict: a bare word under a copy key is as often an option echoing its
  // own wire value (`{ value: "json", label: "json" }`) as it is a label
  for (const m of scanned.matchAll(OBJECT_KEY)) {
    const at = m.index + m[0].length;
    const end = skipExpression(scanned, at, ",;");
    pushAll(stringsIn(scanned.slice(at, end), at), "prop");
  }
  // a lookup table's labels, which no prop name can reach (#1599)
  for (const m of scanned.matchAll(FLAT_OBJECT)) {
    for (const c of codeMapLabels(m[0].slice(1, -1), m.index + 1) ?? [])
      push(c.index, c.text, "prop");
  }
  for (const m of scanned.matchAll(TEXT)) {
    if (!inside.has(m.index)) continue;
    push(m.index, m[1], "text", true);
  }
  for (const m of scanned.matchAll(TEXT_EXPR)) readExpr(m, 1, "text");
  for (const m of scanned.matchAll(TEXT_MIXED)) {
    if (!inside.has(m.index)) continue;
    const text = mixedText(m[1]);
    if (text !== null) push(m.index, text, "text");
    // the expressions beside the prose hold copy of their own (#1371)
    const base = groupAt(m, 1);
    for (const e of m[1].matchAll(EXPR_IN_TEXT)) {
      noteRendered(e[0].slice(1, -1), "text");
      pushAll(stringsIn(e[0], base + e.index, true), "text");
    }
  }
  const children = blankTags(scanned, tags);
  for (const run of childRuns(scanned, tags, inside)) {
    // prose beside an expression `TEXT_MIXED` could not read, because the
    // expression carries a `>` of its own: `{n} nodes {late > 0 && …}` (#1745)
    if (run.text !== null) push(run.start, run.text, "text");
    for (const [from, to] of run.exprs) {
      const expr = children.slice(from, to);
      noteRendered(expr, "text");
      pushAll(stringsIn(expr, from, true), "text");
    }
  }
  // English parked in a local and rendered later: `const cta = add ? "Create" :
  // "Save"` then `<Button>{cta}</Button>` (#1537), or a table indexed where
  // copy goes, `{HINTS[mode]}` (#1745). only a binding whose name is rendered
  // is read, so a string that only ever reaches code stays out. a function is
  // not a held value — its body is the rest of a component
  if (rendered.size || tables.size) {
    for (const m of scanned.matchAll(BINDING)) {
      const at = m.index + m[0].length;
      const value = scanned.slice(at, skipExpression(scanned, at, ",;"));
      const kind = rendered.get(m[1]);
      if (kind && !value.includes("=>") && !/^\s*(?:async\s+)?function\b/.test(value))
        pushAll(stringsIn(value, at, true), kind);
      const table = tables.get(m[1]);
      // only an object literal is a table; `const remove = useMutation({…})`
      // with `{remove.error}` rendered is a hook result, and its options are code
      if (!table || !/^\s*\{/.test(value)) continue;
      // every value of `{ allow: "…", deny: "…" }` is what the index renders
      for (const e of value.matchAll(MAP_ENTRY))
        push(at + (e.index ?? 0) + e[0].lastIndexOf(e[4]), e[4], table, true);
    }
  }
  out.sort((a, b) => a.line - b.line);
  return out;
}

/** Findings not on the allow-list — the ones that fail the build. */
export function newViolations(found: Literal[], allowed: AllowList): Literal[] {
  return found.filter((l) => !Object.prototype.hasOwnProperty.call(allowed[l.file] ?? {}, l.text));
}

/**
 * Allow-list entries no longer found in the source. The string was translated,
 * reworded or deleted; leaving the exception behind would let the same literal
 * come back unnoticed.
 */
export function staleAllowed(found: Literal[], allowed: AllowList): string[] {
  const live = new Set(found.map((l) => `${l.file}\0${l.text}`));
  const stale: string[] = [];
  for (const [file, texts] of Object.entries(allowed)) {
    for (const text of Object.keys(texts)) {
      if (!live.has(`${file}\0${text}`)) stale.push(`${file}: ${text}`);
    }
  }
  return stale;
}

/** Allow-list entries that do not say why — an exception has to be argued. */
export function unexplainedAllowed(allowed: AllowList): string[] {
  const out: string[] = [];
  for (const [file, texts] of Object.entries(allowed)) {
    for (const [text, reason] of Object.entries(texts)) {
      if (!reason.trim()) out.push(`${file}: ${text}`);
    }
  }
  return out;
}
