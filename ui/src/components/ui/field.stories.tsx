import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, within } from "storybook/test";

import { Field } from "./field";
import { Input } from "./input";
import { Combobox } from "./combobox";

const meta = {
  title: "Primitives/Field",
  component: Field,
  parameters: { layout: "padded" },
  args: { label: "Model name" },
} satisfies Meta<typeof Field>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  render: (args) => (
    <Field {...args}>
      <Input defaultValue="gpt-4o" />
    </Field>
  ),
};

export const WithHint: Story = {
  args: { hint: "The alias callers address; rewritten to the upstream name." },
  render: (args) => (
    <Field {...args}>
      <Input defaultValue="gpt-4o" />
    </Field>
  ),
};

export const WithError: Story = {
  args: { error: "A model with this name already exists in the project." },
  render: (args) => (
    <Field {...args}>
      <Input defaultValue="gpt-4o" />
    </Field>
  ),
};

export const WithInfo: Story = {
  args: {
    info: "Callers send this name; rolter maps it onto the upstream model of the target provider.",
  },
  render: (args) => (
    <Field {...args}>
      <Input defaultValue="gpt-4o" />
    </Field>
  ),
};

/**
 * Almost no call site passes `htmlFor`, so the field generates an id and puts
 * it on its single child. That is the whole reason the component exists: before
 * it, every label in the dashboard pointed at nothing and a screen reader
 * announced the control as unlabelled.
 */
export const LabelsItsControl: Story = {
  render: (args) => (
    <Field {...args}>
      <Input defaultValue="gpt-4o" />
    </Field>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByLabelText("Model name")).toHaveValue("gpt-4o");
  },
};

/**
 * An error is announced as well as coloured: it takes `role="alert"`, is tied
 * to the control through `aria-describedby`, and flips `aria-invalid`, so the
 * state and the reason arrive together rather than as a control that silently
 * refuses to submit.
 */
export const ErrorDescribesTheControl: Story = {
  args: { error: "A model with this name already exists in the project." },
  render: (args) => (
    <Field {...args}>
      <Input defaultValue="gpt-4o" />
    </Field>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const control = canvas.getByLabelText("Model name");
    await expect(control).toHaveAttribute("aria-invalid", "true");
    await expect(control).toHaveAccessibleDescription(/already exists/);
    await expect(canvas.getByRole("alert")).toBeVisible();
  },
};

/** The (i) beside the label opens the note without disturbing the control. */
export const InfoHintOpensOnHover: Story = {
  args: {
    info: "Callers send this name; rolter maps it onto the upstream model of the target provider.",
  },
  render: (args) => (
    <Field {...args}>
      <Input defaultValue="gpt-4o" />
    </Field>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.hover(canvas.getByRole("button", { name: /about model name/i }));
    await expect(canvas.getByRole("tooltip")).toHaveTextContent(/upstream model/);
  },
};

const STRATEGIES = [
  { value: "round_robin", label: "Round robin" },
  { value: "least_latency", label: "Least latency" },
];

/** A field wraps whatever control it is handed, not just an `Input`. */
export const AroundACombobox: Story = {
  args: { label: "Strategy", hint: "How requests are balanced across targets." },
  render: (args) => (
    <Field {...args}>
      <Combobox value="round_robin" onChange={() => {}} options={STRATEGIES} />
    </Field>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByLabelText("Strategy")).toHaveValue("Round robin");
  },
};

/**
 * Two children — a control and a note beside it — used to leave the label
 * pointing at nothing: there was no single element to put the generated id on,
 * so the control had no accessible name and `getByLabelText` could not find it
 * either. Nothing about that is visible on screen, which is why it survived
 * ~200 call sites (#1264).
 */
export const LabelsTheFirstOfTwoChildren: Story = {
  args: { label: "Strategy" },
  render: (args) => (
    <Field {...args}>
      <Combobox value="round_robin" onChange={() => {}} options={STRATEGIES} />
      <p className="text-xs text-muted-foreground">
        Least latency needs health data before it can rank anything.
      </p>
    </Field>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByLabelText("Strategy")).toHaveValue("Round robin");
  },
};

/** The hint below a two-child field still describes the control it belongs to. */
export const TwoChildrenKeepTheirDescription: Story = {
  args: { label: "Model name", error: "A model with this name already exists." },
  render: (args) => (
    <Field {...args}>
      <Input defaultValue="gpt-4o" />
      <p className="text-xs text-muted-foreground">Rewritten to the upstream name.</p>
    </Field>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const control = canvas.getByLabelText("Model name");
    await expect(control).toHaveAttribute("aria-invalid", "true");
    await expect(control).toHaveAccessibleDescription(/already exists/);
  },
};

/**
 * An explicit `htmlFor` over several children names the control but used to
 * skip its description: the error was on screen and `aria-invalid` was never
 * set, so the input sounded valid to a screen reader (#1527).
 */
export const ExplicitIdKeepsTheDescription: Story = {
  args: {
    label: "API base",
    htmlFor: "field-api-base",
    error: "Must start with http:// or https://.",
  },
  render: (args) => (
    <Field {...args}>
      <Input id="field-api-base" defaultValue="api.example.com" />
      <p className="text-xs text-muted-foreground">
        Resolves to api.example.com/v1/chat/completions
      </p>
    </Field>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const control = canvas.getByLabelText("API base");
    await expect(control).toHaveAttribute("aria-invalid", "true");
    await expect(control).toHaveAccessibleDescription(/must start with http/i);
  },
};
