import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, waitFor, within } from "storybook/test";

import GuardrailPolicies from "./GuardrailPolicies";
import {
  Harness as ScreenHarness,
  Toasted,
  cancelConfirmation,
  confirmDestructive,
  expectAllowed,
  expectEmptyState,
  expectForbidden,
  expectListTable,
  expectLoadError,
  expectNoFalseEmpty,
  expectSheetClosed,
  expectSkeleton,
  expectToast,
  json,
  pending,
  recording,
  type FetchStub,
  type Recorder,
  type StoryRole,
} from "./story-harness";
import type { GuardrailPolicyInput, GuardrailPolicyRow, GuardrailProviderRow } from "@/lib/api";

const provider = (id: string, name: string): GuardrailProviderRow => ({
  id,
  name,
  enabled: true,
  kind: "webhook",
  url: `https://guardrails.internal/${name}`,
  model_path: null,
  model_sha256: null,
  stage: "pre_call",
  timeout_ms: 2000,
  max_retries: 0,
  failure_mode: "fail_closed",
  max_body_bytes: 65536,
  max_scored_bytes: null,
  on_truncated: "score",
  auth_kind: "none",
  auth_env: null,
  created_at: "2026-08-02T00:00:00Z",
  updated_at: "2026-08-02T00:00:00Z",
});

const PROVIDERS = [provider("prov-tox", "toxicity"), provider("prov-pii", "pii")];

const POLICIES: GuardrailPolicyRow[] = [
  {
    id: "pol-deployment",
    name: "baseline",
    enabled: true,
    scope_kind: "deployment",
    scope_id: null,
    provider_ids: ["prov-tox"],
    thresholds: { "*": 0.95 },
    on_violation: "annotate",
    streaming: "reject",
    description: "Deployment-wide default",
    created_at: "2026-09-01T00:00:00Z",
    updated_at: "2026-09-01T00:00:00Z",
  },
  {
    id: "pol-route",
    name: "support-bot",
    enabled: false,
    scope_kind: "route",
    scope_id: "gpt-4o",
    provider_ids: ["prov-pii", "prov-tox"],
    thresholds: { violence: 0.8, hate: 0.6 },
    on_violation: "block",
    streaming: "buffer",
    description: null,
    created_at: "2026-09-02T00:00:00Z",
    updated_at: "2026-09-02T00:00:00Z",
  },
];

/** answer the two reads (`write` answers everything that is not a GET) */
function registry(policies: GuardrailPolicyRow[], write?: FetchStub): FetchStub {
  return async (input, init) => {
    if (write && init?.method && init.method !== "GET") return write(input, init);
    const path = new URL(String(input), "http://localhost").pathname;
    return json(path === "/api/v1/guardrails/providers" ? PROVIDERS : policies);
  };
}

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
          <GuardrailPolicies />
        </Toasted>
      ) : (
        <GuardrailPolicies />
      )}
    </ScreenHarness>
  );
}

const meta = {
  title: "Screens/GuardrailPolicies",
  component: GuardrailPolicies,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof GuardrailPolicies>;
export default meta;
type Story = StoryObj<typeof meta>;

/** Each policy names its scope, its providers in order and its thresholds. */
export const Loaded: Story = {
  render: () => <Harness fetchStub={registry(POLICIES)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText("baseline");
    await expectListTable(canvasElement, "Guardrail Policies");
    await expect(canvas.getByText(/Exactly one policy applies to a request/)).toBeVisible();
    // provider ids are resolved to names, in the order the policy consults them
    await expect(canvas.getByText("1. pii")).toBeVisible();
    await expect(canvas.getByText("2. toxicity")).toBeVisible();
    await expect(canvas.getByText("violence ≥ 0.8")).toBeVisible();
    await expect(canvas.getByText("* ≥ 0.95")).toBeVisible();
    await expect(canvas.getByText("gpt-4o")).toBeVisible();
    await expect(canvas.getByText("Paused")).toBeVisible();
    await expect(canvas.getByText("Enabled")).toBeVisible();
  },
};

export const LoadedInRussian: Story = {
  globals: { locale: "ru" },
  render: () => <Harness fetchStub={registry(POLICIES)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText("baseline");
    await expect(canvas.getByText(/К запросу применяется ровно одна политика/)).toBeVisible();
    await expect(canvas.getByText("Приостановлена")).toBeVisible();
  },
};

export const Loading: Story = {
  render: () => <Harness fetchStub={pending} />,
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
    await expectNoFalseEmpty(canvasElement, /No guardrail policies/);
  },
};

