import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { expect, userEvent, within } from "storybook/test";

import { Button } from "./button";
import { Field } from "./field";
import { Input } from "./input";
import { Sheet, SheetBody, SheetError, SheetFooter, SheetHeader } from "./sheet";

const meta = {
  title: "Overlays/Sheet",
  component: Sheet,
  parameters: { layout: "fullscreen" },
  // the editor owns the open state; these satisfy the required-prop type
  args: { open: false, onOpenChange: () => {}, children: null },
} satisfies Meta<typeof Sheet>;

export default meta;
type Story = StoryObj<typeof meta>;

function Editor() {
  const [open, setOpen] = React.useState(false);
  return (
    <div>
      <Button onClick={() => setOpen(true)}>Edit route</Button>
      <Sheet open={open} onOpenChange={setOpen}>
        <SheetHeader title="Edit route" subtitle="gpt-4o" onClose={() => setOpen(false)} />
        <SheetBody>
          <Field label="Model name" htmlFor="model">
            <Input id="model" defaultValue="gpt-4o" />
          </Field>
          <Field label="Strategy" htmlFor="strategy">
            <Input id="strategy" defaultValue="round_robin" />
          </Field>
        </SheetBody>
        <SheetFooter>
          <Button variant="outline" onClick={() => setOpen(false)}>
            Cancel
          </Button>
          <Button onClick={() => setOpen(false)}>Save</Button>
        </SheetFooter>
      </Sheet>
    </div>
  );
}

export const Default: Story = { render: () => <Editor /> };

// interaction: the sheet opens from the trigger and the header close button
// dismisses it. content is portalled to document.body.
export const OpensAndDismisses: Story = {
  render: () => <Editor />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const body = within(document.body);
    await expect(body.queryByRole("dialog")).toBeNull();

    await userEvent.click(canvas.getByRole("button", { name: "Edit route" }));
    const sheet = await body.findByRole("dialog");
    await expect(sheet).toBeInTheDocument();
    await expect(body.getByLabelText("Model name")).toHaveValue("gpt-4o");

    await userEvent.click(body.getByRole("button", { name: "Close" }));
    await expect(body.queryByRole("dialog")).toBeNull();
  },
};

/**
 * The failure line above the buttons.
 *
 * `role="alert"` is the whole reason this is a component: five sheets rendered
 * the same `<p>` and only one of them carried the live region, so the same save
 * failure was announced in `ModelSheet` and silent everywhere else — a
 * difference nothing on screen shows (#1658).
 */
function FailedSave() {
  return (
    <Sheet open onOpenChange={() => {}}>
      <SheetHeader title="Edit route" subtitle="gpt-4o" onClose={() => {}} />
      <SheetBody>
        <Field label="Model name" htmlFor="model">
          <Input id="model" defaultValue="gpt-4o" />
        </Field>
      </SheetBody>
      <SheetFooter>
        <SheetError message="route name already taken" />
        <Button>Save</Button>
      </SheetFooter>
    </Sheet>
  );
}

export const SaveFailed: Story = {
  render: () => <FailedSave />,
  play: async () => {
    const body = within(document.body);
    const alert = await body.findByRole("alert");
    await expect(alert).toHaveTextContent("route name already taken");
  },
};

/** No message, no element — so a caller can pass its error straight through. */
export const NoErrorRendersNothing: Story = {
  render: () => (
    <Sheet open onOpenChange={() => {}}>
      <SheetHeader title="Edit route" subtitle="gpt-4o" onClose={() => {}} />
      <SheetBody>
        <Field label="Model name" htmlFor="model">
          <Input id="model" defaultValue="gpt-4o" />
        </Field>
      </SheetBody>
      <SheetFooter>
        <SheetError />
        <Button>Save</Button>
      </SheetFooter>
    </Sheet>
  ),
  play: async () => {
    await expect(within(document.body).queryByRole("alert")).not.toBeInTheDocument();
  },
};
