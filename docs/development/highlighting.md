# Dashboard code highlighting and markdown

Every structured payload the dashboard shows — a request body in the Logs and
MCP Logs drawers, an expanded audit entry's detail, an effective-config
section, a model's config preview, the client example request, the
OpenTelemetry collector document, a copy-as-code snippet, a fenced block in a
model reply — renders through one component:

```tsx
import { CodeBlock } from "@/components/ui/code-block";

<CodeBlock value={json} language="json" label="Request body" wrap />;
```

**Never hand-roll a `<pre>` for structured content.** A raw `<pre>` gets the
keyboard story wrong (a scroll container has to be focusable, #1181), has no
copy affordance, and is one more place the palette can drift. `CodeBlock` owns
all three.

## The component

| Prop | Default | Notes |
|---|---|---|
| `value` | — | the source, verbatim; also exactly what the copy button writes |
| `language` | `"text"` | one of `CODE_LANGUAGES` in `ui/src/lib/code.ts` |
| `label` | — | names the scroll region. Pass one wherever more than one block shares a screen, so the regions are told apart by name rather than by position |
| `wrap` | `false` | soft-wrap instead of scrolling sideways. On in a narrow drawer, off for a snippet — a line broken mid-token reads worse than one that scrolls (#948) |
| `maxHeight` | — | caps the scroll region; a bare number is pixels |
| `lineNumbers` | `false` | a gutter drawn with a CSS counter |
| `copy` | `true` | the copy button, off only where the caller already provides one |
| `density` | `"default"` | `"compact"` is the tighter scale the log drawers use |

The gutter is a **CSS counter**, never a DOM node. Generated content is not part
of the document text, so a selection dragged across a numbered block copies the
code and not the numbers — which is why `lineNumbers` is safe to turn on for a
config section an operator is about to paste into a ticket.

## Why Prism (refractor), not Shiki

Both were measured as a browser bundle carrying the grammars the dashboard
needs (`bun build --minify`, gzipped):

| Candidate | Minified | Gzipped |
|---|---|---|
| `refractor` core + 7 grammars | 56.6 kB | **20.4 kB** |
| `shiki/core` + JS engine + 6 grammars, no theme | 470 kB | 88.1 kB |

Shiki is 4.3× the size before a theme is added, and its accuracy advantage —
TextMate grammars with full scope resolution — buys nothing here: the dashboard
colours nine token roles, not a hundred scopes. Prism's grammars are regex
tables, they cover `csv` and `log` out of the box (which Shiki does not), and
`refractor` ships them as ESM returning hast rather than as a global-mutating
UMD bundle.

Everything is vendored through `bun add` and bundled. **There is no runtime
network access of any kind** — no CDN grammar, no remote theme, no WASM fetch.
The dashboard has to work air-gapped.

## Lazy by construction

`ui/src/lib/code-highlight.ts` is the only module that imports `refractor`, and
`CodeBlock` reaches it through a dynamic `import()` inside an effect. So:

- a screen that shows no code never downloads a grammar;
- the first frame is the plain text, which is also what a screen keeps if the
  chunk fails to load;
- a value past `HIGHLIGHT_CHAR_LIMIT` (100 000 characters) never loads the chunk
  at all and renders unhighlighted. Prism tokenises synchronously on the main
  thread; on a multi-megabyte body colour is the right thing to give up, and the
  content is never truncated.

The chunks are visible in the build output as `code-highlight-*.js` (~24 kB
gzipped) and `markdown-*.js` (~13 kB gzipped).

## The palette comes from the design tokens

`ui/src/index.css` defines a `--code-*` token per role, and every one of them
resolves to a token that already exists:

| Role | Token | Reads as |
|---|---|---|
| keyword, atrule, builtin, variable | `--code-keyword` | `--red-folk-text` |
| string, url, attr-value | `--code-string` | `--status-success-text` |
| number, boolean, constant | `--code-number` | `--status-warning-text` |
| property, key, class-name | `--code-property` | `--status-info-text` |
| comment, separator, null | `--code-comment` | `--text-subtle` |
| punctuation, operator | `--code-punctuation` | `--text-muted` |
| gutter | `--code-gutter` | `--text-subtle` |
| log level error / warning / info | `--code-level-*` | the matching `--status-*-text` |

Two consequences worth stating. A code block always sits on
`--surface-subtle`, which is the surface those `-text` tokens were contrast-
tuned against, so the numbers recorded in `docs/development/dashboard-theme.md`
hold here unchanged. And a log line that says `ERROR` is the same red as the
failing row in the table it was opened from, because both read
`--status-danger-text` — the colours are one vocabulary, not two.

Never add a hex to a highlighting rule. Add a `--code-*` token that resolves to
an existing design token, and if no existing token fits, add the pair (fill and
text) as `dashboard-theme.md` describes.

## Adding a language

1. Add the name to `CODE_LANGUAGES` in `ui/src/lib/code.ts`, and any informal
   spellings a model or an operator might write to `ALIASES` beside it
   (`resolveLanguage` is fed raw fence tags, so it must never throw).
2. Add the display name to `LANGUAGE_NAMES` in
   `ui/src/components/ui/code-block.tsx` — it is the accessible name of the
   scroll region, and it is a code or a proper noun rather than copy.
3. Import the grammar in `ui/src/lib/code-highlight.ts` and add it to
   `GRAMMARS`. `refractor` resolves each grammar's own dependencies, so import
   the language, not its graph.
4. Check the token classes it emits against the `--code-*` rules in
   `index.css`. Anything unstyled falls back to `--code-plain`, which is
   correct but colourless; add the class to the nearest role group if it means
   something a reader should see.
5. Add a story to `code-block.stories.tsx` asserting the token kinds that make
   that language worth colouring, and extend the round-trip case in
   `ui/src/lib/code.test.ts`.

The bundle grows by the grammar, so the list is closed on purpose: add a
language because a surface displays it, not because it might.

## Markdown in the Playground

The Playground renders model output as markdown (#955), through
`ui/src/components/Markdown.tsx` and `ui/src/lib/markdown.ts`. Fenced blocks go
through `CodeBlock`, so they are highlighted and copyable, and a raw/rendered
toggle in the column header switches to the literal characters for when the
question is what the model actually emitted.

**Model output is untrusted input.** Whatever is in the prompt, a retrieved
document, or a tool result can steer it. The rule that makes this safe is that
no HTML is ever produced:

- `marked` is used as a **lexer only** — `marked.lexer()`, never
  `marked.parse()` — and its tokens are mapped onto a small tree with no node
  that can carry markup. `Markdown.tsx` renders that tree as React elements, so
  there is no `dangerouslySetInnerHTML` in the path and no sanitiser to keep in
  step with a parser.
- `html` tokens become **literal text**: `<script>…</script>` renders as the
  characters an operator can read, and nothing runs.
- hrefs are allowlisted to `http`, `https`, `mailto` and relative URLs.
  `javascript:` and `data:` keep their label and lose their link. A rendered
  link carries `rel="noreferrer"`.
- images are **never** emitted as `<img>`. A remote fetch would breach the
  air-gapped guarantee and would tell a third party that the reply had been
  read; the alt text renders instead.

Partial input is normal — a reply is rendered on every SSE chunk — so the parse
is total: an unterminated fence is treated as the code block it is about to
become, and `ui/src/lib/markdown.test.ts` asserts that every prefix of a reply
parses without throwing.