export const Empty: Story = {
  render: () => <Harness fetchStub={registry([])} />,
  play: async ({ canvasElement }) => {
    await expectEmptyState(canvasElement, /No guardrail policies/, /Add first policy/);
  },
};

export const Error: Story = {
  render: () => (
    <Harness fetchStub={async () => json({ error: { message: "registry offline" } }, 503)} />
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /failed to return guardrail policies/);
    await expectNoFalseEmpty(canvasElement, /No guardrail policies/);
  },
};

export const Forbidden: Story = {
  render: () => <Harness fetchStub={async () => json({ error: { message: "forbidden" } }, 403)} />,
  play: async ({ canvasElement }) => {
    await expect(
      await within(canvasElement).findByText(/do not have access to guardrail policies/i),
    ).toBeVisible();
    await expect(within(canvasElement).queryByRole("button", { name: /try again/i })).toBeNull();
  },
};

// guardrail policies are deployment-wide: `guardrail_policy` is superadmin at
// every action, so an org role is refused before the screen asks. The stub
// answers with a good payload on purpose: a dropped `superadminOnly` would
// render it
export const RefusedToAnAdmin: Story = {
  render: () => <Harness fetchStub={registry(POLICIES)} role="admin" />,
  play: async ({ canvasElement }) => expectForbidden(canvasElement),
};

export const RefusedToAViewer: Story = {
  render: () => <Harness fetchStub={registry(POLICIES)} role="viewer" />,
  play: async ({ canvasElement }) => expectForbidden(canvasElement),
};

export const AllowedToASuperadmin: Story = {
  render: () => <Harness fetchStub={registry(POLICIES)} role="superadmin" />,
  play: async ({ canvasElement }) => {
    await within(canvasElement).findByText("baseline");
    await expectAllowed(canvasElement, "Add policy");
    await expectAllowed(canvasElement, "Edit policy baseline");
    await expectAllowed(canvasElement, "Delete policy baseline");
  },
};

const dialogOf = async () => within(await within(document.body).findByRole("dialog"));

let creates: Recorder;
/** Every field of a new policy reaches the request body as the operator set it. */
export const CreatesAPolicy: Story = {
  render: () => {
    creates = recording(registry(POLICIES, async () => json(POLICIES[1], 201)));
    return <Harness fetchStub={creates.stub} toasted />;
  },
  play: async ({ canvasElement }) => {
    await userEvent.click(await within(canvasElement).findByRole("button", { name: "Add policy" }));
    const form = await dialogOf();
    // a deployment policy has no scope id
    await expect(form.queryByLabelText("Public model name")).toBeNull();

    await userEvent.type(form.getByLabelText(/^Name/), "route-strict");
    await userEvent.click(form.getByRole("radio", { name: "Route" }));
    await userEvent.type(form.getByLabelText(/Public model name/), "gpt-4o");
    await userEvent.click(form.getByRole("button", { name: "toxicity" }));
    await userEvent.click(form.getByRole("button", { name: "Add threshold" }));
    await userEvent.type(form.getByLabelText("Category"), "violence");
    const value = form.getByLabelText("Threshold");
    await userEvent.clear(value);
    await userEvent.type(value, "0.7");
    await userEvent.click(form.getByRole("radio", { name: "Annotate" }));
    await userEvent.click(form.getByRole("radio", { name: "Buffer streams" }));
    await userEvent.click(form.getByRole("switch", { name: "Enabled" }));
    await userEvent.click(form.getByRole("button", { name: "Create policy" }));

    const body = await creates.expectSentBody<GuardrailPolicyInput>("POST", "/guardrails/policies");
    await expect(body).toEqual({
      name: "route-strict",
      enabled: true,
      scope_kind: "route",
      scope_id: "gpt-4o",
      provider_ids: ["prov-tox"],
      thresholds: { violence: 0.7 },
      on_violation: "annotate",
      streaming: "buffer",
      description: null,
    });
    await expectSheetClosed();
    await expectToast(canvasElement, /route-strict created/);
  },
};

