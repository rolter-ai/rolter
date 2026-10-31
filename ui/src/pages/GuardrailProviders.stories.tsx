import type { Meta, StoryObj } from "@storybook/react-vite";
import { MemoryRouter } from "react-router";
import { expect, userEvent, waitFor, within } from "storybook/test";

import GuardrailProviders from "./GuardrailProviders";
import {
  cancelConfirmation,
  confirmDestructive,
  expectAnalyticsUnavailable,
  expectEmptyState,
  expectForbidden,
  expectLoadError,
  expectSheetClosed,
  expectSkeleton,
  expectToast,
  expectUxEvent,
  Harness as ScreenHarness,
  json,
  pickOption,
  recordUxEvents,
  recording,
  Toasted,
  type FetchStub,
  type Recorder,
  type StoryRole,
} from "./story-harness";
import type { GuardrailPolicyRow, GuardrailProviderInput, GuardrailProviderRow } from "@/lib/api";
import ru from "@/lib/i18n/locales/ru.json";
import en from "@/lib/i18n/locales/en.json";
import { expectFieldHint } from "@/lib/story-hints";
import { phoneFits } from "@/lib/story-viewport";
import { UxScreenProvider } from "@/lib/ux-react";

const PROVIDERS: GuardrailProviderRow[] = [
  {
    id: "provider-primary",
    name: "Production LLM Guard",
    enabled: true,
    kind: "webhook",
    url: "https://guardrails.internal/v1/evaluate",
    model_path: null,
    model_sha256: null,
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
    kind: "webhook",
    url: "https://guardrails.staging.internal/evaluate",
    model_path: null,
    model_sha256: null,
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

/** a provider saved at the post-call stage, which the gateway does not run */
const OUTPUT_EVALUATOR: GuardrailProviderRow = {
  id: "provider-output",
  name: "Output evaluator",
  enabled: true,
  kind: "webhook",
  url: "https://guardrails.internal/v1/output",
  model_path: null,
  model_sha256: null,
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

/** a provider the gateways run, as `semantic.providers` lists it */
const running = (row: GuardrailProviderRow) => ({
  name: row.name,
  url: row.url,
  stage: row.stage,
  failure_mode: row.failure_mode,
  timeout_ms: row.timeout_ms,
  source: "registry",
  provider_id: row.id,
});

/** a provider the config file defines, which no registry row stands behind */
const FILE_PROVIDER = {
  name: "Policy file guard",
  url: "https://policy.corp.internal/evaluate",
  stage: "pre_call",
  failure_mode: "fail_closed",
  timeout_ms: 1500,
  source: "file",
  provider_id: null,
};

/** a policy as the registry stores it: `route-strict` pins the primary provider to one route */
const ROUTE_POLICY: GuardrailPolicyRow = {
  id: "policy-route",
  name: "route-strict",
  enabled: true,
  scope_kind: "route",
  scope_id: "gpt-4o",
  provider_ids: ["provider-primary"],
  thresholds: { violence: 0.8 },
  on_violation: "block",
  streaming: "reject",
  description: null,
  created_at: "2026-09-01T00:00:00Z",
  updated_at: "2026-09-01T00:00:00Z",
};

/**
 * The `semantic` object of the effective endpoint for `rows`: every enabled
 * row runs, plus `extra` from the config file, and a provider no policy lists
 * is unattached. `policies` are the registry policies in force.
 */
function semanticFor(
  rows: GuardrailProviderRow[],
  extra: object[] = [],
  policies: GuardrailPolicyRow[] = [],
) {
  const enabled = rows.filter((row) => row.enabled);
  const listed = new Set(policies.flatMap((policy) => policy.provider_ids));
  return {
    providers: [...extra, ...enabled.map(running)],
    policies: policies.map((policy) => ({
      name: policy.name,
      enabled: policy.enabled,
      scope: policy.scope_kind,
      scope_id: policy.scope_id,
      providers: policy.provider_ids.map((id) => rows.find((row) => row.id === id)?.name ?? id),
      thresholds: policy.thresholds,
      on_violation: policy.on_violation,
      streaming: policy.streaming,
      source: "registry",
      policy_id: policy.id,
    })),
    unattached: enabled.filter((row) => !listed.has(row.id)).map((row) => row.name),
  };
}

/** nothing is enabled in the registry or the file */
const NO_PROVIDERS = { providers: [], policies: [], unattached: [] };

/**
 * Answer the three reads the screen makes: the registry, the effective
 * document the banner takes the running providers from (#459), and the
 * policies the confirmations name scopes from. `semantic` defaults to what the
 * rows imply, so a list that changes under a story changes its banner too.
 * `write` answers every request that is not a GET.
 */
function registry(
  providers: GuardrailProviderRow[] | (() => GuardrailProviderRow[]),
  semantic?: object | (() => object),
  write?: FetchStub,
  policies: GuardrailPolicyRow[] = [],
): FetchStub {
  const rows = () => (typeof providers === "function" ? providers() : providers);
  return async (input, init) => {
    const path = new URL(String(input), "http://localhost").pathname;
    if (write && init?.method && init.method !== "GET") return write(input, init);
    if (path === "/api/v1/guardrails/effective") {
      const doc = typeof semantic === "function" ? semantic() : semantic;
      return json({
        source: "none",
        semantic:
          doc ??
          semanticFor(
            rows(),
            [],
            policies.filter((policy) => policy.enabled),
          ),
      });
    }
    if (path === "/api/v1/guardrails/policies") return json(policies);
    return json(rows());
  };
}

/** answer the guardrail outcome rollup (#2158) with `outcomes`, everything else with `list` */
const withOutcomes =
  (list: FetchStub, outcomes: () => Response): FetchStub =>
  async (input, init) =>
    new URL(String(input), "http://localhost").pathname === "/api/v1/analytics/guardrails"
      ? outcomes()
      : list(input, init);

/** the card a provider is drawn on, found by its title */
const card = (canvasElement: HTMLElement, name: string) => {
  const heading = within(canvasElement).getByRole("heading", { name });
  return within(heading.closest("article") as HTMLElement);
};

const GREEN = /Running:/;

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
  // the outcome counts link to LLM Logs, and a link needs a router
  return (
    <MemoryRouter>
      <ScreenHarness fetchStub={fetchStub} role={role}>
        {reported ? (
          <UxScreenProvider screen="guardrail-providers">{screen}</UxScreenProvider>
        ) : (
          screen
        )}
      </ScreenHarness>
    </MemoryRouter>
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
      name: "Running: Production LLM Guard",
    });
    await expect(
      within(banner).getByText(
        "Production LLM Guard: requests stop when the service cannot decide.",
      ),
    ).toBeVisible();
    await expect(card(canvasElement, "Production LLM Guard").getByText("enforced")).toBeVisible();
    await expect(card(canvasElement, "Staging evaluator").getByText("paused")).toBeVisible();
    // the detail line pluralises and formats its numbers (#2164)
    await expect(
      card(canvasElement, "Production LLM Guard").getByText(
        "1,800 ms timeout · 1 retry · 64 KiB cap",
      ),
    ).toBeVisible();
    await expect(
      card(canvasElement, "Staging evaluator").getByText(
        "2,500 ms timeout · 0 retries · 32 KiB cap",
      ),
    ).toBeVisible();
    // fail-closed is the safe default, so it is not painted as a fault
    await expect(card(canvasElement, "Production LLM Guard").getByText("fail-closed")).toHaveClass(
      /bg-muted/,
    );
    // the shell's ScreenHeader owns the page's only h1 (#2241)
    await expect(within(canvasElement).queryAllByRole("heading", { level: 1 })).toHaveLength(0);
  },
};

