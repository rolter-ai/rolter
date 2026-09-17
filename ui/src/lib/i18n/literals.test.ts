import { describe, expect, test } from "bun:test";

import {
  findLiterals,
  newViolations,
  staleAllowed,
  unexplainedAllowed,
  type AllowList,
} from "./literals";

/** shorthand: the texts a scan found, in order */
function texts(source: string): string[] {
  return findLiterals(source, "f.tsx").map((l) => l.text);
}

describe("findLiterals", () => {
  test("catches the #871 regression it was written for", () => {
    const source = `
      const guard = () => window.confirm("Discard unsaved changes?");
      export function S({ cancelLabel = "Cancel" }) { return null; }
    `;
    expect(texts(source)).toEqual(["Discard unsaved changes?", "Cancel"]);
  });

  test("accepts the same code once it goes through t()", () => {
    const source = `
      const guard = () => window.confirm(t("common.discardChanges"));
      <Button>{cancelLabel ?? t("common.cancel")}</Button>
    `;
    expect(texts(source)).toEqual([]);
  });

  test("catches prose in a user-facing prop and in a JSX text node", () => {
    expect(texts('<Field label="Upstream model name" />')).toEqual(["Upstream model name"]);
    expect(texts("<p>No events in window.</p>")).toEqual(["No events in window."]);
  });

  // the `>` of an arrow is not a closing tag. without this, every `.tsx` file
  // returning a generic from an arrow function reports the type name as copy
  test("ignores a generic return type on an arrow function", () => {
    const source = `
      export type FetchStub = (input: RequestInfo) => Promise<Response>;
      export const pending: FetchStub = scoped(() => new Promise<Response>(() => {}));
      const pick = <T,>(xs: T[]): Array<T> => xs;
    `;
    expect(texts(source)).toEqual([]);
  });

  test("ignores props nobody reads", () => {
    const source = `
      <div className="grid gap-3 md:grid-cols-2" id="model-form" data-testid="sheet" />
      <input type="text" name="apiBase" autoComplete="off" />
    `;
    expect(texts(source)).toEqual([]);
  });

  test("ignores codes, identifiers, urls and numbers", () => {
    const cases = [
      '<Field label="USD" />',
      '<Field label="gpt-4o" />',
      '<Field label="api_key_env" />',
      '<Field placeholder="0.00" />',
      '<Field placeholder="https://api.openai.com" />',
      "<span>RPM</span>",
    ];
    for (const source of cases) {
      expect(texts(source)).toEqual([]);
    }
  });

  test("ignores comments, which ship to nobody", () => {
    const source = `
      // <p>Not actually rendered</p>
      /* <Field label="Also not rendered" /> */
      /**
       * <span>Nor this one</span>
       */
    `;
    expect(texts(source)).toEqual([]);
  });

  test("normalizes whitespace so reflowing a string is not a new violation", () => {
    expect(texts("<p>Two   words</p>")).toEqual(["Two words"]);
  });

  test("reports the line and the kind so the message points somewhere", () => {
    const found = findLiterals('\n\n<Field label="Model type" />', "src/x.tsx");
    expect(found).toEqual([{ file: "src/x.tsx", line: 3, text: "Model type", kind: "prop" }]);
  });
});

