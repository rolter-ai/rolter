import { expect } from "storybook/test";

// Viewport fixtures for the responsive stories (#959, #1203).
//
// Two things have to agree for a "does it fit at 375px" story to mean
// anything. In the Storybook UI the viewport toolbar sizes the preview iframe;
// under the test runner nothing sizes it, so `.storybook/test-runner.ts` reads
// `parameters.viewportSize` and calls `page.setViewportSize` before the story
// renders. Both are set from the same constant here, so a story cannot claim a
// width in one place and be measured at another.
//
// Not a `.stories.tsx` file: it is a fixture, like `pages/story-harness.tsx`.

/** iPhone 12 mini / SE — the width #959 was reported at */
export const MOBILE = { width: 375, height: 812 } as const;
/** iPad portrait — the `md`…`lg` band where the rail is an icon strip */
export const TABLET = { width: 768, height: 1024 } as const;
/**
 * A 1280×720 screen at 200 % zoom, the short window #2003 lost a dialog's
 * title and buttons in. WCAG 1.4.10 asks for reflow at that size
 */
export const SHORT = { width: 640, height: 360 } as const;

const OPTIONS = {
  rolterMobile: { name: "Mobile 375", styles: { width: "375px", height: "812px" } },
  rolterTablet: { name: "Tablet 768", styles: { width: "768px", height: "1024px" } },
  rolterShort: { name: "Short 640×360", styles: { width: "640px", height: "360px" } },
};

/** Story fields that pin a story to one of the two widths above. */
export const atMobile = {
  parameters: { viewportSize: MOBILE, viewport: { options: OPTIONS } },
  globals: { viewport: { value: "rolterMobile", isRotated: false } },
};

export const atTablet = {
  parameters: { viewportSize: TABLET, viewport: { options: OPTIONS } },
  globals: { viewport: { value: "rolterTablet", isRotated: false } },
};

export const atShort = {
  parameters: { viewportSize: SHORT, viewport: { options: OPTIONS } },
  globals: { viewport: { value: "rolterShort", isRotated: false } },
};

/**
 * Nothing on the page is wider than the page.
 *
 * The assertion the audit in #959 could not make by eye: the shell clipped its
 * overflow, so `scrollWidth` read 375 while stat values were cut mid-digit.
 * Measuring the document rather than a screenshot is the whole point.
 */
export async function expectNoHorizontalOverflow(): Promise<void> {
  await expect(document.documentElement.scrollWidth).toBeLessThanOrEqual(window.innerWidth);
  await expect(document.body.scrollWidth).toBeLessThanOrEqual(window.innerWidth);
}

/**
 * Every animation that ends has ended.
 *
 * A sheet slides in from the right edge (`rl-sheet-in`), so a box read the
 * moment the panel is in the document is wherever the slide has got to. A
 * spinner or a skeleton pulse loops forever and is left alone, since waiting
 * on one would never return.
 */
export async function animationsSettled(): Promise<void> {
  const ending = document.getAnimations().filter((animation) => {
    const end = animation.effect?.getComputedTiming().endTime;
    return typeof end === "number" && Number.isFinite(end);
  });
  await Promise.all(ending.map((animation) => animation.finished.catch(() => undefined)));
}

/**
 * The whole of `el` is on screen, so it can be seen and pressed.
 *
 * `expectNoHorizontalOverflow` cannot see this one: a dialog or a sheet is
 * `position: fixed`, which adds nothing to the document's scroll width, and
 * the page behind it has its scrolling locked. A button pushed past the edge
 * of a fixed panel is out of reach while the document measures clean (#2003),
 * so the box itself is what gets measured.
 */
export async function expectInViewport(el: Element): Promise<void> {
  await animationsSettled();
  const box = el.getBoundingClientRect();
  await expect(box.left).toBeGreaterThanOrEqual(0);
  await expect(box.top).toBeGreaterThanOrEqual(0);
  await expect(box.right).toBeLessThanOrEqual(window.innerWidth);
  await expect(box.bottom).toBeLessThanOrEqual(window.innerHeight);
}
