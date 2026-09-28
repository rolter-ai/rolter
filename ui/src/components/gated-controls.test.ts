import { describe, expect, it } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { Glob } from "bun";

import { stripComments } from "../../scripts/check-ui-primitives";

// #1750: `refused_click` records `<control>:<resource>:<action>`, and the
// control slug is the only part that says *which* refused control on a screen
// was reached for. The type makes `control` required, so a call site cannot
// leave it off — but a required `string` still accepts the button's label, a
// row's name, or the same slug pasted onto two controls, and every one of those
// turns the target back into the non-answer the slug exists to replace. So the
// shape is checked here, from the source, for every shipped call site.
//
// #1759: and the primitives are the only way in. A screen that disables a
// control from its own `useGate` disables it silently — the capture-phase
// wrapper `useRefusedClick` hangs off is on the primitive, not the screen, so
// the reach for it never reaches the stream. Keys and Models shipped exactly
// that for their row edit and delete.

const ROOT = join(import.meta.dir, "..", "..");
/** always gated, so every call site names itself */
const GATED = ["GatedButton", "GatedSwitch", "GatedCombobox", "RowIconButton"];
/** gated only when it is handed a `gate`, and then it names itself too */
const OPTIONALLY_GATED = ["DeleteIconButton", "SwitchRow"];
const OPENING = new RegExp(`<(${[...GATED, ...OPTIONALLY_GATED].join("|")})\\b`, "g");
/**
 * The files that *are* the gated controls, and so the only ones allowed to call
 * `useGate` without saying why: each pairs it with `useRefusedClick`.
 */
const GATE_OWNERS = new Set([
  "src/components/GatedButton.tsx",
  "src/components/GatedSwitch.tsx",
  "src/components/GatedCombobox.tsx",
  "src/components/screen.tsx",
  "src/components/ui/delete-icon-button.tsx",
  "src/components/ui/switch-row.tsx",
  "src/lib/can.tsx",
]);
/** the waiver a genuine non-control use carries in the comment above it */
const WAIVER = /use-gate-allow:\s*\S/;
/** kebab case, at least `<noun>-<verb>` (docs/dev-docs/development/ux-telemetry.md) */
const SLUG = /^[a-z][a-z0-9]*(?:-[a-z0-9]+)+$/;

interface Site {
  file: string;
  line: number;
  tag: string;
  /** the literal slug, or null when the attribute is missing or not a literal */
  slug: string | null;
}

/**
 * The opening tag of every gated control in `source`, read the way the other
 * source guards read the tree (`scripts/check-ui-primitives.ts`): comments
 * blanked, then a scan that tracks braces and quotes so an arrow's `=>` or a
 * `>` inside an expression does not end the tag early. TypeScript 7 ships no
 * JS compiler API, so there is no AST to lean on.
 */
function callSites(file: string, source: string): Site[] {
  const code = stripComments(source);
  const sites: Site[] = [];
  for (const m of code.matchAll(OPENING)) {
    const from = m.index + m[0].length;
    let depth = 0;
    let quote: string | null = null;
    let i = from;
    for (; i < code.length; i += 1) {
      const c = code[i];
      if (quote) {
        if (c === "\\") i += 1;
        else if (c === quote) quote = null;
      } else if (c === '"' || c === "'" || c === "`") quote = c;
      else if (c === "{") depth += 1;
      else if (c === "}") depth -= 1;
      else if (c === ">" && depth === 0) break;
    }
    // only the tag's own attributes, so a nested gated control is never read
    // as this one's
    const attrs = code.slice(from, i);
    if (!GATED.includes(m[1]) && !/(?:^|\s)gate=/.test(attrs)) continue;
    const literal = /(?:^|\s)control=(?:"([^"]*)"|\{\s*"([^"]*)"\s*\})/.exec(attrs);
    sites.push({
      file,
      line: code.slice(0, m.index).split("\n").length,
      tag: m[1],
      slug: literal ? (literal[1] ?? literal[2]) : null,
    });
  }
  return sites;
}

/**
 * Every `useGate(` in `source` that is not waived: a `use-gate-allow: <reason>`
 * somewhere in the run of `//` comment lines directly above the call. The
 * reason is mandatory, the same bargain `check-ui-primitives.ts` strikes — the
 * waiver lives at the point of use, so it cannot outlive the code it excuses.
 */
