// markdown for the Playground (#955).
//
// Models reply in markdown. Showing the backticks instead of the formatting
// makes the Playground a worse client than any client an operator would
// actually use, which defeats the point of a screen that exists to judge
// whether a route behaves.
//
// ## Untrusted by construction
//
// A model reply is attacker-influenced text: whatever is in the prompt, the
// retrieved documents or the tool output can steer it. So this module never
// produces HTML. `marked` is used as a *lexer* only — `marked.lexer()`, never
// `marked.parse()` — and its tokens are mapped onto the small tree below,
// which `Markdown.tsx` renders as React elements. React escapes text nodes, so
// there is no `dangerouslySetInnerHTML` anywhere in the path and no sanitiser
// to keep in step with a parser. The three things a reply could otherwise do:
//
//   - raw HTML: `html` tokens become literal text, so `<script>…</script>`
//     renders as the characters an operator can read and nothing runs
//   - a `javascript:` link: hrefs are allowlisted to http/https/mailto and
//     anything else drops to plain text rather than becoming a live anchor
//   - a remote image: never emitted as `<img>` at all. It would breach the
//     air-gapped guarantee and tell a third party that the reply was read
//
// The module is loaded through a dynamic `import()` so `marked` sits in its own
// chunk; until it resolves the Playground shows the raw text, which is exactly
// the "raw" view the toggle offers anyway.
import { marked, type Token, type Tokens } from "marked";

import { resolveLanguage, type CodeLanguage } from "@/lib/code";

/** Inline content: everything that can appear inside a paragraph. */
export type MdInline =
  | { type: "text"; value: string }
  | { type: "code"; value: string }
  | { type: "strong"; children: MdInline[] }
  | { type: "em"; children: MdInline[] }
  | { type: "del"; children: MdInline[] }
  /** `href` is null when the URL failed the allowlist: rendered as plain text */
  | { type: "link"; href: string | null; children: MdInline[] }
  /** never an `<img>`; see the header */
  | { type: "image"; alt: string; href: string }
  | { type: "break" };

/** Block content. */
export type MdBlock =
  | { type: "paragraph"; children: MdInline[] }
  | { type: "heading"; depth: number; children: MdInline[] }
  | { type: "code"; language: CodeLanguage; value: string }
  | { type: "list"; ordered: boolean; start: number; items: MdBlock[][] }
  | { type: "table"; header: MdInline[][]; rows: MdInline[][][] }
  | { type: "blockquote"; children: MdBlock[] }
  | { type: "hr" }
  /** raw html and anything unmapped, shown verbatim rather than interpreted */
  | { type: "raw"; value: string };

/** URL schemes a rendered link may carry. Everything else is inert text. */
const SAFE_SCHEMES = ["http:", "https:", "mailto:"];

/**
 * The href to put on an anchor, or null to render the label as plain text.
 *
 * Relative links are allowed — they cannot leave the dashboard's own origin —
 * and every absolute URL has to name an allowlisted scheme, which is what stops
 * `javascript:`, `data:` and `vbscript:` from becoming a live anchor.
 */
export function safeHref(href: string | null | undefined): string | null {
  if (!href) return null;
  const raw = href.trim();
  if (!raw) return null;
  // a scheme-relative or absolute url is parsed; anything else is relative
  if (!/^[a-z][a-z0-9+.-]*:/i.test(raw)) return raw.startsWith("//") ? null : raw;
  try {
    return SAFE_SCHEMES.includes(new URL(raw).protocol) ? raw : null;
  } catch {
    return null;
  }
}

/**
 * Lex `source` into the tree above.
 *
 * Total: any input produces a tree, including a half-streamed reply whose fence
 * has not closed yet — `marked` treats an unterminated ``` as a code block
 * running to the end of the input, which is exactly the right behaviour while
 * tokens are still arriving.
 */
export function parseMarkdown(source: string): MdBlock[] {
  let tokens: Token[];
  try {
    tokens = marked.lexer(source, { gfm: true, breaks: false });
  } catch {
    // a lexer that throws must not cost the operator the reply
    return [{ type: "raw", value: source }];
  }
  return blocks(tokens);
}

function blocks(tokens: readonly Token[]): MdBlock[] {
  const out: MdBlock[] = [];
  for (const token of tokens) {
    const block = mapBlock(token);
    if (block) out.push(block);
  }
  return out;
}

function mapBlock(token: Token): MdBlock | null {
  switch (token.type) {
    case "space":
    case "def":
      return null;
    case "heading": {
      const t = token as Tokens.Heading;
      return { type: "heading", depth: t.depth, children: inlines(t.tokens) };
    }
    case "code": {
      const t = token as Tokens.Code;
      return { type: "code", language: resolveLanguage(t.lang), value: t.text };
    }
    case "hr":
      return { type: "hr" };
    case "blockquote": {
      const t = token as Tokens.Blockquote;
      return { type: "blockquote", children: blocks(t.tokens ?? []) };
    }
    case "list": {
      const t = token as Tokens.List;
      return {
        type: "list",
        ordered: t.ordered,
        start: typeof t.start === "number" ? t.start : 1,
        items: t.items.map((item) => blocks(item.tokens ?? [])),
      };
    }
    case "table": {
      const t = token as Tokens.Table;
      return {
        type: "table",
        header: t.header.map((cell) => inlines(cell.tokens)),
        rows: t.rows.map((row) => row.map((cell) => inlines(cell.tokens))),
      };
    }
    case "html":
      // the whole point: markup from a model is text an operator reads, never
      // markup a browser runs
      return { type: "raw", value: (token as Tokens.HTML).raw };
    case "paragraph": {
      const t = token as Tokens.Paragraph;
      return { type: "paragraph", children: inlines(t.tokens) };
    }
    case "text": {
      const t = token as Tokens.Text;
      return { type: "paragraph", children: t.tokens ? inlines(t.tokens) : [{ type: "text", value: t.text }] };
    }
    default:
      return { type: "raw", value: (token as Tokens.Generic).raw ?? "" };
  }
}

function inlines(tokens: readonly Token[] | undefined): MdInline[] {
  if (!tokens) return [];
  const out: MdInline[] = [];
  for (const token of tokens) {
    const node = mapInline(token);
    if (node) out.push(node);
  }
  return out;
}

function mapInline(token: Token): MdInline | null {
  switch (token.type) {
    case "text":
    case "escape":
      return { type: "text", value: (token as Tokens.Text).text };
    case "codespan":
      return { type: "code", value: (token as Tokens.Codespan).text };
    case "strong":
      return { type: "strong", children: inlines((token as Tokens.Strong).tokens) };
    case "em":
      return { type: "em", children: inlines((token as Tokens.Em).tokens) };
    case "del":
      return { type: "del", children: inlines((token as Tokens.Del).tokens) };
    case "br":
      return { type: "break" };
    case "link": {
      const t = token as Tokens.Link;
      return { type: "link", href: safeHref(t.href), children: inlines(t.tokens) };
    }
    case "image": {
      const t = token as Tokens.Image;
      return { type: "image", alt: t.text ?? "", href: t.href ?? "" };
    }
    case "html":
    case "tag":
      return { type: "text", value: (token as Tokens.Generic).raw ?? "" };
    default:
      return { type: "text", value: (token as Tokens.Generic).raw ?? "" };
  }
}
