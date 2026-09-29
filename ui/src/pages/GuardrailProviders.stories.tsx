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

/** the effective webhook the postgres store builds from `PROVIDERS[0]` */
const PRIMARY_WEBHOOK = {
  enabled: true,
  url: "https://guardrails.internal/v1/evaluate",
  stage: "pre_call",
  timeout_ms: 1800,
  max_retries: 1,
  failure_mode: "fail_closed",
  max_body_bytes: 65536,
  auth: { bearer: { token_env: "ROLTER_GUARDRAIL_TOKEN" } },
};

/** `[guardrail_webhook]` off in the file and no registry row enabled */
const NO_WEBHOOK = {
  enabled: false,
  url: "",
  stage: "pre_call",
  timeout_ms: 2000,
  max_retries: 0,
  failure_mode: "fail_open",
  max_body_bytes: 65536,
};

/** an enabled `[guardrail_webhook]` in the config file, which wins over the registry */
const FILE_WEBHOOK = {
  enabled: true,
  url: "https://policy.corp.internal/evaluate",
  stage: "pre_call",
  timeout_ms: 1500,
  max_retries: 0,
  failure_mode: "fail_closed",
  max_body_bytes: 32768,
};

/** a provider saved at the post-call stage, which the gateway does not run */
const OUTPUT_EVALUATOR: GuardrailProviderRow = {
  id: "provider-output",
  name: "Output evaluator",
  enabled: true,
  url: "https://guardrails.internal/v1/output",
  stage: "post_call",
  timeout_ms: 2000,
  max_retries: 0,
  failure_mode: "fail_closed",
  max_body_bytes: 65536,
  auth_kind: "none",
  auth_env: null,
  created_at: "2026-08-03T00:00:00Z",
  updated_at: "2026-08-03T00:00:00Z",
};

/**
 * Activating the post-call row paused the pre-call one that was enforcing,
 * which is how #2162 was found.
 */
const POST_CALL_TOOK_OVER = [{ ...PROVIDERS[0], enabled: false }, PROVIDERS[1], OUTPUT_EVALUATOR];
const POST_CALL_WEBHOOK = {
  enabled: true,
  url: OUTPUT_EVALUATOR.url,
  stage: "post_call",
  timeout_ms: 2000,
  max_retries: 0,
  failure_mode: "fail_closed",
  max_body_bytes: 65536,
};

const configWith = (webhook: unknown) => ({
  providers: [],
  routes: [],
  virtual_keys: [],
  guardrail_webhook: webhook,
});

/**
 * Answer the two reads the screen makes: the registry, and the effective config
 * the banner takes the webhook from (#2162). `write` answers every request that
 * is not a GET.
 */
function registry(
  providers: GuardrailProviderRow[] | (() => GuardrailProviderRow[]),
  webhook: object | (() => object) = PRIMARY_WEBHOOK,
  write?: FetchStub,
): FetchStub {
  return async (input, init) => {
    if (String(input).includes("/api/v1/config")) {
      return json(configWith(typeof webhook === "function" ? webhook() : webhook));
    }
    if (write && init?.method && init.method !== "GET") return write(input, init);
    return json(typeof providers === "function" ? providers() : providers);
  };
}

/** the card a provider is drawn on, found by its title */
const card = (canvasElement: HTMLElement, name: string) => {
  const heading = within(canvasElement).getByRole("heading", { name });
  return within(heading.closest("article") as HTMLElement);
};

const GREEN = /owns external enforcement/;

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

const meta = {
  title: "Screens/GuardrailProviders",
  component: GuardrailProviders,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof GuardrailProviders>;
export default meta;
type Story = StoryObj<typeof meta>;

/**
 * The registry row in force is the effective webhook, at the stage the gateway
 * runs, so the banner is green and names it.
 */
export const Loaded: Story = {
  render: () => <Harness fetchStub={registry(PROVIDERS)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const banner = await canvas.findByRole("region", {
      name: "Production LLM Guard owns external enforcement",
    });
    await expect(
      within(banner).getByText("Requests stop when the service cannot decide."),
    ).toBeVisible();
    await expect(card(canvasElement, "Production LLM Guard").getByText("enforced")).toBeVisible();
    await expect(card(canvasElement, "Staging evaluator").getByText("paused")).toBeVisible();
  },
};
export const Loading: Story = {
  render: () => <Harness fetchStub={() => new Promise<Response>(() => {})} />,
  play: async ({ canvasElement }) => expectSkeleton(canvasElement),
};
export const Empty: Story = {
  render: () => <Harness fetchStub={registry([], NO_WEBHOOK)} />,
  play: async ({ canvasElement }) => {
    await expectEmptyState(canvasElement, /No guardrail providers/, /Register first provider/);
    // nothing is on in the file either, and the empty state already says so
    await expect(within(canvasElement).queryByRole("region", { name: /enforc/i })).toBeNull();
  },
};
export const Error: Story = {
  render: () => (
    <Harness fetchStub={async () => json({ error: { message: "registry offline" } }, 503)} />
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /failed to return guardrail providers/);
  },
};