describe("allow-list", () => {
  const found = [
    { file: "a.tsx", line: 1, text: "n=1", kind: "text" as const },
    { file: "a.tsx", line: 9, text: "Brand new", kind: "text" as const },
  ];
  const allowed: AllowList = { "a.tsx": { "n=1": "a request parameter" } };

  test("only the literal that is not allowed fails the build", () => {
    expect(newViolations(found, allowed).map((l) => l.text)).toEqual(["Brand new"]);
  });

  test("a literal allowed under a different file still fails", () => {
    expect(newViolations(found, { "b.tsx": { "n=1": "x", "Brand new": "x" } })).toHaveLength(2);
  });

  test("an inherited object key is not an allowed literal", () => {
    const proto = [{ file: "a.tsx", line: 1, text: "toString", kind: "text" as const }];
    expect(newViolations(proto, allowed)).toHaveLength(1);
  });

  test("an entry whose literal left the source is reported so it cannot come back unnoticed", () => {
    expect(staleAllowed(found, { "a.tsx": { "n=1": "x", "Since translated": "x" } })).toEqual([
      "a.tsx: Since translated",
    ]);
    expect(staleAllowed(found, allowed)).toEqual([]);
  });

  test("an entry has to say why it is not copy", () => {
    expect(unexplainedAllowed({ "a.tsx": { "n=1": "  ", "v{…}": "a version" } })).toEqual(["a.tsx: n=1"]);
    expect(unexplainedAllowed(allowed)).toEqual([]);
  });
});

// the blind spots the first scanner had (#1200): one finding per line, prose
// that wraps, strings inside expressions, a confirm split across lines
describe("findLiterals sees what the line-at-a-time scan missed", () => {
  test("reports every literal on a dense line, not just the first", () => {
    const source = '<Button>Cancel</Button><Button>Delete</Button>';
    expect(texts(source)).toEqual(["Cancel", "Delete"]);
  });

  test("reads a paragraph that wraps across lines", () => {
    const source = `
      <p className="text-sm">
        This invitation link is not valid. It may have been used,
        revoked, or expired.
      </p>`;
    expect(texts(source)).toEqual([
      "This invitation link is not valid. It may have been used, revoked, or expired.",
    ]);
  });

  test("reads the strings inside an expression in text position", () => {
    const source = '<Button>{pending ? "Saving…" : "Save group"}</Button>';
    expect(texts(source)).toEqual(["Saving…", "Save group"]);
  });

  test("reads the strings inside an expression-valued user-facing prop", () => {
    const source = '<Sheet title={initial ? "Configure tool group" : "Create tool group"} />';
    expect(texts(source)).toEqual(["Configure tool group", "Create tool group"]);
  });

  test("reads a window.confirm whose literal sits on the next line", () => {
    const source = `
      if (
        window.confirm(
          "Delete this rule? Traffic that matched it will pass through unchecked.",
        )
      ) remove.mutate(id);`;
    expect(texts(source)).toEqual([
      "Delete this rule? Traffic that matched it will pass through unchecked.",
    ]);
  });

  test("still ignores keyboard keys, header names and class lists in expressions", () => {
    const source = `
      <div onKeyDown={(e) => e.key === "Escape" && close()} className={cn("flex items-center", open && "bg-muted")}>
        {label}
      </div>`;
    expect(texts(source)).toEqual([]);
  });

  test("blanks a t() call that spans lines", () => {
    const source = `
      <p>
        {t("pages.acceptInvite.intro", {
          email,
        })}
      </p>`;
    expect(texts(source)).toEqual([]);
  });
});

// an error's message is what LoadError prints under its heading (#1200)
test("reads the message of a thrown error", () => {
  expect(texts('throw new ApiError("gateway request failed: ${res.status}", 502);')).toEqual([
    "gateway request failed: ${res.status}",
  ]);
  expect(texts('throw new Error("scope-1");')).toEqual([]);
});

