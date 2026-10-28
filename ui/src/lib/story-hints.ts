import { expect, userEvent, within } from "storybook/test";

// Fixture for stories that assert the (i) note beside a form field (#487).
//
// `Field` (and `FieldLabel`, `SwitchRow`) draw an `InfoHint` when they are given
// `info`: a button named "About <label>" (`common.aboutField`) that opens a
// `role="tooltip"` note on hover or focus. A story points at the button, reads
// the note and moves away, so each screen asserts its own copy without
// restating the open-and-close dance (the primitive's own behaviour is in
// `components/ui/info-hint.stories.tsx`).
//
// Not a `.stories.tsx` file: it is a fixture, like `story-tokens.ts`.

/**
 * Point at the (i) named `trigger` inside `scope`, check the note it opens says
 * `says` and is whole on screen, then move away and check it closes.
 *
 * "Whole" is hit-tested at both edges of the note rather than read from its
 * rectangle: a note that an ancestor's `overflow` cuts off keeps its rectangle
 * and loses the clipped part, which no pointer can reach (the dialog body cut
 * the old, absolutely placed note at either edge, #487).
 */
export async function expectFieldHint(
  scope: HTMLElement,
  trigger: string | RegExp,
  says: string | RegExp,
): Promise<void> {
  const button = await within(scope).findByRole("button", { name: trigger });
  await userEvent.hover(button);
  const note = within(scope).getByRole("tooltip");
  await expect(note).toHaveTextContent(says);
  await expect(button).toHaveAccessibleDescription(says);

  const box = note.getBoundingClientRect();
  await expect(box.left).toBeGreaterThanOrEqual(0);
  await expect(box.right).toBeLessThanOrEqual(window.innerWidth);
  const middle = box.top + box.height / 2;
  for (const x of [box.left + 2, box.right - 2]) {
    await expect(note.contains(document.elementFromPoint(x, middle))).toBe(true);
  }

  await userEvent.unhover(button);
  await expect(within(scope).queryByRole("tooltip")).toBeNull();
}

/**
 * `expectFieldHint` for several fields of one form, in the order they sit.
 *
 * `aboutField` is the catalog's `common.aboutField`, so the button each note is
 * reached by is named in the locale the story runs in. Each pair is the field's
 * visible label and what its note says.
 */
export async function expectFieldHints(
  scope: HTMLElement,
  aboutField: string,
  notes: readonly (readonly [label: string, says: string])[],
): Promise<void> {
  for (const [label, says] of notes) {
    await expectFieldHint(scope, aboutField.replace("{{label}}", label), says);
  }
}
