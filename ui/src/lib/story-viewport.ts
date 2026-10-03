import type * as React from "react";
import { expect, within } from "storybook/test";

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
/** The narrowest phone the dashboard is held to (#2004) */
export const SMALL = { width: 320, height: 640 } as const;
/** iPad portrait — the `md`…`lg` band where the rail is an icon strip */
export const TABLET = { width: 768, height: 1024 } as const;
/** A 1440 px laptop window, wider than the runner's 1280×800 default */
export const WIDE = { width: 1440, height: 900 } as const;
/**
 * A small laptop, in the band between `lg` and `xl`: wide enough for the
 * sidebar, too narrow for the LLM Logs drawer beside it (#1986)
 */
export const LAPTOP = { width: 1100, height: 800 } as const;
/**
 * A 1280×720 screen at 200 % zoom, the short window #2003 lost a dialog's
 * title and buttons in. WCAG 1.4.10 asks for reflow at that size
 */
export const SHORT = { width: 640, height: 360 } as const;

const OPTIONS = {
  rolterMobile: { name: "Mobile 375", styles: { width: "375px", height: "812px" } },
  rolterSmall: { name: "Small 320", styles: { width: "320px", height: "640px" } },
  rolterTablet: { name: "Tablet 768", styles: { width: "768px", height: "1024px" } },
  rolterWide: { name: "Wide 1440", styles: { width: "1440px", height: "900px" } },
  rolterLaptop: { name: "Laptop 1100", styles: { width: "1100px", height: "800px" } },
  rolterShort: { name: "Short 640×360", styles: { width: "640px", height: "360px" } },
};

/** Story fields that pin a story to one of the two widths above. */
export const atMobile = {
  parameters: { viewportSize: MOBILE, viewport: { options: OPTIONS } },
  globals: { viewport: { value: "rolterMobile", isRotated: false } },
};

export const atSmall = {
  parameters: { viewportSize: SMALL, viewport: { options: OPTIONS } },
  globals: { viewport: { value: "rolterSmall", isRotated: false } },
};

export const atTablet = {
  parameters: { viewportSize: TABLET, viewport: { options: OPTIONS } },
  globals: { viewport: { value: "rolterTablet", isRotated: false } },
};

export const atWide = {
  parameters: { viewportSize: WIDE, viewport: { options: OPTIONS } },
  globals: { viewport: { value: "rolterWide", isRotated: false } },
};

export const atLaptop = {
  parameters: { viewportSize: LAPTOP, viewport: { options: OPTIONS } },
  globals: { viewport: { value: "rolterLaptop", isRotated: false } },
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

/**
 * `el` is inside `frame`, the part of a scroll container the reader can see.
 *
 * `expectInViewport` measures against the window, which a box can pass while a
 * scroll container's own edge cuts it: the frame of a table in a card is
 * narrower than the window by the page gutters, and the table scrolls sideways
 * inside it (#2420). The frame is the padding box, less the scrollbar, which is
 * what `clientLeft` and `clientWidth` say. `el` may be a `Range`, for the width
 * of a line of text rather than the block that holds it.
 */
export async function expectInFrame(el: Element | Range, frame: Element): Promise<void> {
  await animationsSettled();
  const box = el.getBoundingClientRect();
  const left = frame.getBoundingClientRect().left + frame.clientLeft;
  await expect(box.left).toBeGreaterThanOrEqual(left - 0.5);
  await expect(box.right).toBeLessThanOrEqual(left + frame.clientWidth + 0.5);
}

/**
 * A phone-width "nothing is wider than the page" story, for a locale and a
 * width (#2004).
 *
 * A check written once in English passes the shorter copy and says nothing
 * about Russian, whose labels run a third longer: the audit found twice as
 * many screens overflowing in `ru` as in `en`. A screen states its render and
 * what it waits for once, and takes one story per locale and width from the
 * returned function, so the two cannot drift into different assertions.
 */
export function phoneFits(opts: {
  render: () => React.ReactElement;
  /** Waits for the loaded screen, then asserts anything width-specific. */
  ready: (canvas: ReturnType<typeof within>, locale: "en" | "ru") => Promise<unknown>;
}) {
  return (width: "mobile" | "small", locale: "en" | "ru") => {
    const at = width === "small" ? atSmall : atMobile;
    return {
      parameters: at.parameters,
      globals: { ...at.globals, locale },
      render: opts.render,
      play: async ({ canvasElement }: { canvasElement: HTMLElement }) => {
        await opts.ready(within(canvasElement), locale);
        await expectNoHorizontalOverflow();
      },
    };
  };
}
