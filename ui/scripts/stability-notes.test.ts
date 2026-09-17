import { describe, expect, it } from "bun:test";
import { readFileSync } from "node:fs";
import { fileURLToPath, URL } from "node:url";

import en from "../src/lib/i18n/locales/en.json";

// the experimental note is rendered from the catalogs, keyed by subsystem id,
// rather than from the English prose the control plane sends (#1401). that
// moves the copy into `ui/`, so this is what stops a subsystem being marked in
// `crates/rolter-core/src/stability.rs` without a note the rail can show, and a
// note outliving the row it explained. `check:i18n` then holds every other
// catalog to the same key set as `en`
const STABILITY = fileURLToPath(
  new URL("../../crates/rolter-core/src/stability.rs", import.meta.url),
);

/** the `id`s of `SUBSYSTEMS`, in table order */
function subsystemIds(source: string): string[] {
  const start = source.indexOf("pub const SUBSYSTEMS");
  if (start < 0) throw new Error("SUBSYSTEMS not found in stability.rs");
  const end = source.indexOf("\n];", start);
  const table = source.slice(start, end < 0 ? undefined : end);
  return [...table.matchAll(/\bid:\s*"([a-z0-9_]+)"/g)].map((m) => m[1]);
}

describe("stability notes", () => {
  it("reads the ids out of the table", () => {
    const source = `
pub const SUBSYSTEMS: &[SubsystemStability] = &[
    SubsystemStability {
        id: "labels",
        note: "an id: \\"elsewhere\\" in prose is not a row",
    },
    SubsystemStability { id: "realtime", note: "x" },
];
const OTHER: &[&str] = &[id: "not_a_row"];
`;
    expect(subsystemIds(source)).toEqual(["labels", "realtime"]);
  });

  it("every experimental subsystem has exactly one catalog note", () => {
    const ids = subsystemIds(readFileSync(STABILITY, "utf8"));
    expect(ids.length).toBeGreaterThan(0);
    const notes = (en as { stability: { notes: Record<string, string> } }).stability.notes;
    expect(Object.keys(notes).sort()).toEqual([...ids].sort());
  });
});