let reorders: Recorder;
/** Providers are consulted in the order the sheet lists them, and the order can change. */
export const OrdersTheProviders: Story = {
  render: () => {
    reorders = recording(registry(POLICIES, async () => json(POLICIES[1], 201)));
    return <Harness fetchStub={reorders.stub} />;
  },
  play: async ({ canvasElement }) => {
    await userEvent.click(await within(canvasElement).findByRole("button", { name: "Add policy" }));
    const form = await dialogOf();
    await userEvent.type(form.getByLabelText(/^Name/), "ordered");
    await userEvent.click(form.getByRole("button", { name: "toxicity" }));
    await userEvent.click(form.getByRole("button", { name: "pii" }));
    const order = form.getByRole("list", { name: "Providers in the order they are consulted" });
    await expect(within(order).getAllByRole("listitem")[0]).toHaveTextContent("toxicity");

    await userEvent.click(within(order).getByRole("button", { name: "Move pii up" }));
    await expect(within(order).getAllByRole("listitem")[0]).toHaveTextContent("pii");
    await userEvent.click(form.getByRole("button", { name: "Create policy" }));
    const body = await reorders.expectSentBody<GuardrailPolicyInput>(
      "POST",
      "/guardrails/policies",
    );
    await expect(body.provider_ids).toEqual(["prov-pii", "prov-tox"]);
  },
};

let edits: Recorder;
/** An existing policy opens as stored and saves with a PUT to its own id. */
export const EditsAPolicy: Story = {
  render: () => {
    edits = recording(registry(POLICIES, async () => json(POLICIES[1])));
    return <Harness fetchStub={edits.stub} />;
  },
  play: async ({ canvasElement }) => {
    await userEvent.click(
      await within(canvasElement).findByRole("button", { name: "Edit policy support-bot" }),
    );
    const form = await dialogOf();
    await expect(form.getByLabelText(/^Name/)).toHaveValue("support-bot");
    await expect(form.getByRole("radio", { name: "Route" })).toBeChecked();
    await expect(form.getByLabelText(/Public model name/)).toHaveValue("gpt-4o");
    await expect(form.getByRole("radio", { name: "Buffer streams" })).toBeChecked();
    await expect(form.getAllByLabelText("Category")).toHaveLength(2);

    await userEvent.click(form.getByRole("radio", { name: "Reject streams" }));
    await userEvent.click(form.getByRole("button", { name: "Save policy" }));
    const body = await edits.expectSentBody<GuardrailPolicyInput>(
      "PUT",
      "/guardrails/policies/pol-route",
    );
    await expect(body).toMatchObject({
      name: "support-bot",
      scope_kind: "route",
      scope_id: "gpt-4o",
      provider_ids: ["prov-pii", "prov-tox"],
      thresholds: { violence: 0.8, hate: 0.6 },
      streaming: "reject",
      enabled: false,
    });
    await expectSheetClosed();
  },
};

/** A threshold outside 0 to 1 is named at its row and keeps Save off. */
export const ThresholdOutOfRangeBlocksSaving: Story = {
  render: () => <Harness fetchStub={registry(POLICIES)} />,
  play: async ({ canvasElement }) => {
    await userEvent.click(await within(canvasElement).findByRole("button", { name: "Add policy" }));
    const form = await dialogOf();
    await userEvent.type(form.getByLabelText(/^Name/), "strict");
    await userEvent.click(form.getByRole("button", { name: "Add threshold" }));
    await userEvent.type(form.getByLabelText("Category"), "violence");
    const value = form.getByLabelText("Threshold");
    await userEvent.clear(value);
    await userEvent.type(value, "1.5");

    await expect(await form.findByText("Enter a number from 0 to 1.")).toBeVisible();
    await expect(value).toHaveAttribute("aria-invalid", "true");
    await expect(value).toHaveAccessibleDescription("Enter a number from 0 to 1.");
    await expect(form.getByRole("button", { name: "Create policy" })).toBeDisabled();

    await userEvent.clear(value);
    await userEvent.type(value, "0.9");
    await waitFor(() => expect(form.getByRole("button", { name: "Create policy" })).toBeEnabled());
    await expect(form.queryByText("Enter a number from 0 to 1.")).toBeNull();
  },
};

