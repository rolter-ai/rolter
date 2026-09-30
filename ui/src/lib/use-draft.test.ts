import { describe, expect, it } from "bun:test";

import { changedKeys, draftReducer, type DraftState } from "@/lib/use-draft";

interface Form {
  on: boolean;
  text: string;
  secret: string;
}

const LOADED: Form = { on: true, text: "a\nb", secret: "" };
const EMPTY: DraftState<Form> = { saved: null, draft: null };

describe("the draft reducer", () => {
  it("seeds the saved copy and the form from the first load", () => {
    expect(draftReducer(EMPTY, { type: "seed", value: LOADED })).toEqual({
      saved: LOADED,
      draft: LOADED,
    });
  });

  it("keeps an edit when a later load arrives, so a refetch cannot eat it", () => {
    const editing = draftReducer(draftReducer(EMPTY, { type: "seed", value: LOADED }), {
      type: "patch",
      patch: { text: "a\nb\nc" },
    });
    const refetched = draftReducer(editing, {
      type: "seed",
      value: { ...LOADED, text: "changed elsewhere" },
    });
    expect(refetched).toBe(editing);
    expect(refetched.draft?.text).toBe("a\nb\nc");
    expect(refetched.saved).toEqual(LOADED);
  });

  it("patches only the fields it is given", () => {
    const seeded = draftReducer(EMPTY, { type: "seed", value: LOADED });
    const edited = draftReducer(seeded, { type: "patch", patch: { on: false } });
    expect(edited.draft).toEqual({ ...LOADED, on: false });
    expect(edited.saved).toEqual(LOADED);
  });

  it("ignores a patch before anything was loaded", () => {
    expect(draftReducer(EMPTY, { type: "patch", patch: { on: false } })).toBe(EMPTY);
  });

  it("resets every field to what was loaded", () => {
    const seeded = draftReducer(EMPTY, { type: "seed", value: LOADED });
    const edited = draftReducer(seeded, {
      type: "patch",
      patch: { on: false, text: "x", secret: "s3cret" },
    });
    expect(draftReducer(edited, { type: "reset" }).draft).toEqual(LOADED);
  });

  it("adopts the answer of a save as both copies", () => {
    const seeded = draftReducer(EMPTY, { type: "seed", value: LOADED });
    const edited = draftReducer(seeded, { type: "patch", patch: { text: "x", secret: "s3cret" } });
    const answered: Form = { on: true, text: "x", secret: "" };
    const saved = draftReducer(edited, { type: "commit", value: answered });
    expect(saved).toEqual({ saved: answered, draft: answered });
    expect(changedKeys(saved.saved!, saved.draft!)).toEqual([]);
  });
});

describe("the fields that changed", () => {
  it("is empty for a pristine form", () => {
    expect(changedKeys(LOADED, { ...LOADED })).toEqual([]);
  });

  it("names each field that differs, in the form's order", () => {
    expect(changedKeys(LOADED, { on: false, text: "a\nb", secret: "x" })).toEqual(["on", "secret"]);
  });

  it("uses a field's own comparison, so blank lines are not an edit", () => {
    const entries = (a: string, b: string) => {
      const lines = (text: string) =>
        text
          .split("\n")
          .map((l) => l.trim())
          .filter(Boolean);
      return lines(a).join("\n") === lines(b).join("\n");
    };
    expect(changedKeys(LOADED, { ...LOADED, text: "a\n\n b \n" }, { text: entries })).toEqual([]);
    expect(changedKeys(LOADED, { ...LOADED, text: "a\nc" }, { text: entries })).toEqual(["text"]);
  });

  it("goes back to unchanged when an edit is undone", () => {
    const seeded = draftReducer(EMPTY, { type: "seed", value: LOADED });
    const off = draftReducer(seeded, { type: "patch", patch: { on: false } });
    const on = draftReducer(off, { type: "patch", patch: { on: true } });
    expect(changedKeys(on.saved!, on.draft!)).toEqual([]);
  });
});
