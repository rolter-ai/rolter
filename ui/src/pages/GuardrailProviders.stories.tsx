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
  expectUxEvent,
  Harness as ScreenHarness,
  json,
  recordUxEvents,
  recording,
  Toasted,
  type FetchStub,
  type Recorder,
  type StoryRole,
} from "./story-harness";
import type { GuardrailProviderInput, GuardrailProviderRow } from "@/lib/api";
import { UxScreenProvider } from "@/lib/ux-react";

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
 * being able to. `reported` mounts the screen key the UX stream needs, for a
 * story that asserts what a confirmation emitted.
 */
function Harness({
  fetchStub,
  role,
  toasted,
  reported,
}: {
  fetchStub: FetchStub;
  role?: StoryRole;
  toasted?: boolean;
  reported?: boolean;
}) {
  const screen = toasted ? (
    <Toasted>
      <GuardrailProviders />
    </Toasted>
  ) : (
    <GuardrailProviders />
  );
  return (
    <ScreenHarness fetchStub={fetchStub} role={role}>
      {reported ? (
        <UxScreenProvider screen="guardrail-providers">{screen}</UxScreenProvider>
      ) : (
        screen
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

/** the row the control plane answers a registration with */
const POLICY_SERVICE: GuardrailProviderRow = {
  ...PROVIDERS[0],
  id: "provider-policy",
  name: "Policy service",
  enabled: false,
  url: "https://policy.corp.internal/evaluate",
  auth_kind: "none",
  auth_env: null,
};

/** open the register dialog and fill in the two fields it cannot save without */
async function registerPolicyService(canvasElement: HTMLElement) {
  await userEvent.click(
    await within(canvasElement).findByRole("button", { name: /add provider/i }),
  );
  const dialog = within(
    await within(document.body).findByRole("dialog", { name: "Register guardrail provider" }),
  );
  await userEvent.type(dialog.getByLabelText("Provider name"), POLICY_SERVICE.name);
  await userEvent.type(dialog.getByLabelText("Evaluation URL"), POLICY_SERVICE.url);
  return dialog;
}

/** the activation confirmation, found by the provider its title names */
const activationFor = async (name: string) =>
  within(await within(document.body).findByRole("dialog", { name: `Activate ${name}?` }));

/**
 * The evaluation URL starts empty, with the example as its placeholder rather
 * than a value a hurried save would keep, and nothing saves without one
 * (#2163). A provider registered switched off takes nothing over, so it saves
 * without a confirmation.
 */
let registers: Recorder;
export const RegistersProvider: Story = {
  render: () => {
    registers = recording(
      registry(PROVIDERS, PRIMARY_WEBHOOK, async () => json(POLICY_SERVICE, 201)),
    );
    return <Harness fetchStub={registers.stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: /add provider/i }));
    const dialog = within(await within(document.body).findByRole("dialog"));
    const url = dialog.getByLabelText("Evaluation URL");
    await expect(url).toHaveValue("");
    await expect(url).toHaveAttribute("placeholder", "https://guardrails.internal/v1/evaluate");
    await expect(url).toBeRequired();

    await userEvent.type(dialog.getByLabelText("Provider name"), "Policy service");
    const save = dialog.getByRole("button", { name: "Save provider" });
    await expect(save).toBeDisabled();

    // leaving the field says what it needs, tied to the field
    await userEvent.click(url);
    await userEvent.tab();
    await expect(url).toHaveAttribute("aria-invalid", "true");
    await expect(url).toHaveAccessibleDescription(
      "Enter the URL the gateway posts each request to.",
    );
    await userEvent.type(url, "policy.corp.internal/evaluate");
    await userEvent.tab();
    await expect(url).toHaveAccessibleDescription("Start the URL with http:// or https://.");
    await expect(save).toBeDisabled();

    await userEvent.clear(url);
    await userEvent.type(url, "https://policy.corp.internal/evaluate");
    await expect(url).not.toHaveAttribute("aria-invalid", "true");
    await userEvent.click(save);
    const body = await registers.expectSentBody<GuardrailProviderInput>(
      "POST",
      "/guardrails/providers",
    );
    await expect(body).toMatchObject({
      name: "Policy service",
      url: "https://policy.corp.internal/evaluate",
      enabled: false,
    });
    await expectSheetClosed();
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
    await userEvent.type(
      within(dialog).getByLabelText("Evaluation URL"),
      "https://policy.corp.internal/evaluate",
    );
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

// #2163: switching a provider on hands it every request, and a fail-closed one
// pointed at a host that does not answer refuses them all. The save that does
// it goes through a confirmation naming what it replaces and what a failure
// then does to traffic

/**
 * The switch names the provider it would pause, and the save asks before it
 * sends anything. A cancel returns to the form as it was; a confirm registers
 * the provider switched on, holds both buttons while the request is out, and
 * closes both dialogs once it lands.
 */
let replacing: Recorder;
let landRegistration: () => void = () => {};
export const ActivationNamesTheProviderItReplaces: Story = {
  beforeEach: recordUxEvents,
  render: () => {
    replacing = recording(
      registry(
        PROVIDERS,
        PRIMARY_WEBHOOK,
        () =>
          new Promise<Response>((resolve) => {
            landRegistration = () => resolve(json({ ...POLICY_SERVICE, enabled: true }, 201));
          }),
      ),
    );
    return <Harness fetchStub={replacing.stub} toasted reported />;
  },
  play: async ({ canvasElement }) => {
    const form = await registerPolicyService(canvasElement);
    const activate = form.getByRole("switch", { name: "Activate provider" });
    await expect(activate).toHaveAccessibleDescription(
      "Takes enforcement over from Production LLM Guard, which is then paused.",
    );
    await userEvent.click(activate);
    await userEvent.click(form.getByRole("button", { name: "Save provider" }));

    const confirm = await activationFor("Policy service");
    await expect(
      confirm.getByText("It takes enforcement over from Production LLM Guard, which is paused."),
    ).toBeVisible();
    await expect(
      confirm.getByText(/It fails closed: when Policy service times out or errors/),
    ).toBeVisible();
    await expect(confirm.getByText("guardrail_blocked")).toBeVisible();
    replacing.expectNotSent("POST", "/guardrails/providers");

    // a cancel is a way back to the form, not out of it
    await userEvent.click(confirm.getByRole("button", { name: "Cancel" }));
    await waitFor(() =>
      expect(
        within(document.body).queryByRole("dialog", { name: "Activate Policy service?" }),
      ).toBeNull(),
    );
    replacing.expectNotSent("POST", "/guardrails/providers");
    await expect(form.getByLabelText("Provider name")).toHaveValue("Policy service");
    await expect(activate).toBeChecked();
    await expect((await expectUxEvent("form_abandon", "guardrail-provider-activate")).outcome).toBe(
      "cancelled",
    );

    await userEvent.click(form.getByRole("button", { name: "Save provider" }));
    const again = await activationFor("Policy service");
    await userEvent.click(again.getByRole("button", { name: "Save and activate" }));
    const body = await replacing.expectSentBody<GuardrailProviderInput>(
      "POST",
      "/guardrails/providers",
    );
    await expect(body).toMatchObject({
      name: "Policy service",
      url: "https://policy.corp.internal/evaluate",
      enabled: true,
      failure_mode: "fail_closed",
    });
    // the request is on the wire, so nothing looks like it could call it back
    await waitFor(() => expect(again.getByRole("button", { name: "Cancel" })).toBeDisabled());

    landRegistration();
    await expectSheetClosed();
    await expectToast(canvasElement, /Policy service created/);
    await expectUxEvent("save_confirmed", "guardrail-provider-activate");
  },
};

/**
 * With nothing active the confirmation says so, and a fail-open provider says
 * what a failure then does: the request goes upstream unchecked.
 */
let fromNothing: Recorder;
export const ActivationWithNoProviderActive: Story = {
  render: () => {
    fromNothing = recording(
      registry(
        PROVIDERS.map((row) => ({ ...row, enabled: false })),
        NO_WEBHOOK,
        async () => json({ ...PROVIDERS[1], enabled: true }),
      ),
    );
    return <Harness fetchStub={fromNothing.stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", { name: "Edit provider Staging evaluator" }),
    );
    const form = within(
      await within(document.body).findByRole("dialog", { name: "Edit guardrail provider" }),
    );
    const activate = form.getByRole("switch", { name: "Activate provider" });
    await expect(activate).toHaveAccessibleDescription(
      "No provider is active now, so this one would be the only one.",
    );
    await userEvent.click(activate);
    await userEvent.click(form.getByRole("button", { name: "Save provider" }));

    const confirm = await activationFor("Staging evaluator");
    await expect(confirm.getByText("No provider is active now, so none is paused.")).toBeVisible();
    await expect(
      confirm.getByText(
        /It fails open: when Staging evaluator times out or errors, the request goes upstream unchecked/,
      ),
    ).toBeVisible();
    await expect(confirm.queryByText("guardrail_blocked")).toBeNull();

    await userEvent.click(confirm.getByRole("button", { name: "Save and activate" }));
    const body = await fromNothing.expectSentBody<GuardrailProviderInput>(
      "PUT",
      "/guardrails/providers/provider-fallback",
    );
    await expect(body).toMatchObject({ enabled: true, failure_mode: "fail_open" });
    await expectSheetClosed();
  },
};

/**
 * Saving the provider that is already active hands nothing over, so it saves
 * without asking: a confirmation on every edit trains the click-through.
 */
let editsActive: Recorder;
export const SavingTheActiveProviderDoesNotAsk: Story = {
  render: () => {
    editsActive = recording(registry(PROVIDERS, PRIMARY_WEBHOOK, async () => json(PROVIDERS[0])));
    return <Harness fetchStub={editsActive.stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", { name: "Edit provider Production LLM Guard" }),
    );
    const form = within(
      await within(document.body).findByRole("dialog", { name: "Edit guardrail provider" }),
    );
    await expect(
      form.getByRole("switch", { name: "Activate provider" }),
    ).toHaveAccessibleDescription(
      "This provider is active. Turning it off pauses it, and no other provider takes its place.",
    );
    const timeout = form.getByLabelText("Timeout (ms)");
    await userEvent.clear(timeout);
    await userEvent.type(timeout, "2500");
    await userEvent.click(form.getByRole("button", { name: "Save provider" }));

    // the PUT left on the save itself, with no confirmation pressed
    const body = await editsActive.expectSentBody<GuardrailProviderInput>(
      "PUT",
      "/guardrails/providers/provider-primary",
    );
    await expect(body).toMatchObject({ enabled: true, timeout_ms: 2500 });
    await expectSheetClosed();
  },
};

/**
 * An enabled config-file webhook wins over the registry, so the confirmation
 * says the new provider is ignored until that webhook is turned off.
 */
export const ActivationUnderAConfigFileWebhook: Story = {
  render: () => <Harness fetchStub={registry(PROVIDERS, FILE_WEBHOOK)} />,
  play: async ({ canvasElement }) => {
    const form = await registerPolicyService(canvasElement);
    await userEvent.click(form.getByRole("switch", { name: "Activate provider" }));
    await userEvent.click(form.getByRole("button", { name: "Save provider" }));

    const confirm = await activationFor("Policy service");
    await expect(
      confirm.getByText(
        "The config-file webhook stays in force while it is enabled, so Policy service is ignored until that webhook is turned off.",
      ),
    ).toBeVisible();
  },
};

/**
 * A provider saved at the post-call stage checks nothing, so activating one
 * says that instead of a failure policy that never applies.
 */
export const ActivatingAPostCallProviderSaysItChecksNothing: Story = {
  render: () => (
    <Harness fetchStub={registry([PROVIDERS[0], { ...OUTPUT_EVALUATOR, enabled: false }])} />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", { name: "Edit provider Output evaluator" }),
    );
    const form = within(
      await within(document.body).findByRole("dialog", { name: "Edit guardrail provider" }),
    );
    await userEvent.click(form.getByRole("switch", { name: "Activate provider" }));
    await userEvent.click(form.getByRole("button", { name: "Save provider" }));

    const confirm = await activationFor("Output evaluator");
    await expect(
      confirm.getByText("It takes enforcement over from Production LLM Guard, which is paused."),
    ).toBeVisible();
    await expect(
      confirm.getByText(/so no request is sent to it and its failure policy never applies/),
    ).toBeVisible();
    await expect(confirm.queryByText(/fails closed/)).toBeNull();
  },
};

/**
 * The registry did not load, so the screen cannot know which provider is
 * active. The switch and the confirmation say so rather than claim none is.
 */
export const ActivationWhileTheRegistryIsUnreadable: Story = {
  render: () => (
    <Harness fetchStub={async () => json({ error: { message: "registry offline" } }, 503)} />
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /failed to return guardrail providers/);
    const form = await registerPolicyService(canvasElement);
    const activate = form.getByRole("switch", { name: "Activate provider" });
    await expect(activate).toHaveAccessibleDescription(
      "Takes enforcement ownership from the currently active provider.",
    );
    await userEvent.click(activate);
    await userEvent.click(form.getByRole("button", { name: "Save provider" }));

    const confirm = await activationFor("Policy service");
    await expect(
      confirm.getByText(
        "The provider list did not load, so this screen cannot say which provider it replaces.",
      ),
    ).toBeVisible();
    await expect(confirm.queryByText(/No provider is active now/)).toBeNull();
  },
};