/** A scoped policy cannot be saved without the id it is pinned to. */
export const ScopedPolicyNeedsItsId: Story = {
  render: () => <Harness fetchStub={registry(POLICIES)} />,
  play: async ({ canvasElement }) => {
    await userEvent.click(await within(canvasElement).findByRole("button", { name: "Add policy" }));
    const form = await dialogOf();
    await userEvent.type(form.getByLabelText(/^Name/), "per-project");
    await userEvent.click(form.getByRole("radio", { name: "Project" }));
    await expect(form.getByLabelText(/Project id/)).toBeVisible();
    await expect(form.getByRole("button", { name: "Create policy" })).toBeDisabled();
    await userEvent.type(form.getByLabelText(/Project id/), "proj-1");
    await waitFor(() => expect(form.getByRole("button", { name: "Create policy" })).toBeEnabled());
  },
};

/**
 * The control plane refuses a second enabled policy for one scope: the sheet
 * stays open with every field as it was and shows the control plane's words.
 */
export const ConflictKeepsTheSheetOpen: Story = {
  render: () => (
    <Harness
      fetchStub={registry(POLICIES, async () =>
        json(
          {
            error: {
              code: "conflict",
              message: "an enabled policy already exists for route gpt-4o",
            },
          },
          409,
        ),
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    await userEvent.click(await within(canvasElement).findByRole("button", { name: "Add policy" }));
    const form = await dialogOf();
    await userEvent.type(form.getByLabelText(/^Name/), "second");
    await userEvent.click(form.getByRole("button", { name: "Create policy" }));
    await expect(
      await form.findByText(/an enabled policy already exists for route gpt-4o/),
    ).toBeVisible();
    await expect(form.getByLabelText(/^Name/)).toHaveValue("second");
  },
};

/** A refusal that names a field says which one. */
export const InvalidFieldNamesTheField: Story = {
  render: () => (
    <Harness
      fetchStub={registry(POLICIES, async () =>
        json(
          {
            error: {
              code: "invalid_field",
              message: "thresholds: out of range",
              field: "thresholds",
            },
          },
          400,
        ),
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    await userEvent.click(await within(canvasElement).findByRole("button", { name: "Add policy" }));
    const form = await dialogOf();
    await userEvent.type(form.getByLabelText(/^Name/), "bad");
    await userEvent.click(form.getByRole("button", { name: "Create policy" }));
    await expect(await form.findByText(/The value of thresholds is not accepted/)).toBeVisible();
  },
};

let deletes: Recorder;
let deleted = false;
/** Delete asks first, names the policy, and the row goes once the DELETE lands. */
export const ConfirmsBeforeDeletingAPolicy: Story = {
  render: () => {
    deleted = false;
    deletes = recording(async (input, init) => {
      if (init?.method === "DELETE") {
        deleted = true;
        return json({}, 204);
      }
      return registry(deleted ? POLICIES.slice(1) : POLICIES)(input, init);
    });
    return <Harness fetchStub={deletes.stub} toasted />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const del = async () => canvas.findByRole("button", { name: "Delete policy baseline" });

    await userEvent.click(await del());
    await cancelConfirmation();
    deletes.expectNotSent("DELETE", "/guardrails/policies/pol-deployment");

    await userEvent.click(await del());
    await confirmDestructive(/baseline/, "Delete");
    await deletes.expectSent("DELETE", "/guardrails/policies/pol-deployment");
    await expectSheetClosed();
    await expectToast(canvasElement, /baseline deleted/);
    await waitFor(() => expect(canvas.queryByText("baseline")).not.toBeInTheDocument());
  },
};
