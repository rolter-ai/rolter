import type { Meta, StoryObj } from "@storybook/react";
import { expect, fn, userEvent, within } from "storybook/test";

import { contrastRatio } from "@/lib/contrast";
import { mediaDeclarations, resolvePx } from "@/lib/story-css";
import { resolveColorToken } from "@/lib/story-tokens";
import { Button } from "./button";

const meta = {
  title: "Primitives/Button",
  component: Button,
  parameters: { layout: "padded" },
  args: { children: "Save changes", onClick: fn() },
  argTypes: {
    variant: {
      control: "select",
      options: ["default", "outline", "ghost", "destructive", "brand"],
    },
    size: { control: "select", options: ["default", "sm", "lg", "icon"] },
  },
} satisfies Meta<typeof Button>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {};

export const Variants: Story = {
  render: (args) => (
    <div className="flex flex-wrap items-center gap-3">
      <Button {...args} variant="default">
        Default
      </Button>
      <Button {...args} variant="outline">
        Outline
      </Button>
      <Button {...args} variant="ghost">
        Ghost
      </Button>
      <Button {...args} variant="destructive">
        Delete
      </Button>
      <Button {...args} variant="brand">
        Sign in
      </Button>
    </div>
  ),
};

/**
 * The brand button is the folk-red fill with white text (#1993). Screens
 * ask for it with `variant="brand"` and never spell the classes; the colours
 * here are read back from the rendered button, so a drift from the
 * tokens fails. The press colour is a `:hover` rule a story cannot switch on
 * (a synthetic pointer sets no `:hover`), so it is asserted as the class.
 */
export const BrandIsTheFolkRedFill: Story = {
  args: { variant: "brand", children: "Sign in" },
  play: async ({ canvasElement }) => {
    const button = within(canvasElement).getByRole("button", { name: "Sign in" });
    const fill = resolveColorToken("--red-folk");
    await expect(getComputedStyle(button).backgroundColor).toBe(fill);
    await expect(getComputedStyle(button).color).toBe("rgb(255, 255, 255)");
    await expect(contrastRatio(getComputedStyle(button).color, fill)).toBeGreaterThanOrEqual(4.5);
    await expect(button).toHaveClass("hover:bg-brand-press");
  },
};

export const Disabled: Story = {
  args: { disabled: true, children: "Disabled" },
};

// interaction: a click fires the handler exactly once, and a disabled button
// swallows the click
export const FiresOnClick: Story = {
  args: { children: "Click me" },
  play: async ({ args, canvasElement }) => {
    const canvas = within(canvasElement);
    const button = canvas.getByRole("button", { name: "Click me" });
    await userEvent.click(button);
    await expect(args.onClick).toHaveBeenCalledTimes(1);
  },
};

export const DisabledSwallowsClick: Story = {
  args: { disabled: true, children: "Nope" },
  play: async ({ args, canvasElement }) => {
    const canvas = within(canvasElement);
    const button = canvas.getByRole("button", { name: "Nope" });
    await expect(button).toBeDisabled();
    // force the click past userEvent's pointer-events guard to prove the handler
    // still never fires on a disabled control
    await userEvent.click(button, { pointerEventsCheck: 0 });
    await expect(args.onClick).not.toHaveBeenCalled();
  },
};

// the story browser has a fine pointer, so desktop density is read for real
// and the coarse floor is read from the stylesheet the build emitted (#2040)
export const DesktopDensityIsUnchanged: Story = {
  render: () => (
    <div className="flex items-center gap-3">
      <Button>Default</Button>
      <Button size="sm">Small</Button>
      <Button size="icon" aria-label="Icon" />
    </div>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(
      canvas.getByRole("button", { name: "Default" }).getBoundingClientRect().height,
    ).toBe(36);
    await expect(canvas.getByRole("button", { name: "Small" }).getBoundingClientRect().height).toBe(
      32,
    );
    await expect(canvas.getByRole("button", { name: "Icon" }).getBoundingClientRect().height).toBe(
      36,
    );
  },
};

/**
 * Under `@media (pointer: coarse)` every size reaches 44px. A story cannot make
 * its browser coarse, so this reads the rule the build emitted and draws what it
 * declares.
 */
export const CoarsePointerReaches44px: Story = {
  render: () => (
    <div className="flex items-center gap-3">
      <Button>Default</Button>
      <Button size="sm">Small</Button>
      <Button size="lg">Large</Button>
      <Button size="icon" aria-label="Icon" />
    </div>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const coarse = /pointer:\s*coarse/;
    const height = mediaDeclarations(coarse, /pointer-coarse\\:h-11$/);
    await expect(height).not.toBeNull();
    await expect(resolvePx(height?.height ?? "")).toBe(44);
    const width = mediaDeclarations(coarse, /pointer-coarse\\:w-11$/);
    await expect(resolvePx(width?.width ?? "")).toBe(44);
    for (const name of ["Default", "Small", "Large", "Icon"]) {
      await expect(canvas.getByRole("button", { name })).toHaveClass("pointer-coarse:h-11");
    }
    await expect(canvas.getByRole("button", { name: "Icon" })).toHaveClass("pointer-coarse:w-11");
  },
};

/**
 * Forced colors strips the box-shadow the ring is drawn with, so the shipped
 * stylesheet puts an outline back on every focused control (#1999).
 */
export const FocusSurvivesForcedColors: Story = {
  play: async ({ canvasElement }) => {
    const button = within(canvasElement).getByRole("button", { name: "Save changes" });
    await userEvent.tab();
    await expect(button).toHaveFocus();
    const forced = mediaDeclarations(/forced-colors:\s*active/, ":focus-visible");
    await expect(forced).not.toBeNull();
    await expect(forced?.getPropertyValue("outline")).toMatch(/canvastext/i);
    await expect(forced?.getPropertyPriority("outline")).toBe("important");
    await expect(forced?.getPropertyValue("outline-offset")).toBe("2px");
  },
};

// a native button inside a <form> submits it, so `Button` says "button" unless
// the caller says otherwise (#1205): Cancel and a row's delete never submit
export const DoesNotSubmitAFormUnlessAskedTo: Story = {
  render: (args) => {
    const onSubmit = fn((event: React.FormEvent) => event.preventDefault());
    return (
      <form onSubmit={onSubmit} aria-label="demo form">
        <Button {...args}>Cancel</Button>
        <Button {...args} type="submit">
          Save
        </Button>
      </form>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const form = canvas.getByRole("form", { name: "demo form" });
    const submits: Event[] = [];
    form.addEventListener("submit", (event) => submits.push(event));
    await expect(canvas.getByRole("button", { name: "Cancel" })).toHaveAttribute("type", "button");
    await userEvent.click(canvas.getByRole("button", { name: "Cancel" }));
    await expect(submits).toHaveLength(0);
    await userEvent.click(canvas.getByRole("button", { name: "Save" }));
    await expect(submits).toHaveLength(1);
  },
};
