import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { BELOW_LG, BELOW_MD, useMediaQuery } from "@/lib/use-media-query";

// The shell asks this hook whether the rail is a drawer and whether a detail
// panel is a sheet (#959, #1203), so a wrong answer is a layout that cannot be
// operated rather than one that looks off. The real `matchMedia` cannot be
// driven from a test, so the stories stand one in and change its answer.

interface FakeList {
  matches: boolean;
  listeners: Set<() => void>;
}

/** a matchMedia whose answers the story controls */
function installMatchMedia(state: Map<string, FakeList>) {
  const original = window.matchMedia;
  window.matchMedia = ((query: string) => {
    const entry = state.get(query) ?? { matches: false, listeners: new Set() };
    state.set(query, entry);
    return {
      media: query,
      get matches() {
        return entry.matches;
      },
      addEventListener: (_: string, fn: () => void) => entry.listeners.add(fn),
      removeEventListener: (_: string, fn: () => void) => entry.listeners.delete(fn),
      // the deprecated pair, which the hook must not be reaching for
      addListener: () => {
        throw new Error("addListener is deprecated; the hook uses addEventListener");
      },
      removeListener: () => {
        throw new Error("removeListener is deprecated");
      },
      dispatchEvent: () => false,
      onchange: null,
    } as unknown as MediaQueryList;
  }) as typeof window.matchMedia;
  return () => {
    window.matchMedia = original;
  };
}

function resize(state: Map<string, FakeList>, query: string, matches: boolean) {
  const entry = state.get(query);
  if (!entry) return;
  entry.matches = matches;
  for (const listener of entry.listeners) listener();
}

function Harness({ start = false }: { start?: boolean }) {
  const state = React.useRef(new Map<string, FakeList>()).current;
  // installed during render, before the hook below subscribes
  React.useState(() => {
    state.set(BELOW_MD, { matches: start, listeners: new Set() });
    state.set(BELOW_LG, { matches: start, listeners: new Set() });
    return null;
  });
  const restore = React.useRef<(() => void) | null>(null);
  restore.current ??= installMatchMedia(state);
  React.useEffect(() => () => restore.current?.(), []);

  const belowMd = useMediaQuery(BELOW_MD);
  const belowLg = useMediaQuery(BELOW_LG);
  return (
    <dl className="grid grid-cols-[10rem_1fr] gap-1 font-mono text-sm">
      <dt>below md</dt>
      <dd data-testid="below-md">{String(belowMd)}</dd>
      <dt>below lg</dt>
      <dd data-testid="below-lg">{String(belowLg)}</dd>
      <dd className="col-span-2 flex gap-2">
        <button type="button" onClick={() => resize(state, BELOW_MD, true)}>
          narrow to phone
        </button>
        <button type="button" onClick={() => resize(state, BELOW_MD, false)}>
          widen to desktop
        </button>
        <button type="button" onClick={() => resize(state, BELOW_LG, true)}>
          narrow to tablet
        </button>
      </dd>
    </dl>
  );
}

const meta = {
  title: "Behaviour/MediaQuery",
  component: Harness,
  parameters: { layout: "padded" },
} satisfies Meta<typeof Harness>;

export default meta;
type Story = StoryObj<typeof meta>;

/** a matching query is read after mount, not guessed */
export const ReadsTheQueryOnMount: Story = {
  args: { start: true },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByTestId("below-md")).toHaveTextContent("true"));
    await expect(canvas.getByTestId("below-lg")).toHaveTextContent("true");
  },
};

/** and a change to the viewport reaches the component that asked */
export const FollowsTheViewport: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByTestId("below-md")).toHaveTextContent("false");
    await userEvent.click(canvas.getByRole("button", { name: "narrow to phone" }));
    await waitFor(() => expect(canvas.getByTestId("below-md")).toHaveTextContent("true"));
    await userEvent.click(canvas.getByRole("button", { name: "widen to desktop" }));
    await waitFor(() => expect(canvas.getByTestId("below-md")).toHaveTextContent("false"));
  },
};

/**
 * Each query is its own subscription: the tablet breakpoint moving must not
 * drag the phone one with it, or the rail and the detail panel change shape
 * together at one width instead of at two.
 */
export const TheTwoBreakpointsAreIndependent: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole("button", { name: "narrow to tablet" }));
    await waitFor(() => expect(canvas.getByTestId("below-lg")).toHaveTextContent("true"));
    await expect(canvas.getByTestId("below-md")).toHaveTextContent("false");
  },
};
