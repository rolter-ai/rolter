// the dashboard's keyboard shortcuts, as data (#1676).
//
// #1198 shipped ⌘K and `/` as two `if`s inside the shell's keydown handler,
// which is fine until something has to *list* them. A reference sheet written
// by hand is a second source of truth: the shortcut that lands next quarter
// works and is missing from the sheet, and nothing anywhere fails.
//
// So the table below is the only place a shortcut is declared. The shell
// dispatches by walking it, and the sheet renders by mapping it — neither
// names a key of its own. `ShortcutHandlers` is a `Record` over the id union,
// so an entry added here fails to compile until the shell handles it, and
// `shortcuts.test.ts` fails until every catalog names it.

import { isNavSearchShortcut, isPaletteShortcut, isTextEntry } from "@/lib/command-palette";

/** stable ids; the copy is `shell.shortcuts.items.<id>` in every catalog */
export type ShortcutId = "palette" | "navSearch" | "help";

/**
 * The platform modifier, in a chord.
 *
 * Rendered `⌘` on a Mac and `Ctrl` everywhere else — the same either/or the
 * matchers accept, since a keystroke carries one modifier but a *label* has to
 * pick the one this reader's keyboard actually has.
 */
export const MOD = "$mod";

/** one chord: `[MOD, "K"]`, `["/"]` — rendered one `Kbd` per entry */
export type Chord = readonly string[];

/** the sliver of `KeyboardEvent` the matchers read, so a test can fake one */
export interface ShortcutEvent {
  key: string;
  metaKey: boolean;
  ctrlKey: boolean;
  altKey: boolean;
  target: EventTarget | null;
}

export interface ShortcutDef {
  id: ShortcutId;
  /** what the sheet and the inline hints print */
  chord: Chord;
  /** does this keystroke mean this shortcut, right now? */
  matches: (e: ShortcutEvent) => boolean;
}

/**
 * Does `?` mean "show me the shortcuts" right now?
 *
 * The same guard `isNavSearchShortcut` uses, for the same reason: `?` is a
 * character in a prompt, a query string and a model name, and a sheet that
 * opened over the field someone was typing in would be worse than no sheet.
 *
 * Shift is not rejected — on most layouts `?` *is* Shift and a matcher that
 * demanded a bare one would never fire. The other three modifiers are, since
 * `⌥?` and `⌘?` are the OS's or another app's, not ours.
 */
export function isHelpShortcut(e: ShortcutEvent): boolean {
  if (e.key !== "?" || e.metaKey || e.ctrlKey || e.altKey) return false;
  return !isTextEntry(e.target);
}

/**
 * Every shortcut the dashboard binds, in the order the sheet lists them.
 *
 * Order is deliberate: the two a reader is most likely to want first, then the
 * one that showed them this list.
 */
export const SHORTCUTS: readonly ShortcutDef[] = [
  { id: "palette", chord: [MOD, "K"], matches: isPaletteShortcut },
  { id: "navSearch", chord: ["/"], matches: isNavSearchShortcut },
  { id: "help", chord: ["?"], matches: isHelpShortcut },
];

/** what the shell must supply: one action per registered shortcut */
export type ShortcutHandlers = Record<ShortcutId, () => void>;

/**
 * Run the shortcut `e` means, and say whether one was found.
 *
 * The caller preventDefaults on `true`: every chord here is a character or a
 * browser binding that would otherwise land somewhere.
 */
export function dispatchShortcut(e: ShortcutEvent, handlers: ShortcutHandlers): boolean {
  for (const shortcut of SHORTCUTS) {
    if (!shortcut.matches(e)) continue;
    handlers[shortcut.id]();
    return true;
  }
  return false;
}

/**
 * Is this an Apple keyboard — the one with a Command key?
 *
 * `navigator.platform` is deprecated and `userAgentData` is Chromium-only, so
 * both are read and either answer is taken. Getting it wrong prints the wrong
 * glyph in a hint; it never changes which keystroke works, because the
 * matchers accept Meta *and* Control regardless.
 */
export function isApplePlatform(nav: Navigator | null = globalThis.navigator ?? null): boolean {
  if (!nav) return false;
  const hinted = (nav as Navigator & { userAgentData?: { platform?: string } }).userAgentData;
  const source = `${hinted?.platform ?? ""} ${nav.platform ?? ""} ${nav.userAgent ?? ""}`;
  return /mac|iphone|ipad|ipod/i.test(source);
}

/** a chord as the keys to print, with `MOD` resolved for this keyboard */
export function chordKeys(chord: Chord, apple = isApplePlatform()): string[] {
  return chord.map((key) => (key === MOD ? (apple ? "⌘" : "Ctrl") : key));
}

/**
 * A chord as one string, for a `title`, an `aria-label` or an interpolation.
 *
 * No separator on a Mac (`⌘K` is how the glyph is written) and a `+` elsewhere
 * (`Ctrl+K`), which is what each platform's own menus print.
 */
export function chordText(chord: Chord, apple = isApplePlatform()): string {
  return chordKeys(chord, apple).join(apple ? "" : "+");
}

/** the chord registered for `id` — how a hint reaches the table it belongs to */
export function shortcutChord(id: ShortcutId): Chord {
  // non-null by construction: `ShortcutId` is the union of the table's ids,
  // and `shortcuts.test.ts` fails if the table ever drops one
  return SHORTCUTS.find((s) => s.id === id)!.chord;
}
