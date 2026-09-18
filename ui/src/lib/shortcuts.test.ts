import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, it } from "bun:test";

import {
  MOD,
  SHORTCUTS,
  chordKeys,
  chordText,
  dispatchShortcut,
  isApplePlatform,
  isHelpShortcut,
  shortcutChord,
  type ShortcutEvent,
  type ShortcutHandlers,
} from "./shortcuts";

// the runner has no DOM, so the event target is duck-typed, exactly as
// `command-palette.test.ts` does it
function field(tagName: string, isContentEditable = false): EventTarget {
  return { tagName, isContentEditable } as unknown as EventTarget;
}

const bare: ShortcutEvent = {
  key: "?",
  metaKey: false,
  ctrlKey: false,
  altKey: false,
  target: null,
};

function handlers(fired: string[]): ShortcutHandlers {
  return {
    palette: () => fired.push("palette"),
    navSearch: () => fired.push("navSearch"),
    help: () => fired.push("help"),
  };
}

describe("isHelpShortcut", () => {
  it("opens on a bare question mark", () => {
    expect(isHelpShortcut(bare)).toBe(true);
    // shift is how `?` is typed on most layouts, so it cannot disqualify it
    expect(isHelpShortcut({ ...bare, key: "/" })).toBe(false);
  });

  it("stays out of the way of every other modifier", () => {
    expect(isHelpShortcut({ ...bare, metaKey: true })).toBe(false);
    expect(isHelpShortcut({ ...bare, ctrlKey: true })).toBe(false);
    expect(isHelpShortcut({ ...bare, altKey: true })).toBe(false);
  });

  it("never fires while text is being typed", () => {
    expect(isHelpShortcut({ ...bare, target: field("INPUT") })).toBe(false);
    expect(isHelpShortcut({ ...bare, target: field("TEXTAREA") })).toBe(false);
    expect(isHelpShortcut({ ...bare, target: field("SELECT") })).toBe(false);
    expect(isHelpShortcut({ ...bare, target: field("DIV", true) })).toBe(false);
    // a plain element is not a field, and the sheet opens over it
    expect(isHelpShortcut({ ...bare, target: field("DIV") })).toBe(true);
  });
});

describe("dispatchShortcut", () => {
  it("runs the one shortcut a keystroke means", () => {
    const fired: string[] = [];
    expect(dispatchShortcut(bare, handlers(fired))).toBe(true);
    expect(dispatchShortcut({ ...bare, key: "/" }, handlers(fired))).toBe(true);
    expect(dispatchShortcut({ ...bare, key: "k", metaKey: true }, handlers(fired))).toBe(true);
    expect(fired).toEqual(["help", "navSearch", "palette"]);
  });

  it("leaves a keystroke nothing is bound to alone", () => {
    const fired: string[] = [];
    expect(dispatchShortcut({ ...bare, key: "q" }, handlers(fired))).toBe(false);
    // the guard applies through the dispatcher too, not only to the matcher
    expect(dispatchShortcut({ ...bare, target: field("INPUT") }, handlers(fired))).toBe(false);
    expect(fired).toEqual([]);
  });
});

describe("chords", () => {
  it("prints the modifier this keyboard actually has", () => {
    expect(chordKeys([MOD, "K"], true)).toEqual(["⌘", "K"]);
    expect(chordKeys([MOD, "K"], false)).toEqual(["Ctrl", "K"]);
    expect(chordText([MOD, "K"], true)).toBe("⌘K");
    expect(chordText([MOD, "K"], false)).toBe("Ctrl+K");
    // a chord with no modifier reads the same either way
    expect(chordText(["/"], true)).toBe("/");
    expect(chordText(["/"], false)).toBe("/");
  });

  it("reads the platform from either hint, and survives having neither", () => {
    const apple = { platform: "MacIntel", userAgent: "" } as Navigator;
    const linux = { platform: "Linux x86_64", userAgent: "X11; Linux" } as Navigator;
    const hinted = {
      platform: "",
      userAgent: "",
      userAgentData: { platform: "macOS" },
    } as unknown as Navigator;
    expect(isApplePlatform(apple)).toBe(true);
    expect(isApplePlatform(hinted)).toBe(true);
    expect(isApplePlatform(linux)).toBe(false);
    // neither hint, and no navigator at all: `Ctrl` is the safer guess
    expect(isApplePlatform({} as Navigator)).toBe(false);
    expect(isApplePlatform(null)).toBe(false);
  });

  it("hands a hint the chord off the table rather than a copy of it", () => {
    expect(shortcutChord("palette")).toEqual([MOD, "K"]);
    expect(shortcutChord("navSearch")).toEqual(["/"]);
    expect(shortcutChord("help")).toEqual(["?"]);
  });
});

// the reference sheet is `SHORTCUTS` mapped, so it cannot omit a shortcut —
// but it renders one catalog key per row, and a key that is missing renders as
// the key itself. this is what stops that landing silently (#1676)
describe("the catalogs cover the table", () => {
  const LOCALES = join(import.meta.dir, "i18n", "locales");

  it("names every registered shortcut in every locale", () => {
    const locales = readdirSync(LOCALES).filter((f) => f.endsWith(".json"));
    expect(locales.length).toBeGreaterThan(1);
    for (const file of locales) {
      const catalog = JSON.parse(readFileSync(join(LOCALES, file), "utf8")) as {
        shell: { shortcuts: { items: Record<string, string> } };
      };
      const items = catalog.shell.shortcuts.items;
      // the id is in the message so a failure names the shortcut and the
      // locale rather than only "expected true"
      for (const shortcut of SHORTCUTS) {
        const named = (items[shortcut.id] ?? "").trim() !== "";
        expect(`${file} names ${shortcut.id}: ${named}`).toBe(`${file} names ${shortcut.id}: true`);
      }
      // and no copy left behind for a shortcut that has been removed
      expect(Object.keys(items).sort()).toEqual([...SHORTCUTS].map((s) => s.id).sort());
    }
  });

  it("keeps every id in the table reachable through the union", () => {
    // `shortcutChord` asserts non-null; this is what makes that true
    for (const shortcut of SHORTCUTS) expect(shortcutChord(shortcut.id)).toBe(shortcut.chord);
  });
});
