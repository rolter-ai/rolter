import type { Meta, StoryObj } from "@storybook/react-vite";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { ParamsEditor, type ParamsEditorResult, type ParamsEditorValue } from "./ParamsEditor";
import { pickOption } from "@/pages/story-harness";

const PARAMS = { temperature: 0.7, max_tokens: 1024, stream: false };
const ALLOW_ALL = { mode: "allow", allow: [], deny: [] };

/** `variant="edit"` owns a Save button; this captures what it hands back. */
function EditHarness({
  params = PARAMS,
  paramPolicy = ALLOW_ALL,
  saving = false,
  error,
}: {
  params?: Record<string, unknown>;
  paramPolicy?: Record<string, unknown>;
  saving?: boolean;
  error?: string;
}) {
  const [saved, setSaved] = React.useState<ParamsEditorValue | null>(null);
  return (
    <div className="max-w-xl space-y-3">
      <ParamsEditor
        params={params}
        paramPolicy={paramPolicy}
        saving={saving}
        error={error}
        onSave={setSaved}
      />
      {saved && (
        <pre className="whitespace-pre-wrap font-mono text-xs" data-testid="saved">
          {JSON.stringify(saved)}
        </pre>
      )}
    </div>
  );
}

/**
 * `variant="create"` is controlled: it reports the serialized value — or the
 * validation error — up on every keystroke, so the route can be created first
 * and the params persisted after.
 */
function CreateHarness() {
  const [result, setResult] = React.useState<ParamsEditorResult | null>(null);
  return (
    <div className="max-w-xl space-y-3">
      <ParamsEditor variant="create" params={PARAMS} onChange={setResult} />
      <pre className="whitespace-pre-wrap font-mono text-xs" data-testid="reported">
        {JSON.stringify(result)}
      </pre>
    </div>
  );
}

const meta = {
  title: "Components/ParamsEditor",
  component: ParamsEditor,
  parameters: { layout: "padded" },
  args: { params: PARAMS, paramPolicy: ALLOW_ALL, saving: false, onSave: () => {} },
} satisfies Meta<typeof ParamsEditor>;

export default meta;

// `ParamsEditor` takes a discriminated union of props (`edit` | `create`),
// which the story types cannot narrow through `meta.args` — every story would
// have to restate the whole union. Each one drives the component through its
// own harness anyway, so the story type only has to allow a bare `render`.
type Story = StoryObj;

/** Seeded from a route's stored params, one typed row each. */
export const Loaded: Story = {
  render: () => <EditHarness />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // the rows are seeded in an effect, so the first query has to wait for it
    const names = await canvas.findAllByLabelText("Param name");
    await expect(names).toHaveLength(3);
    await expect(names[0]).toHaveValue("temperature");
    // the type is inferred from the stored value, not guessed at save time
    await expect(canvas.getAllByLabelText("Param type")[1]).toHaveValue("number");
  },
};

/** A route with no admin defaults says so instead of showing an empty box. */
export const NoParams: Story = {
  render: () => <EditHarness params={{}} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByText("No default params.")).toBeVisible();
    await expect(canvas.getByRole("button", { name: /add param/i })).toBeEnabled();
  },
};

/** Saving: the button goes busy rather than accepting a second press. */
export const Saving: Story = {
  render: () => <EditHarness saving />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByRole("button", { name: "Save params" })).toBeDisabled();
  },
};

/** A rejected save keeps the rows on screen with the reason under them. */
export const SaveRejected: Story = {
  render: () => <EditHarness error="route not found — it may have been deleted" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByText(/route not found/)).toBeVisible();
    await expect((await canvas.findAllByLabelText("Param name"))[0]).toBeVisible();
  },
};

/** Adding a row and saving reports the typed value, not the raw text. */
export const AddsAParam: Story = {
  render: () => <EditHarness params={{}} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole("button", { name: /add param/i }));
    await userEvent.type(canvas.getByLabelText("Param name"), "top_p");
    await pickOption(canvas.getByLabelText("Param type"), "number");
    await userEvent.type(canvas.getByLabelText("Param value"), "0.9");
    await userEvent.click(canvas.getByRole("button", { name: "Save params" }));
    await waitFor(() => expect(canvas.getByTestId("saved")).toHaveTextContent('"top_p":0.9'));
  },
};

/**
 * Keys are provider-agnostic and a caller may legitimately pass a
 * provider-specific extra, so an unrecognised one is a warning and still
 * saves — not a block.
 */
export const UnknownParamKeyWarns: Story = {
  render: () => <EditHarness params={{ temperture: 0.7 }} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(/Not a standard OpenAI\/Anthropic param/)).toBeVisible();
    await expect(canvas.getByLabelText("Unrecognized param key")).toBeVisible();
    await expect(canvas.getByRole("button", { name: "Save params" })).toBeEnabled();
  },
};

/** Malformed JSON is caught here rather than by the control plane. */
export const InvalidJsonValue: Story = {
  render: () => <EditHarness params={{ stop: ["\n"] }} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const value = await canvas.findByLabelText("Param value");
    await userEvent.clear(value);
    // not `[oops`: userEvent reads square brackets as key descriptors, and a
    // bare word is just as invalid as JSON
    await userEvent.type(value, "oops");
    await userEvent.click(canvas.getByRole("button", { name: "Save params" }));
    await expect(canvas.getByText(/invalid JSON value/)).toBeVisible();
    await expect(canvas.queryByTestId("saved")).not.toBeInTheDocument();
  },
};

/**
 * "Manual" is the UI's affordance for a mixed policy: the backend models only
 * allow/deny with exception lists, so a locked row is serialized as
 * allow-mode plus a deny entry for that key.
 */
export const ManualPolicyLocksOneParam: Story = {
  render: () => <EditHarness />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole("button", { name: "Manual" }));
    await userEvent.click(canvas.getAllByRole("button", { name: /click to lock/i })[0]);
    await userEvent.click(canvas.getByRole("button", { name: "Save params" }));
    await waitFor(() =>
      expect(canvas.getByTestId("saved")).toHaveTextContent('"deny":["temperature"]'),
    );
  },
};

/** Locking everything by default is one press, and the hint changes with it. */
export const LockedByDefault: Story = {
  render: () => <EditHarness />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole("button", { name: "Locked by default" }));
    await expect(canvas.getByText(/the admin values always win/)).toBeVisible();
    await userEvent.click(canvas.getByRole("button", { name: "Save params" }));
    await waitFor(() => expect(canvas.getByTestId("saved")).toHaveTextContent('"mode":"deny"'));
  },
};

/** The create variant reports upward instead of rendering its own Save. */
export const CreateVariantReportsUpward: Story = {
  render: () => <CreateHarness />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.queryByRole("button", { name: "Save params" })).not.toBeInTheDocument();
    await waitFor(() => expect(canvas.getByTestId("reported")).toHaveTextContent('"ok":true'));
    const value = canvas.getAllByLabelText("Param value")[0];
    await userEvent.clear(value);
    await userEvent.type(value, "0.2");
    await waitFor(() =>
      expect(canvas.getByTestId("reported")).toHaveTextContent('"temperature":0.2'),
    );
  },
};

/** Removing a row drops it from what gets saved. */
export const RemovesAParam: Story = {
  render: () => <EditHarness />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click((await canvas.findAllByRole("button", { name: /remove param/i }))[0]);
    await expect(canvas.getAllByLabelText("Param name")).toHaveLength(2);
    await userEvent.click(canvas.getByRole("button", { name: "Save params" }));
    await waitFor(() => expect(canvas.getByTestId("saved")).not.toHaveTextContent("temperature"));
  },
};