// the ratchet only holds if the detected set is a property of the code and not
// of where the lines break (#1143)
describe("findLiterals is independent of formatting", () => {
  /** the same component, once as dense one-line JSX and once as a formatter
   * would wrap it. nothing is added or removed, only whitespace moves */
  const oneLine = `
    export function Composer({ pick, busy, cancelLabel = "Cancel" }: Props) {
      return <div className="flex gap-2"><input type="file" accept="image/*" onChange={pick} /><Button aria-label="Attach image"><Paperclip className="h-4 w-4" /></Button><Field label="Upstream model name" placeholder="Message…" /><p className="text-sm">Upload an audio file to transcribe.</p><Button disabled={busy}>{busy ? "Sending…" : "Send prompt"}</Button></div>;
    }
    /* ---------------- transcript ---------------- */
  `;
  const wrapped = `
    export function Composer({
      pick,
      busy,
      cancelLabel = "Cancel",
    }: Props) {
      return (
        <div className="flex gap-2">
          <input
            type="file"
            accept="image/*"
            onChange={pick}
          />
          <Button aria-label="Attach image">
            <Paperclip className="h-4 w-4" />
          </Button>
          <Field
            label="Upstream model name"
            placeholder="Message…"
          />
          <p className="text-sm">
            Upload an audio file to transcribe.
          </p>
          <Button disabled={busy}>
            {busy ? "Sending…" : "Send prompt"}
          </Button>
        </div>
      );
    }
    /* ---------------- transcript ---------------- */
  `;

  test("one-line and wrapped spellings yield the identical literal set", () => {
    expect(new Set(texts(wrapped))).toEqual(new Set(texts(oneLine)));
  });

  test("and that set is the copy actually in the source", () => {
    expect([...new Set(texts(wrapped))].sort()).toEqual([
      "Attach image",
      "Cancel",
      "Message…",
      "Send prompt",
      "Sending…",
      "Upload an audio file to transcribe.",
      "Upstream model name",
    ]);
  });

  // `accept="image/*"` opened a block comment for the comment-stripping regex,
  // so every literal between it and the next `*/` — a hundred lines later in
  // `Playground.tsx` — was invisible to the gate (#1143)
  test("a `/*` inside a string literal does not open a comment", () => {
    const source = `
      <input accept="image/*" />
      <Button aria-label="Attach image">Attach</Button>
      /* ---------------- next section ---------------- */
    `;
    expect(texts(source)).toEqual(["Attach image", "Attach"]);
  });

  // `HEADER_NAME = /^[A-Za-z0-9!#$%&'*+.^_\`|~-]+$/` in ClientSettings.tsx carries
  // a quote, a backtick and a `*`: read as ordinary code it desynchronises
  // everything after it
  test("a regex literal is not read as a string, a comment or copy", () => {
    const source = [
      "const HEADER_NAME = /^[A-Za-z0-9!#$%&'*+.^_`|~-]+$/;",
      '<Field label="Header name" />',
    ].join("\n");
    expect(texts(source)).toEqual(["Header name"]);
  });

  // JSX prose is not code: an apostrophe in it does not open a string, and a
  // URL in it does not open a comment
  test("keeps reading prose that carries an apostrophe or a url", () => {
    expect(texts("<p>The gateway&apos;s base URL</p>")).toEqual(["The gateway&apos;s base URL"]);
    expect(texts("<code>https://your-rolter-host/scim/v2</code>")).toEqual([]);
    expect(texts("<p>Paste it into your connector</p><p>Rotate the token</p>")).toEqual([
      "Paste it into your connector",
      "Rotate the token",
    ]);
  });

  // a multi-line template literal is a value: its newlines are not formatting
  test("leaves a multi-line template literal alone", () => {
    const source = [
      "const snippet = `curl ${base}/v1/chat/completions \\\\",
      '  -H "Authorization: Bearer $KEY"`;',
      '<Field label="Request preview" />',
    ].join("\n");
    expect(texts(source)).toEqual(["Request preview"]);
  });
});

