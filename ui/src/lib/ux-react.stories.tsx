import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { pendingUxEvents, resetUxForTests } from "@/lib/ux";
import {
  UxScreenProvider,
  type FormTelemetry,
  useEmptyState,
  useErrorState,
  useFormTelemetry,
  useScreenReady,
} from "@/lib/ux-react";

// `ux.ts` is the queue and is unit-tested; this file is the layer that decides
// *when* the dashboard emits, and every one of those decisions is a React
// effect. There is no DOM under `bun test`, so the decisions are asserted here
// against the queue itself (#1603).
//
// The screen key travels through context rather than props on purpose — a
// shared `EmptyState` is rendered from forty-odd screens — so the stories also
// pin the half of that contract that a missing provider is silent rather than
// mislabelled.

/** the queued events, as `action:screen:target`, for readable assertions */
const queued = () =>
  pendingUxEvents().map((e) =>
    [e.action, e.screen, (e as { target?: string }).target ?? ""].join(":").replace(/:$/, ""),
  );

function Queue() {
  const [, force] = React.useReducer((n: number) => n + 1, 0);
  React.useEffect(() => {
    const timer = setInterval(force, 50);
    return () => clearInterval(timer);
  }, []);
  return (
    <ul data-testid="queue" className="font-mono text-xs">
      {queued().map((line, i) => (
        <li key={`${line}-${i}`}>{line}</li>
      ))}
    </ul>
  );
}

/** a screen that reports what it is doing, driven by the story's buttons */
function Screen({ screen = "providers" }: { screen?: string }) {
  const [ready, setReady] = React.useState(false);
  const [failed, setFailed] = React.useState(false);
  const [empty, setEmpty] = React.useState(false);
  const [open, setOpen] = React.useState(false);
  const [dirty, setDirty] = React.useState(false);
  const form = React.useRef<FormTelemetry | null>(null);
  const body = (
    <>
      <Reporter
        ready={ready}
        failed={failed}
        empty={empty}
        open={open}
        dirty={dirty}
        expose={form}
      />
    </>
  );
  return (
    <div className="flex flex-col items-start gap-3">
      <div className="flex flex-wrap gap-2">
        <button type="button" onClick={() => setReady(true)}>
          finish loading
        </button>
        <button type="button" onClick={() => setFailed(true)}>
          fail the load
        </button>
        <button type="button" onClick={() => setEmpty(true)}>
          render the empty state
        </button>
        <button type="button" onClick={() => setOpen(true)}>
          open the sheet
        </button>
        <button type="button" onClick={() => setOpen(false)}>
          close the sheet
        </button>
        <button type="button" onClick={() => setDirty(true)}>
          type into the sheet
        </button>
        <button type="button" onClick={() => form.current?.submitted()}>
          save
        </button>
        <button type="button" onClick={() => form.current?.failed()}>
          fail the save
        </button>
      </div>
      {screen ? <UxScreenProvider screen={screen}>{body}</UxScreenProvider> : body}
      <Queue />
    </div>
  );
}

function Reporter({
  ready,
  failed,
  empty,
  open,
  dirty,
  expose,
}: {
  ready: boolean;
  failed: boolean;
  empty: boolean;
  open: boolean;
  dirty: boolean;
  expose: React.RefObject<FormTelemetry | null>;
}) {
  useScreenReady(ready);
  useErrorState(failed, "list");
  // the story drives `submitted`/`failed` from its buttons, which is the only
  // way to exercise the retry edge: the hook is the thing under test, not a
  // sheet that happens to call it
  expose.current = useFormTelemetry("provider", open, { dirty });
  return empty ? <EmptyProbe /> : null;
}

// `useEmptyState` fires on mount, the way the shared EmptyState does
function EmptyProbe() {
  useEmptyState("list");
  return <p>no providers yet</p>;
}

const meta = {
  title: "Behaviour/UxStream",
  component: Screen,
  parameters: { layout: "padded" },
  beforeEach: () => {
    resetUxForTests();
    return () => resetUxForTests();
  },
} satisfies Meta<typeof Screen>;

export default meta;
type Story = StoryObj<typeof meta>;

const lines = (canvasElement: HTMLElement) =>
  Array.from(within(canvasElement).getByTestId("queue").children).map((li) => li.textContent ?? "");

/**
 * Time-to-interactive is reported once, on the edge where the screen becomes
 * usable — not on every later render that is also "ready".
 */