// the two failures the bespoke panel got wrong (#1259). a deployment-scoped
// screen is refused to every non-superadmin, and a "try again" on a permission
// suggests the refusal was transient
export const Forbidden: Story = {
  render: () => <Harness fetchStub={async () => json({ error: { message: "forbidden" } }, 403)} />,
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
  render: () => <Harness fetchStub={() => Promise.reject(new TypeError("Failed to fetch"))} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(/cannot reach the control plane/i)).toBeVisible();
    await expect(await canvas.findByRole("button", { name: /try again/i })).toBeVisible();
  },
};

export const RegistersProvider: Story = {
  render: () => (
    <Harness
      fetchStub={registry(PROVIDERS, PRIMARY_WEBHOOK, async () => json(PROVIDERS[0], 201))}
    />
  ),
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
      fetchStub={registry(PROVIDERS, PRIMARY_WEBHOOK, async () =>
        json({ error: { message: "the evaluator did not answer its health probe" } }, 502),
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: /add provider/i }));
    const dialog = within(document.body).getByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText("Provider name"), "Policy service");
    await userEvent.click(within(dialog).getByRole("button", { name: "Save provider" }));

    await expectToast(canvasElement, /health probe/, "error");
    await waitFor(() => expect(within(document.body).getByRole("dialog")).toBeInTheDocument());
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
const deletes = recording(
  registry(
    () => (providerDeleted ? PROVIDERS.filter((row) => row.id !== "provider-primary") : PROVIDERS),
    () => (providerDeleted ? NO_WEBHOOK : PRIMARY_WEBHOOK),
    async () => {
      providerDeleted = true;
      return json({}, 204);
    },
  ),
);

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
    await waitFor(() => expect(canvas.queryByText("Production LLM Guard")).not.toBeInTheDocument());
    // and the banner stops claiming enforcement the moment nothing is left to do it
    await expect(
      await canvas.findByRole("region", { name: "No external guardrail is enforcing" }),
    ).toBeVisible();
    await expect(canvas.queryByText(GREEN)).toBeNull();
  },
};

export const ExplainsFailOpen: Story = {
  render: () => <Harness fetchStub={registry(PROVIDERS)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", { name: "Edit provider Staging evaluator" }),
    );
    await waitFor(() =>
      expect(within(document.body).getByText(/fail-open favors availability/i)).toBeVisible(),
    );
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
  render: () => <Harness fetchStub={registry(PROVIDERS)} role="admin" />,
  play: async ({ canvasElement }) => expectForbidden(canvasElement),
};

export const RefusedToAViewer: Story = {
  render: () => <Harness fetchStub={registry(PROVIDERS)} role="viewer" />,
  play: async ({ canvasElement }) => expectForbidden(canvasElement),
};

// #2162: a switched-on row is not the same as a request the gateway checks.
// Each story below serves the registry and the effective webhook as the control
// plane would, and asserts the screen reports the webhook rather than the switch

/**
 * Activating a post-call provider paused the pre-call one that was working.
 * The gateway does not run the post-call stage, so nothing is checked and the
 * banner must not be green.
 */
export const PostCallProviderIsNotEnforcing: Story = {
  render: () => <Harness fetchStub={registry(POST_CALL_TOOK_OVER, POST_CALL_WEBHOOK)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const banner = await canvas.findByRole("region", {
      name: "Output evaluator is not enforcing",
    });
    await expect(within(banner).getByText(/a stage the gateway does not run yet/)).toBeVisible();
    await expect(canvas.queryByText(GREEN)).toBeNull();

    const output = card(canvasElement, "Output evaluator");
    await expect(output.getByText("not enforcing")).toBeVisible();
    await expect(output.queryByText("enforced")).toBeNull();
    await expect(output.getByText(/so it checks nothing/)).toBeVisible();
    await expect(card(canvasElement, "Production LLM Guard").getByText("paused")).toBeVisible();
  },
};

/**
 * An enabled webhook in the config file stays authoritative, and the registry
 * row that is switched on is ignored. The banner names the file's webhook by
 * its endpoint, and the row says it is overridden rather than enforced.
 */
export const ConfigFileWebhookOwnsEnforcement: Story = {
  render: () => <Harness fetchStub={registry(PROVIDERS, FILE_WEBHOOK)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const banner = await canvas.findByRole("region", {
      name: "The config-file webhook owns external enforcement",
    });
    await expect(within(banner).getByText(FILE_WEBHOOK.url)).toBeVisible();
    await expect(
      within(banner).getByText(
        "Production LLM Guard is active here but ignored while the config-file webhook is enabled.",
      ),
    ).toBeVisible();
    await expect(canvas.queryByText("Production LLM Guard owns external enforcement")).toBeNull();

    const primary = card(canvasElement, "Production LLM Guard");
    await expect(primary.getByText("overridden by config file")).toBeVisible();
    await expect(primary.queryByText("enforced")).toBeNull();
  },
};