/** The same detail line in Russian, whose retry count takes a form per number. */
export const LoadedInRussian: Story = {
  globals: { locale: "ru" },
  render: () => <Harness fetchStub={registry(PROVIDERS)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText("Production LLM Guard");
    await expect(canvasElement).toHaveTextContent(
      /таймаут 1\s800 мс · 1 повтор · лимит тела 64 КиБ/,
    );
    await expect(canvasElement).toHaveTextContent(
      /таймаут 2\s500 мс · 0 повторов · лимит тела 32 КиБ/,
    );
  },
};

/**
 * The fail-open warning sits under the failure policy it explains, the field
 * the dialog describes itself by, and the body cap says its unit (#2164).
 */
export const FailOpenWarningSitsWithTheFailurePolicy: Story = {
  render: () => <Harness fetchStub={registry(PROVIDERS)} />,
  play: async ({ canvasElement }) => {
    await userEvent.click(
      await within(canvasElement).findByRole("button", { name: /Edit provider Staging evaluator/ }),
    );
    const dialog = within(await within(document.body).findByRole("dialog"));
    await waitFor(() => expect(dialog.getByLabelText("Body cap (bytes)")).toBeVisible());
    await expect(dialog.getByLabelText("Failure policy")).toHaveAccessibleDescription(
      /Fail-open favors availability/,
    );
  },
};
export const Loading: Story = {
  render: () => <Harness fetchStub={() => new Promise<Response>(() => {})} />,
  play: async ({ canvasElement }) => expectSkeleton(canvasElement),
};
export const Empty: Story = {
  render: () => <Harness fetchStub={registry([], NO_PROVIDERS)} />,
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
    registers = recording(registry(PROVIDERS, undefined, async () => json(POLICY_SERVICE, 201)));
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
    await expect(save).toHaveAttribute("aria-disabled", "true");

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
    await expect(save).toHaveAttribute("aria-disabled", "true");

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
      fetchStub={registry(PROVIDERS, undefined, async () =>
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

    await expectToast(canvasElement, /The control plane hit an error/, "error");
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
    undefined,
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

/**
 * The fields whose effect is hardest to predict explain themselves in an (i)
 * beside the label (#487): what the stage covers, what a failure does to a
 * request, and what the three limits cost. The notes sit whole inside the
 * dialog, also for the last column of the limits row.
 */
function explainsEveryField(locale: "en" | "ru"): Story {
  const catalog = locale === "ru" ? ru : en;
  const p = catalog.pages.guardrailProviders;
  const about = (label: string) => catalog.common.aboutField.replace("{{label}}", label);
  return {
    ...(locale === "ru" ? { globals: { locale: "ru" } } : {}),
    render: () => <Harness fetchStub={registry(PROVIDERS)} />,
    play: async ({ canvasElement }) => {
      await userEvent.click(
        await within(canvasElement).findByRole("button", { name: p.addProvider }),
      );
      const dialog = await within(document.body).findByRole("dialog", { name: p.dialogAddTitle });
      for (const [label, says] of [
        [p.fieldName, p.nameInfo],
        [p.fieldStage, p.stageInfo],
        [p.fieldFailure, p.failureInfo],
        [p.fieldTimeout, p.timeoutInfo],
        [p.fieldRetries, p.retriesInfo],
        [p.fieldBodyCap, p.bodyCapInfo],
        [p.fieldAuth, p.authInfo],
      ] as const) {
        await expectFieldHint(dialog, about(label), says);
      }
    },
  };
}
export const ExplainsEachFieldInAHint: Story = explainsEveryField("en");
export const ExplainsEachFieldInAHintInRussian: Story = explainsEveryField("ru");

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
 * A post-call provider runs on the response body now (#459), so it is
 * enforced like any other and runs alongside the pre-call one.
 */
export const PostCallProviderRunsAlongside: Story = {
  render: () => <Harness fetchStub={registry([PROVIDERS[0], OUTPUT_EVALUATOR])} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const banner = await canvas.findByRole("region", {
      name: "Running: Production LLM Guard, Output evaluator",
    });
    await expect(
      within(banner).getByText(
        "Production LLM Guard, Output evaluator: requests stop when the service cannot decide.",
      ),
    ).toBeVisible();
    await expect(
      within(banner).getByText(
        "Not listed by any policy, so they check every request no policy covers: Production LLM Guard, Output evaluator.",
      ),
    ).toBeVisible();
    await expect(card(canvasElement, "Output evaluator").getByText("enforced")).toBeVisible();
    await expect(card(canvasElement, "Production LLM Guard").getByText("enforced")).toBeVisible();
    await expect(card(canvasElement, "Output evaluator").queryByText(/checks nothing/)).toBeNull();
  },
};
/**
 * Providers the config file defines run beside the registry's, and the banner
 * names them, with a note that says where they come from.
 */
export const ConfigFileProvidersRunBesideTheRegistry: Story = {
  render: () => (
    <Harness
      fetchStub={registry(PROVIDERS, {
        ...semanticFor(PROVIDERS),
        providers: [FILE_PROVIDER, running(PROVIDERS[0])],
      })}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const banner = await canvas.findByRole("region", {
      name: "Running: Policy file guard, Production LLM Guard",
    });
    await expect(
      within(banner).getByText(
        "Defined in the config file: Policy file guard. A file provider wins a name shared with a registry row.",
      ),
    ).toBeVisible();
    await expect(card(canvasElement, "Production LLM Guard").getByText("enforced")).toBeVisible();
  },
};
/** A file provider wins a name it shares with a registry row, and that row says so. */
export const ConfigFileProviderWinsANameCollision: Story = {
  render: () => {
    const shadowed = { ...PROVIDERS[0], name: FILE_PROVIDER.name };
    return (
      <Harness
        fetchStub={registry([shadowed, PROVIDERS[1]], {
          providers: [FILE_PROVIDER],
          policies: [],
          unattached: [FILE_PROVIDER.name],
        })}
      />
    );
  },
  play: async ({ canvasElement }) => {
    await within(canvasElement).findByRole("region", { name: "Running: Policy file guard" });
    const row = card(canvasElement, "Policy file guard");
    await expect(row.getByText("overridden by config file")).toBeVisible();
    await expect(row.queryByText("enforced")).toBeNull();
  },
};
/**
 * A deployment that configures its providers only in the file has no registry
 * rows at all, and they are still running.
 */
export const ConfigFileProvidersWithAnEmptyRegistry: Story = {
  render: () => (
    <Harness
      fetchStub={registry([], { providers: [FILE_PROVIDER], policies: [], unattached: [] })}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByRole("region", { name: "Running: Policy file guard" });
    await expectEmptyState(canvasElement, /No guardrail providers/, /Register first provider/);
  },
};
/** Nothing is switched on anywhere, which is a state worth saying out loud. */
export const NothingIsEnforcing: Story = {
  render: () => (
    <Harness
      fetchStub={registry(
        PROVIDERS.map((row) => ({ ...row, enabled: false })),
        NO_PROVIDERS,
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
const configFailsOnce: FetchStub = async (input, init) => {
  if (String(input).includes("/api/v1/guardrails/effective")) {
    configRequests += 1;
    return configRequests === 1
      ? json({ error: { message: "config store unavailable" } }, 503)
      : registry(PROVIDERS)(input, init);
  }
  return registry(PROVIDERS)(input, init);
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
      await canvas.findByRole("region", { name: "Running: Production LLM Guard" }),
    ).toBeVisible();
  },
};

/** Both stages can be picked: the gateway checks the response body too (#459). */
export const BeforeResponseIsOffered: Story = {
  render: () => <Harness fetchStub={registry(PROVIDERS)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: /add provider/i }));
    const dialog = within(await within(document.body).findByRole("dialog"));
    const stage = dialog.getByRole("combobox", { name: "Stage" });
    await userEvent.click(stage);
    const post = await dialog.findByRole("option", { name: /Before response/ });
    await expect(post).not.toHaveAttribute("aria-disabled", "true");
    await userEvent.click(post);
    await expect(stage).toHaveValue("Before response");
  },
};
// #2163: switching a provider on puts it in front of requests, and a
// fail-closed one pointed at a host that does not answer refuses them. The save
// that does it goes through a confirmation naming the traffic it will see and
// what a failure then does to it. Enabling never pauses another provider (#459)

/**
 * The switch names the providers it runs beside, and the save asks before it
 * sends anything. A cancel returns to the form as it was; a confirm registers
 * the provider switched on, holds both buttons while the request is out, and
 * closes both dialogs once it lands.
 */
let replacing: Recorder;
let landRegistration: () => void = () => {};
export const ActivationJoinsTheActiveProviders: Story = {
  beforeEach: recordUxEvents,
  render: () => {
    replacing = recording(
      registry(
        PROVIDERS,
        undefined,
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
    const activate = form.getByRole("switch", { name: "Enable provider" });
    await expect(activate).toHaveAccessibleDescription(
      "Runs alongside Production LLM Guard. None of them is paused.",
    );
    await userEvent.click(activate);
    await userEvent.click(form.getByRole("button", { name: "Save provider" }));

    const confirm = await activationFor("Policy service");
    await expect(
      confirm.getByText(
        "It is added to the active providers (Production LLM Guard). None of them is paused.",
      ),
    ).toBeVisible();
    await expect(
      confirm.getByText(
        'No policy lists it, so it will check every request that no policy covers (reported as "default").',
      ),
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
 * With nothing else running the confirmation says so, and a fail-open provider says
 * what a failure then does: the request goes upstream unchecked.
 */
let fromNothing: Recorder;
export const ActivationWithNoOtherProviderActive: Story = {
  render: () => {
    fromNothing = recording(
      registry(
        PROVIDERS.map((row) => ({ ...row, enabled: false })),
        NO_PROVIDERS,
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
    const activate = form.getByRole("switch", { name: "Enable provider" });
    await expect(activate).toHaveAccessibleDescription(
      "No other provider is active now, so this one would be the only one.",
    );
    await userEvent.click(activate);
    await userEvent.click(form.getByRole("button", { name: "Save provider" }));

    const confirm = await activationFor("Staging evaluator");
    await expect(
      confirm.getByText(
        "No other provider is active now. This one is added and nothing is paused.",
      ),
    ).toBeVisible();
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
    editsActive = recording(registry(PROVIDERS, undefined, async () => json(PROVIDERS[0])));
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
    await expect(form.getByRole("switch", { name: "Enable provider" })).toHaveAccessibleDescription(
      "This provider is enabled. Turning it off pauses only this one; the other active providers keep running.",
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
 * A config-file provider of the same name is running and wins the name, so the
 * confirmation says the new one is ignored until the name is free.
 */
export const ActivationUnderAFileProviderOfTheSameName: Story = {
  render: () => (
    <Harness
      fetchStub={registry(PROVIDERS, {
        ...semanticFor(PROVIDERS),
        providers: [{ ...FILE_PROVIDER, name: "Policy service" }, running(PROVIDERS[0])],
      })}
    />
  ),
  play: async ({ canvasElement }) => {
    const form = await registerPolicyService(canvasElement);
    await userEvent.click(form.getByRole("switch", { name: "Enable provider" }));
    await userEvent.click(form.getByRole("button", { name: "Save provider" }));

    const confirm = await activationFor("Policy service");
    await expect(
      confirm.getByText(
        "A provider named Policy service from the config file is already running and wins the name, so this one is ignored until that name is free.",
      ),
    ).toBeVisible();
  },
};

/**
 * Policies that list the provider decide what it sees: the confirmation names
 * the scopes of the enabled ones instead of claiming every request.
 */
export const ActivationNamesTheScopesOfTheListingPolicies: Story = {
  render: () => (
    <Harness
      fetchStub={registry(PROVIDERS, undefined, undefined, [
        ROUTE_POLICY,
        {
          ...ROUTE_POLICY,
          id: "policy-project",
          name: "project-strict",
          scope_kind: "project",
          scope_id: "proj-1",
          provider_ids: ["provider-fallback"],
        },
      ])}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", { name: "Edit provider Staging evaluator" }),
    );
    const form = within(
      await within(document.body).findByRole("dialog", { name: "Edit guardrail provider" }),
    );
    await userEvent.click(form.getByRole("switch", { name: "Enable provider" }));
    await userEvent.click(form.getByRole("button", { name: "Save provider" }));
    const confirm = await activationFor("Staging evaluator");
    await expect(
      confirm.getByText(
        "Policies list it, so it will check only the requests they apply to: Project proj-1.",
      ),
    ).toBeVisible();
    await expect(confirm.queryByText(/every request that no policy covers/)).toBeNull();
  },
};

/**
 * A provider only paused policies list is not unattached: pausing a policy never
 * widens its providers to all traffic, so it checks nothing until one is enabled.
 */
export const ActivationOfAProviderOnlyPausedPoliciesListChecksNothing: Story = {
  render: () => (
    <Harness
      fetchStub={registry(PROVIDERS, undefined, undefined, [
        { ...ROUTE_POLICY, enabled: false, provider_ids: ["provider-fallback"] },
      ])}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", { name: "Edit provider Staging evaluator" }),
    );
    const form = within(
      await within(document.body).findByRole("dialog", { name: "Edit guardrail provider" }),
    );
    await userEvent.click(form.getByRole("switch", { name: "Enable provider" }));
    await userEvent.click(form.getByRole("button", { name: "Save provider" }));
    const confirm = await activationFor("Staging evaluator");
    await expect(
      confirm.getByText(
        "Policies list it, but all of them are paused, so it checks no request until one is enabled.",
      ),
    ).toBeVisible();
  },
};

/**
 * A post-call provider checks the response body, so enabling one says that and
 * how a streamed response is handled instead of the request-time failure line.
 */
export const ActivatingAPostCallProviderChecksTheResponse: Story = {
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
    await userEvent.click(form.getByRole("switch", { name: "Enable provider" }));
    await userEvent.click(form.getByRole("button", { name: "Save provider" }));

    const confirm = await activationFor("Output evaluator");
    await expect(
      confirm.getByText(
        "It is added to the active providers (Production LLM Guard). None of them is paused.",
      ),
    ).toBeVisible();
    await expect(confirm.getByText(/It checks the response body/)).toBeVisible();
    await expect(confirm.queryByText(/fails closed/)).toBeNull();
  },
};

/**
 * The registry did not load, so the screen cannot know which providers are
 * running. The switch and the confirmation say so rather than claim none is.
 */
export const ActivationWhileTheRegistryIsUnreadable: Story = {
  render: () => (
    <Harness fetchStub={async () => json({ error: { message: "registry offline" } }, 503)} />
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /failed to return guardrail providers/);
    const form = await registerPolicyService(canvasElement);
    const activate = form.getByRole("switch", { name: "Enable provider" });
    await expect(activate).toHaveAccessibleDescription(
      "Adds this provider to the active ones; no other provider is paused.",
    );
    await userEvent.click(activate);
    await userEvent.click(form.getByRole("button", { name: "Save provider" }));

    const confirm = await activationFor("Policy service");
    await expect(confirm.getByText(/did not say which providers are running/)).toBeVisible();
    await expect(confirm.queryByText(/No other provider is active now/)).toBeNull();
  },
};

// #2271: switching the active provider off stops external checks for every
// request, the same effect on traffic as deleting it, from a form that also
// holds the timeout and the URL. The save that does it asks first, through the
// confirmation activation uses

/** open the active provider, switch it off and save, and return the form */
async function pauseProductionGuard(canvasElement: HTMLElement) {
  await userEvent.click(
    await within(canvasElement).findByRole("button", {
      name: "Edit provider Production LLM Guard",
    }),
  );
  const form = within(
    await within(document.body).findByRole("dialog", { name: "Edit guardrail provider" }),
  );
  await userEvent.click(form.getByRole("switch", { name: "Enable provider" }));
  await userEvent.click(form.getByRole("button", { name: "Save provider" }));
  return form;
}

/** the pause confirmation, found by the provider its title names */
const pauseFor = async (name: string) =>
  within(await within(document.body).findByRole("dialog", { name: `Pause ${name}?` }));

const NOTHING_CHECKS =
  "No other provider is active, so no external guardrail checks requests afterwards. Every request goes upstream without one.";

/**
 * Saving the active provider switched off names it and says nothing checks
 * requests afterwards. A cancel returns to the form with the switch as it was
 * left; a confirm saves it switched off, holds both buttons while the request
 * is out, and once it lands the banner stops claiming enforcement.
 */
let pauses: Recorder;
let providerPaused = false;
let landPause: () => void = () => {};
export const PausingTheActiveProviderAsksFirst: Story = {
  beforeEach: recordUxEvents,
  render: () => {
    providerPaused = false;
    pauses = recording(
      registry(
        () => (providerPaused ? PROVIDERS.map((row) => ({ ...row, enabled: false })) : PROVIDERS),
        undefined,
        () =>
          new Promise<Response>((resolve) => {
            landPause = () => {
              providerPaused = true;
              resolve(json({ ...PROVIDERS[0], enabled: false }));
            };
          }),
      ),
    );
    return <Harness fetchStub={pauses.stub} toasted reported />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const form = await pauseProductionGuard(canvasElement);

    const confirm = await pauseFor("Production LLM Guard");
    await expect(confirm.getByText(NOTHING_CHECKS)).toBeVisible();
    pauses.expectNotSent("PUT", "/guardrails/providers/provider-primary");

    // a cancel is a way back to the form, not out of it
    await userEvent.click(confirm.getByRole("button", { name: "Cancel" }));
    await waitFor(() =>
      expect(
        within(document.body).queryByRole("dialog", { name: "Pause Production LLM Guard?" }),
      ).toBeNull(),
    );
    pauses.expectNotSent("PUT", "/guardrails/providers/provider-primary");
    await expect(form.getByRole("switch", { name: "Enable provider" })).not.toBeChecked();
    await expect((await expectUxEvent("form_abandon", "guardrail-provider-pause")).outcome).toBe(
      "cancelled",
    );

    await userEvent.click(form.getByRole("button", { name: "Save provider" }));
    const again = await pauseFor("Production LLM Guard");
    await userEvent.click(again.getByRole("button", { name: "Save and pause" }));
    const body = await pauses.expectSentBody<GuardrailProviderInput>(
      "PUT",
      "/guardrails/providers/provider-primary",
    );
    await expect(body).toMatchObject({ name: "Production LLM Guard", enabled: false });
    // the request is on the wire, so nothing looks like it could call it back
    await waitFor(() => expect(again.getByRole("button", { name: "Cancel" })).toBeDisabled());

    landPause();
    await expectSheetClosed();
    await expectToast(canvasElement, /Production LLM Guard updated/);
    await expectUxEvent("save_confirmed", "guardrail-provider-pause");
    await expect(
      await canvas.findByRole("region", { name: "No external guardrail is enforcing" }),
    ).toBeVisible();
  },
};

/**
 * Pausing one of several running providers says the others keep running, and
 * names the policies that lose it.
 */
let pausesOne: Recorder;
export const PausingKeepsTheOtherProvidersRunning: Story = {
  render: () => {
    const both = [PROVIDERS[0], { ...PROVIDERS[1], enabled: true }];
    pausesOne = recording(
      registry(both, undefined, async () => json({ ...PROVIDERS[0], enabled: false }), [
        ROUTE_POLICY,
      ]),
    );
    return <Harness fetchStub={pausesOne.stub} />;
  },
  play: async ({ canvasElement }) => {
    await pauseProductionGuard(canvasElement);

    const confirm = await pauseFor("Production LLM Guard");
    await expect(
      confirm.getByText("The other active providers keep running: Staging evaluator."),
    ).toBeVisible();
    await expect(
      confirm.getByText("These policies lose it and stop consulting it: route-strict."),
    ).toBeVisible();
    await expect(confirm.queryByText(NOTHING_CHECKS)).toBeNull();

    await userEvent.click(confirm.getByRole("button", { name: "Save and pause" }));
    const body = await pausesOne.expectSentBody<GuardrailProviderInput>(
      "PUT",
      "/guardrails/providers/provider-primary",
    );
    await expect(body).toMatchObject({ enabled: false });
    await expectSheetClosed();
  },
};

/**
 * The effective document did not load, so the screen cannot tell whether another
 * provider still checks requests once the row is paused. It says so rather than
 * claim nothing does.
 */
export const PausingWhileEnforcementIsUnknown: Story = {
  render: () => (
    <Harness
      fetchStub={async (input, init) =>
        String(input).includes("/api/v1/guardrails/effective")
          ? json({ error: { message: "config store unavailable" } }, 503)
          : registry(PROVIDERS)(input, init)
      }
    />
  ),
  play: async ({ canvasElement }) => {
    await within(canvasElement).findByRole("region", {
      name: "External enforcement status unknown",
    });
    await pauseProductionGuard(canvasElement);

    const confirm = await pauseFor("Production LLM Guard");
    await expect(
      confirm.getByText(/cannot say whether any still checks requests afterwards/),
    ).toBeVisible();
    await expect(confirm.queryByText(NOTHING_CHECKS)).toBeNull();
  },
};

// the cards at a phone's width in both languages (#2252)
const providersFit = phoneFits({
  render: () => <Harness fetchStub={registry(PROVIDERS)} />,
  ready: async (canvas, locale) => {
    await canvas.findByText("Production LLM Guard");
    await waitFor(() =>
      expect(document.body).toHaveTextContent((locale === "ru" ? ru : en).guardrailPanel.enforced),
    );
  },
});
export const SmallPhone: Story = providersFit("small", "en");
export const SmallPhoneInRussian: Story = providersFit("small", "ru");

/**
 * The provider the gateway sends requests to states what it did over the last
 * seven days; a paused one, which the gateway never calls, states nothing
 * (#2158).
 */
export const ShowsWhatTheActiveProviderDid: Story = {
  render: () => (
    <Harness
      fetchStub={withOutcomes(registry(PROVIDERS), () =>
        json({
          recorded: true,
          data: [
            { action: "provider_blocked", rule: "Production LLM Guard", requests: "4" },
            { action: "provider_transformed", rule: "Production LLM Guard", requests: "9" },
          ],
        }),
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    await within(canvasElement).findByRole("heading", { name: "Production LLM Guard" });
    const active = card(canvasElement, "Production LLM Guard");
    await expect(await active.findByText("Matches in the last 7 days")).toBeVisible();
    await expect(active.getByRole("link", { name: /4 requests Blocked/ })).toHaveAttribute(
      "href",
      "/logs?window=7d&guardrail=provider_blocked%3AProduction%20LLM%20Guard",
    );
    await expect(active.getByRole("link", { name: /9 requests Transformed/ })).toHaveAttribute(
      "href",
      "/logs?window=7d&guardrail=provider_transformed%3AProduction%20LLM%20Guard",
    );
    const paused = card(canvasElement, "Staging evaluator");
    await expect(paused.queryByText("Matches in the last 7 days")).toBeNull();
    await expect(paused.queryByText(/No matches/)).toBeNull();
  },
};

/**
 * Counts follow the provider that recorded them: a provider switched away from
 * keeps its history off the one now in force, and rows written before the name
 * was recorded are stated apart rather than added to the card's own (#3038).
 */
export const CountsFollowTheProviderThatRecordedThem: Story = {
  render: () => (
    <Harness
      fetchStub={withOutcomes(registry(PROVIDERS), () =>
        json({
          recorded: true,
          data: [
            { action: "provider_blocked", rule: "Production LLM Guard", requests: "4" },
            { action: "provider_blocked", rule: "Staging evaluator", requests: "30" },
            { action: "provider_transformed", rule: "", requests: "7" },
          ],
        }),
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    await within(canvasElement).findByRole("heading", { name: "Production LLM Guard" });
    const active = card(canvasElement, "Production LLM Guard");
    await expect(await active.findByText("Matches in the last 7 days")).toBeVisible();
    await expect(active.getByRole("link", { name: /4 requests Blocked/ })).toBeVisible();
    // the other provider's 30 never lands on this card
    await expect(active.queryByRole("link", { name: /30 requests/ })).toBeNull();
    await expect(active.getByText("Recorded without a provider name")).toBeVisible();
    await expect(active.getByRole("link", { name: /7 requests Transformed/ })).toHaveAttribute(
      "href",
      "/logs?window=7d&guardrail=provider_transformed",
    );
  },
};

/** A provider that has not blocked or changed anything yet says so. */
export const ActiveProviderNoMatchesYet: Story = {
  render: () => (
    <Harness
      fetchStub={withOutcomes(registry(PROVIDERS), () => json({ recorded: true, data: [] }))}
    />
  ),
  play: async ({ canvasElement }) => {
    await within(canvasElement).findByRole("heading", { name: "Production LLM Guard" });
    const active = card(canvasElement, "Production LLM Guard");
    await expect(await active.findByText("No matches in the last 7 days.")).toBeVisible();
  },
};

/** No ClickHouse: one calm notice, and no claim on the card either way. */
export const OutcomesNeedAnalytics: Story = {
  render: () => (
    <Harness
      fetchStub={withOutcomes(registry(PROVIDERS), () =>
        json({ error: { message: "analytics unavailable: no clickhouse_url configured" } }, 503),
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    await expectAnalyticsUnavailable(
      canvasElement,
      "Match counts need an analytics store",
      "analytics unavailable: no clickhouse_url configured",
    );
    await expect(canvasElement).not.toHaveTextContent(/No matches in the last 7 days/);
  },
};

const isProviderTrial = (input: RequestInfo | URL) =>
  new URL(String(input), "http://localhost").pathname === "/api/v1/guardrails/providers/test";

/** the register dialog with its URL filled in and the trial panel expanded */
async function openProviderTrial(canvasElement: HTMLElement) {
  await userEvent.click(
    await within(canvasElement).findByRole("button", { name: /add provider/i }),
  );
  const dialog = within(await within(document.body).findByRole("dialog"));
  await userEvent.type(dialog.getByLabelText("Evaluation URL"), "https://policy.corp.internal/x");
  await userEvent.click(dialog.getByRole("button", { name: "Send a test envelope" }));
  return dialog;
}

const providerTrials = recording(
  registry(PROVIDERS, undefined, async (input) =>
    isProviderTrial(input)
      ? json({
          ok: true,
          decision: { action: "block", reason: "pii detected" },
          credential: "not_configured",
          http_status: 200,
          latency_ms: 42,
          truncated: false,
        })
      : json(POLICY_SERVICE, 201),
  ),
);

/**
 * A draft provider is sent one sample envelope before it is saved (#2265): the
 * decision and its reason come back with the round trip, and nothing is saved.
 */
export const TestsADraftProviderBeforeSaving: Story = {
  render: () => <Harness fetchStub={providerTrials.stub} />,
  play: async ({ canvasElement }) => {
    const dialog = await openProviderTrial(canvasElement);
    await userEvent.type(dialog.getByLabelText("Sample message"), "hello");
    await userEvent.click(dialog.getByRole("button", { name: "Send test" }));
    const result = await dialog.findByRole("status");
    await expect(within(result).getByText("Block")).toBeVisible();
    await expect(within(result).getByText("Reason: pii detected")).toBeVisible();
    await expect(within(result).getByText("42 ms round trip")).toBeVisible();

    const sent = await providerTrials.expectSentBody<Record<string, unknown>>(
      "POST",
      "/providers/test",
    );
    await expect(sent).toMatchObject({
      url: "https://policy.corp.internal/x",
      stage: "pre_call",
      auth_kind: "none",
      sample: "hello",
    });
    await expect(
      providerTrials.calls.filter(
        (call) => call.method === "POST" && !call.url.endsWith("/providers/test"),
      ),
    ).toHaveLength(0);
  },
};

/**
 * A credential variable only the gateways have is reported as unset here, not
 * as the webhook rejecting the call, and the dialog offers to test without it.
 */
export const SaysWhenTheCredentialIsNotSetOnTheControlPlane: Story = {
  render: () => {
    const sent: Record<string, unknown>[] = [];
    return (
      <Harness
        fetchStub={registry(PROVIDERS, undefined, async (input, init) => {
          if (!isProviderTrial(input)) return json(POLICY_SERVICE, 201);
          const body = JSON.parse(String(init?.body)) as Record<string, unknown>;
          sent.push(body);
          return body.skip_credential
            ? json({
                ok: true,
                decision: { action: "allow" },
                credential: "skipped",
                http_status: 200,
                latency_ms: 12,
                truncated: false,
              })
            : json({
                ok: false,
                error: {
                  kind: "credential_unset",
                  message: "ROLTER_GUARDRAIL_TOKEN is not set on the control plane.",
                },
                credential: "unset",
                latency_ms: 0,
                truncated: false,
              });
        })}
      />
    );
  },
  play: async ({ canvasElement }) => {
    const dialog = await openProviderTrial(canvasElement);
    await pickOption(dialog.getByLabelText("Authentication"), "Bearer token");
    await userEvent.type(
      dialog.getByLabelText("Secret environment variable"),
      "ROLTER_GUARDRAIL_TOKEN",
    );
    await userEvent.click(dialog.getByRole("button", { name: "Send test" }));
    const result = await dialog.findByRole("status");
    await expect(within(result).getByText("Credential not available here")).toBeVisible();
    await expect(within(result).getByText(/is not set on the control plane/)).toBeVisible();
    await expect(within(result).queryByText(/On live traffic/)).toBeNull();

    await userEvent.click(dialog.getByRole("switch", { name: "Send without the credential" }));
    await userEvent.click(dialog.getByRole("button", { name: "Send test" }));
    await waitFor(() => expect(dialog.getByRole("status")).toHaveTextContent("Allow"));
    await expect(dialog.getByRole("status")).toHaveTextContent("without the credential");
  },
};

/** a timeout is a result, and says what the failure policy would do to live traffic */
export const ShowsATimeoutWithItsLiveEffect: Story = {
  render: () => (
    <Harness
      fetchStub={registry(PROVIDERS, undefined, async (input) =>
        isProviderTrial(input)
          ? json({
              ok: false,
              error: { kind: "timeout", message: "the webhook did not answer in time" },
              credential: "not_configured",
              latency_ms: 1800,
              truncated: false,
            })
          : json(POLICY_SERVICE, 201),
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    const dialog = await openProviderTrial(canvasElement);
    await userEvent.click(dialog.getByRole("button", { name: "Send test" }));
    const result = await dialog.findByRole("status");
    await expect(within(result).getByText("No answer within the timeout")).toBeVisible();
    // the draft's failure policy starts as fail closed
    await expect(within(result).getByText(/rejects the request \(fail closed\)/)).toBeVisible();
    await expect(within(result).getByText("1800 ms round trip")).toBeVisible();
  },
};

const LOCAL_SHA = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/** an in-process classifier: no URL, no credential, no retries */
const LOCAL_CLASSIFIER: GuardrailProviderRow = {
  ...PROVIDERS[1],
  id: "provider-local",
  name: "Local toxicity model",
  kind: "local",
  url: "",
  model_path: "/etc/rolter/models/toxicity.json",
  model_sha256: LOCAL_SHA,
  failure_mode: "fail_closed",
  max_retries: 0,
};

/** a local row shows its kind and model path where a webhook shows its URL */
export const ListsALocalClassifier: Story = {
  render: () => <Harness fetchStub={registry([...PROVIDERS, LOCAL_CLASSIFIER])} />,
  play: async ({ canvasElement }) => {
    await within(canvasElement).findByRole("heading", { name: "Local toxicity model" });
    const local = card(canvasElement, "Local toxicity model");
    await expect(local.getByText("/etc/rolter/models/toxicity.json")).toBeVisible();
    await expect(local.getByText("Local classifier")).toBeVisible();
    await expect(local.getByText("2,500 ms timeout · 32 KiB cap")).toBeVisible();
    await expect(card(canvasElement, "Production LLM Guard").getByText("Webhook")).toBeVisible();
  },
};

/**
 * Picking the local kind swaps the URL and credential rows for the model's
 * path and digest, hides the webhook trial, and saves the shape the server
 * expects: an empty URL, no credential and no retries.
 */
let savesLocal: Recorder;
export const RegistersALocalClassifier: Story = {
  render: () => {
    savesLocal = recording(registry([], undefined, async () => json(LOCAL_CLASSIFIER, 201)));
    return <Harness fetchStub={savesLocal.stub} />;
  },
  play: async ({ canvasElement }) => {
    await userEvent.click(
      await within(canvasElement).findByRole("button", { name: /register first provider/i }),
    );
    const dialog = within(await within(document.body).findByRole("dialog"));
    await userEvent.type(dialog.getByLabelText("Provider name"), "Local toxicity model");
    await userEvent.click(dialog.getByRole("radio", { name: "Local classifier" }));
    await expect(dialog.queryByLabelText("Evaluation URL")).toBeNull();
    await expect(dialog.queryByLabelText("Authentication")).toBeNull();
    await expect(dialog.queryByText("Send a test envelope")).toBeNull();
    await waitFor(() =>
      expect(dialog.getByText(/cannot be tested from the control plane/)).toBeVisible(),
    );

    const save = dialog.getByRole("button", { name: "Save provider" });
    await expect(save).toHaveAttribute("aria-disabled", "true");
    const path = dialog.getByLabelText("Model path");
    const sha = dialog.getByLabelText("Model SHA-256");
    await userEvent.click(path);
    await userEvent.tab();
    await expect(path).toHaveAttribute("aria-invalid", "true");
    await userEvent.type(path, "/etc/rolter/models/toxicity.json");
    await userEvent.type(sha, "abc123");
    await userEvent.tab();
    await expect(sha).toHaveAttribute("aria-invalid", "true");
    await expect(save).toHaveAttribute("aria-disabled", "true");
    await userEvent.clear(sha);
    await userEvent.type(sha, LOCAL_SHA);
    await expect(sha).not.toHaveAttribute("aria-invalid", "true");
    await userEvent.click(save);
    await expect(
      await savesLocal.expectSentBody<GuardrailProviderInput>("POST", "/guardrails/providers"),
    ).toMatchObject({
      kind: "local",
      url: "",
      auth_kind: "none",
      auth_env: null,
      max_retries: 0,
      model_path: "/etc/rolter/models/toxicity.json",
      model_sha256: LOCAL_SHA,
    });
    await expectSheetClosed();
  },
};

/** editing a local row keeps its kind and shows its model */
export const EditsALocalClassifier: Story = {
  render: () => <Harness fetchStub={registry([LOCAL_CLASSIFIER])} />,
  play: async ({ canvasElement }) => {
    await userEvent.click(
      await within(canvasElement).findByRole("button", {
        name: "Edit provider Local toxicity model",
      }),
    );
    const dialog = within(await within(document.body).findByRole("dialog"));
    await expect(await dialog.findByRole("radio", { name: "Local classifier" })).toBeChecked();
    await expect(dialog.getByLabelText("Model path")).toHaveValue(
      "/etc/rolter/models/toxicity.json",
    );
    await expect(dialog.getByLabelText("Model SHA-256")).toHaveValue(LOCAL_SHA);
  },
};
