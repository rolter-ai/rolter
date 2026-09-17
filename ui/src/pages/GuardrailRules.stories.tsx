import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, waitFor, within } from "storybook/test";

import GuardrailRules from "./GuardrailRules";
import {
  cancelConfirmation,
  confirmDestructive,
  expectEmptyState,
  expectForbidden,
  expectLoadError,
  expectSkeleton,
  expectToast,
  Harness as ScreenHarness,
  json,
  pickOption,
  recording,
  Toasted,
  type FetchStub,
  type StoryRole,
} from "./story-harness";
import type { GuardrailRuleRow } from "@/lib/api";

const RULES: GuardrailRuleRow[] = [
  {
    id: "rule-email",
    name: "Redact customer email",
    enabled: true,
    source_type: "builtin",
    builtin: "email",
    pattern: null,
    stage: "pre_call",
    action: "redact",
    replacement: "[REDACTED:EMAIL]",
    include_system: false,
    position: 10,
    created_at: "2026-08-02T00:00:00Z",
    updated_at: "2026-08-02T00:00:00Z",
  },
  {
    id: "rule-injection",
    name: "Block override attempts",
    enabled: true,
    source_type: "pattern",
    builtin: null,
    pattern: "(?i)ignore previous instructions",
    stage: "pre_call",
    action: "block",
    replacement: null,
    include_system: true,
    position: 20,
    created_at: "2026-08-02T00:00:00Z",
    updated_at: "2026-08-02T00:00:00Z",
  },
];


/**
 * The screen under the shared fetch-stub harness, with a role to render as.
 *
 * `role` is what a story needs to mount a `CapabilityProvider` at all: with no
 * provider above it `can()` answers "unknown", the `superadminOnly` wrapper
 * never blocks, and a story can only reach the 403 by stubbing one — which
 * tests the screen's own error path rather than the gate (#1606).
 *
 * `toasted` is opt-in: the Toaster contributes its own role="status" and
 * role="alert" regions, and the stories that query those by role would stop
 * being able to.
 */
function Harness({
  fetchStub,
  role,
  toasted,
}: {
  fetchStub: FetchStub;
  role?: StoryRole;
  toasted?: boolean;
}) {
  return (
    <ScreenHarness fetchStub={fetchStub} role={role}>
      {toasted ? (
        <Toasted>
          <GuardrailRules />
        </Toasted>
      ) : (
        <GuardrailRules />
      )}
    </ScreenHarness>
  );
}

const meta = { title: "Screens/GuardrailRules", component: GuardrailRules, parameters: { layout: "fullscreen" } } satisfies Meta<typeof GuardrailRules>;
export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = { render: () => <Harness fetchStub={async () => json(RULES)} /> };
export const Loading: Story = {
  render: () => <Harness fetchStub={() => new Promise<Response>(() => {})} />,
  play: async ({ canvasElement }) => expectSkeleton(canvasElement),
};
export const Empty: Story = {
  render: () => <Harness fetchStub={async () => json([])} />,
  play: async ({ canvasElement }) => {
    await expectEmptyState(canvasElement, /No inspection rules/, /Add first rule/);
  },
};
export const Error: Story = {
  render: () => <Harness fetchStub={async () => json({ error: { message: "registry offline" } }, 503)} />,
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /failed to return guardrail rules/);
  },
};

// the two failures the bespoke panel got wrong (#1259). a deployment-scoped
// screen is refused to every non-superadmin, and a "try again" on a permission
// suggests the refusal was transient
export const Forbidden: Story = {
  render: () => (
    <Harness fetchStub={async () => json({ error: { message: "forbidden" } }, 403)} />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(
      await canvas.findByText(/do not have access to guardrail rules/i),
    ).toBeVisible();
    await expect(canvas.queryByRole("button", { name: /try again/i })).toBeNull();
  },
};

