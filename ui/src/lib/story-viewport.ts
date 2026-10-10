import type * as React from "react";
import { expect, within } from "storybook/test";

// Viewport fixtures for the responsive stories (#959, #1203).
//
// A story names the size it is drawn at with `globals.viewport` and the options
// that give the key its size, and each fixture below sets both from one
// constant, so a story cannot claim a width in one place and be measured at
// another. In the Storybook UI the viewport toolbar sizes the preview iframe
// from them; under the story tests (vitest) `@storybook/addon-vitest` reads the
// same two and calls `page.viewport`, which resizes the test's iframe, so
// `window.innerWidth` and every media query are the story's. A story that names
// nothing is drawn at `DESKTOP`, through `initialGlobals` in `vitest.config.ts`.
//
// Not a `.stories.tsx` file: it is a fixture, like `pages/story-harness.tsx`.

/**
 * The size of a story that names none, and the key it is registered under in
 * `.storybook/preview.ts`. The dashboard's breakpoints are drawn for it: the LLM
 * Logs drawer needs `xl`, which starts at 1280.
 */
export const DESKTOP = { width: 1280, height: 800 } as const;
/** iPhone 12 mini / SE — the width #959 was reported at */
export const MOBILE = { width: 375, height: 812 } as const;
/** The narrowest phone the dashboard is held to (#2004) */
export const SMALL = { width: 320, height: 640 } as const;
/**
 * A window barely wider than a phone, the width #2837 lost a table's status
 * badge in: the Logs table has room to draw its columns here and no more
 */
export const NARROW = { width: 560, height: 800 } as const;
/** iPad portrait — the `md`…`lg` band where the rail is an icon strip */
export const TABLET = { width: 768, height: 1024 } as const;
/** A 1440 px laptop window, wider than the 1280×800 `DESKTOP` default */
export const WIDE = { width: 1440, height: 900 } as const;
/**
 * A small laptop, in the band between `lg` and `xl`: wide enough for the
 * sidebar, too narrow for the LLM Logs drawer beside it (#1986)
 */
export const LAPTOP = { width: 1100, height: 800 } as const;
/**
 * A laptop window split in two, the width the #1789 livetest pass met the
 * truncations of #2812 at: 1024 is the narrowest the rail is still the full,
 * resizable one (`BELOW_LG` ends at 1023.98), and the screens beside it have
 * 1024 less the rail's 232 to draw their tables and sheets in
 */
export const SPLIT = { width: 1024, height: 768 } as const;
/**
 * A 1280×720 screen at 200 % zoom, the short window #2003 lost a dialog's
 * title and buttons in. WCAG 1.4.10 asks for reflow at that size
 */
export const SHORT = { width: 640, height: 360 } as const;

/** A `parameters.viewport.options` entry for a size, so the name and the pixels cannot drift. */
function option(name: string, size: { width: number; height: number }) {
  return { name, styles: { width: `${size.width}px`, height: `${size.height}px` } };
}

/** The option `.storybook/preview.ts` registers for `DESKTOP`, under `rolterDesktop`. */
export const DESKTOP_VIEWPORT = option("Desktop 1280", DESKTOP);

const OPTIONS = {
  rolterMobile: option("Mobile 375", MOBILE),
  rolterSmall: option("Small 320", SMALL),
  rolterNarrow: option("Narrow 560", NARROW),
  rolterTablet: option("Tablet 768", TABLET),
  rolterWide: option("Wide 1440", WIDE),
  rolterLaptop: option("Laptop 1100", LAPTOP),
  rolterSplit: option("Split 1024", SPLIT),
  rolterShort: option("Short 640×360", SHORT),
};

/** Story fields that pin a story to one of the sizes above. */
export const atMobile = {
  parameters: { viewport: { options: OPTIONS } },
  globals: { viewport: { value: "rolterMobile", isRotated: false } },
};

export const atSmall = {
  parameters: { viewport: { options: OPTIONS } },
  globals: { viewport: { value: "rolterSmall", isRotated: false } },
};

export const atNarrow = {
  parameters: { viewport: { options: OPTIONS } },
  globals: { viewport: { value: "rolterNarrow", isRotated: false } },
};

export const atTablet = {
  parameters: { viewport: { options: OPTIONS } },
  globals: { viewport: { value: "rolterTablet", isRotated: false } },
};

export const atWide = {
  parameters: { viewport: { options: OPTIONS } },
  globals: { viewport: { value: "rolterWide", isRotated: false } },
};

export const atLaptop = {
  parameters: { viewport: { options: OPTIONS } },
  globals: { viewport: { value: "rolterLaptop", isRotated: false } },
};

export const atSplit = {
  parameters: { viewport: { options: OPTIONS } },
  globals: { viewport: { value: "rolterSplit", isRotated: false } },
};

export const atShort = {
  parameters: { viewport: { options: OPTIONS } },
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
 * The text of `el` is shown whole: nothing is cut by the element's own edge, so
 * no ellipsis is drawn, and a line that did not fit wrapped instead.
 *
 * `toBeVisible` passes a clipped label, and the computed `text-overflow` of a
 * `truncate` element reads `ellipsis` whether or not it truncates anything, so
 * this reads the geometry: the content may not be wider than the element, and
 * the text's own rectangle (a `Range` over its contents) must end inside the
 * element's padding box. `el` has to be a block, flex or grid item: an inline
 * element reports a `clientWidth` of 0 whatever it holds.
 */
export async function expectNotTruncated(el: HTMLElement): Promise<void> {
  await expect(el.scrollWidth).toBeLessThanOrEqual(el.clientWidth + 1);
  const text = document.createRange();
  text.selectNodeContents(el);
  await expectInFrame(text, el);
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
