import type { Meta, StoryObj } from "@storybook/react-vite";
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
    // nothing was fetched to draw it: a second factor sent to a chart service
    // is not a second factor, and the dashboard has to work air-gapped
    await expect(svg.querySelectorAll("image")).toHaveLength(0);
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
