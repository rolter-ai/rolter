import type { Meta, StoryObj } from "@storybook/react-vite";
import qrcode from "qrcode-generator";
import * as React from "react";
import { expect, within } from "storybook/test";

import { QrCode } from "./QrCode";

/** the enrolment payload TwoFactorPanel feeds it, shape and all */
const OTPAUTH =
  "otpauth://totp/rolter:ilya%40example.com?secret=JBSWY3DPEHPK3PXP&issuer=rolter&algorithm=SHA1&digits=6&period=30";

/** the code is aria-hidden, so it is reachable only as the one svg in the canvas */
const svgOf = (canvasElement: HTMLElement): SVGSVGElement => {
  const svg = canvasElement.querySelector("svg");
  if (!svg) throw new Error("no qr svg rendered");
  return svg as SVGSVGElement;
};

/** the module grid the viewBox describes, quiet zone included */
const extentOf = (svg: SVGSVGElement): number =>
  Number(svg.getAttribute("viewBox")?.split(" ")[3]);

const meta = {
  title: "Components/QrCode",
  component: QrCode,
  args: { value: OTPAUTH },
} satisfies Meta<typeof QrCode>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  play: async ({ canvasElement }) => {
    const svg = svgOf(canvasElement);
    // dark modules are one path, not a rect each — a version-4 code is ~700
    // elements and laying out each one costs more than the enrolment sheet has
    await expect(svg.querySelectorAll("path")).toHaveLength(1);
    await expect(svg.querySelector("path")?.getAttribute("d") ?? "").not.toBe("");
  },
};

/** every module the path paints, as `col,row` in grid coordinates (quiet zone removed) */
const paintedModules = (svg: SVGSVGElement): Set<string> => {
  const d = svg.querySelector("path")?.getAttribute("d") ?? "";
  return new Set([...d.matchAll(/M(\d+) (\d+)h1v1h-1z/g)].map(([, col, row]) => `${Number(col) - 4},${Number(row) - 4}`));
};

/**
 * The drawing is the encoder's matrix, module for module. Nothing else here
 * would notice rows and columns swapped, a grid shifted by one or a dropped
 * module — and each of those is a code that renders fine and does not scan,
 * which a user finds out at the one moment they cannot go back.
 */
export const PaintsExactlyTheEncodedMatrix: Story = {
  play: async ({ canvasElement }) => {
    const qr = qrcode(0, "M");
    qr.addData(OTPAUTH);
    qr.make();
    const expected = new Set<string>();
    for (let row = 0; row < qr.getModuleCount(); row += 1) {
      for (let col = 0; col < qr.getModuleCount(); col += 1) {
        if (qr.isDark(row, col)) expected.add(`${col},${row}`);
      }
    }
    const painted = paintedModules(svgOf(canvasElement));
    await expect(painted.size).toBe(expected.size);
    await expect([...painted].filter((m) => !expected.has(m))).toEqual([]);
    // a QR is not symmetric, so a transposed drawing is a different set: say
    // so, or the comparison above could not tell the two apart
    const transposed = [...expected].map((m) => m.split(",").reverse().join(","));
    await expect(transposed.some((m) => !expected.has(m))).toBe(true);
  },
};

// every request the page makes while the code is on screen
const requests: string[] = [];

function Watched({ children }: { children: React.ReactNode }) {
  const original = React.useRef<typeof globalThis.fetch | null>(null);
  React.useState(() => {
    requests.length = 0;
    const real = globalThis.fetch;
    original.current = real;
    globalThis.fetch = ((input: RequestInfo | URL, init?: RequestInit) => {
      requests.push(String(input));
      return real(input, init);
    }) as typeof globalThis.fetch;
    return null;
  });
  // hand the real one back, or every later story runs behind this recorder
  React.useEffect(
    () => () => {
      if (original.current) globalThis.fetch = original.current;
    },
    [],
  );
  return <>{children}</>;
}

