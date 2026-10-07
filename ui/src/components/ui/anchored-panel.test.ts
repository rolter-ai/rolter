import { describe, expect, test } from "bun:test";

import { placePanel, type PlaceOptions } from "./anchored-panel";

// a 52px rail at the left of a 1280x800 window, with an icon 32px tall in it
const base: PlaceOptions = {
  anchor: { left: 10, top: 300, right: 42, bottom: 332 },
  edge: 52,
  panel: { width: 200, height: 120 },
  viewport: { width: 1280, height: 800 },
  side: "right",
  align: "start",
};

describe("placePanel", () => {
  test("beside the rail, level with the anchor's top", () => {
    expect(placePanel(base)).toEqual({ left: 58, top: 300 });
  });

  test("beside the rail, level with the anchor's bottom, so a menu grows upward", () => {
    expect(placePanel({ ...base, align: "end" })).toEqual({ left: 58, top: 212 });
  });

  test("nudged up when the viewport is too short to hold it below", () => {
    const anchor = { left: 10, top: 760, right: 42, bottom: 792 };
    expect(placePanel({ ...base, anchor })).toEqual({ left: 58, top: 672 });
  });

  test("never above the top margin", () => {
    const anchor = { left: 10, top: 20, right: 42, bottom: 52 };
    expect(placePanel({ ...base, anchor, align: "end" }).top).toBe(8);
  });

  test("under the anchor, lined up with its left edge", () => {
    const anchor = { left: 8, top: 80, right: 224, bottom: 112 };
    expect(placePanel({ ...base, anchor, side: "below" })).toEqual({ left: 8, top: 118 });
  });

  test("under the anchor, lined up with its right edge", () => {
    const anchor = { left: 8, top: 80, right: 224, bottom: 112 };
    expect(placePanel({ ...base, anchor, side: "below", align: "end" })).toEqual({
      left: 24,
      top: 118,
    });
  });

  test("over the anchor, with the gap between", () => {
    const anchor = { left: 8, top: 700, right: 224, bottom: 732 };
    expect(placePanel({ ...base, anchor, side: "above" })).toEqual({ left: 8, top: 574 });
  });

  test("turns over when there is no room below and more above", () => {
    const anchor = { left: 8, top: 700, right: 224, bottom: 732 };
    expect(placePanel({ ...base, anchor, side: "below" })).toEqual({ left: 8, top: 574 });
  });

  test("stays below when it fits neither side, and is clamped", () => {
    const anchor = { left: 8, top: 40, right: 224, bottom: 72 };
    const tall = { ...base, anchor, side: "below" as const, panel: { width: 200, height: 760 } };
    expect(placePanel(tall).top).toBe(32);
  });

  test("clamped to the viewport's right edge", () => {
    const anchor = { left: 1200, top: 80, right: 1272, bottom: 112 };
    expect(placePanel({ ...base, anchor, side: "below" }).left).toBe(1072);
  });
});