function unwaivedGates(file: string, source: string): string[] {
  const code = stripComments(source);
  const lines = source.split("\n");
  const found: string[] = [];
  for (const m of code.matchAll(/\buseGate\(/g)) {
    const line = code.slice(0, m.index).split("\n").length;
    let waived = false;
    for (let j = line - 2; j >= 0 && /^\s*\/\//.test(lines[j]); j -= 1) {
      if (WAIVER.test(lines[j])) waived = true;
    }
    if (!waived) found.push(`${file}:${line}`);
  }
  return found;
}

/** fixtures, not shipped UI — a story repeats one slug across its variants */
const FIXTURE = /\.(stories|test)\.tsx?$/;

function shippedSites(): Site[] {
  const sites: Site[] = [];
  for (const file of new Glob("src/**/*.tsx").scanSync({ cwd: ROOT })) {
    if (FIXTURE.test(file)) continue;
    sites.push(...callSites(file, readFileSync(join(ROOT, file), "utf8")));
  }
  return sites;
}

function shippedUnwaivedGates(): string[] {
  const found: string[] = [];
  for (const file of new Glob("src/**/*.{ts,tsx}").scanSync({ cwd: ROOT })) {
    if (FIXTURE.test(file) || GATE_OWNERS.has(file)) continue;
    found.push(...unwaivedGates(file, readFileSync(join(ROOT, file), "utf8")));
  }
  return found;
}

const where = (s: Site) => `${s.file}:${s.line} <${s.tag}>`;

describe("every gated control names itself for refused_click", () => {
  const sites = shippedSites();

  it("passes a literal slug rather than a label or a row's value", () => {
    const bad = sites.filter((s) => s.slug === null).map(where);
    expect(bad).toEqual([]);
  });

  it("spells the slug as kebab-case <noun>-<verb>", () => {
    const bad = sites
      .filter((s) => s.slug !== null && !SLUG.test(s.slug))
      .map((s) => `${where(s)} control="${s.slug}"`);
    expect(bad).toEqual([]);
  });

  it("never gives two controls in one file the same slug", () => {
    const seen = new Map<string, Site>();
    const dupes: string[] = [];
    for (const s of sites) {
      if (s.slug === null) continue;
      const key = `${s.file}\0${s.slug}`;
      const first = seen.get(key);
      if (first) dupes.push(`${where(s)} repeats "${s.slug}" from line ${first.line}`);
      else seen.set(key, s);
    }
    expect(dupes).toEqual([]);
  });

  it("covers every call site, so the check cannot pass by finding nothing", () => {
    expect(sites.length).toBeGreaterThan(100);
  });
});

describe("no screen gates a control by hand", () => {
  it("calls useGate only inside the gated primitives, or says why", () => {
    expect(shippedUnwaivedGates()).toEqual([]);
  });
});

describe("the call-site reader", () => {
  it("rejects an expression, so a label cannot pass as a slug", () => {
    const [site] = callSites(
      "x.tsx",
      `const a = <GatedButton gate="provider:create" control={t("pages.providers.add")} />;`,
    );
    expect(site.slug).toBeNull();
  });

  it("reads both attribute spellings of a literal", () => {
    const sites = callSites(
      "x.tsx",
      `const a = <><RowIconButton control="row-a" /><GatedSwitch control={"row-b"} /></>;`,
    );
    expect(sites.map((s) => s.slug)).toEqual(["row-a", "row-b"]);
  });

  it("flags a call site with no control at all", () => {
    const [site] = callSites(
      "x.tsx",
      `const a = <GatedButton gate="provider:create">Add</GatedButton>;`,
    );
    expect(site.slug).toBeNull();
  });

  it("reads a DeleteIconButton only when it is gated", () => {
    const sites = callSites(
      "x.tsx",
      `const a = <><DeleteIconButton label="x" /><DeleteIconButton gate="provider:delete" label="x" /></>;`,
    );
    expect(sites.map((s) => [s.tag, s.slug])).toEqual([["DeleteIconButton", null]]);
  });
});

describe("the useGate reader", () => {
  it("flags a bare useGate", () => {
    expect(unwaivedGates("x.tsx", `const g = useGate("route:delete");`)).toEqual(["x.tsx:1"]);
  });

  it("honours a waiver anywhere in the comment run above, with a reason", () => {
    const source = [
      "// use-gate-allow: picks a link or a button",
      "// and the button is a GatedButton",
      'const g = useGate("route:create");',
    ].join("\n");
    expect(unwaivedGates("x.tsx", source)).toEqual([]);
  });

  it("refuses a waiver with no reason", () => {
    const source = ["// use-gate-allow:", 'const g = useGate("route:create");'].join("\n");
    expect(unwaivedGates("x.tsx", source)).toEqual(["x.tsx:2"]);
  });

  it("ignores a mention inside a comment", () => {
    expect(unwaivedGates("x.tsx", "// never call useGate( on a screen")).toEqual([]);
  });
});
