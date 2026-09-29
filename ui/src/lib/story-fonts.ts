import { expect } from "storybook/test";

// Font fixture for the Storybook preview and the stories that check it (#2051).
//
// `lib/fonts.ts` puts the vendored @font-face rules on the page, but with
// `font-display: swap` a face is fetched only once some text asks for it, and
// until it arrives the browser draws the fallback. The app can live with that
// for a frame. A story cannot: an overflow or truncation check that runs in
// that window measures the fallback's metrics rather than the product's, and a
// screenshot taken in it judges type the dashboard never ships. So the preview
// loads every face before the first story renders, and `lib/fonts.stories.tsx`
// asserts that the faces are really the ones on screen.
//
// Not a `.stories.tsx` file: it is a fixture, like `story-viewport.ts`.

/** The tokens the dashboard sets all of its type from. */
export const FONT_TOKENS = ["--font-sans", "--font-mono"] as const;
export type FontToken = (typeof FONT_TOKENS)[number];

function unquote(family: string): string {
  return family.trim().replace(/^["']|["']$/g, "");
}

/** The first family `var(<token>)` names on this page: the vendored face, ahead of its fallbacks. */
export function tokenFamily(token: FontToken): string {
  const stack = getComputedStyle(document.documentElement).getPropertyValue(token);
  return unquote(stack.split(",")[0] ?? "");
}

/** Every @font-face on the page for `family`, one per subset. */
export function facesOf(family: string): FontFace[] {
  return [...document.fonts].filter((face) => unquote(face.family) === family);
}

/** Why a family has no face on the page, for the preview's log and the story's failure. */
function missingFaces(family: string, token: FontToken): string {
  return (
    `no @font-face for "${family}" (${token}) on this page: import src/lib/fonts.ts from ` +
    ".storybook/preview.ts, or every story renders in a fallback font (#2051)"
  );
}

/**
 * Load every face of the token families before the first story renders.
 *
 * A family with no face at all means `lib/fonts.ts` never reached the page.
 * That is logged rather than thrown: a rejected `beforeAll` stops the preview
 * from starting, and every story would then fail on a Storybook error that
 * names neither the fonts nor this file. `lib/fonts.stories.tsx` fails on it
 * instead, with the message below.
 */
export async function loadTokenFonts(): Promise<void> {
  for (const token of FONT_TOKENS) {
    const family = tokenFamily(token);
    const faces = facesOf(family);
    if (faces.length === 0) console.error(missingFaces(family, token));
    await Promise.all(faces.map((face) => face.load()));
  }
}

/** The code points one `unicode-range` descriptor covers, as inclusive pairs. */
export function parseUnicodeRange(range: string): [number, number][] {
  return range
    .split(",")
    .map((part) => part.trim().replace(/^U\+/i, ""))
    .filter(Boolean)
    .map((part) => {
      const [from, to = from] = part.split("-");
      // a wildcard such as `4??` is the span its `?`s allow
      return [parseInt(from.replace(/\?/g, "0"), 16), parseInt(to.replace(/\?/g, "f"), 16)];
    });
}

/**
 * Every visible character of `el` is drawn in the first family of `token`.
 *
 * Three things have to hold. The element asks for the token's stack; for each
 * character there is a *loaded* face of the family whose `unicode-range`
 * covers it, which is what makes the browser pick that face over the
 * fallbacks behind it; and the face really has the glyphs, which a canvas
 * shows by measuring the text differently in that family than in the
 * fallback alone. The last check runs over the non-ASCII characters when
 * there are any, since those are the ones a subset can miss: in #2051 the
 * "мс" unit in a mono cell was drawn in a serif.
 */
export async function expectDrawnIn(el: HTMLElement, token: FontToken): Promise<void> {
  const family = tokenFamily(token);
  const style = getComputedStyle(el);
  await expect(unquote(style.fontFamily.split(",")[0] ?? "")).toBe(family);

  const text = (el.textContent ?? "").replace(/\s/g, "");
  await expect(text.length).toBeGreaterThan(0);
  await expect(facesOf(family).length, missingFaces(family, token)).toBeGreaterThan(0);
  const loaded = facesOf(family).filter((face) => face.status === "loaded");
  const uncovered = [...new Set(text)].filter((char) => {
    const code = char.codePointAt(0) ?? 0;
    return !loaded.some((face) =>
      parseUnicodeRange(face.unicodeRange).some(([from, to]) => code >= from && code <= to),
    );
  });
  await expect(uncovered).toEqual([]);

  const sample = text.replace(/[\u0000-\u007f]/g, "") || text;
  const context = document.createElement("canvas").getContext("2d");
  await expect(context).not.toBeNull();
  if (!context) return;
  const measure = (stack: string) => {
    context.font = `${style.fontWeight} ${style.fontSize} ${stack}`;
    return context.measureText(sample).width;
  };
  await expect(measure(`"${family}", serif`)).not.toBe(measure("serif"));
}
