import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { expect, userEvent, within } from "storybook/test";

import { describedBy, FieldError } from "./field-error";
import { FieldLabel } from "./field-label";
import { Input } from "./input";

const meta = {
  title: "Forms/FieldError",
  component: FieldError,
  parameters: { layout: "padded" },
  args: { id: "fe-message", error: "Alias must be unique within the project." },
} satisfies Meta<typeof FieldError>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {};

/** No error, no node: a valid field is not a field with an empty red line. */
export const NoError: Story = {
  args: { error: undefined },
  render: (args) => (
    <div className="max-w-sm space-y-1.5" data-testid="slot">
      <FieldError {...args} />
    </div>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // bites if the component starts rendering an empty <p> for a clean field
    await expect(canvas.getByTestId("slot")).toBeEmptyDOMElement();
  },
};

/**
 * The error's `id` is what the control points at, so the reason is announced
 * where the field is — not only when the operator reaches the footer.
 */
export const DescribesItsControl: Story = {
  render: (args) => (
    <div className="max-w-sm space-y-1.5">
      <FieldLabel label="Alias" htmlFor="fe-alias" required />
      <Input
        id="fe-alias"
        defaultValue="gpt-4o"
        aria-invalid
        aria-describedby={describedBy("fe-hint", !!args.error && "fe-message")}
      />
      <p id="fe-hint" className="text-xs text-muted-foreground">
        The name clients route by.
      </p>
      <FieldError {...args} />
    </div>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const input = canvas.getByLabelText(/Alias/);
    // bites if the id stops reaching the <p>: the description loses the reason
    await expect(input).toHaveAccessibleDescription(
      "The name clients route by. Alias must be unique within the project.",
    );
    await expect(input).toHaveAttribute("aria-invalid", "true");
  },
};

/**
 * `describedBy` joins the ids a control actually has and drops the rest, so a
 * field with no error does not point at an element that is not rendered.
 */
export const DescribedByDropsAbsentIds: Story = {
  render: () => {
    const [error, setError] = React.useState<string | undefined>();
    return (
      <div className="max-w-sm space-y-1.5">
        <FieldLabel label="Weight" htmlFor="fe-weight" />
        <Input
          id="fe-weight"
          defaultValue="1"
          aria-describedby={describedBy("fe-weight-hint", !!error && "fe-weight-error")}
        />
        <p id="fe-weight-hint" className="text-xs text-muted-foreground">
          Relative share of traffic.
        </p>
        <FieldError id="fe-weight-error" error={error} />
        <button type="button" onClick={() => setError("Weight must be a positive integer.")}>
          Reject the value
        </button>
      </div>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const input = canvas.getByLabelText(/Weight/);
    // bites if `describedBy` stops filtering: a clean field would point at an
    // error element that is not in the document
    await expect(input).toHaveAttribute("aria-describedby", "fe-weight-hint");
    await expect(input).toHaveAccessibleDescription("Relative share of traffic.");
    await userEvent.click(canvas.getByRole("button", { name: "Reject the value" }));
    await expect(input).toHaveAttribute("aria-describedby", "fe-weight-hint fe-weight-error");
    await expect(input).toHaveAccessibleDescription(
      "Relative share of traffic. Weight must be a positive integer.",
    );
  },
};
