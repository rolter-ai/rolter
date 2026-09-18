import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, waitFor, within } from "storybook/test";

import GuardrailProviders from "./GuardrailProviders";
import {
  cancelConfirmation,
  confirmDestructive,
  expectEmptyState,
  expectForbidden,
  expectLoadError,
  expectSheetClosed,
  expectSkeleton,
  expectToast,
  Harness as ScreenHarness,
  json,
  recording,
  Toasted,
  type FetchStub,
  type StoryRole,
} from "./story-harness";
import type { GuardrailProviderRow } from "@/lib/api";

const PROVIDERS: GuardrailProviderRow[] = [
  {
    id: "provider-primary",
    name: "Production LLM Guard",
    enabled: true,
    url: "https://guardrails.internal/v1/evaluate",
    stage: "pre_call",
    timeout_ms: 1800,
    max_retries: 1,
    failure_mode: "fail_closed",
    max_body_bytes: 65536,
    auth_kind: "bearer",
    auth_env: "ROLTER_GUARDRAIL_TOKEN",
    created_at: "2026-08-02T00:00:00Z",
    updated_at: "2026-08-02T00:00:00Z",
  },
  {
    id: "provider-fallback",
    name: "Staging evaluator",
    enabled: false,
    url: "https://guardrails.staging.internal/evaluate",
    stage: "pre_call",
    timeout_ms: 2500,
    max_retries: 0,
    failure_mode: "fail_open",
    max_body_bytes: 32768,
    auth_kind: "none",
    auth_env: null,
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
          <GuardrailProviders />
        </Toasted>
      ) : (
        <GuardrailProviders />
      )}
    </ScreenHarness>
  );
}

const meta = { title: "Screens/GuardrailProviders", component: GuardrailProviders, parameters: { layout: "fullscreen" } } satisfies Meta<typeof GuardrailProviders>;
export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = { render: () => <Harness fetchStub={async () => json(PROVIDERS)} /> };
export const Loading: Story = {
  render: () => <Harness fetchStub={() => new Promise<Response>(() => {})} />,
  play: async ({ canvasElement }) => expectSkeleton(canvasElement),
};
export const Empty: Story = {
  render: () => <Harness fetchStub={async () => json([])} />,
  play: async ({ canvasElement }) => {
    await expectEmptyState(canvasElement, /No guardrail providers/, /Register first provider/);
  },
};
export const Error: Story = {
  render: () => <Harness fetchStub={async () => json({ error: { message: "registry offline" } }, 503)} />,
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /failed to return guardrail providers/);
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
      await canvas.findByText(/do not have access to guardrail providers/i),
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

export const RegistersProvider: Story = {
  render: () => <Harness fetchStub={async (_input, init) => init?.method === "POST" ? json(PROVIDERS[0], 201) : json(PROVIDERS)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: /add provider/i }));
    const dialog = within(document.body).getByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText("Provider name"), "Policy service");
    await expect(within(dialog).getByRole("button", { name: "Save provider" })).toBeEnabled();
  },
};


/**
 * The provider is refused (#1607).
 *
 * The register dialog closes on success, so a rejected save is the only case
 * where it has to stay — and it has to, because the URL, the timeout and the
 * failure mode were all chosen deliberately.
 */
export const RegisterRejectedByTheServer: Story = {
  render: () => (
    <Harness
      toasted
      fetchStub={async (_input, init) =>
        init?.method === "POST"
          ? json({ error: { message: "the evaluator did not answer its health probe" } }, 502)
          : json(PROVIDERS)
      }
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: /add provider/i }));
    const dialog = within(document.body).getByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText("Provider name"), "Policy service");
    await userEvent.click(within(dialog).getByRole("button", { name: "Save provider" }));

    await expectToast(canvasElement, /health probe/, "error");
    await waitFor(() =>
      expect(within(document.body).getByRole("dialog")).toBeInTheDocument(),
    );
    await expect(within(dialog).getByLabelText("Provider name")).toHaveValue("Policy service");
  },
};

// deleting an enabled provider stops external enforcement outright, which is
// too large a change for a bare window.confirm to carry (#1179)
//
// the list shrinks once the DELETE lands, so the story can assert the outcome
// — the toast, the row gone — rather than that the request left. A stub that
// answers the full list forever passes either way, which is how a 204 fixture
// that threw went unnoticed (#1260)
let providerDeleted = false;
const deletes = recording(async (_input, init) => {
  if (init?.method === "DELETE") {
    providerDeleted = true;
    return json({}, 204);
  }
  return json(
    providerDeleted ? PROVIDERS.filter((row) => row.id !== "provider-primary") : PROVIDERS,
  );
});

export const ConfirmsBeforeDeletingAProvider: Story = {
  render: () => {
    providerDeleted = false;
    return <Harness fetchStub={deletes.stub} toasted />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // by name, not by index: each row control names its own provider (#1214)
    const del = async () =>
      canvas.findByRole("button", { name: "Delete provider Production LLM Guard" });

    await userEvent.click(await del());
    await cancelConfirmation();
    deletes.expectNotSent("DELETE", "/guardrails/providers/provider-primary");

    await userEvent.click(await del());
    await confirmDestructive(/Production LLM Guard/, "Delete");
    await deletes.expectSent("DELETE", "/guardrails/providers/provider-primary");

    // the outcome, not just the request: the confirmation closes, the queue
    // announces it, and the row is gone from the list
    await expectSheetClosed();
    await expectToast(canvasElement, /Production LLM Guard deleted/);
    await waitFor(() =>
      expect(canvas.queryByText("Production LLM Guard")).not.toBeInTheDocument(),
    );
  },
};

export const ExplainsFailOpen: Story = {
  render: () => <Harness fetchStub={async () => json(PROVIDERS)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", { name: "Edit provider Staging evaluator" }),
    );
    await waitFor(() => expect(within(document.body).getByText(/fail-open favors availability/i)).toBeVisible());
  },
};

// What a non-superadmin gets: the screen refused before it asks (#1606).
//
// Guardrails are deployment-wide — `guardrail_provider` is superadmin at every
// action — so `superadminOnly` never mounts the screen for an org role however
// high. The stub answers with a good payload on purpose: if the wrapper is
// dropped the screen renders that payload and this story fails, which the
// `Forbidden` story cannot do, since it stubs the 403 itself.
export const RefusedToAnAdmin: Story = {
  render: () => <Harness fetchStub={async () => json(PROVIDERS)} role="admin" />,
  play: async ({ canvasElement }) => expectForbidden(canvasElement),
};

export const RefusedToAViewer: Story = {
  render: () => <Harness fetchStub={async () => json(PROVIDERS)} role="viewer" />,
  play: async ({ canvasElement }) => expectForbidden(canvasElement),
};
