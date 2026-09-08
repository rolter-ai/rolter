// the dashboard's syntax-highlighting model (#949).
//
// Every structured surface in the dashboard — a request payload in the Logs
// drawer, an effective-config section, the collector document, a copy-as-code
// snippet, a fenced block in a model reply — used to render as undifferentiated
// monospace. This module is the shared vocabulary those surfaces speak:
// a closed language set, an alias table for whatever tag a model or a caller
// happens to write, and a tiny token tree the renderer walks.
//
// Nothing here imports the tokeniser. `code-highlight.ts` does, and it is
// reached through a dynamic `import()` from `CodeBlock`, so a screen that never
// shows code pays nothing for the grammars. See docs/development/highlighting.md.

/**
 * The languages the dashboard highlights.
 *
 * Closed on purpose: each entry costs a Prism grammar in the lazy chunk, and
 * the list is the union of what the dashboard actually displays — its own
 * config formats, the payloads it proxies, the snippet languages, and the
 * handful of fence tags a model realistically emits.
 */
export const CODE_LANGUAGES = [
  "text",
  "json",
  "toml",
  "yaml",
  "bash",
  "csv",
  "log",
  "python",
  "javascript",
  "typescript",
  "sql",
  "markdown",
] as const;

export type CodeLanguage = (typeof CODE_LANGUAGES)[number];

/** Anything with no grammar renders as plain text rather than not at all. */
export const DEFAULT_LANGUAGE: CodeLanguage = "text";

/**
 * Fence tags and informal names that mean one of the languages above.
 *
 * Model output is the reason this is generous: a reply says ```js, ```sh,
 * ```console or ```yml far more often than it says the canonical name, and a
 * fence that falls through to plain text is a visibly worse answer than the
 * same fence highlighted.
 */
const ALIASES: Record<string, CodeLanguage> = {
  js: "javascript",
  jsx: "javascript",
  mjs: "javascript",
  cjs: "javascript",
  node: "javascript",
  ts: "typescript",
  tsx: "typescript",
  py: "python",
  python3: "python",
  sh: "bash",
  shell: "bash",
  zsh: "bash",
  console: "bash",
  curl: "bash",
  yml: "yaml",
  jsonl: "json",
  json5: "json",
  ndjson: "json",
  md: "markdown",
  mdx: "markdown",
  logs: "log",
  postgres: "sql",
  postgresql: "sql",
  psql: "sql",
  plain: "text",
  plaintext: "text",
  txt: "text",
};

/**
 * Normalise a language tag to one this dashboard can highlight.
 *
 * Untrusted input reaches this: a fence tag comes straight out of a model
 * reply, so it can be anything at all. Unknown tags resolve to `text` instead
 * of throwing — an unrecognised fence still has to render.
 */
export function resolveLanguage(tag: string | null | undefined): CodeLanguage {
  if (!tag) return DEFAULT_LANGUAGE;
  // a fence may carry metadata after the tag: ```ts title="x"
  const name = tag.trim().split(/[\s,{]/)[0].toLowerCase();
  if ((CODE_LANGUAGES as readonly string[]).includes(name)) return name as CodeLanguage;
  return ALIASES[name] ?? DEFAULT_LANGUAGE;
}

/**
 * A highlighted token tree: a string is literal text, an element is a span
 * carrying Prism's token classes, which `index.css` colours from the design
 * tokens.
 *
 * Deliberately not hast. The renderer only ever needs a class and children, and
 * a plain structure like this is comparable in a unit test without a DOM.
 */
export type CodeNode = string | CodeElement;

export interface CodeElement {
  /** the token classes, space-separated, e.g. `token string` */
  className: string;
  children: CodeNode[];
}

/**
 * Above this many characters a value renders unhighlighted.
 *
 * Prism tokenises synchronously on the main thread, and a snapshot of a large
 * deployment's effective config or a multi-megabyte response body would hold it
 * long enough to drop frames. The text is still shown in full; only the colour
 * is dropped, which is the right thing to give up.
 */
export const HIGHLIGHT_CHAR_LIMIT = 100_000;

/** Concatenate a token tree back to its source text. */
export function nodeText(nodes: readonly CodeNode[]): string {
  let out = "";
  for (const node of nodes) {
    out += typeof node === "string" ? node : nodeText(node.children);
  }
  return out;
}

/**
 * Cut a token tree into one tree per line.
 *
 * Line numbers are drawn by a CSS counter on each line element, so the gutter
 * never becomes part of a selection or a copy. That needs a real element per
 * line, and a token can straddle a newline — a block comment, a multi-line
 * string — so the enclosing spans are reopened on the following line rather
 * than the line being split naively on `\n`.
 */
export function splitLines(nodes: readonly CodeNode[]): CodeNode[][] {
  const lines: CodeNode[][] = [[]];
  for (const node of nodes) {
    const produced = splitNode(node);
    lines[lines.length - 1].push(...produced[0]);
    for (let i = 1; i < produced.length; i++) lines.push(produced[i]);
  }
  return lines;
}

function splitNode(node: CodeNode): CodeNode[][] {
  if (typeof node === "string") {
    return node.split("\n").map((part) => (part ? [part] : []));
  }
  return splitLines(node.children).map((line) =>
    line.length ? [{ className: node.className, children: line }] : [],
  );
}
