import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { Button } from "./button";
import { Toaster } from "./toaster";
import { ToastProvider, useToast } from "@/lib/toast";

const meta = {
  title: "Feedback/Toaster",
  component: Toaster,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof Toaster>;

export default meta;
type Story = StoryObj<typeof meta>;

function Demo() {
  const toast = useToast();
  return (
    <div className="flex gap-2 p-6">
      <Button onClick={() => toast.push({ tone: "success", title: "Provider saved" })}>
        Success
      </Button>
      <Button
        variant="outline"
        onClick={() =>
          toast.push({
            tone: "error",
            title: "Could not delete openai-prod",
            detail: "provider is still the target of 3 routes",
          })
        }
      >
        Error
      </Button>
      <Button
        variant="ghost"
        onClick={() => toast.push({ tone: "info", title: "Snapshot version 42 applied" })}
      >
        Info
      </Button>
    </div>
  );
}

const Stage = () => (
  <ToastProvider>
    <Demo />
    <Toaster />
  </ToastProvider>
);

export const Default: Story = { render: () => <Stage /> };

// a success is announced politely, a failure assertively, and either can be
// dismissed by hand before its timer runs out
export const AnnouncesAndDismisses: Story = {
  render: () => <Stage />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole("button", { name: "Success" }));
    const status = canvas.getByRole("status");
    // the card fades in, so visibility is awaited rather than asserted at once
    await waitFor(() => expect(within(status).getByText("Provider saved")).toBeVisible());

    await userEvent.click(canvas.getByRole("button", { name: "Error" }));
    const alert = canvas.getByRole("alert");
    await waitFor(() => expect(within(alert).getByText(/could not delete/i)).toBeVisible());
    await expect(within(alert).getByText(/still the target/i)).toBeInTheDocument();

    await userEvent.click(within(alert).getByRole("button", { name: "Dismiss notification" }));
    await waitFor(() => expect(within(alert).queryByText(/could not delete/i)).toBeNull());
    // the success is still on its own timer
    await expect(within(status).getByText("Provider saved")).toBeInTheDocument();
  },
};

function Timed({ tone, duration }: { tone: "success" | "error"; duration?: number }) {
  const toast = useToast();
  return (
    <Button
      onClick={() =>
        toast.push({
          tone,
          title: tone === "error" ? "An error notice" : "A success notice",
          duration,
        })
      }
    >
      Push
    </Button>
  );
}

/**
 * #2005 (WCAG 2.2.1): a toast is not taken away while it is being read. The
 * pointer resting on the card stops its clock, and leaving it starts the clock
 * again.
 */
export const PausesWhileHovered: Story = {
  render: () => (
    <ToastProvider>
      <Timed tone="success" duration={600} />
      <Toaster />
    </ToastProvider>
  ),
  play: async ({ canvas }) => {
    await userEvent.click(canvas.getByRole("button", { name: "Push" }));
    const message = await canvas.findByText("A success notice");
    await userEvent.hover(message);
    // well past its 600ms: still here because the pointer is on it
    await new Promise((resolve) => setTimeout(resolve, 1200)); // story-wait-allow: a timer that must not fire
    await expect(canvas.getByText("A success notice")).toBeInTheDocument();
    await userEvent.unhover(message);
    await waitFor(() => expect(canvas.queryByText("A success notice")).toBeNull());
  },
};

/** focus inside the card stops the clock the same way, for a keyboard user */
export const PausesWhileFocused: Story = {
  render: () => (
    <ToastProvider>
      <Timed tone="success" duration={600} />
      <Toaster />
    </ToastProvider>
  ),
  play: async ({ canvas }) => {
    await userEvent.click(canvas.getByRole("button", { name: "Push" }));
    const dismiss = await canvas.findByRole("button", { name: "Dismiss notification" });
    dismiss.focus();
    await new Promise((resolve) => setTimeout(resolve, 1200)); // story-wait-allow: a timer that must not fire
    await expect(canvas.getByText("A success notice")).toBeInTheDocument();
    dismiss.blur();
    await waitFor(() => expect(canvas.queryByText("A success notice")).toBeNull());
  },
};

/**
 * An error carries the control plane's message and has to be read, so it has no
 * clock: it outlives a success (4s) and goes only when dismissed.
 */
export const ErrorStaysUntilDismissed: Story = {
  render: () => (
    <ToastProvider>
      <Timed tone="success" />
      <Timed tone="error" />
      <Toaster />
    </ToastProvider>
  ),
  play: async ({ canvas }) => {
    const [pushSuccess, pushError] = canvas.getAllByRole("button", { name: "Push" });
    await userEvent.click(pushSuccess);
    await userEvent.click(pushError);
    await canvas.findByText("An error notice");
    await waitFor(() => expect(canvas.queryByText("A success notice")).toBeNull(), {
      timeout: 6000,
    });
    await expect(canvas.getByText("An error notice")).toBeInTheDocument();
    await userEvent.click(canvas.getByRole("button", { name: "Dismiss notification" }));
    await waitFor(() => expect(canvas.queryByText("An error notice")).toBeNull());
  },
};