// prose that shares a text node with an interpolation was invisible to the
// gate: `TEXT` matches a run that may not contain a brace, so a whole
// grammatical class of copy — the class that most needs a catalog entry,
// because interpolation order differs by language — never entered the ratchet
// (#1355)
describe("findLiterals sees prose beside an interpolation", () => {
  test("reads text that follows an interpolation", () => {
    expect(texts("<p>{active.name} owns external enforcement</p>")).toEqual([
      "{…} owns external enforcement",
    ]);
  });

  test("reads text that precedes an interpolation", () => {
    expect(texts("<p>Charged to {team.name}</p>")).toEqual(["Charged to {…}"]);
  });

  test("reads text on both sides, and between two of them, as one sentence", () => {
    expect(texts("<p>Charged to {name} monthly, {plan} plan</p>")).toEqual([
      "Charged to {…} monthly, {…} plan",
    ]);
    expect(texts("<span>{used} of {limit} keys</span>")).toEqual(["{…} of {…} keys"]);
  });

  // a separator between two values is not copy, and a baseline full of `{…} ·
  // {…}` would bury the sentences that are
  test("ignores a run that is only punctuation or whitespace", () => {
    expect(texts("<span>{a} · {b}</span>")).toEqual([]);
    expect(texts("<span>{count} ({total})</span>")).toEqual([]);
    expect(texts("<span>{first} — {second}</span>")).toEqual([]);
    expect(texts("<Badge>{status}</Badge>")).toEqual([]);
  });

  // `{" "}` is how a formatter is told to keep a space; it is whitespace, not a
  // value, so it does not become a placeholder in the middle of a sentence
  test("treats the explicit JSX space as a space", () => {
    const source = ['<p>', '  Governs {affected.length}{" "}', "  routes today", "</p>"].join("\n");
    expect(texts(source)).toEqual(["Governs {…} routes today"]);
  });

  // the closing `>` of a generic is not the end of a tag. unlike `TEXT`, this
  // pattern spans braces, so a misread `>` swallows whole statements — both of
  // these reported code as copy while the check was missing
  test("does not read a closing generic as a tag", () => {
    const source = [
      "const [rows, setRows] = React.useState<Row[]>({ ok: true });",
      "async function getText(url: string): Promise<string> { return (await fetch(url)).text(); }",
      "export interface StatCardProps extends React.HTMLAttributes<HTMLDivElement> {",
      "  label: React.ReactNode;",
      "}",
      "const ARROWS: Record<string, string> = { up: '^' };",
    ].join("\n");
    expect(texts(source)).toEqual([]);
  });

  // half a sentence through the catalogs is still half a sentence hardcoded
  test("reports the prose left beside a translated fragment", () => {
    expect(texts('<p>{t("pages.x.owner", { name })} today</p>')).toEqual(["{…} today"]);
    expect(texts('<p>{t("pages.x.a")} {t("pages.x.b")}</p>')).toEqual([]);
  });

  test("reports the sentence once, on the line the text starts", () => {
    const found = findLiterals("\n<p>Charged to {name} monthly</p>", "src/x.tsx");
    expect(found).toEqual([
      { file: "src/x.tsx", line: 2, text: "Charged to {…} monthly", kind: "text" },
    ]);
  });
});

// a comparison operator survived `TEXT`'s `(?<!=)` lookbehind, so the `>` of
// `a > b` read as a closing tag and the code up to the next `<` read as a text
// node — code recorded as copy, and #958 would have tried to translate it
// (#1370)
describe("findLiterals tells a comparison from a tag", () => {
  test("ignores a comparison operator", () => {
    const source = [
      "const f = (a: number, b: number, c: number, d: number) => a > b && c < d;",
      "const over = usage.total > limit.total && usage.spend < cap;",
    ].join("\n");
    expect(texts(source)).toEqual([]);
  });

  test("ignores an arrow and a `>=`", () => {
    const source = [
      "const at = (n: number) => n >= threshold && n <= ceiling;",
      "const hot = rows.filter((r) => r.count >= 10 && r.count < 99);",
    ].join("\n");
    expect(texts(source)).toEqual([]);
  });

  test("ignores a closing generic", () => {
    const source = [
      "export interface TableProps<T> extends React.HTMLAttributes<HTMLDivElement> { rows: T[] }",
      "const rows = useQuery<Row[], Error>({ queryKey: ['rows'] });",
    ].join("\n");
    expect(texts(source)).toEqual([]);
  });

  // the comparison must not eat the element behind it either: the run it
  // reported ended at the `<` of the very tag that carries the copy
  test("still reads an element that follows a comparison", () => {
    const source = ["const over = used > limit;", "return <p>Usage is over the limit.</p>;"].join(
      "\n",
    );
    expect(texts(source)).toEqual(["Usage is over the limit."]);
  });

  // the `=>` of a handler sits inside the tag it belongs to, so a tag scan that
  // stopped at the first `>` would drop the label of every button in the app
  test("reads an element whose attributes hold an arrow handler", () => {
    const source = '<Button variant="outline" onClick={() => setOpen(false)}>Cancel</Button>';
    expect(texts(source)).toEqual(["Cancel"]);
  });
});

