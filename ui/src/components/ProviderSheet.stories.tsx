import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { ProviderSheet } from "./ProviderSheet";
import { Toaster } from "./ui/toaster";
import type { ProviderRow, ProviderTestResult } from "@/lib/api";
import ru from "@/lib/i18n/locales/ru.json";
import { atMobile, expectInViewport } from "@/lib/story-viewport";
import { ToastProvider } from "@/lib/toast";
import {
  answerDiscardPrompt,
  discardPrompt,
  recording,
  scoped,
  type Recorder,
} from "@/pages/story-harness";

const PROVIDER: ProviderRow = {
  id: "prov-1",
  org_id: "11111111-1111-1111-1111-111111111111",
  name: "openai-primary",
  slug: "openai-primary",
  kind: "openai",
  api_base: "https://api.openai.com",
  api_key_env: null,
  egress_proxy: null,
  egress_proxies: [],
  created_at: "2026-08-01T10:00:00Z",
};

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });

type FetchStub = (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;

/// Answer the probe with a fixed outcome; everything else is inert. The sheet
/// only calls the network when the operator presses the button.
// what the control plane answers for the kinds these stories open: the endpoint
// each is sent to and the header its key travels in, as the forwarder has them
const CHAT = "/v1/chat/completions";
const KINDS = [
  {
    kind: "openai",
    base_includes_v1: false,
    request: "chat",
    request_path: CHAT,
    auth_header: "authorization",
  },
  {
    kind: "openai_compatible",
    base_includes_v1: false,
    request: "chat",
    request_path: CHAT,
    auth_header: "authorization",
  },
  {
    kind: "anthropic",
    base_includes_v1: false,
    request: "chat",
    request_path: "/v1/messages",
    auth_header: "x-api-key",
  },
  {
    kind: "tei",
    base_includes_v1: false,
    request: "embeddings",
    request_path: "/v1/embeddings",
    auth_header: "authorization",
  },
  {
    kind: "gemini_native",
    base_includes_v1: false,
    request: "chat",
    request_path: "/models/{model}:generateContent",
    auth_header: "x-goog-api-key",
  },
  {
    kind: "mistral",
    base_includes_v1: true,
    request: "chat",
    request_path: CHAT,
    auth_header: "authorization",
  },
];

function stub(test: () => Promise<Response>): FetchStub {
  return async (input) => {
    if (String(input).endsWith("/test")) return test();
    // the sheet asks per kind whether /v1 belongs in api_base (#947)
    if (String(input).includes("/provider-kinds")) return json(KINDS);
    return json({});
  };
}

// installed during render, not in an effect: child effects run before the
// parent's, so an effect would let the first real fetch through
function Harness({
  fetchStub,
  provider = PROVIDER,
  mode = "edit",
  onOpenChange = () => {},
  onDone = () => {},
}: {
  fetchStub: FetchStub;
  provider?: ProviderRow;
  /** `add` renders the create sheet, which has no provider behind it yet */
  mode?: "add" | "edit";
  onDone?: (created?: ProviderRow) => void;
  /**
   * Threaded through so a story can assert the sheet was never *asked* to
   * close. Rendering `open` unconditionally means the sheet stays on screen
   * whatever the component decides, so "is the dialog still there" would pass
   * against a sheet that closed itself (#1607).
   */
  onOpenChange?: (open: boolean) => void;
}) {
  const original = React.useRef<typeof globalThis.fetch | null>(null);
  const client = React.useMemo(() => {
    original.current ??= globalThis.fetch;
    // the scope picker reads the org's teams and projects as the sheet opens
    globalThis.fetch = scoped(fetchStub) as typeof globalThis.fetch;
    return new QueryClient({ defaultOptions: { queries: { retry: false } } });
  }, [fetchStub]);
  React.useEffect(
    () => () => {
      if (original.current) globalThis.fetch = original.current;
    },
    [],
  );
  return (
    <QueryClientProvider client={client}>
      <ProviderSheet
        open
        mode={mode}
        onOpenChange={onOpenChange}
        orgId={PROVIDER.org_id}
        provider={mode === "edit" ? provider : null}
        onDone={onDone}
      />
    </QueryClientProvider>
  );
}

const result = (over: Partial<ProviderTestResult> = {}): ProviderTestResult => ({
  reachable: true,
  probed_url: "https://api.openai.com/v1/models",
  status: 200,
  latency_ms: 142,
  credential: "stored",
  models_found: 38,
  error: null,
  ...over,
});

// every story renders through `Harness`, which owns the props; these satisfy the
// component's required-prop contract for the docs page and are not otherwise read
const meta = {
  title: "Overlays/ProviderSheet",
  component: ProviderSheet,
  parameters: { layout: "fullscreen" },
  args: {
    open: true,
    mode: "edit" as const,
    onOpenChange: () => {},
    orgId: PROVIDER.org_id,
    provider: PROVIDER,
    onDone: () => {},
  },
} satisfies Meta<typeof ProviderSheet>;

export default meta;
type Story = StoryObj<typeof meta>;

// the Sheet renders through a portal onto document.body, so the story's
// canvasElement is empty — query the whole document instead
const screen = () => within(document.body);

const press = async () =>
  userEvent.click(await screen().findByRole("button", { name: /test connection/i }));

export const Reachable: Story = {
  render: () => <Harness fetchStub={stub(async () => json(result()))} />,
  play: async () => {
    const canvas = screen();
    await press();
    await waitFor(() => expect(canvas.getByRole("status")).toBeVisible());
    // the count is what tells a reachable provider from a *useful* one
    await expect(canvas.getByRole("status")).toHaveTextContent("38 models");
    // the probed URL is always shown: a doubled /v1 is the most common cause of
    // a failure and is invisible without it
    await expect(canvas.getByText("https://api.openai.com/v1/models")).toBeVisible();
  },
};

/**
 * The edit footer on a phone, in Russian (#2003). Three buttons on one line
 * that could not wrap pushed "Проверить подключение" 151px past the left
 * edge; the test now sits above the Cancel/Save pair, and all three can be
 * pressed.
 */
export const FooterFitsAPhoneInRussian: Story = {
  ...atMobile,
  globals: { ...atMobile.globals, locale: "ru" },
  render: () => <Harness fetchStub={stub(async () => json(result()))} />,
  play: async () => {
    const canvas = screen();
    const test = await canvas.findByRole("button", { name: ru.providerSheet.testConnection });
    const cancel = canvas.getByRole("button", { name: ru.common.cancel });
    const save = canvas.getByRole("button", { name: ru.providerSheet.cta.save });
    for (const button of [test, cancel, save]) await expectInViewport(button);
    await expect(test.getBoundingClientRect().bottom).toBeLessThanOrEqual(
      save.getBoundingClientRect().top,
    );
    // and it still works from there
    await userEvent.click(test);
    await waitFor(() => expect(canvas.getByRole("status")).toBeVisible());
  },
};

// the case the button exists for: the row saved fine and the credential is wrong
export const RejectedCredential: Story = {
  render: () => (
    <Harness
      fetchStub={stub(async () =>
        json(
          result({
            reachable: false,
            status: 401,
            models_found: null,
            error: "401 Unauthorized: the upstream rejected the credential (resolved from: stored)",
          }),
        ),
      )}
    />
  ),
  play: async () => {
    const canvas = screen();
    await press();
    await waitFor(() => expect(canvas.getByText(/rejected the credential/)).toBeVisible());
    // naming where the credential came from is what separates "wrong key" from
    // "no key configured"
    await expect(canvas.getByText(/resolved from: stored/)).toBeVisible();
  },
};

// the third outcome (#980): the host answered 2xx, but with something that is
// not a model catalogue. rendering that in the same red as a refused connection
// is accurate and useless — it is the URL or the service behind it that is
// wrong, not the network (#1034)
export const AnsweredButNotACatalogue: Story = {
  render: () => (
    <Harness
      fetchStub={stub(async () =>
        json(
          result({
            reachable: false,
            probed_url: "https://gw.example.com/v1/v1/models",
            status: 200,
            models_found: null,
            error:
              "200: https://gw.example.com/v1/v1/models answered, but not with a model list. Something other than the provider's API is serving that URL — check api_base for a duplicated path segment, a catch-all route or a login portal.",
          }),
        ),
      )}
    />
  ),
  play: async () => {
    const canvas = screen();
    await press();
    const status = await waitFor(() => canvas.getByRole("status"));
    // the heading says what happened, rather than flattening to "test failed"
    await expect(status).toHaveTextContent(/answered, but not with a model list/i);
    await expect(status).not.toHaveTextContent(/could not reach this provider/i);
    await expect(canvas.getByText(/duplicated path segment/)).toBeVisible();
  },
};

export const Unreachable: Story = {
  render: () => (
    <Harness
      fetchStub={stub(async () =>
        json(
          result({
            reachable: false,
            probed_url: "http://vllm.internal:8000/v1/models",
            status: null,
            models_found: null,
            error: "could not connect — check the host, port and TLS",
          }),
        ),
      )}
    />
  ),
  play: async () => {
    const canvas = screen();
    await press();
    await waitFor(() => expect(canvas.getByText(/could not connect/)).toBeVisible());
  },
};

// a KEK mismatch is not a provider problem, and the upstream is never contacted
export const StoredKeyUnreadable: Story = {
  render: () => (
    <Harness
      fetchStub={stub(async () =>
        json(
          result({
            reachable: false,
            probed_url: "",
            status: null,
            latency_ms: 0,
            credential: "stored (KEK unset)",
            models_found: null,
            error:
              "the stored credential for 'openai-primary' could not be read: ROLTER_KEK is unset or does not match the key it was sealed with. The upstream was not contacted.",
          }),
        ),
      )}
    />
  ),
  play: async () => {
    const canvas = screen();
    await press();
    await waitFor(() => expect(canvas.getByText(/ROLTER_KEK is unset/)).toBeVisible());
    await expect(canvas.getByText(/upstream was not contacted/)).toBeVisible();
  },
};

// the button must go busy, or an operator presses it repeatedly against a
// provider that is simply slow to answer
export const Testing: Story = {
  render: () => <Harness fetchStub={stub(() => new Promise<Response>(() => {}))} />,
  play: async () => {
    const canvas = screen();
    await press();
    await waitFor(() => expect(canvas.getByRole("button", { name: /testing/i })).toBeDisabled());
  },
};

/**
 * #947: the base-URL placeholder used to read `https://api.openai.com/v1` for
 * every kind. For openai-shaped kinds rolter appends `/v1` itself, so that
 * advice produced `/v1/v1/chat/completions` and every request 404'd.
 *
 * The resolved URL is now previewed live, so the doubling is visible before
 * saving rather than at the first inference call.
 */
export const BaseUrlDoublesTheVersionPrefix: Story = {
  render: () => (
    <Harness
      fetchStub={stub(async () => json(result()))}
      provider={{ ...PROVIDER, api_base: "https://gpustack.localhost/v1" }}
    />
  ),
  play: async () => {
    const canvas = screen();
    await expect(
      await canvas.findByText("https://gpustack.localhost/v1/v1/chat/completions"),
    ).toBeVisible();
    await expect(canvas.getByText(/remove the trailing \/v1/i)).toBeVisible();
    // the warning is the input's own description, not text beside it, so a
    // screen reader hears the misconfiguration on the field itself (#1544)
    const base = canvas.getByLabelText("API base");
    await expect(base).toHaveAttribute("aria-invalid", "true");
    await expect(base).toHaveAccessibleDescription(/remove the trailing \/v1/i);
  },
};

/** A well-formed openai-shaped base: previewed, not flagged. */
export const BaseUrlIsWellFormed: Story = {
  render: () => <Harness fetchStub={stub(async () => json(result()))} />,
  play: async () => {
    const canvas = screen();
    await expect(
      await canvas.findByText("https://api.openai.com/v1/chat/completions"),
    ).toBeVisible();
    await expect(canvas.queryByText(/remove the trailing/i)).not.toBeInTheDocument();
    // a sound base is described by the kind's hint alone and is not invalid
    const base = canvas.getByLabelText("API base");
    await expect(base).not.toHaveAttribute("aria-invalid");
    await expect(base).toHaveAccessibleDescription(/Leave off the version prefix/);
  },
};

/**
 * The other half of the rule: for a stripping kind the trailing `/v1` is
 * required, so the same spelling must be accepted and the hint must invert.
 */
export const StrippingKindWantsV1InTheBase: Story = {
  render: () => (
    <Harness
      fetchStub={stub(async () => json(result()))}
      provider={{ ...PROVIDER, kind: "mistral", api_base: "https://api.mistral.ai/v1" }}
    />
  ),
  play: async () => {
    const canvas = screen();
    await expect(
      await canvas.findByText("https://api.mistral.ai/v1/chat/completions"),
    ).toBeVisible();
    await expect(canvas.queryByText(/remove the trailing/i)).not.toBeInTheDocument();
  },
};

/**
 * The save outcome, which the sheet itself cannot report: it closes the moment
 * the write lands, taking any inline confirmation with it (#1197).
 *
 * The toast renders inside the story canvas while the sheet portals onto the
 * body, so the two are queried through different roots.
 */
export const SavingAnnouncesTheOutcome: Story = {
  render: () => (
    <ToastProvider>
      <Harness
        fetchStub={async (input) => {
          if (String(input).includes("/provider-kinds")) return json(KINDS);
          return json(PROVIDER);
        }}
      />
      <Toaster />
    </ToastProvider>
  ),
  play: async ({ canvasElement }) => {
    await userEvent.click(await screen().findByRole("button", { name: "Save provider" }));
    const canvas = within(canvasElement);
    const status = await waitFor(() => canvas.getByRole("status"));
    // the card fades in, so visibility is awaited rather than asserted at once
    await waitFor(() => expect(within(status).getByText("Saved")).toBeVisible());
    await expect(within(status).getByText(/openai-primary updated/)).toBeInTheDocument();
  },
};

/**
 * The save is refused (#1607).
 *
 * `SavingAnnouncesTheOutcome` covers the answer; this covers the other one. The
 * refusal reaches the toast queue, and the sheet is never asked to close — an
 * API key is typed once and a sheet that closed on a rejected save would make
 * the operator fetch it again.
 */
export const SaveRejectedByTheServer: Story = {
  render: () => {
    const closes: boolean[] = [];
    closeRequests = closes;
    return (
      <ToastProvider>
        <Harness
          onOpenChange={(open) => closes.push(open)}
          fetchStub={async (input, init) => {
            if (String(input).includes("/provider-kinds")) return json(KINDS);
            if (init?.method === "PUT" || init?.method === "POST") {
              return json({ error: { message: "the api key was rejected upstream" } }, 502);
            }
            return json(PROVIDER);
          }}
        />
        <Toaster />
      </ToastProvider>
    );
  },
  play: async ({ canvasElement }) => {
    await userEvent.click(await screen().findByRole("button", { name: "Save provider" }));
    const canvas = within(canvasElement);
    const alert = await waitFor(() => canvas.getByRole("alert"));
    await waitFor(() => expect(within(alert).getByText(/rejected upstream/)).toBeVisible());
    // never asked to close, and the draft is still in the fields
    await expect(closeRequests).toEqual([]);
    await expect(screen().getByLabelText("Name")).toHaveValue("openai-primary");
  },
};

/**
 * Dismissing a dirty provider draft goes through the shared prompt, not
 * `window.confirm` (#1463). Cancelling keeps the typing *and* leaves the sheet
 * unasked to close — which is the half "is the dialog still there" cannot see,
 * because this harness renders `open` unconditionally.
 */
export const DiscardGuardKeepsTheDraft: Story = {
  render: () => {
    const closes: boolean[] = [];
    closeRequests = closes;
    return (
      <Harness onOpenChange={(open) => closes.push(open)} fetchStub={stub(async () => json({}))} />
    );
  },
  play: async () => {
    // the name is fixed once a provider exists, so the editable field is the
    // base url. wait for the seed: typing that lands before it is overwritten
    await waitFor(() =>
      expect(screen().getByLabelText("API base")).toHaveValue("https://api.openai.com"),
    );
    await userEvent.type(screen().getByLabelText("API base"), "/eu");
    await userEvent.click(screen().getByRole("button", { name: /close/i }));
    await discardPrompt();
    await answerDiscardPrompt(false);
    await expect(screen().getByLabelText("API base")).toHaveValue("https://api.openai.com/eu");
    await expect(closeRequests).toEqual([]);
  },
};

/** Confirming is the only path that asks the sheet to close. */
export const DiscardGuardThrowsItAway: Story = {
  render: () => {
    const closes: boolean[] = [];
    closeRequests = closes;
    return (
      <Harness onOpenChange={(open) => closes.push(open)} fetchStub={stub(async () => json({}))} />
    );
  },
  play: async () => {
    await waitFor(() =>
      expect(screen().getByLabelText("API base")).toHaveValue("https://api.openai.com"),
    );
    await userEvent.type(screen().getByLabelText("API base"), "/eu");
    await userEvent.click(screen().getByRole("button", { name: "Cancel" }));
    await answerDiscardPrompt(true);
    await waitFor(() => expect(closeRequests).toEqual([false]));
  },
};

// the story's own recorder, hoisted so `play` can read what `render` wired up
let closeRequests: boolean[] = [];

// ------------------------------------------------ test right after a create (#2142)

const CREATED: ProviderRow = {
  ...PROVIDER,
  id: "prov-new",
  name: "vllm-eu",
  slug: "vllm-eu",
  api_base: "http://vllm.internal:8000",
};

/** answers the create with `CREATED` and the probe with `test`, everything else inert */
function createStub(test: () => Promise<Response> = async () => json(result())): FetchStub {
  return async (input, init) => {
    const url = String(input);
    if (url.includes("/provider-kinds")) return json(KINDS);
    if (url.endsWith("/test")) return test();
    if (init?.method === "POST") return json(CREATED);
    return json({});
  };
}

const fillTheForm = async (name: string, apiBase: string) => {
  const canvas = screen();
  await userEvent.type(await canvas.findByLabelText("Name"), name);
  await userEvent.type(canvas.getByLabelText("API base"), apiBase);
};

let creation: Recorder;
let doneCalls: (ProviderRow | undefined)[] = [];

/**
 * A create does not close the sheet. It stays open on the new provider with the
 * test one click away, and does not run it: the test is a call to the upstream,
 * so the operator chooses when it is spent. Focus moves to the button, since
 * the one that was pressed is now Save with nothing to save.
 */
export const CreateOffersTheTestWithoutRunningIt: Story = {
  render: () => {
    const closes: boolean[] = [];
    closeRequests = closes;
    doneCalls = [];
    creation = recording(createStub());
    return (
      <Harness
        mode="add"
        fetchStub={creation.stub}
        onOpenChange={(open) => closes.push(open)}
        onDone={(row) => doneCalls.push(row)}
      />
    );
  },
  play: async () => {
    const canvas = screen();
    // a name is permanent, and the create form says so before it is typed
    const name = await canvas.findByLabelText("Name");
    await expect(name).toBeEnabled();
    await expect(name).toHaveAccessibleDescription(/Fixed once the provider is created/);
    await expect(canvas.queryByRole("button", { name: "Test connection" })).toBeNull();

    await fillTheForm("vllm-eu", "http://vllm.internal:8000");
    await userEvent.click(canvas.getByRole("button", { name: "Create provider" }));
    const body = await creation.expectSentBody<{ name: string; api_base: string }>(
      "POST",
      "/orgs/",
    );
    await expect(body).toMatchObject({ name: "vllm-eu", api_base: "http://vllm.internal:8000" });

    // the sheet says what happened and what is next, and is now the edit sheet
    await canvas.findByText(/vllm-eu is created but not tested yet/);
    await expect(canvas.getByRole("heading", { name: "Edit vllm-eu" })).toBeVisible();
    await expect(canvas.getByLabelText("Name")).toBeDisabled();
    const test = canvas.getByRole("button", { name: "Test connection" });
    await waitFor(() => expect(test).toHaveFocus());
    await waitFor(() => expect(test).toBeEnabled());
    // Cancel has become Done, and Save waits for an edit
    await expect(canvas.getByRole("button", { name: "Done" })).toBeVisible();
    await expect(canvas.queryByRole("button", { name: "Cancel" })).toBeNull();
    await waitFor(() =>
      expect(canvas.getByRole("button", { name: "Save provider" })).toBeDisabled(),
    );
    // nothing closed the sheet and nothing probed the upstream
    await expect(closeRequests).toEqual([]);
    await expect(doneCalls).toEqual([CREATED]);
    creation.expectNotSent("POST", "/test");

    await userEvent.click(test);
    await creation.expectSent("POST", "/providers/prov-new/test");
    await waitFor(() => expect(canvas.getByText(/Reachable · 38 models/)).toBeVisible());
    // the result replaces the note rather than standing beside it
    await expect(canvas.queryByText(/not tested yet/)).toBeNull();
  },
};

/** a create the control plane refuses is still a create: nothing to test yet */
export const RefusedCreateStaysACreate: Story = {
  render: () => (
    <Harness
      mode="add"
      fetchStub={async (input, init) => {
        if (String(input).includes("/provider-kinds")) return json(KINDS);
        if (init?.method === "POST") {
          return json({ error: { message: "provider name 'vllm-eu' is already in use" } }, 409);
        }
        return json({});
      }}
    />
  ),
  play: async () => {
    const canvas = screen();
    await fillTheForm("vllm-eu", "http://vllm.internal:8000");
    await userEvent.click(canvas.getByRole("button", { name: "Create provider" }));
    const alert = await canvas.findByRole("alert");
    await expect(alert).toHaveTextContent(/already in use/);
    await expect(canvas.getByRole("heading", { name: "Add provider" })).toBeVisible();
    await expect(canvas.queryByRole("button", { name: "Test connection" })).toBeNull();
    await expect(canvas.queryByText(/not tested yet/)).toBeNull();
    await expect(canvas.getByRole("button", { name: "Cancel" })).toBeVisible();
  },
};

/** the created footer on a phone, in Russian: note, test, Done and Save all reachable */
export const CreatedFooterFitsAPhoneInRussian: Story = {
  ...atMobile,
  globals: { ...atMobile.globals, locale: "ru" },
  render: () => <Harness mode="add" fetchStub={createStub()} />,
  play: async () => {
    const canvas = screen();
    await userEvent.type(await canvas.findByLabelText(ru.providerSheet.fields.name), "vllm-eu");
    await userEvent.type(
      canvas.getByLabelText(ru.providerSheet.fields.apiBase),
      "http://vllm.internal:8000",
    );
    await userEvent.click(canvas.getByRole("button", { name: ru.providerSheet.cta.create }));
    const note = await canvas.findByText(/vllm-eu создан, но ещё не проверен/);
    const test = canvas.getByRole("button", { name: ru.providerSheet.testConnection });
    const done = canvas.getByRole("button", { name: ru.common.done });
    const save = canvas.getByRole("button", { name: ru.providerSheet.cta.save });
    for (const element of [note, test, done, save]) await expectInViewport(element);
  },
};

let tested: Recorder;

/**
 * The probe reads the saved row, so with edits in the form its answer would
 * speak for the old values. The button is off and the reason is on screen, as
 * the button's own description, since a disabled control cannot carry a tooltip.
 * Putting the value back makes the form match what is stored, and the test is
 * offered again.
 */
export const TestIsOffWhileTheFormHasUnsavedEdits: Story = {
  render: () => {
    tested = recording(stub(async () => json(result())));
    return <Harness fetchStub={tested.stub} />;
  },
  play: async () => {
    const canvas = screen();
    const test = await canvas.findByRole("button", { name: "Test connection" });
    await waitFor(() =>
      expect(canvas.getByLabelText("API base")).toHaveValue("https://api.openai.com"),
    );
    await expect(canvas.queryByText(/checks the saved provider/)).toBeNull();

    await userEvent.type(canvas.getByLabelText("API base"), "/eu");
    await waitFor(() => expect(test).toBeDisabled());
    await waitFor(() => expect(canvas.getByText(/checks the saved provider/)).toBeVisible());
    await expect(test).toHaveAccessibleDescription(/Save first, then test/);

    await userEvent.type(canvas.getByLabelText("API base"), "{Backspace}{Backspace}{Backspace}");
    await waitFor(() => expect(test).toBeEnabled());
    await expect(canvas.queryByText(/checks the saved provider/)).toBeNull();
    tested.expectNotSent("POST", "/test");
  },
};

/** the name is fixed after create, and the field says so instead of just greying out */
export const NameIsFixedAfterCreate: Story = {
  render: () => <Harness fetchStub={stub(async () => json(result()))} />,
  play: async () => {
    const name = await screen().findByLabelText("Name");
    await expect(name).toBeDisabled();
    await expect(name).toHaveAccessibleDescription(/Fixed once the provider is created/);
  },
};

/** what the egress proxy is for, what it takes and what it touches, on the field itself */
export const EgressProxyHintSaysWhatItTakes: Story = {
  render: () => <Harness fetchStub={stub(async () => json(result()))} />,
  play: async () => {
    const proxy = await screen().findByLabelText("Egress proxy (optional)");
    await expect(proxy).toHaveAccessibleDescription(/Only this provider's upstream calls/);
    await expect(proxy).toHaveAccessibleDescription(/http, https, socks5 or socks5h/);
    await expect(proxy).toHaveAccessibleDescription(/\$\{ENV_VAR\}/);
  },
};

// ------------------------------------------------ the sheet follows the kind (#2811)

/** a saved provider of `kind`, so the sheet opens on it with the base filled in */
const ofKind = (kind: string, api_base: string): ProviderRow => ({
  ...PROVIDER,
  name: `${kind}-primary`,
  slug: `${kind}-primary`,
  kind,
  api_base,
});

const previewOf = (
  kind: string,
  apiBase: string,
  expected: { url: string; label: string; keyHint: string; kindName: string },
): Story => ({
  render: () => (
    <Harness fetchStub={stub(async () => json(result()))} provider={ofKind(kind, apiBase)} />
  ),
  play: async () => {
    const canvas = screen();
    // the address is the kind's own endpoint, from the control plane's table
    const url = await canvas.findByText(expected.url);
    await expect(url).toBeVisible();
    await expect(url.parentElement).toHaveTextContent(expected.label);
    await expect(canvas.getByRole("combobox", { name: "Kind" })).toHaveValue(expected.kindName);
    // and the key hint says how this kind's API reads the key
    await expect(canvas.getByLabelText(/^Provider key \(optional\)/)).toHaveAccessibleDescription(
      new RegExp(expected.keyHint),
    );
    await expect(canvas.queryByText(/remove the trailing/i)).not.toBeInTheDocument();
  },
});

/** an OpenAI-compatible server: chat completions, key as a bearer token */
export const PreviewForAnOpenaiCompatibleServer: Story = previewOf(
  "openai_compatible",
  "http://vllm.internal:8000",
  {
    url: "http://vllm.internal:8000/v1/chat/completions",
    label: "Chat requests resolve to",
    keyHint: "Sent upstream as a bearer token",
    kindName: "OpenAI-compatible",
  },
);

/** Anthropic is called at /v1/messages with x-api-key, which the sheet used to get wrong on both counts */
export const PreviewForAnthropic: Story = previewOf("anthropic", "https://api.anthropic.com", {
  url: "https://api.anthropic.com/v1/messages",
  label: "Chat requests resolve to",
  keyHint: "Sent upstream in the x-api-key header",
  kindName: "Anthropic",
});

/** a text-embeddings server is asked for embeddings, not a chat completion it would 404 */
export const PreviewForTextEmbeddingsInference: Story = previewOf("tei", "http://tei.internal:80", {
  url: "http://tei.internal:80/v1/embeddings",
  label: "Embedding requests resolve to",
  keyHint: "Sent upstream as a bearer token",
  kindName: "Text Embeddings Inference",
});

/** native gemini carries the model in the path, shown as the placeholder it is */
export const PreviewForNativeGemini: Story = previewOf(
  "gemini_native",
  "https://generativelanguage.googleapis.com/v1beta",
  {
    url: "https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent",
    label: "Chat requests resolve to",
    keyHint: "Sent upstream in the x-goog-api-key header",
    kindName: "Google Gemini (native)",
  },
);

/**
 * Picking another kind in the create sheet moves the address, the label and the
 * key hint with it, and the picker lists display names with a line each.
 */
export const KindPickerFollowsThroughToThePreview: Story = {
  render: () => <Harness mode="add" fetchStub={createStub()} />,
  play: async () => {
    const canvas = screen();
    await userEvent.type(await canvas.findByLabelText("API base"), "https://gw.example.com");
    const kind = canvas.getByRole("combobox", { name: "Kind" });
    // the default kind is the first the control plane lists
    await expect(
      await canvas.findByText("https://gw.example.com/v1/chat/completions"),
    ).toBeVisible();

    await userEvent.click(kind);
    const listbox = await canvas.findByRole("listbox");
    // names, not ids, each with what it is for; the id is kept for the filter
    const options = within(listbox).getAllByRole("option");
    await expect(options.map((o) => o.textContent)).toContain(
      "OpenAI-compatiblevLLM, TGI, LM Studio or any server that speaks the OpenAI API",
    );
    await expect(within(listbox).getByRole("option", { name: /^Anthropic/ })).toBeVisible();
    await expect(within(listbox).queryByText("openai_compatible")).toBeNull();

    await userEvent.click(within(listbox).getByRole("option", { name: /^Anthropic/ }));
    await waitFor(() => expect(kind).toHaveValue("Anthropic"));
    await expect(await canvas.findByText("https://gw.example.com/v1/messages")).toBeVisible();
    // the stored id and the one-line description sit under the picker
    await expect(kind).toHaveAccessibleDescription(/Anthropic's Messages API for Claude models/);
    await expect(kind).toHaveAccessibleDescription(/anthropic/);
    await expect(canvas.getByLabelText(/^Provider key \(optional\)/)).toHaveAccessibleDescription(
      /Sent upstream in the x-api-key header/,
    );

    await userEvent.click(kind);
    await userEvent.click(
      await within(await canvas.findByRole("listbox")).findByRole("option", {
        name: /^Text Embeddings Inference/,
      }),
    );
    await expect(await canvas.findByText("https://gw.example.com/v1/embeddings")).toBeVisible();
    await expect(canvas.getByText(/Embedding requests resolve to/)).toBeVisible();
    await expect(canvas.queryByText(/Chat requests resolve to/)).toBeNull();
  },
};

/** the filter matches the display name, the description and the stored id alike */
export const KindPickerFiltersByNameDescriptionAndId: Story = {
  render: () => <Harness mode="add" fetchStub={createStub()} />,
  play: async () => {
    const canvas = screen();
    const kind = await canvas.findByRole("combobox", { name: "Kind" });
    await userEvent.click(kind);
    await userEvent.keyboard("claude");
    const listbox = await canvas.findByRole("listbox");
    await waitFor(() => expect(within(listbox).getAllByRole("option")).toHaveLength(1));
    await expect(within(listbox).getByRole("option")).toHaveTextContent("Anthropic");
    await userEvent.clear(kind);
    await userEvent.keyboard("openai_compat");
    await waitFor(() => expect(within(listbox).getAllByRole("option")).toHaveLength(1));
    await expect(within(listbox).getByRole("option")).toHaveTextContent("OpenAI-compatible");
  },
};

/** the names and descriptions are in the Russian catalog too */
export const KindNamesAreTranslated: Story = {
  globals: { locale: "ru" },
  render: () => (
    <Harness
      fetchStub={stub(async () => json(result()))}
      provider={ofKind("openai_compatible", "http://vllm.internal:8000")}
    />
  ),
  play: async () => {
    const canvas = screen();
    const kind = await canvas.findByRole("combobox", { name: ru.providerSheet.fields.kind });
    await expect(kind).toHaveValue("OpenAI-совместимый");
    await expect(kind).toHaveAccessibleDescription(/vLLM, TGI, LM Studio/);
    await expect(
      await canvas.findByText("http://vllm.internal:8000/v1/chat/completions"),
    ).toBeVisible();
    await expect(canvas.getByText(ru.providerSheet.apiBase.resolvesTo.chat)).toBeVisible();
  },
};

/**
 * The slug placeholder is what the control plane will derive from the name, so
 * the hint that says "derived from the name" shows the result instead of a
 * fixed example.
 */
export const SlugPlaceholderFollowsTheName: Story = {
  render: () => <Harness mode="add" fetchStub={createStub()} />,
  play: async () => {
    const canvas = screen();
    const slug = await canvas.findByLabelText("Slug (optional)");
    await expect(slug).toHaveAttribute("placeholder", "openai-primary");

    await userEvent.type(canvas.getByLabelText("Name"), "My vLLM Fleet (EU)");
    await expect(slug).toHaveAttribute("placeholder", "my-vllm-fleet-eu");
    // a placeholder is a preview, not a value: the field stays empty
    await expect(slug).toHaveValue("");

    // a name with nothing to derive from falls back to the example
    await userEvent.clear(canvas.getByLabelText("Name"));
    await userEvent.type(canvas.getByLabelText("Name"), "非");
    await expect(slug).toHaveAttribute("placeholder", "openai-primary");

    // typing a slug of one's own wins, and the placeholder is out of the way
    await userEvent.type(slug, "eu-fleet");
    await expect(slug).toHaveValue("eu-fleet");
  },
};

/**
 * Whose environment: the variable is read by the gateway for every request and
 * by the control plane for Test connection, and the hint says both.
 */
export const EnvVarHintNamesBothProcesses: Story = {
  render: () => <Harness mode="add" fetchStub={createStub()} />,
  play: async () => {
    const env = await screen().findByLabelText("Provider key env var (optional)");
    await expect(env).toHaveAccessibleDescription(/gateway's environment/);
    await expect(env).toHaveAccessibleDescription(/control plane's/);
    await expect(env).toHaveAccessibleDescription(/Test connection runs/);
  },
};

/** the test failure for a variable the control plane does not have says so and which process to fix */
export const UnsetEnvVarFailureNamesTheProcess: Story = {
  render: () => (
    <Harness
      fetchStub={stub(async () =>
        json(
          result({
            reachable: false,
            status: 401,
            models_found: null,
            credential: "env (unset)",
            error:
              "401: the upstream rejected the request, and the key's environment variable OPENAI_API_KEY is not set in the control plane's environment (resolved from: env (unset)). This test runs in the control plane, so set OPENAI_API_KEY there; the gateway reads it from its own environment, so set it there too.",
          }),
        ),
      )}
    />
  ),
  play: async () => {
    const canvas = screen();
    await press();
    await waitFor(() =>
      expect(canvas.getByText(/not set in the control plane's environment/)).toBeVisible(),
    );
    await expect(canvas.getByText(/the gateway reads it from its own environment/)).toBeVisible();
  },
};

/**
 * Until the control plane has said which endpoint the kind is sent to there is
 * no address to preview, and the sheet does not guess one: a chat-completions
 * guess is the wrong address for Anthropic and TEI. The picker still lists the
 * kinds from the bundled list.
 */
export const NoPreviewUntilTheKindIsKnown: Story = {
  render: () => (
    <Harness
      mode="add"
      fetchStub={async (input) => {
        if (String(input).includes("/provider-kinds")) return json({ error: "down" }, 500);
        return json({});
      }}
    />
  ),
  play: async () => {
    const canvas = screen();
    await userEvent.type(await canvas.findByLabelText("API base"), "https://gw.example.com");
    // the picker works from the bundled list, with display names
    await userEvent.click(canvas.getByRole("combobox", { name: "Kind" }));
    await expect(
      await within(await canvas.findByRole("listbox")).findByRole("option", { name: /^Anthropic/ }),
    ).toBeVisible();
    await userEvent.keyboard("{Escape}");
    await expect(canvas.queryByText(/requests resolve to/)).toBeNull();
    await expect(canvas.queryByText(/chat\/completions/)).toBeNull();
  },
};