/**
 * A deployment that configures its webhook only in the file has no registry
 * rows at all, and the webhook is still in force.
 */
export const ConfigFileWebhookWithAnEmptyRegistry: Story = {
  render: () => <Harness fetchStub={registry([], FILE_WEBHOOK)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const banner = await canvas.findByRole("region", {
      name: "The config-file webhook owns external enforcement",
    });
    await expect(
      within(banner).getByText(
        "Registry providers are ignored while the config-file webhook is enabled.",
      ),
    ).toBeVisible();
    await expectEmptyState(canvasElement, /No guardrail providers/, /Register first provider/);
  },
};

/**
 * The worst pairing: a post-call webhook in the file overrides a working
 * pre-call registry row, so nothing enforces, and the banner says both halves.
 */
export const ConfigFilePostCallWebhookOverridesAWorkingProvider: Story = {
  render: () => (
    <Harness fetchStub={registry(PROVIDERS, { ...FILE_WEBHOOK, stage: "post_call" })} />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const banner = await canvas.findByRole("region", {
      name: "The config-file webhook is not enforcing",
    });
    await expect(within(banner).getByText(/Production LLM Guard is active here/)).toBeVisible();
    await expect(canvas.queryByText(GREEN)).toBeNull();
    await expect(
      card(canvasElement, "Production LLM Guard").getByText("overridden by config file"),
    ).toBeVisible();
  },
};

/** Nothing is switched on anywhere, which is a state worth saying out loud. */
export const NothingIsEnforcing: Story = {
  render: () => (
    <Harness
      fetchStub={registry(
        PROVIDERS.map((row) => ({ ...row, enabled: false })),
        NO_WEBHOOK,
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(
      await canvas.findByRole("region", { name: "No external guardrail is enforcing" }),
    ).toBeVisible();
    await expect(canvas.queryByText(GREEN)).toBeNull();
  },
};

/**
 * The effective config did not load, so the screen cannot confirm what the
 * gateway enforces. It says so instead of trusting the registry's switch, and
 * a retry that reaches the config turns it green.
 */
let configRequests = 0;
const configFailsOnce: FetchStub = async (input) => {
  if (String(input).includes("/api/v1/config")) {
    configRequests += 1;
    return configRequests === 1
      ? json({ error: { message: "config store unavailable" } }, 503)
      : json(configWith(PRIMARY_WEBHOOK));
  }
  return json(PROVIDERS);
};

export const EnforcementUnknownWithoutTheConfig: Story = {
  render: () => {
    configRequests = 0;
    return <Harness fetchStub={configFailsOnce} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const banner = await canvas.findByRole("region", {
      name: "External enforcement status unknown",
    });
    await expect(canvas.queryByText(GREEN)).toBeNull();
    await expect(
      card(canvasElement, "Production LLM Guard").getByText("status unknown"),
    ).toBeVisible();

    await userEvent.click(within(banner).getByRole("button", { name: "Try again" }));
    await expect(
      await canvas.findByRole("region", {
        name: "Production LLM Guard owns external enforcement",
      }),
    ).toBeVisible();
  },
};

/**
 * A new provider cannot pick the stage the gateway does not run: the option
 * is there, marked as not enforced yet, and refuses the click.
 */
export const BeforeResponseIsNotOffered: Story = {
  render: () => <Harness fetchStub={registry(PROVIDERS)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: /add provider/i }));
    const dialog = within(await within(document.body).findByRole("dialog"));
    const stage = dialog.getByRole("combobox", { name: "Stage" });
    await userEvent.click(stage);
    const post = await dialog.findByRole("option", { name: /Before response/ });
    await expect(post).toHaveAttribute("aria-disabled", "true");
    await expect(post).toHaveTextContent("Not enforced yet");
    await userEvent.click(post);
    await userEvent.keyboard("{Escape}");
    await expect(stage).toHaveValue("Before upstream");
    await expect(dialog.queryByText(/does not run the Before response stage/)).toBeNull();
  },
};

/**
 * A row saved at the post-call stage before the option was disabled still
 * opens with it, and the dialog says what that means, tied to the control.
 */
export const EditingAPostCallProviderWarns: Story = {
  render: () => <Harness fetchStub={registry(POST_CALL_TOOK_OVER, POST_CALL_WEBHOOK)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", { name: "Edit provider Output evaluator" }),
    );
    const dialog = within(await within(document.body).findByRole("dialog"));
    const stage = dialog.getByRole("combobox", { name: "Stage" });
    await expect(stage).toHaveValue("Before response");
    await expect(stage).toHaveAccessibleDescription(
      /does not run the Before response stage yet, so this provider checks nothing/,
    );

    // moving it to the stage the gateway runs clears the warning
    await userEvent.click(stage);
    await userEvent.click(await dialog.findByRole("option", { name: /Before upstream/ }));
    await expect(stage).toHaveValue("Before upstream");
    await expect(dialog.queryByText(/does not run the Before response stage/)).toBeNull();
  },
};