// prose that reaches the screen without ever sitting in a JSX prop or a text
// node: the donut built its tail slice as `{ label: `Other (${n})` }` (#1482),
// and `ProviderSheet` held its CTA in a `const` it rendered later (#1531).
// both were invisible to the gate (#1537)
describe("findLiterals follows copy through object keys and local bindings", () => {
  test("reads a template literal assigned to a user-facing object key", () => {
    const source = "const tail = { label: `Other (${rest.length})`, value: sum, color: PALETTE[5] };";
    expect(texts(source)).toEqual(["Other ({…})"]);
  });

  test("reads a plain string and a ternary under a user-facing object key", () => {
    expect(texts('const TABS = [{ key: "limits", title: "Rate limits" }];')).toEqual(["Rate limits"]);
    expect(texts('const row = { description: ok ? "Healthy upstream" : "Degraded upstream" };')).toEqual([
      "Healthy upstream",
      "Degraded upstream",
    ]);
    expect(texts('const a11y = { "aria-label": "Close dialog" };')).toEqual(["Close dialog"]);
  });

  test("ignores keys nobody reads, type annotations and a variable named like a key", () => {
    const source = [
      'const opts = { id: "Some Thing", className: "Flex Row", kind: "Upstream Model" };',
      "interface Props { label: string; title?: React.ReactNode }",
      "const shown = open ? label : fallback;",
      "const tail = { label: `${a}/${b}`, title: `/v1/${id}` };",
    ].join("\n");
    expect(texts(source)).toEqual([]);
  });

  test("reads a template literal in a user-facing JSX prop", () => {
    expect(texts("<Sheet title={`Edit ${provider.name}`} />")).toEqual(["Edit {…}"]);
  });

  test("reads English held in a local binding and rendered later", () => {
    const source = `
      const cta = mode === "add" ? "Create provider" : "Save provider";
      const title = \`Edit \${initial.name}\`;
      return (
        <Sheet>
          <SheetHeader title={title} />
          <Button>{cta}</Button>
        </Sheet>
      );`;
    expect(texts(source)).toEqual(["Create provider", "Save provider", "Edit {…}"]);
  });

  test("ignores a local binding that never reaches the screen", () => {
    const source = `
      const kind = "Upstream Model";
      const header = \`Bearer \${token}\`;
      send(kind, { headers: { Authorization: header } });
      return <Button>{t("common.save")}</Button>;`;
    expect(texts(source)).toEqual([]);
  });
});

// `{cond ? "Yes" : "No"} today` reported the prose but not the labels inside
// the expression beside it (#1371)
describe("findLiterals reads expressions that share a text node with prose", () => {
  test("reads the strings inside an interpolation beside prose", () => {
    expect(texts('<span>{healthy ? "All good" : "Degraded"} today</span>')).toEqual([
      "{…} today",
      "All good",
      "Degraded",
    ]);
  });
});

// a template message carrying an embedded quote closed the `THROWN` match on
// the quote rather than on the backtick, so it never matched (#1390)
describe("findLiterals reads template-literal error messages", () => {
  test("reads a thrown template literal with embedded quotes", () => {
    expect(texts('throw new Error(`duplicate param "${key}"`);')).toEqual(['duplicate param "{…}"']);
    expect(texts('throw new Error(`"${key}": not a valid number`);')).toEqual(['"{…}": not a valid number']);
    // a template without quotes in it matched the old pattern too; it is one
    // finding, not the raw `${…}` spelling beside the placeholder one
    expect(texts("throw new Error(`Request failed: ${res.status}`);")).toEqual(["Request failed: {…}"]);
  });
});

