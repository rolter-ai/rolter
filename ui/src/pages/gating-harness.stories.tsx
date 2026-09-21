import { useQuery } from "@tanstack/react-query";
import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { expect, within } from "storybook/test";

import {
  clickWhenEnabled,
  expectRefused,
  Harness,
  json,
  scopeResponse,
  type FetchStub,
} from "./story-harness";
import { GatedButton } from "@/components/GatedButton";
import { Button } from "@/components/ui/button";

// What #1670 cost, pinned so it cannot come back.
//
// `expectRefused` and `clickWhenEnabled` used to look their control up once and
// then poll that reference. A screen that re-renders in between — the scope
// chain resolving re-keys the query behind it, which sends the screen back to
// its skeleton for a single frame — leaves React building a *new* button when
// it returns, and the captured node is detached. Its `title` never changes
// again, so `Screens/Rbac › RefusedToAViewer` failed identically at 50ms of
// injected latency and at 900ms and read as a gate that never resolved, while
// the live control was refused exactly as it should be.
//
// The fixtures below stage that ordering deterministically rather than by
// racing a real screen: the control is replaced *before* the answer it is
// waiting for arrives, which is the only ordering in which a captured
// reference can never catch up.

/** The org chain, answered late enough that the gate is still open at capture. */
const slowScope: FetchStub = async (input) => {
  await new Promise((resolve) => setTimeout(resolve, 200));
  return scopeResponse(String(input)) ?? json([]);
};

/**
 * A control whose DOM node is replaced early, while its gate is still in
 * flight.
 *
 * The tag changes rather than the class: React reconciles by element type, so
 * this is what guarantees the node is rebuilt instead of updated.
 */
function Replaced({ children }: { children: React.ReactNode }) {
  const settled = useQuery({
    queryKey: ["gating-harness", "frame"],
    // well inside the scope chain's 200ms, so the replacement lands first
    queryFn: async () => {
      await new Promise((resolve) => setTimeout(resolve, 30));
      return true;
    },
  });
  return settled.data ? <section>{children}</section> : <div>{children}</div>;
}

// the meta points at a props-less fixture: `Replaced` takes children, and a
// meta on it would make every story restate them as args
function Gating() {
  return null;
}

const meta = {
  title: "Harness/Gating",
  component: Gating,
  parameters: { layout: "padded" },
} satisfies Meta<typeof Gating>;

export default meta;
type Story = StoryObj<typeof meta>;

/** The refusal is asserted against the control that is on screen *now*. */
export const RefusedSurvivesARemount: Story = {
  render: () => (
    <Harness fetchStub={slowScope} role="viewer">
      <Replaced>
        <GatedButton gate="custom_role:create" control="role-new">
          New role
        </GatedButton>
      </Replaced>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, /new role/i);
  },
};

/** A control that only becomes clickable after the same replacement. */
function LateEnabled() {
  const ready = useQuery({
    queryKey: ["gating-harness", "ready"],
    queryFn: async () => {
      await new Promise((resolve) => setTimeout(resolve, 200));
      return true;
    },
  });
  const [clicked, setClicked] = React.useState(false);
  return (
    <>
      <Replaced>
        <Button disabled={!ready.data} onClick={() => setClicked(true)}>
          Save
        </Button>
      </Replaced>
      {clicked && <p>saved</p>}
    </>
  );
}

/** And so is the click the enabled case waits for. */
export const ClickSurvivesARemount: Story = {
  render: () => (
    <Harness fetchStub={slowScope}>
      <LateEnabled />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "Save");
    await expect(within(canvasElement).getByText("saved")).toBeVisible();
  },
};