/**
 * Nothing leaves the browser to draw it: a second factor sent to a chart
 * service is not a second factor, and the dashboard has to work air-gapped.
 * So no request, and nothing in the drawing that could make one later.
 */
export const DrawsWithoutTheNetwork: Story = {
  render: (args) => (
    <Watched>
      <QrCode {...args} />
    </Watched>
  ),
  play: async ({ canvasElement }) => {
    const svg = svgOf(canvasElement);
    await expect(requests).toEqual([]);
    await expect(canvasElement.querySelectorAll("img, image, use, iframe, object")).toHaveLength(0);
    for (const node of [svg, ...svg.querySelectorAll("*")]) {
      for (const attr of node.getAttributeNames()) {
        await expect(attr).not.toMatch(/^(src|href|xlink:href)$/);
        await expect(node.getAttribute(attr) ?? "").not.toMatch(/https?:|\/\/|url\(/);
      }
    }
    // and the secret itself is nowhere in the markup but the modules
    await expect(canvasElement.innerHTML).not.toContain("JBSWY3DPEHPK3PXP");
  },
};

/**
 * Unreadable to a screen reader by nature, so it is hidden from one outright —
 * the same secret sits beside it as selectable base32 text, which is both the
 * accessible path and the one a user with no camera needs anyway.
 */
export const HiddenFromAssistiveTech: Story = {
  play: async ({ canvasElement }) => {
    const svg = svgOf(canvasElement);
    await expect(svg).toHaveAttribute("aria-hidden", "true");
    await expect(svg).toHaveAttribute("focusable", "false");
    await expect(within(canvasElement).queryByRole("img")).toBeNull();
  },
};

/**
 * Four modules of quiet zone on every side. Scanners really do fail without it
 * when the code sits on a bordered card, and the backdrop must be painted
 * white whatever surface it lands on — a QR inverted by the dark theme does
 * not scan.
 */
export const KeepsItsQuietZoneAndWhiteBackdrop: Story = {
  play: async ({ canvasElement }) => {
    const svg = svgOf(canvasElement);
    const extent = extentOf(svg);
    const rect = svg.querySelector("rect");
    await expect(rect).toHaveAttribute("fill", "#ffffff");
    await expect(Number(rect?.getAttribute("width"))).toBe(extent);
    // the quiet zone is the gap between the grid and the viewBox: every dark
    // module starts at 4 or beyond, and none reaches the far edge
    const starts = [...(svg.querySelector("path")?.getAttribute("d") ?? "").matchAll(/M(\d+) (\d+)/g)];
    await expect(starts.length).toBeGreaterThan(0);
    for (const [, col, row] of starts) {
      await expect(Number(col)).toBeGreaterThanOrEqual(4);
      await expect(Number(row)).toBeGreaterThanOrEqual(4);
      await expect(Number(col)).toBeLessThan(extent - 4);
      await expect(Number(row)).toBeLessThan(extent - 4);
    }
  },
};

/**
 * A longer payload picks a denser version rather than overflowing the one it
 * has — the encoder is asked for version `0`, the smallest that fits.
 */
export const GrowsWithThePayload: Story = {
  render: () => (
    <div className="flex items-start gap-4">
      <div data-testid="short">
        <QrCode value={OTPAUTH} />
      </div>
      <div data-testid="long">
        <QrCode value={`${OTPAUTH}&note=${"x".repeat(400)}`} />
      </div>
    </div>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const short = extentOf(svgOf(canvas.getByTestId("short")));
    const long = extentOf(svgOf(canvas.getByTestId("long")));
    await expect(long).toBeGreaterThan(short);
  },
};

/** the rendered edge length is the caller's, independent of the module count */
export const SizedByTheCaller: Story = {
  args: { size: 96 },
  play: async ({ canvasElement }) => {
    const svg = svgOf(canvasElement);
    await expect(svg).toHaveAttribute("width", "96");
    await expect(svg).toHaveAttribute("height", "96");
    // the grid is unchanged: only the box it is painted into shrank
    await expect(extentOf(svg)).toBeGreaterThan(20);
  },
};