// the prop list was closed at ten names, so `FeatureFlags.tsx` kept six English
// descriptions under `desc:` that the gate never reported (#1545). every name
// below is one a dashboard component renders as copy
describe("findLiterals reads every copy-carrying prop name", () => {
  const names: [string, string][] = [
    ["desc", "RelatedLink, FeatureFlags and the settings rows"],
    ["hint", "Field and SwitchRow"],
    ["info", "the InfoHint behind Field and SwitchRow"],
    ["text", "InfoHint"],
    ["tooltip", "a hover explanation"],
    ["detail", "the toast's second line"],
    ["message", "an inline or toast message"],
    ["body", "a dialog or notice body"],
    ["note", "ScopeNote and the cost attribution notes"],
    ["error", "Field's validation line"],
    ["errorMessage", "the create dialogs' failure line"],
    ["alt", "an image's accessible name"],
  ];

  for (const [name, where] of names) {
    test(`${name} (${where})`, () => {
      expect(texts(`<Row ${name}="Retries upstream calls" />`)).toEqual(["Retries upstream calls"]);
      expect(texts(`<Row ${name}={ok ? "Healthy upstream" : "Degraded upstream"} />`)).toEqual([
        "Healthy upstream",
        "Degraded upstream",
      ]);
      expect(texts(`const ROWS = [{ key: "retries", ${name}: "Retries upstream calls" }];`)).toEqual([
        "Retries upstream calls",
      ]);
      expect(texts(`function Row({ ${name} = "Retries upstream calls" }) { return null; }`)).toEqual([
        "Retries upstream calls",
      ]);
    });
  }

  test("a longer name that only ends in one is not a copy prop", () => {
    expect(texts('<Row onError="Retries upstream calls" helpText2="Retries upstream calls" />')).toEqual([]);
  });

  test("a wire value under one of the new keys is still not copy", () => {
    const source = [
      'const COLORS = { error: "var(--status-danger)", text: "text-[color:var(--text-subtle)]" };',
      'const r = { error: "metadataInvalid", body: "{}", message: "ok" };',
    ].join("\n");
    expect(texts(source)).toEqual([]);
  });
});

// any all-lowercase run of class-list characters read as a Tailwind class list,
// so lowercase copy was dropped while the capitalised spelling was reported
// (#1546). a class list is told apart by its tokens, not by its case
describe("findLiterals tells lowercase prose from a class list", () => {
  test("reports a lowercase error message, prop and text node", () => {
    expect(texts("throw new Error(`request failed: ${res.status}`);")).toEqual(["request failed: {…}"]);
    expect(texts('throw new Error("not a valid number");')).toEqual(["not a valid number"]);
    expect(texts('<Field hint="no events yet" />')).toEqual(["no events yet"]);
    expect(texts("<p>none available</p>")).toEqual(["none available"]);
    expect(texts('<Badge label={armed ? "secret set" : "no secret"} />')).toEqual(["secret set", "no secret"]);
  });

  test("reports lowercase prose that carries a class-list character", () => {
    expect(texts('<p title="per-model limits apply" />')).toEqual(["per-model limits apply"]);
    expect(texts('<Field hint="retry in 5s" />')).toEqual(["retry in 5s"]);
    expect(texts("<span>voice: nova</span>")).toEqual(["voice: nova"]);
  });

  test("still ignores real class lists, standalone utilities included", () => {
    const source = [
      'const a = { title: "flex items-center gap-2" };',
      'const b = { title: "relative flex" };',
      'const c = { title: "hidden sm:block truncate" };',
      'const d = { title: "border-b border-[color:var(--border-subtle)] px-3.5 py-[9px]" };',
      'const e = { title: "group relative overflow-hidden" };',
      'const f = { title: "sr-only md:not-sr-only" };',
      'const g = { title: "1px solid transparent" };',
      '<div className={cn("absolute inset-0", open && "block italic")} />',
    ].join("\n");
    expect(texts(source)).toEqual([]);
  });

  // arbitrary values and css functions sat in the literal baseline as copy (#958)
  test("ignores arbitrary values and css functions", () => {
    const source = [
      'const a = { title: "grid grid-cols-[1fr_1.1fr_2fr] items-center gap-3 px-3.5" };',
      'const b = { title: "absolute bottom-[calc(100%+6px)] z-40 rounded-lg shadow-lg" };',
      'const c = { title: "left-0 w-[min(260px,calc(100vw_-_1.5rem))]" };',
      'const d = { title: "color-mix(in srgb, var(--status-success) 32%, transparent)" };',
    ].join("\n");
    expect(texts(source)).toEqual([]);
  });

  test("still reports prose that carries a comma or an underscore", () => {
    expect(texts('<Field hint="retry later, or check request_id" />')).toEqual([
      "retry later, or check request_id",
    ]);
  });
});