export const ReportsTimeToInteractiveOnce: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(lines(canvasElement)).toEqual([]);
    await userEvent.click(canvas.getByRole("button", { name: "finish loading" }));
    await waitFor(() => expect(lines(canvasElement)).toEqual(["time_to_interactive:providers"]));
    await userEvent.click(canvas.getByRole("button", { name: "finish loading" }));
    await expect(lines(canvasElement)).toEqual(["time_to_interactive:providers"]);
  },
};

/** an error and an empty placeholder are each attributed to the screen and the target */
export const ReportsTheErrorAndEmptyPlaceholders: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole("button", { name: "fail the load" }));
    await waitFor(() => expect(lines(canvasElement)).toContain("error_state:providers:list"));
    await userEvent.click(canvas.getByRole("button", { name: "render the empty state" }));
    await waitFor(() => expect(lines(canvasElement)).toContain("empty_state:providers:list"));
    // the error is an edge too: still one row after a re-render
    await expect(lines(canvasElement).filter((l) => l.startsWith("error_state"))).toHaveLength(1);
  },
};

/**
 * A sheet closed without saving is an abandonment, which is the number the
 * stream exists to produce — a form nobody finishes is a design problem.
 */
export const ReportsAnAbandonedForm: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole("button", { name: "open the sheet" }));
    // opening on its own says nothing: the dwell is only known at the close
    await expect(lines(canvasElement).filter((l) => l.includes("form"))).toHaveLength(0);
    await userEvent.click(canvas.getByRole("button", { name: "close the sheet" }));
    await waitFor(() => expect(lines(canvasElement)).toContain("form_abandon:providers:provider"));
  },
};

/**
 * Outside a provider nothing is emitted at all, rather than rows attributed to
 * an empty screen key — a mislabelled row is worse than a missing one, since
 * nothing downstream can tell it apart from a real screen. Both layers hold
 * that line: the hooks skip an empty key, and `track` drops what reaches it
 * without one anyway. This asserts the outcome, which is what the forty-odd
 * screens rendering a shared `EmptyState` depend on.
 */
export const OutsideAProviderNothingIsEmitted: Story = {
  args: { screen: "" },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole("button", { name: "finish loading" }));
    await userEvent.click(canvas.getByRole("button", { name: "fail the load" }));
    await userEvent.click(canvas.getByRole("button", { name: "render the empty state" }));
    await waitFor(() => expect(lines(canvasElement)).toEqual([]));
  },
};

/**
 * A form filled in and then closed is a different finding from one closed
 * untouched: the first is somebody who gave up, the second is a misclick. They
 * want different fixes, so they are different actions (#1731).
 */
export const SeparatesADirtyAbandonFromAClean: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole("button", { name: "open the sheet" }));
    await userEvent.click(canvas.getByRole("button", { name: "close the sheet" }));
    await waitFor(() => expect(lines(canvasElement)).toContain("form_abandon:providers:provider"));

    await userEvent.click(canvas.getByRole("button", { name: "open the sheet" }));
    await userEvent.click(canvas.getByRole("button", { name: "type into the sheet" }));
    await userEvent.click(canvas.getByRole("button", { name: "close the sheet" }));
    await waitFor(() => expect(lines(canvasElement)).toContain("abandon_dirty:providers:provider"));
    // and the clean one did not turn into a dirty one retroactively
    await expect(lines(canvasElement).filter((l) => l.startsWith("form_abandon"))).toHaveLength(1);
  },
};

/**
 * The submit after a failed one is a retry. It used to be a second identical
 * `form_submit`, so the moment somebody did not understand the first failure
 * had to be reconstructed from two rows and their timestamps.
 */
export const ReportsARetryAfterAFailedSave: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole("button", { name: "open the sheet" }));
    await userEvent.click(canvas.getByRole("button", { name: "save" }));
    await waitFor(() => expect(lines(canvasElement)).toContain("form_submit:providers:provider"));
    // the first attempt is a plain submit, never a retry
    await expect(lines(canvasElement).filter((l) => l.startsWith("retry_submit"))).toHaveLength(0);

    await userEvent.click(canvas.getByRole("button", { name: "fail the save" }));
    await userEvent.click(canvas.getByRole("button", { name: "save" }));
    await waitFor(() => expect(lines(canvasElement)).toContain("retry_submit:providers:provider"));

    // and the retry flag is spent: a third save after a success is a fresh
    // attempt, not a retry of a failure two saves ago
    await userEvent.click(canvas.getByRole("button", { name: "save" }));
    await waitFor(() =>
      expect(lines(canvasElement).filter((l) => l.startsWith("form_submit"))).toHaveLength(3),
    );
    await expect(lines(canvasElement).filter((l) => l.startsWith("retry_submit"))).toHaveLength(1);
  },
};
