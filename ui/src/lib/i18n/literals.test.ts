import { describe, expect, test } from "bun:test";

import {
  findLiterals,
  newViolations,
  staleBaseline,
  toBaseline,
  type Baseline,
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

describe("baseline", () => {
  const found = [
    { file: "a.tsx", line: 1, text: "Old one", kind: "prop" as const },
    { file: "a.tsx", line: 9, text: "Brand new", kind: "text" as const },
  ];
  const baseline: Baseline = { "a.tsx": ["Old one"] };

  test("only the unrecorded literal fails the build", () => {
    expect(newViolations(found, baseline).map((l) => l.text)).toEqual(["Brand new"]);
  });

  test("a literal recorded under a different file still fails", () => {
    expect(newViolations(found, { "b.tsx": ["Old one", "Brand new"] })).toHaveLength(2);
  });

  test("a paid-off baseline entry is reported so it cannot come back unnoticed", () => {
    expect(staleBaseline(found, { "a.tsx": ["Old one", "Since translated"] })).toEqual([
      "a.tsx: Since translated",
    ]);
    expect(staleBaseline(found, baseline)).toEqual([]);
  });

  test("a recorded baseline is deduplicated and stably ordered", () => {
    const recorded = toBaseline([
      { file: "b.tsx", line: 2, text: "Zeta", kind: "prop" as const },
      { file: "a.tsx", line: 1, text: "Beta", kind: "prop" as const },
      { file: "a.tsx", line: 5, text: "Alpha", kind: "prop" as const },
      { file: "a.tsx", line: 7, text: "Alpha", kind: "text" as const },
    ]);
    expect(Object.keys(recorded)).toEqual(["a.tsx", "b.tsx"]);
    expect(recorded["a.tsx"]).toEqual(["Alpha", "Beta"]);
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