// fetch never connected, so there is no status to read: that one *is* worth
// retrying, and the control plane's own message is still printed underneath
export const Unreachable: Story = {
  render: () => (
    <Harness fetchStub={() => Promise.reject(new TypeError("Failed to fetch"))} />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(
      await canvas.findByText(/cannot reach the control plane/i),
    ).toBeVisible();
    await expect(
      await canvas.findByRole("button", { name: /try again/i }),
    ).toBeVisible();
  },
};

export const CreatesCustomRule: Story = {
  render: () => <Harness fetchStub={async (_input, init) => init?.method === "POST" ? json(RULES[1], 201) : json(RULES)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: /add rule/i }));
    const dialog = within(document.body).getByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText("Rule name"), "Prompt injection policy");
    await pickOption(within(dialog).getByLabelText("Source"), "Custom regex");
    await userEvent.type(within(dialog).getByLabelText("Regular expression"), "ignore previous instructions");
    await expect(within(dialog).getByRole("button", { name: "Publish rule" })).toBeEnabled();
  },
};

/**
 * The rule is refused (#1607).
 *
 * The regex was written by hand, so a dialog that closed on a rejected publish
 * would cost the pattern as well as the name.
 */
export const CreateRejectedByTheServer: Story = {
  render: () => (
    <Harness
      toasted
      fetchStub={async (_input, init) =>
        init?.method === "POST"
          ? json({ error: { message: "the pattern does not compile: unbalanced group" } }, 422)
          : json(RULES)
      }
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: /add rule/i }));
    const dialog = within(document.body).getByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText("Rule name"), "Prompt injection policy");
    await pickOption(within(dialog).getByLabelText("Source"), "Custom regex");
    await userEvent.type(
      within(dialog).getByLabelText("Regular expression"),
      "ignore previous instructions",
    );
    await userEvent.click(within(dialog).getByRole("button", { name: "Publish rule" }));

    await expectToast(canvasElement, /does not compile/, "error");
    await waitFor(() =>
      expect(within(document.body).getByRole("dialog")).toBeInTheDocument(),
    );
    await expect(within(dialog).getByLabelText("Regular expression")).toHaveValue(
      "ignore previous instructions",
    );
  },
};

// the delete used to be a bare window.confirm — unstyled, untranslated, and a
// modal the story runner cannot answer. It is a real dialog now (#1179)
const deletes = recording(async (_input, init) =>
  init?.method === "DELETE" ? json({}, 204) : json(RULES),
);

export const ConfirmsBeforeDeletingARule: Story = {
  render: () => <Harness fetchStub={deletes.stub} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // by name, not by index: each row control names its own rule (#1214)
    const del = async () =>
      canvas.findByRole("button", { name: "Delete rule Redact customer email" });

    await userEvent.click(await del());
    await cancelConfirmation();
    deletes.expectNotSent("DELETE", "/guardrails/rules/rule-email");

    await userEvent.click(await del());
    await confirmDestructive(/Redact customer email/, "Delete");
    await deletes.expectSent("DELETE", "/guardrails/rules/rule-email");
  },
};

export const EditsRule: Story = {
  render: () => <Harness fetchStub={async () => json(RULES)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", { name: "Edit rule Redact customer email" }),
    );
    await waitFor(() => expect(within(document.body).getByRole("dialog")).toBeVisible());
    await expect(within(document.body).getByLabelText("Rule name")).toHaveValue("Redact customer email");
  },
};

// What a non-superadmin gets: the screen refused before it asks (#1606).
//
// `guardrail_rule` is superadmin at every action, so `superadminOnly` never
// mounts the screen for an org role however high. The stub answers with a good
// payload on purpose: if the wrapper is dropped the screen renders that payload
// and this story fails, which the `Forbidden` story cannot do.
export const RefusedToAnAdmin: Story = {
  render: () => <Harness fetchStub={async () => json(RULES)} role="admin" />,
  play: async ({ canvasElement }) => expectForbidden(canvasElement),
};

export const RefusedToAViewer: Story = {
  render: () => <Harness fetchStub={async () => json(RULES)} role="viewer" />,
  play: async ({ canvasElement }) => expectForbidden(canvasElement),
};