// copy that is never JSX text: a dropdown's options were `<option>Off</option>`
// until the combobox migration (#968) moved them into `label:`/`group:`
// properties, and a table's column headers were always data (#1594)
describe("findLiterals reads copy that only ever sits in data", () => {
  test("reads an option's label and description", () => {
    const source = [
      "<Combobox",
      "  options={[",
      '    { value: "off", label: "Off", description: "Send the whole answer at once" },',
      '    { value: "on", label: "Streaming on" },',
      "  ]}",
      "/>",
    ].join("\n");
    expect(texts(source)).toEqual(["Off", "Send the whole answer at once", "Streaming on"]);
  });

  test("reads the group header an option sits under", () => {
    expect(
      texts('const OPTIONS = [{ value: "gpt-4o", label: t("models.gpt4o"), group: "Chat models" }];'),
    ).toEqual(["Chat models"]);
  });

  test("reads a table column's header", () => {
    const source = [
      "const columns: TableColumn<Row>[] = [",
      '  { key: "at", header: "Time", mono: true },',
      '  { key: "action", header: "Action", render: (v) => <Badge>{v}</Badge> },',
      "];",
    ].join("\n");
    expect(texts(source)).toEqual(["Time", "Action"]);
  });

  test("still ignores a wire value or a class list under the new keys", () => {
    const source = [
      'const a = { header: "x-request-id", group: "chat" };',
      'const b = { header: "flex items-center gap-2" };',
      'const c = { headers: { Authorization: "Bearer abc" }, headerGroup: "Chat models" };',
    ].join("\n");
    expect(texts(source)).toEqual([]);
  });

  test("reads every expression child, not only an element's single one", () => {
    const source = [
      "<GatedButton onClick={() => drain.mutate(row.id)}>",
      '  {pending && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}',
      '  {draining ? "Return to service" : "Drain"}',
      "</GatedButton>",
    ].join("\n");
    expect(texts(source)).toEqual(["Return to service", "Drain"]);
  });

  test("does not read a nested element's props as children", () => {
    const source = [
      "<div>",
      "  {icon && (",
      '    <span className="inline-flex h-11 w-11 items-center [&>svg]:h-5">{icon}</span>',
      "  )}",
      '  {rows.map((r) => <circle key={r.id} transform={`rotate(-90 ${r.x} ${r.y})`} />)}',
      "</div>",
    ].join("\n");
    expect(texts(source)).toEqual([]);
  });

  test("does not read a prop's object value as a child", () => {
    const source = [
      '<div style={{ color: "red" }} aria-hidden="true" />',
      "<Chart data={rows} options={{ legend: false }} />",
    ].join("\n");
    expect(texts(source)).toEqual([]);
  });
});
