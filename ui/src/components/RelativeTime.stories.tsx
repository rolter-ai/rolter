import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { expect, waitFor, within } from "storybook/test";

import { RelativeTime } from "./RelativeTime";

// a screen that shows relative times. its own renders and the leaf's are counted
// through the DOM (a ref write, not state) so counting cannot cause a render
function Screen() {
  const screenRenders = React.useRef(0);
  const leafRenders = React.useRef(0);
  const leafOut = React.useRef<HTMLSpanElement>(null);
  const screenOut = React.useRef<HTMLSpanElement>(null);
  screenRenders.current += 1;
  React.useEffect(() => {
    if (screenOut.current) screenOut.current.textContent = String(screenRenders.current);
  });
  // a fixed instant, so the text itself is not what is under test
  const at = React.useMemo(() => Date.now() - 5_000, []);
  return (
    <div>
      <p>
        screen renders: <span ref={screenOut} data-testid="screen-count" />
      </p>
      <p>
        leaf renders: <span ref={leafOut} data-testid="leaf-count" />
      </p>
      <p data-testid="time">
        <RelativeTime
          at={at}
          intervalMs={100}
          onRender={() => {
            leafRenders.current += 1;
            if (leafOut.current) leafOut.current.textContent = String(leafRenders.current);
          }}
        />
      </p>
    </div>
  );
}

const meta = {
  title: "Components/RelativeTime",
  component: RelativeTime,
  args: { at: Date.now() - 90_000 },
} satisfies Meta<typeof RelativeTime>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  play: async ({ canvasElement }) => {
    await expect(within(canvasElement).getByText(/ago/)).toBeVisible();
  },
};

/**
 * The clock lives in the leaf (#1997): over a dozen ticks the leaf renders each
 * time and the screen that holds it renders once.
 */
export const TickingLeavesTheScreenAlone: Story = {
  render: () => <Screen />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() =>
      expect(Number(canvas.getByTestId("leaf-count").textContent)).toBeGreaterThan(8),
    );
    await expect(canvas.getByTestId("screen-count")).toHaveTextContent("1");
  },
};
