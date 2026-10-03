import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { MemoryRouter } from "react-router";
import { expect, fireEvent, spyOn, userEvent, waitFor, within } from "storybook/test";

import Playground from "./Playground";
import {
  Harness,
  NEEDS_MEMBER,
  clickWhenEnabled,
  expectAllowed,
  expectLoadError,
  expectRefused,
  expectSkeleton,
  json,
  recording,
  scopeResponse,
  StaleSession,
  type FetchStub,
  type StoryRole,
} from "./story-harness";
import { setKeyPropagationForTests, setPlaygroundKey } from "@/lib/gateway";
import en from "@/lib/i18n/locales/en.json";
import ru from "@/lib/i18n/locales/ru.json";
import { atMobile, expectNoHorizontalOverflow } from "@/lib/story-viewport";
import { UxScreenProvider } from "@/lib/ux-react";
import { expectUxEvent, recordUxEvents } from "@/pages/story-harness";

/** What the gateway serves: a route, a provider pin, and a provider group. */
const GATEWAY_MODELS = {
  data: [
    { id: "minicpm5-1b", object: "model", owned_by: "rolter" },
    { id: "fake-llm", object: "model", owned_by: "rolter" },
    { id: "gpustack/minicpm5-1b", object: "model", owned_by: "vllm-test" },
    { id: "abc/minicpm5-1b", object: "model", owned_by: "abc" },
  ],
};

/** What the control plane serves: bare route ids, nothing else. */
const ROUTES = [{ id: "r-1", model: "minicpm5-1b", strategy: "round_robin" }];

/**
 * The dogfood stack from #1853: the store lists a route the snapshot prunes,
 * and it sorts first, so a picker that trusts the store opens on it.
 */
const DOGFOOD_ROUTES = [
  { model: "claude-sonnet-4", strategy: "round_robin", targets: 0, source: "db" },
  { model: "minicpm5-1b", strategy: "round_robin", targets: 1, source: "db" },
];

/** What `/api/v1/config/problems` says about the route above. */
const DOGFOOD_PROBLEMS = {
  problems: [
    "route 'claude-sonnet-4' omitted from the snapshot: it has no target that references a known provider with a positive weight",
  ],
};

const MINT_PATH = "/playground-key";

/**
 * A column's Send button, which names the model it sends to (#2330). The model
 * is whatever the column opened on, so the stories that do not care which one
 * match the start of the name.
 */
const SEND = /^Send to /;

/** What a held-back Send says, read out of the catalog so rewording cannot strand the stories. */
const SEND_NEEDS_KEY = en.pages.playground.sendNeedsKey;
const SEND_WAITING = en.pages.playground.sendWaiting;

/** Every mint the screen asked for, from a recorder's calls. */
const mintsIn = (calls: { method: string; url: string }[]) =>
  calls.filter((c) => c.method === "POST" && c.url.includes(MINT_PATH)).length;

/** Half an hour out, the lifetime `PLAYGROUND_KEY_TTL_MINUTES` fixes. */
const expiry = () => new Date(Date.now() + 30 * 60_000).toISOString();

/** The row `POST /api/v1/me/projects/{id}/playground-key` answers with. */
const minted = (key = "sk-rolter-minted") => ({
  id: "vk-1",
  project_id: "project-1",
  key_hash: "hash",
  key_prefix: key.slice(0, 12),
  name: "Playground",
  models: ["minicpm5-1b"],
  providers: [],
  disabled: false,
  expires_at: expiry(),
  cache_enabled: null,
  created_by: "user-1",
  business_unit_id: null,
  customer_id: null,
  purpose: "playground",
  created_at: new Date().toISOString(),
  key,
});

/**
 * What the screen sent to the gateway, in order: the virtual key from
 * `x-rolter-gateway-key` and, beside it, the dashboard session from
 * `Authorization`, which the control plane's `/gw` proxy requires (#2486).
 */
interface Sent {
  keys: string[];
  sessions?: (string | null)[];
}

/** The body of a chat completion request, as the gateway reads it. */
interface ChatRequest {
  model: string;
  messages: { role: string; content: unknown }[];
  stream?: boolean;
}

/** The body the gateway answers a non-streaming chat completion with. */
const completion = (text: string) =>
  json({ choices: [{ index: 0, message: { role: "assistant", content: text } }] });

const CHAT_PATH = "/gw/v1/chat/completions";
const IMAGE_PATH = "/gw/v1/images/generations";
const EMBED_PATH = "/gw/v1/embeddings";
const SPEECH_PATH = "/gw/v1/audio/speech";
const TRANSCRIBE_PATH = "/gw/v1/audio/transcriptions";

/** A 1x1 png, so the images a story generates are real `data:` URLs the browser decodes. */
const PIXEL =
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

/** The body of an image generation request. */
interface ImageRequest {
  model: string;
  prompt: string;
  n: number;
  size: string;
}

/** The body of an embeddings request. */
interface EmbedRequest {
  model: string;
  input: string[];
}

/** What the gateway answers an image generation with: `n` pictures. */
const pictures = (request: ImageRequest) =>
  json({ data: Array.from({ length: request.n }, () => ({ b64_json: PIXEL })) });

/** What the gateway answers an embeddings request with: one deterministic vector per text. */
const vectors = (request: EmbedRequest) =>
  json({
    data: request.input.map((_, i) => ({
      embedding: Array.from({ length: 8 }, (_, d) => Math.sin((i + 1) * (d + 1))),
    })),
  });

/** What the gateway answers a speech request with: audio bytes, which the screen wraps in an object URL. */
const clip = () =>
  new Response(new Blob([new Uint8Array(64)], { type: "audio/wav" }), {
    status: 200,
    headers: { "Content-Type": "audio/wav" },
  });

/** Each request of a kind a recorder saw, parsed, in the order they went out. */
const bodiesAt = <T,>(calls: { method: string; url: string; body?: string }[], path: string): T[] =>
  calls
    .filter((c) => c.method === "POST" && c.url.includes(path))
    .map((c) => JSON.parse(c.body ?? "{}") as T);

/** The chat requests a recorder saw, parsed, in the order they went out. */
const chatsIn = (calls: { method: string; url: string; body?: string }[]): ChatRequest[] =>
  calls
    .filter((c) => c.method === "POST" && c.url.includes(CHAT_PATH))
    .map((c) => JSON.parse(c.body ?? "{}") as ChatRequest);

/** What the rest of the control plane and the gateway answer, per story. */
interface Upstream {
  /** the store's route list, `GET /api/v1/models` */
  routes?: unknown[];
  /** `GET /api/v1/config/problems` */
  problems?: () => Response;
  /**
   * The gateway's answer to its `n`th model-list call (from zero) made with a
   * key, so a story can refuse a key the gateway has not polled yet.
   */
  gateway?: (n: number) => Response;
  /** The gateway's answer to its `n`th chat completion (from zero). */
  chat?: (request: ChatRequest, n: number) => Response | Promise<Response>;
  /** The gateway's answer to its `n`th image generation (from zero). */
  image?: (request: ImageRequest, n: number) => Response | Promise<Response>;
  /** The gateway's answer to its `n`th embeddings request (from zero). */
  embed?: (request: EmbedRequest, n: number) => Response | Promise<Response>;
  /** The gateway's answer to its `n`th speech synthesis (from zero). */
  speech?: (n: number) => Response | Promise<Response>;
  /** The gateway's answer to its `n`th transcription (from zero). */
  transcription?: (n: number) => Response | Promise<Response>;
}

/**
 * The deployment the Playground normally opens against: a project in scope, a
 * mint endpoint that answers, and a gateway that serves its model list to the
 * key it was handed.
 *
 * `mint` is a function so a story can refuse, stall, or answer twice.
 */
function deployment(
  mint: () => Promise<Response>,
  sent: Sent = { keys: [] },
  projects: unknown[] = [{ id: "project-1", team_id: "team-1", name: "Gateway" }],
  upstream: Upstream = {},
): FetchStub {
  const {
    routes = ROUTES,
    problems = () => json({ problems: [] }),
    gateway = () => json(GATEWAY_MODELS),
    chat = () => completion("Hello from the gateway."),
    image = pictures,
    embed = vectors,
    speech = clip,
    transcription = () => json({ text: "The transcript the gateway heard." }),
  } = upstream;
  let gatewayCalls = 0;
  let chatCalls = 0;
  let imageCalls = 0;
  let embedCalls = 0;
  let speechCalls = 0;
  let transcriptionCalls = 0;
  return async (input, init) => {
    const url = String(input);
    const path = new URL(url, "http://localhost").pathname;
    if (/^\/api\/v1\/teams\/[^/]+\/projects$/.test(path)) return json(projects);
    const scope = scopeResponse(url);
    if (scope) return scope;
    if (url.includes(MINT_PATH)) return mint();
    if (path === "/api/v1/config/problems") return problems();
    if (url.includes("/gw/v1/models")) {
      const headers = new Headers(init?.headers);
      const key = headers.get("x-rolter-gateway-key");
      if (!key) return json({ error: { message: "missing key" } }, 401);
      sent.keys.push(key);
      sent.sessions?.push(headers.get("Authorization"));
      return gateway(gatewayCalls++);
    }
    if (url.includes(CHAT_PATH)) {
      return chat(JSON.parse(String(init?.body ?? "{}")) as ChatRequest, chatCalls++);
    }
    if (url.includes(IMAGE_PATH)) {
      return image(JSON.parse(String(init?.body ?? "{}")) as ImageRequest, imageCalls++);
    }
    if (url.includes(EMBED_PATH)) {
      return embed(JSON.parse(String(init?.body ?? "{}")) as EmbedRequest, embedCalls++);
    }
    if (url.includes(SPEECH_PATH)) return speech(speechCalls++);
    if (url.includes(TRANSCRIBE_PATH)) return transcription(transcriptionCalls++);
    return json(routes);
  };
}

/** The gateway's answer to a key that is not in its snapshot (yet). */
const unknownKey = () => json({ error: { message: "invalid api key" } }, 401);

/**
 * Shortens the wait for a minted key to reach the gateway, for a story that
 * has to see it run out. Ten real seconds would outlast every assertion.
 */
function shortKeyWait() {
  setKeyPropagationForTests({ budgetMs: 300, firstDelayMs: 50, maxDelayMs: 100 });
  return () => setKeyPropagationForTests(null);
}

/**
 * Clears the in-memory key, so one story's key is never another's start state.
 *
 * `role` mounts the screen under the capability gate as that role; left out,
 * no gate is mounted and every control renders enabled. The router is for the
 * routeless project's link to Routing Rules.
 */
function Screen({ fetchStub, role }: { fetchStub: FetchStub; role?: StoryRole }) {
  // during render, not in an effect: the screen's own effects run first, and
  // a key left behind would suppress the automatic mint under test
  React.useState(() => {
    setPlaygroundKey("");
    return null;
  });
  React.useEffect(() => () => setPlaygroundKey(""), []);
  return (
    <MemoryRouter>
      <Harness fetchStub={fetchStub} role={role}>
        <Playground />
      </Harness>
    </MemoryRouter>
  );
}

const meta = {
  title: "Screens/Playground",
  component: Playground,
  parameters: { layout: "fullscreen" },
  // every story starts from an empty UX queue and leaves one behind (#1730)
  beforeEach: recordUxEvents,
} satisfies Meta<typeof Playground>;

export default meta;
type Story = StoryObj<typeof meta>;

const firstMint = { keys: [] as string[] };
const mints = recording(deployment(async () => json(minted()), firstMint));

/**
 * Opening the screen mints a key, with no paste step (#944).
 *
 * The three things the issue asks for are asserted together: the request goes
 * out on its own, the key that comes back is what the gateway calls are
 * authenticated with, and the row says when it stops working.
 */
export const MintsAKeyOnOpen: Story = {
  render: () => <Screen fetchStub={mints.stub} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const rec = mints;
    const sent = firstMint;

    await rec.expectSent("POST", MINT_PATH);
    // no body: the control plane scopes the key itself, and a client that could
    // send `models` could ask for every model there is
    await waitFor(() =>
      expect(rec.calls.find((c) => c.url.includes(MINT_PATH))?.body).toBeUndefined(),
    );

    await waitFor(() => expect(canvas.getByText("Active")).toBeVisible());
    // the expiry is on screen, so a key that stops working is explicable
    await waitFor(() => expect(canvas.getByText(/Expires in \d+ min/)).toBeVisible());

    // and it is the minted key the gateway is called with
    await waitFor(() => expect(sent.keys).toContain("sk-rolter-minted"));

    // the model list is the gateway's own, which is only reachable with a key
    await userEvent.click(canvas.getByRole("combobox", { name: "Model" }));
    const listbox = canvas.getByRole("listbox");
    await waitFor(() =>
      expect(within(listbox).getByRole("option", { name: "abc/minicpm5-1b" })).toBeTruthy(),
    );
    await userEvent.keyboard("{Escape}");
  },
};

/**
 * The key is never written to browser storage (#944).
 *
 * Asserting the old `rolter.playground.key` entry is absent would pass against
 * code that writes a differently named one, so this records every write and
 * asserts none of them carried the secret.
 */
export const KeyNeverReachesLocalStorage: Story = {
  render: () => <Screen fetchStub={deployment(async () => json(minted()))} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const written: string[] = [];
    const original = Storage.prototype.setItem;
    Storage.prototype.setItem = function patched(this: Storage, key: string, value: string) {
      written.push(`${key}=${value}`);
      return original.call(this, key, value);
    };
    try {
      await waitFor(() => expect(canvas.getByText("Active")).toBeVisible());
      await expect(written.some((entry) => entry.includes("sk-rolter-minted"))).toBe(false);
      await expect(localStorage.getItem("rolter.playground.key")).toBeNull();
    } finally {
      Storage.prototype.setItem = original;
    }
  },
};

/**
 * The mint is a request like any other, so it holds a place while it is out —
 * the scope chain resolves, and only the mint stalls.
 */
export const MintingShowsProgress: Story = {
  render: () => <Screen fetchStub={deployment(() => new Promise<Response>(() => {}))} />,
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
  },
};

/**
 * A project with no routes cannot mint: an empty model list on a virtual key
 * means *every* model, so the control plane refuses rather than handing out the
 * widest key in the system (#2061).
 *
 * That refusal is a precondition, not a failure: the band says the project
 * needs a route and links the screen that makes one, instead of an unknown
 * error with the server's line and a retry that can never succeed. The paste
 * field opens, since pasting is the other way on, and Send waits for a key.
 */
const routeless = recording(
  deployment(async () =>
    json(
      {
        error: {
          message:
            "config error: this project has no routes, so there is nothing a playground key could address",
        },
      },
      400,
    ),
  ),
);

export const RoutelessProjectIsRefused: Story = {
  render: () => <Screen fetchStub={routeless.stub} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText(/This project has no routes yet/);
    const link = canvas.getByRole("link", { name: "Open Routing Rules" });
    await expect(link).toHaveAttribute("href", "/routing-rules");

    // not the unknown-failure alert, and no retry of a refusal a retry cannot
    // clear
    await expect(canvas.queryByRole("alert")).toBeNull();
    await expect(canvas.queryByRole("button", { name: "Try again" })).toBeNull();
    await expect(canvas.queryByText(/nothing a playground key could address/)).toBeNull();
    // one message: nothing about a minted key that does not exist
    await expect(canvas.queryByText(/mints this key when you open/)).toBeNull();

    // the paste field is open without being asked for
    await expect(canvas.getByLabelText("Virtual key")).toBeVisible();
    // one automatic attempt, then it is the operator's call
    await expect(mintsIn(routeless.calls)).toBe(1);
    await waitFor(() => {
      const send = canvas.getByRole("button", { name: SEND });
      expect(send).toBeDisabled();
      expect(send).toHaveAttribute("title", SEND_NEEDS_KEY);
    });
  },
};

/**
 * Minting takes `my_virtual_key:create`, which a viewer does not hold (#2061).
 * The screen asks the gate before it mints, so a viewer is never sent into a
 * refusal on arrival: the button says which role it takes, the paste field is
 * open with one line saying why, and a pasted key is what unlocks Send.
 */
const viewerCalls = recording(deployment(async () => json(minted())));

export const ViewerIsOfferedThePasteField: Story = {
  render: () => <Screen role="viewer" fetchStub={viewerCalls.stub} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectRefused(canvasElement, "Mint key", NEEDS_MEMBER);
    await canvas.findByText(/Your role in this project cannot mint keys/);
    // the gate has answered by now, and the mint it refused never left
    viewerCalls.expectNotSent("POST", MINT_PATH);
    await expect(canvas.queryByText(/mints this key when you open/)).toBeNull();

    const field = canvas.getByLabelText("Virtual key");
    await expect(field).toBeVisible();
    await waitFor(() => {
      const send = canvas.getByRole("button", { name: SEND });
      expect(send).toBeDisabled();
      expect(send).toHaveAttribute("title", SEND_NEEDS_KEY);
    });

    await userEvent.type(field, "sk-rolter-given");
    await userEvent.click(canvas.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(canvas.getByText("Pasted")).toBeVisible());
    await expectAllowed(canvasElement, SEND);
  },
};

/** The gate only holds the mint back on a "no": a member still arrives with a key. */
const memberCalls = recording(deployment(async () => json(minted())));

export const MemberMintsOnceTheGateAnswers: Story = {
  render: () => <Screen role="member" fetchStub={memberCalls.stub} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await memberCalls.expectSent("POST", MINT_PATH);
    await waitFor(() => expect(canvas.getByText("Active")).toBeVisible());
    await expectAllowed(canvasElement, "Renew key");
    await expect(mintsIn(memberCalls.calls)).toBe(1);
  },
};

/** No session, no minting: signing in is the fix, and the state says so. */
export const SignedOutCannotMint: Story = {
  render: () => (
    <Screen fetchStub={deployment(async () => json({ error: { message: "unauthorized" } }, 401))} />
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /Sign in to see the playground key/);
  },
};

/** Renewing asks for a fresh key rather than extending the one in hand. */
let issued = 0;
const renewed = { keys: [] as string[] };
const renewals = recording(
  deployment(async () => {
    issued += 1;
    return json(minted(`sk-rolter-${issued}`));
  }, renewed),
);

export const RenewMintsAgain: Story = {
  render: () => <Screen fetchStub={renewals.stub} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const rec = renewals;
    const sent = renewed;

    await waitFor(() => expect(canvas.getByText("Active")).toBeVisible());
    await waitFor(() => expect(sent.keys).toContain("sk-rolter-1"));
    // one automatic mint, not two: the screen must not re-mint on its own
    await expect(
      rec.calls.filter((c) => c.method === "POST" && c.url.includes(MINT_PATH)).length,
    ).toBe(1);

    await clickWhenEnabled(canvasElement, "Renew key");
    await waitFor(() =>
      expect(rec.calls.filter((c) => c.method === "POST" && c.url.includes(MINT_PATH)).length).toBe(
        2,
      ),
    );
    // the second key is the one being sent — a renew that left the old key in
    // place would keep every assertion above passing
    await waitFor(() => expect(sent.keys).toContain("sk-rolter-2"));
  },
};

/**
 * The paste field stays, for testing one specific key on purpose — the case
 * automatic minting cannot serve.
 */
const pastedInto = { keys: [] as string[] };

export const PastedKeyOverridesTheMintedOne: Story = {
  render: () => <Screen fetchStub={deployment(async () => json(minted()), pastedInto)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const sent = pastedInto;
    await waitFor(() => expect(canvas.getByText("Active")).toBeVisible());

    await userEvent.click(canvas.getByRole("button", { name: "Use a specific key" }));
    const field = await canvas.findByLabelText("Virtual key");
    await userEvent.type(field, "sk-rolter-mine");
    await userEvent.click(canvas.getByRole("button", { name: "Save" }));

    // the confirmation is a drawn icon beside the word, not a glyph in it
    const saved = await canvas.findByRole("button", { name: "Saved" });
    await expect(saved.querySelector("svg")).not.toBeNull();
    await expect(saved).not.toHaveTextContent("✓");

    // the badge stops claiming a lifetime rolter chose, because it did not
    await waitFor(() => expect(canvas.getByText("Pasted")).toBeVisible());
    await expect(canvas.queryByText(/Expires in \d+ min/)).toBeNull();
    await waitFor(() => expect(sent.keys).toContain("sk-rolter-mine"));
  },
};

/**
 * No project in scope, nothing to mint against. The band says so once — not
 * beside a hint about a minted key that does not exist (#2061) — the button
 * offers to mint rather than to renew, and the paste field is open. The picker
 * falls back to the control plane's route list with a notice explaining what
 * is missing from it (#946), which describes the list without repeating the
 * band's instruction.
 */
export const NoProjectSaysWhatIsMissing: Story = {
  render: () => <Screen fetchStub={deployment(async () => json(minted()), undefined, [])} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() =>
      expect(canvas.getByText(/Pick a project to mint a key against/)).toBeVisible(),
    );
    await expect(canvas.queryByText(/mints this key when you open/)).toBeNull();
    await expect(canvas.queryByText(/Set a virtual key above/)).toBeNull();
    await expect(canvas.getByRole("button", { name: "Mint key" })).toBeDisabled();
    await expect(canvas.queryByRole("button", { name: "Renew key" })).toBeNull();
    await expect(canvas.getByLabelText("Virtual key")).toBeVisible();

    await waitFor(() => expect(canvas.getByText(/Showing configured routes/)).toBeVisible());
    await userEvent.click(canvas.getByRole("combobox", { name: "Model" }));
    const listbox = canvas.getByRole("listbox");
    // the picker stays usable rather than emptying out
    await expect(within(listbox).getByRole("option", { name: "fake-llm" })).toBeTruthy();
    // and the gateway-only addresses are genuinely absent, as the notice says
    await expect(within(listbox).queryByRole("option", { name: "abc/minicpm5-1b" })).toBeNull();
    await userEvent.keyboard("{Escape}");
  },
};

/**
 * A key the gateway rejects is a different message from no key at all: one is
 * "you have not set one", the other "the one you set did not work".
 */
export const RejectedKeySaysSo: Story = {
  // a minted key's 401 is waited out first (#1853); this one never clears
  beforeEach: shortKeyWait,
  render: () => (
    <Screen
      fetchStub={async (input) => {
        const url = String(input);
        const scope = scopeResponse(url);
        if (scope) return scope;
        if (url.includes(MINT_PATH)) return json(minted("sk-rolter-stale"));
        if (url.includes("/config/problems")) return json({ problems: [] });
        if (url.includes("/gw/v1/models")) return json({ error: { message: "invalid key" } }, 401);
        return json(ROUTES);
      }}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() =>
      expect(canvas.getByText(/Could not read the gateway's model list/)).toBeVisible(),
    );
    await expect(canvas.queryByText(/Showing configured routes/)).toBeNull();

    // the badge follows the gateway's verdict, not the mint's (#2061): no green
    // "Active" beside a key the gateway turned down
    await expect(canvas.getByText("Rejected")).toBeVisible();
    await expect(canvas.queryByText("Active")).toBeNull();
    await expect(canvas.queryByText(/Expires in/)).toBeNull();
    await expect(canvas.getByText(/The gateway refused this key/)).toBeVisible();
    await expect(canvas.getByLabelText("Virtual key")).toBeVisible();

    const send = canvas.getByRole("button", { name: SEND });
    await expect(send).toBeDisabled();
    await expect(send).toHaveAttribute("title", SEND_NEEDS_KEY);
  },
};

/**
 * Send waits for a key the gateway accepts (#2061). Before, it stayed enabled
 * and the first message came back as the gateway's `401`. Enter in the composer
 * is held back too, since it reaches the send without the button.
 */
const unkeyed = recording(deployment(async () => json(minted()), undefined, []));

export const SendWaitsForAKey: Story = {
  render: () => <Screen fetchStub={unkeyed.stub} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => {
      const send = canvas.getByRole("button", { name: SEND });
      expect(send).toBeDisabled();
      expect(send).toHaveAttribute("title", SEND_NEEDS_KEY);
    });
    // the column follows the route list here, so the model is whatever it opened on
    await expect(
      canvas.getByText(/^Send a message to \S+ once there is a key the gateway accepts\.$/),
    ).toBeVisible();

    await userEvent.type(canvas.getByPlaceholderText("Message…"), "hello{Enter}");
    unkeyed.expectNotSent("POST", "/gw/v1/chat/completions");

    // a key the gateway takes is what lets it through
    await userEvent.type(canvas.getByLabelText("Virtual key"), "sk-rolter-mine");
    await userEvent.click(canvas.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(canvas.getByRole("button", { name: SEND })).toBeEnabled());
    await expect(canvas.getByRole("button", { name: SEND })).not.toHaveAttribute("title");
    await expect(canvas.getByText(/^Send a message to \S+\.$/)).toBeVisible();
  },
};

/**
 * The no-database `rolter easy-up`: the gateway holds no keys and no control
 * plane manages it, so it serves anybody, and every `/api/v1/*` route answers
 * the JSON 404 of a control plane with no store. The screen asks the gateway
 * once without a key, and on a yes it says so and leaves Send open, rather
 * than holding back a request the gateway would take (#2061).
 */
const noStore = () =>
  json({ error: { message: "no such endpoint", code: "no_such_endpoint" } }, 404);

export const KeylessGatewayNeedsNoKey: Story = {
  render: () => (
    <Screen
      fetchStub={async (input) => {
        const url = String(input);
        if (url.includes("/gw/v1/models")) return json(GATEWAY_MODELS);
        return noStore();
      }}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText(/This gateway takes requests without a key/);
    await expect(canvas.getByText("No key")).toBeVisible();
    await waitFor(() => expect(canvas.getByRole("button", { name: SEND })).toBeEnabled());
    // the list is the gateway's own, so there is no fallback to explain
    await expect(canvas.queryByText(/Showing configured routes/)).toBeNull();
    await expect(canvas.queryByText(/Pick a project to mint a key against/)).toBeNull();
  },
};

/**
 * The /gw proxy refuses a call without the dashboard session, and the gateway
 * only knows the virtual key, so the screen sends both on separate headers
 * (#2486): the key is never `Authorization`, and a signed-in tab's token is.
 */
const withSession = { keys: [] as string[], sessions: [] as (string | null)[] };
export const SendsTheSessionBesideTheKey: Story = {
  render: () => (
    <StaleSession token="session-abc">
      <Screen fetchStub={deployment(async () => json(minted()), withSession)} />
    </StaleSession>
  ),
  play: async () => {
    await waitFor(() => expect(withSession.keys).toContain("sk-rolter-minted"));
    await expect(withSession.sessions.every((a) => a === "Bearer session-abc")).toBe(true);
  },
};

/**
 * The race from #1853. The mint answers the moment the row is written, but the
 * gateway only learns the key on its next snapshot poll, so the first calls
 * made with it are refused. The screen says it is waiting, asks again, and
 * lands on the gateway's own list — rather than falling back for good to a
 * store list whose first entry is a route the gateway does not serve.
 */
const racing = { keys: [] as string[] };

export const WaitsForTheMintedKeyToGoLive: Story = {
  beforeEach: () => {
    racing.keys = [];
  },
  render: () => (
    <Screen
      fetchStub={deployment(async () => json(minted()), racing, undefined, {
        routes: DOGFOOD_ROUTES,
        problems: () => json(DOGFOOD_PROBLEMS),
        // three refusals keep the waiting state up for well over a second
        gateway: (n) => (n < 3 ? unknownKey() : json(GATEWAY_MODELS)),
      })}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const sent = racing;

    // while the key is on its way the notice says so, not that the list failed
    await canvas.findByText(/Asking the gateway which models this key can use/);
    await expect(canvas.queryByText(/Could not read the gateway's model list/)).toBeNull();
    // and Send says it is waiting, rather than letting a message meet the 401
    await expect(canvas.getByRole("button", { name: SEND })).toHaveAttribute("title", SEND_WAITING);

    // the same key, asked again until the gateway took it
    await waitFor(() => expect(sent.keys.length).toBe(4));
    await expect(new Set(sent.keys)).toEqual(new Set(["sk-rolter-minted"]));

    // the notice clears once the gateway's own list is in
    await waitFor(() =>
      expect(canvas.queryByText(/Asking the gateway which models this key can use/)).toBeNull(),
    );
    await expect(canvas.queryByText(/Could not read the gateway's model list/)).toBeNull();
    await waitFor(() => expect(canvas.getByRole("button", { name: SEND })).toBeEnabled());

    // and the chat column opens on a route the gateway serves, not on the
    // unservable one the store sorts first
    const picker = canvas.getByRole("combobox", { name: "Model" });
    await waitFor(() => expect(picker).toHaveValue("minicpm5-1b"));
    await userEvent.click(picker);
    const listbox = canvas.getByRole("listbox");
    await expect(within(listbox).getByRole("option", { name: "abc/minicpm5-1b" })).toBeTruthy();
    await expect(within(listbox).queryByRole("option", { name: "claude-sonnet-4" })).toBeNull();
    await userEvent.keyboard("{Escape}");
  },
};

/**
 * A key the gateway never accepts ends in the fallback, after a bounded wait.
 * The fallback is the store's route list with the routes the snapshot prunes
 * left out, the notice counts them, and the column opens on one that is served.
 */
const neverLive = { keys: [] as string[] };

export const FallbackLeavesOutUnservedRoutes: Story = {
  beforeEach: () => {
    neverLive.keys = [];
    return shortKeyWait();
  },
  render: () => (
    <Screen
      fetchStub={deployment(async () => json(minted()), neverLive, undefined, {
        routes: DOGFOOD_ROUTES,
        problems: () => json(DOGFOOD_PROBLEMS),
        gateway: unknownKey,
      })}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText(/Could not read the gateway's model list/);
    await expect(
      canvas.getByText(/1 configured route is left out because the gateway does not serve it/),
    ).toBeVisible();
    // it did ask again before giving up, and it did give up
    await expect(neverLive.keys.length).toBeGreaterThan(1);

    const picker = canvas.getByRole("combobox", { name: "Model" });
    await waitFor(() => expect(picker).toHaveValue("minicpm5-1b"));
    await userEvent.click(picker);
    const listbox = canvas.getByRole("listbox");
    await expect(within(listbox).getByRole("option", { name: "minicpm5-1b" })).toBeTruthy();
    await expect(within(listbox).queryByRole("option", { name: "claude-sonnet-4" })).toBeNull();
    await userEvent.keyboard("{Escape}");
  },
};

/**
 * With no word on which routes are pruned, any route in the fallback may be a
 * dead one, so the column stays on the built-in rather than guessing.
 */
export const NoPickWithoutTheProblemList: Story = {
  beforeEach: shortKeyWait,
  render: () => (
    <Screen
      fetchStub={deployment(async () => json(minted()), undefined, undefined, {
        routes: DOGFOOD_ROUTES,
        problems: () => json({ error: { message: "store unavailable" } }, 503),
        gateway: unknownKey,
      })}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText(/Could not read the gateway's model list/);
    await expect(canvas.getByRole("combobox", { name: "Model" })).toHaveValue("fake-llm");
    // the gateway refused the key, so the column waits for one it accepts
    await expect(
      canvas.getByText("Send a message to fake-llm once there is a key the gateway accepts."),
    ).toBeVisible();
  },
};

/**
 * The chat composer once the screen holds a key the gateway took: the column
 * follows the gateway's list to its first route, and Send opens with it.
 */
async function readyComposer(canvas: ReturnType<typeof within>): Promise<HTMLElement> {
  const composer = await canvas.findByRole("textbox", { name: "Message to minicpm5-1b" });
  await waitFor(() => expect(canvas.getByRole("button", { name: SEND })).toBeEnabled());
  return composer;
}

/** Types `text` into the composer and presses the Send button. */
async function sendMessage(canvas: ReturnType<typeof within>, composer: HTMLElement, text: string) {
  await userEvent.type(composer, text);
  await userEvent.click(canvas.getByRole("button", { name: SEND }));
}

/**
 * A finished reply is a labelled turn, and it is announced once (#2062).
 *
 * The roles are words in the dashboard's language rather than the `user` /
 * `assistant` enum. The reply is read out through a polite live region that is
 * not the thread, so the message just typed is not read back, and what goes to
 * the gateway is still the one user turn: multi-turn context is #2058's call.
 */
const replied = recording(
  deployment(async () => json(minted()), undefined, undefined, {
    chat: () => completion("The gateway answered."),
  }),
);

export const ReplyIsLabelledAndAnnounced: Story = {
  render: () => <Screen fetchStub={replied.stub} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const composer = await readyComposer(canvas);
    // nothing is read out before there is a reply
    await expect(canvas.queryByText(/ replied: /)).toBeNull();

    await sendMessage(canvas, composer, "Hello there");

    await waitFor(() => expect(canvas.getByText("The gateway answered.")).toBeVisible());
    await expect(canvas.getByText("You")).toBeVisible();
    await expect(canvas.getByText("Assistant")).toBeVisible();
    await expect(canvas.queryByText("user")).toBeNull();
    await expect(canvas.queryByText("assistant")).toBeNull();

    const announcement = await canvas.findByText("minicpm5-1b replied: The gateway answered.");
    const region = announcement.closest("[aria-live]");
    await expect(region).toHaveAttribute("aria-live", "polite");
    await expect(region).toHaveAttribute("aria-atomic", "true");
    await expect(region).not.toHaveTextContent("Hello there");

    await expect(chatsIn(replied.calls)).toEqual([
      { model: "minicpm5-1b", messages: [{ role: "user", content: "Hello there" }], stream: false },
    ]);
    await expect(composer).toHaveValue("");
  },
};

/** The roles, the composer and the announcement are words in the dashboard's language, not English. */
export const ReplyIsLabelledInRussian: Story = {
  globals: { locale: "ru" },
  render: () => (
    <Screen
      fetchStub={deployment(async () => json(minted()), undefined, undefined, {
        chat: () => completion("Шлюз ответил."),
      })}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const pg = ru.pages.playground;
    const composer = await canvas.findByRole("textbox", {
      name: pg.messageAria.replace("{{model}}", "minicpm5-1b"),
    });
    const send = pg.sendTo.replace("{{model}}", "minicpm5-1b");
    await waitFor(() => expect(canvas.getByRole("button", { name: send })).toBeEnabled());
    await userEvent.type(composer, "Привет");
    await userEvent.click(canvas.getByRole("button", { name: send }));

    await waitFor(() => expect(canvas.getByText("Шлюз ответил.")).toBeVisible());
    await expect(canvas.getByText(pg.roles.user)).toBeVisible();
    await expect(canvas.getByText(pg.roles.assistant)).toBeVisible();
    await expect(canvas.queryByText("user")).toBeNull();
    await expect(canvas.queryByText("assistant")).toBeNull();
    await canvas.findByText(
      pg.replyAnnounce.replace("{{model}}", "minicpm5-1b").replace("{{reply}}", "Шлюз ответил."),
    );
  },
};

/**
 * A reply is markdown, rendered, until the operator asks for the characters
 * the model sent (#955). The same reply in a column must survive both views.
 */
const MARKDOWN_REPLY = [
  "## Plan",
  "",
  "- **first** step with `rolter` inline",
  "- second step",
  "",
  "```bash",
  "curl /gw/v1/models",
  "```",
].join("\n");

export const ReplyRendersMarkdown: Story = {
  render: () => (
    <Screen
      fetchStub={deployment(async () => json(minted()), undefined, undefined, {
        chat: () => completion(MARKDOWN_REPLY),
      })}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const composer = await readyComposer(canvas);
    await sendMessage(canvas, composer, "Plan it");

    await canvas.findByRole("heading", { name: "Plan" });
    await expect(canvas.getAllByRole("listitem")).toHaveLength(2);
    await expect(canvasElement.querySelector("strong")).toHaveTextContent("first");
    await expect(
      canvas.getByRole("region", { name: new RegExp(`^${en.markdown.codeBlock}`) }),
    ).toBeVisible();

    // raw is the characters, markers and all, and it toggles back
    const toggle = canvas.getByRole("button", { name: "Show raw text" });
    await userEvent.click(toggle);
    await expect(toggle).toHaveAttribute("aria-pressed", "true");
    // the toggle is the column's, so the question is shown raw too
    await waitFor(() =>
      expect([...canvasElement.querySelectorAll("pre")].map((pre) => pre.textContent)).toEqual([
        "Plan it",
        MARKDOWN_REPLY,
      ]),
    );
    await expect(canvas.queryByRole("heading", { name: "Plan" })).toBeNull();

    await userEvent.click(toggle);
    await canvas.findByRole("heading", { name: "Plan" });
  },
};

/**
 * A gateway refusal lands in an `ErrorNote`, announced as an alert. The turn
 * that failed leaves its question in the thread, the placeholder for the reply
 * goes, Send opens again, and the next send clears the note.
 */
export const FailedSendShowsTheError: Story = {
  render: () => (
    <Screen
      fetchStub={deployment(async () => json(minted()), undefined, undefined, {
        chat: (_, n) =>
          n === 0
            ? json({ error: { message: "upstream unavailable: no healthy target" } }, 503)
            : completion("Back online."),
      })}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const composer = await readyComposer(canvas);
    await sendMessage(canvas, composer, "Are you there?");

    const alert = await canvas.findByRole("alert");
    await expect(alert).toHaveTextContent("upstream unavailable: no healthy target");
    await expect(canvas.getByText("Are you there?")).toBeVisible();
    await expect(canvas.queryByText("…")).toBeNull();
    await expect(canvas.queryByText("Assistant")).toBeNull();
    await expect(canvas.queryByText(/ replied: /)).toBeNull();
    await waitFor(() => expect(canvas.getByRole("button", { name: SEND })).toBeEnabled());

    await sendMessage(canvas, composer, "Again");
    await canvas.findByText("Back online.");
    await expect(canvas.queryByRole("alert")).toBeNull();
  },
};

/**
 * Enter sends and Shift+Enter adds a line (#2062). The composer grows with its
 * draft and goes back to one line once the message is out; the message that
 * leaves keeps its newline.
 */
const keys = recording(
  deployment(async () => json(minted()), undefined, undefined, {
    chat: () => completion("Two lines received."),
  }),
);

export const EnterSendsShiftEnterAddsALine: Story = {
  render: () => <Screen fetchStub={keys.stub} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const composer = await readyComposer(canvas);
    const oneLine = composer.offsetHeight;

    await userEvent.type(composer, "first line{Shift>}{Enter}{/Shift}second line");
    await expect(composer).toHaveValue("first line\nsecond line");
    keys.expectNotSent("POST", CHAT_PATH);
    await waitFor(() => expect(composer.offsetHeight).toBeGreaterThan(oneLine));

    await userEvent.keyboard("{Enter}");
    await canvas.findByText("Two lines received.");
    await expect(chatsIn(keys.calls)).toHaveLength(1);
    await expect(chatsIn(keys.calls)[0].messages).toEqual([
      { role: "user", content: "first line\nsecond line" },
    ]);
    // a send leaves the box empty and one line tall, with no newline left in it
    await expect(composer).toHaveValue("");
    await waitFor(() => expect(composer.offsetHeight).toBe(oneLine));
  },
};

/**
 * The Enter that confirms an input-method candidate is part of the
 * composition and sends nothing (#2062). Browsers report it as `isComposing`,
 * and Safari as keyCode 229 once the composition has ended.
 */
const composing = recording(
  deployment(async () => json(minted()), undefined, undefined, {
    chat: () => completion("Received."),
  }),
);

export const EnterDuringCompositionDoesNotSend: Story = {
  render: () => <Screen fetchStub={composing.stub} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const composer = await readyComposer(canvas);
    await userEvent.type(composer, "こんにちは");

    fireEvent.keyDown(composer, { key: "Enter", code: "Enter", isComposing: true });
    fireEvent.keyDown(composer, { key: "Enter", code: "Enter", keyCode: 229 });
    composing.expectNotSent("POST", CHAT_PATH);
    await expect(composer).toHaveValue("こんにちは");

    // the Enter after the composition is the send
    await userEvent.keyboard("{Enter}");
    await canvas.findByText("Received.");
    await expect(chatsIn(composing.calls)).toHaveLength(1);
    await expect(chatsIn(composing.calls)[0].messages[0].content).toBe("こんにちは");
  },
};

/**
 * Switching mode tabs keeps the conversation (#2062). The thread, the unsent
 * draft and the column itself are still there on the way back, and going away
 * and back sends nothing.
 */
const tabs = recording(
  deployment(async () => json(minted()), undefined, undefined, {
    chat: () => completion("Kept across tabs."),
  }),
);

export const TabSwitchKeepsTheThread: Story = {
  render: () => <Screen fetchStub={tabs.stub} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const composer = await readyComposer(canvas);
    await sendMessage(canvas, composer, "Remember this");
    await canvas.findByText("Kept across tabs.");
    await userEvent.type(composer, "a draft I have not sent");

    await userEvent.click(canvas.getByRole("tab", { name: "Embeddings" }));
    // the chat is out of the page for everyone, not only out of sight
    await waitFor(() => expect(canvas.queryByRole("textbox", { name: /^Message to / })).toBeNull());
    await canvas.findByRole("textbox", { name: "Text 1" });

    await userEvent.click(canvas.getByRole("tab", { name: "Chat" }));
    await waitFor(() => expect(canvas.getByText("Kept across tabs.")).toBeVisible());
    await expect(canvas.getByText("Remember this")).toBeVisible();
    await expect(canvas.getByRole("textbox", { name: "Message to minicpm5-1b" })).toHaveValue(
      "a draft I have not sent",
    );
    await expect(chatsIn(tabs.calls)).toHaveLength(1);
  },
};

/**
 * A reply still on its way when the operator leaves the tab is not lost (#2062):
 * it lands in its column, and is read out, once it is whole.
 */
let release: () => void = () => {};
const away = recording(
  deployment(async () => json(minted()), undefined, undefined, {
    chat: () =>
      new Promise<Response>((resolve) => {
        release = () => resolve(completion("Late answer."));
      }),
  }),
);

export const ReplyLandsWhileOnAnotherTab: Story = {
  render: () => <Screen fetchStub={away.stub} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const composer = await readyComposer(canvas);
    await sendMessage(canvas, composer, "Take your time");
    // the reply's place is held, and nothing is read out for a placeholder
    await canvas.findByText("…");
    await expect(canvas.queryByText(/ replied: /)).toBeNull();

    await userEvent.click(canvas.getByRole("tab", { name: "Image" }));
    await canvas.findByRole("textbox", { name: "Image prompt" });
    release();
    await userEvent.click(canvas.getByRole("tab", { name: "Chat" }));

    await waitFor(() => expect(canvas.getByText("Late answer.")).toBeVisible());
    await canvas.findByText("minicpm5-1b replied: Late answer.");
    await expect(canvas.queryByText("…")).toBeNull();
    await expect(chatsIn(away.calls)).toHaveLength(1);
  },
};

/**
 * Fills the `{{…}}` placeholders of a catalog string, for the stories that read
 * the Russian copy back out of the catalog rather than writing it out again.
 */
const fill = (template: string, values: Record<string, string | number>) =>
  template.replace(/\{\{(\w+)\}\}/g, (_, key: string) => String(values[key]));

/** Every control the query finds, asserting that no two of them answer to the same name. */
function expectDistinctNames(controls: HTMLElement[], count: number) {
  expect(controls).toHaveLength(count);
  const names = controls.map((el) => el.getAttribute("aria-label"));
  expect(new Set(names).size).toBe(count);
}

/**
 * A reply taller than the column scrolls inside the thread, and the thread is a
 * tab stop (#2330). A prose reply holds nothing focusable, so without this a
 * keyboard user cannot read past the fold: the region is named for its column,
 * Tab lands on it straight after the column's own controls, and the next Tab
 * leaves it for the composer rather than stopping there.
 */
const LONG_REPLY = Array.from(
  { length: 40 },
  (_, i) => `Paragraph ${i + 1} of a long answer.`,
).join("\n\n");

export const ThreadIsAKeyboardStop: Story = {
  render: () => (
    <Screen
      fetchStub={deployment(async () => json(minted()), undefined, undefined, {
        chat: () => completion(LONG_REPLY),
      })}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const composer = await readyComposer(canvas);
    // an empty thread is already a region: it is where the first reply lands
    const thread = canvas.getByRole("region", { name: "Conversation with minicpm5-1b" });
    await expect(thread).toHaveAttribute("tabindex", "0");

    await sendMessage(canvas, composer, "Go on");
    await canvas.findByText("Paragraph 40 of a long answer.");
    // the reply outruns the column, which is the case that needs the keyboard
    await waitFor(() => expect(thread.scrollHeight).toBeGreaterThan(thread.clientHeight));

    canvas.getByRole("button", { name: "Show raw text" }).focus();
    await userEvent.tab();
    await expect(thread).toHaveFocus();
    await userEvent.tab();
    await expect(composer).toHaveFocus();
  },
};

/**
 * Every column repeats its buttons and its thread, so each name says which
 * column it is for (#2330): the model, and in a compare view the place too,
 * since two columns can hold the same model. A lone column is just its model
 * and has no Remove.
 */
export const RepeatedButtonsNameTheirColumn: Story = {
  render: () => <Screen fetchStub={deployment(async () => json(minted()))} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await readyComposer(canvas);

    await canvas.findByRole("button", { name: "Send to minicpm5-1b" });
    await canvas.findByRole("button", { name: "Copy as code for minicpm5-1b" });
    await canvas.findByRole("region", { name: "Conversation with minicpm5-1b" });
    await expect(canvas.queryByRole("button", { name: /^Remove column/ })).toBeNull();

    await userEvent.click(canvas.getByRole("button", { name: "Add model" }));
    // the second column opens on the next model in the list
    for (const [n, model] of [
      [1, "minicpm5-1b"],
      [2, "fake-llm"],
    ] as const) {
      await canvas.findByRole("button", { name: `Send to ${model}, column ${n}` });
      await canvas.findByRole("button", { name: `Copy as code for ${model}, column ${n}` });
      await canvas.findByRole("button", { name: `Remove column ${n} (${model})` });
      await canvas.findByRole("region", { name: `Conversation with ${model}, column ${n}` });
    }
    // the lone column's names went with it rather than staying beside the new ones
    await expect(canvas.queryByRole("button", { name: "Send to minicpm5-1b" })).toBeNull();

    // the same model twice is a fair comparison, and the names still tell the
    // columns apart
    await userEvent.click(canvas.getAllByRole("combobox", { name: "Model" })[1]);
    await userEvent.click(
      within(canvas.getByRole("listbox")).getByRole("option", { name: "minicpm5-1b" }),
    );
    await canvas.findByRole("button", { name: "Send to minicpm5-1b, column 2" });
    expectDistinctNames(canvas.getAllByRole("button", { name: SEND }), 2);
    expectDistinctNames(canvas.getAllByRole("button", { name: /^Copy as code for / }), 2);
    expectDistinctNames(canvas.getAllByRole("button", { name: /^Remove column / }), 2);
    expectDistinctNames(canvas.getAllByRole("region", { name: /^Conversation with / }), 2);

    // a Remove acts on the column it names: the survivor is column 1 again
    await userEvent.click(canvas.getByRole("button", { name: "Remove column 1 (minicpm5-1b)" }));
    await waitFor(() =>
      expect(canvas.queryByRole("button", { name: /^Remove column / })).toBeNull(),
    );
    await canvas.findByRole("button", { name: "Send to minicpm5-1b" });
  },
};

/** The same names in the dashboard's other language, read from its catalog. */
export const RepeatedButtonNamesInRussian: Story = {
  globals: { locale: "ru" },
  render: () => <Screen fetchStub={deployment(async () => json(minted()))} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const pg = ru.pages.playground;
    const model = "minicpm5-1b";
    await canvas.findByRole("button", { name: fill(pg.sendTo, { model }) });
    await userEvent.click(canvas.getByRole("button", { name: pg.addModel }));

    const first = fill(pg.columnName, { model, n: 1 });
    const second = fill(pg.columnName, { model: "fake-llm", n: 2 });
    await canvas.findByRole("button", { name: fill(pg.sendTo, { model: first }) });
    await canvas.findByRole("button", { name: fill(pg.sendTo, { model: second }) });
    await canvas.findByRole("button", { name: fill(pg.copyAsCodeFor, { model: second }) });
    await canvas.findByRole("button", { name: fill(pg.removeColumn, { model: "fake-llm", n: 2 }) });
    await canvas.findByRole("region", { name: fill(pg.threadAria, { model: first }) });

    await userEvent.click(canvas.getByRole("tab", { name: pg.modes.embeddings }));
    await canvas.findByRole("button", { name: fill(pg.removeText, { n: 3 }) });
  },
};

/**
 * An embedding row's Remove names the row by its number, so six of them are
 * not six identical buttons (#2330), and each acts on the row it names.
 */
export const RemoveTextButtonsNameTheirRow: Story = {
  render: () => <Screen fetchStub={deployment(async () => json(minted()))} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await readyComposer(canvas);
    await userEvent.click(canvas.getByRole("tab", { name: "Embeddings" }));

    for (const n of [1, 2, 3, 4, 5, 6]) {
      await canvas.findByRole("button", { name: `Remove text ${n}` });
    }
    expectDistinctNames(canvas.getAllByRole("button", { name: /^Remove text / }), 6);

    const second = canvas.getByRole("textbox", { name: "Text 2" });
    const third = canvas.getByRole("textbox", { name: "Text 3" });
    await userEvent.clear(second);
    await userEvent.type(second, "the row I remove");
    await userEvent.clear(third);
    await userEvent.type(third, "the row that moves up");

    await userEvent.click(canvas.getByRole("button", { name: "Remove text 2" }));
    // five rows, renumbered, and the one that was third is second now
    await waitFor(() => expect(canvas.queryByRole("textbox", { name: "Text 6" })).toBeNull());
    await expect(canvas.getByRole("textbox", { name: "Text 2" })).toHaveValue(
      "the row that moves up",
    );
    await expect(canvas.queryByDisplayValue("the row I remove")).toBeNull();
    expectDistinctNames(canvas.getAllByRole("button", { name: /^Remove text / }), 5);
  },
};

/** Goes to another mode tab and comes back, with the other mode proven out of the page in between. */
async function awayAndBack(
  canvas: ReturnType<typeof within>,
  away: string,
  home: string,
  gone: () => HTMLElement | null,
) {
  await userEvent.click(canvas.getByRole("tab", { name: away }));
  // out of the page for everyone, not only out of sight
  await waitFor(() => expect(gone()).toBeNull());
  await userEvent.click(canvas.getByRole("tab", { name: home }));
}

/**
 * The embedding texts and their projection survive a visit to another tab
 * (#2329): the edited row, the six vectors, and no second request.
 */
const embedded = recording(deployment(async () => json(minted())));

export const EmbeddingsKeepTheirWorkAcrossATabSwitch: Story = {
  render: () => <Screen fetchStub={embedded.stub} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await readyComposer(canvas);
    await userEvent.click(canvas.getByRole("tab", { name: "Embeddings" }));

    const first = await canvas.findByRole("textbox", { name: "Text 1" });
    await userEvent.clear(first);
    await userEvent.type(first, "a text I changed");
    await clickWhenEnabled(canvasElement, "Embed & project");
    await canvas.findByRole("img", { name: en.pages.playground.pcaChartAria });
    await canvas.findByText("PCA projection · 6 vectors");

    await awayAndBack(canvas, "Chat", "Embeddings", () =>
      canvas.queryByRole("textbox", { name: "Text 1" }),
    );
    await expect(await canvas.findByRole("textbox", { name: "Text 1" })).toHaveValue(
      "a text I changed",
    );
    await expect(canvas.getByRole("img", { name: en.pages.playground.pcaChartAria })).toBeVisible();
    await expect(canvas.getByText("PCA projection · 6 vectors")).toBeVisible();
    // coming back asked for nothing
    const requests = bodiesAt<EmbedRequest>(embedded.calls, EMBED_PATH);
    await expect(requests).toHaveLength(1);
    await expect(requests[0].input[0]).toBe("a text I changed");
  },
};

/**
 * The image prompt and the pictures generated from it survive a visit to
 * another tab (#2329). Generating costs money, so the prompt and the result
 * are the expensive thing to lose, and coming back must not send it again.
 */
const painted = recording(deployment(async () => json(minted())));

export const ImageKeepsItsPromptAndResultAcrossATabSwitch: Story = {
  render: () => <Screen fetchStub={painted.stub} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await readyComposer(canvas);
    await userEvent.click(canvas.getByRole("tab", { name: "Image" }));

    const prompt = await canvas.findByRole("textbox", { name: "Image prompt" });
    await userEvent.clear(prompt);
    await userEvent.type(prompt, "a fox made of red thread");
    await clickWhenEnabled(canvasElement, "Generate");
    await waitFor(() => expect(canvas.getAllByRole("img", { name: /^sample / })).toHaveLength(4));
    await canvas.findByText("Output · 4 samples");

    await awayAndBack(canvas, "Chat", "Image", () =>
      canvas.queryByRole("textbox", { name: "Image prompt" }),
    );
    await expect(await canvas.findByRole("textbox", { name: "Image prompt" })).toHaveValue(
      "a fox made of red thread",
    );
    await waitFor(() => expect(canvas.getAllByRole("img", { name: /^sample / })).toHaveLength(4));
    await expect(canvas.getByText("Output · 4 samples")).toBeVisible();
    const requests = bodiesAt<ImageRequest>(painted.calls, IMAGE_PATH);
    await expect(requests).toHaveLength(1);
    await expect(requests[0].prompt).toBe("a fox made of red thread");
  },
};

/**
 * A generation still out when the operator leaves the tab is not lost (#2329).
 * Coming back while it is pending finds the button still busy rather than a
 * tab that forgot it asked, and a result that lands while another tab is up is
 * there on the way back.
 */
let developed: () => void = () => {};
const slowImage = recording(
  deployment(async () => json(minted()), undefined, undefined, {
    image: (request) =>
      new Promise<Response>((resolve) => {
        developed = () => resolve(pictures(request));
      }),
  }),
);

export const LateImageLandsWhileOnAnotherTab: Story = {
  render: () => <Screen fetchStub={slowImage.stub} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await readyComposer(canvas);
    await userEvent.click(canvas.getByRole("tab", { name: "Image" }));
    await clickWhenEnabled(canvasElement, "Generate");
    await slowImage.expectSent("POST", IMAGE_PATH);
    await waitFor(() => expect(canvas.getByRole("button", { name: "Generate" })).toBeDisabled());

    // away and back with the request still out: it is still the same request
    await awayAndBack(canvas, "Chat", "Image", () =>
      canvas.queryByRole("textbox", { name: "Image prompt" }),
    );
    await waitFor(() => expect(canvas.getByRole("button", { name: "Generate" })).toBeDisabled());
    await expect(canvas.queryAllByRole("img", { name: /^sample / })).toHaveLength(0);

    // and away again while it lands
    await userEvent.click(canvas.getByRole("tab", { name: "Chat" }));
    await waitFor(() => expect(canvas.queryByRole("textbox", { name: "Image prompt" })).toBeNull());
    developed();
    await userEvent.click(canvas.getByRole("tab", { name: "Image" }));

    await waitFor(() => expect(canvas.getAllByRole("img", { name: /^sample / })).toHaveLength(4));
    await waitFor(() => expect(canvas.getByRole("button", { name: "Generate" })).toBeEnabled());
    await expect(bodiesAt<ImageRequest>(slowImage.calls, IMAGE_PATH)).toHaveLength(1);
  },
};

/**
 * The Audio panel stays mounted under another tab, so a clip still playing
 * there would have no control left to stop it: leaving the tab pauses it (#2329).
 */
const playing = recording(deployment(async () => json(minted())));

export const LeavingAudioPausesTheClip: Story = {
  render: () => <Screen fetchStub={playing.stub} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const pause = spyOn(HTMLMediaElement.prototype, "pause");
    try {
      await readyComposer(canvas);
      await userEvent.click(canvas.getByRole("tab", { name: "Audio" }));
      await clickWhenEnabled(canvasElement, "Synthesize");
      await waitFor(() => expect(canvasElement.querySelector("audio")).not.toBeNull());
      // staying on the tab pauses nothing
      await expect(pause).not.toHaveBeenCalled();

      await userEvent.click(canvas.getByRole("tab", { name: "Chat" }));
      await waitFor(() => expect(pause).toHaveBeenCalledTimes(1));
    } finally {
      pause.mockRestore();
    }
  },
};

/**
 * The speech text and the clip, and the transcript of an uploaded file, survive
 * a visit to another tab (#2329), along with which of the two halves was open.
 */
const spoken = recording(deployment(async () => json(minted())));

export const AudioKeepsItsTextClipAndTranscriptAcrossATabSwitch: Story = {
  render: () => <Screen fetchStub={spoken.stub} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await readyComposer(canvas);
    await userEvent.click(canvas.getByRole("tab", { name: "Audio" }));

    const text = await canvas.findByRole("textbox", { name: "Text to synthesize" });
    await userEvent.clear(text);
    await userEvent.type(text, "read this back to me");
    await clickWhenEnabled(canvasElement, "Synthesize");
    const audio = await waitFor(() => {
      const found = canvasElement.querySelector("audio");
      expect(found).not.toBeNull();
      return found as HTMLAudioElement;
    });
    const clipUrl = audio.src;
    await expect(clipUrl).toMatch(/^blob:/);

    await awayAndBack(canvas, "Chat", "Audio", () =>
      canvas.queryByRole("textbox", { name: "Text to synthesize" }),
    );
    await expect(await canvas.findByRole("textbox", { name: "Text to synthesize" })).toHaveValue(
      "read this back to me",
    );
    // the clip is the one already made, not a second request's
    await expect(canvasElement.querySelector("audio")?.src).toBe(clipUrl);
    await expect(spoken.calls.filter((c) => c.url.includes(SPEECH_PATH))).toHaveLength(1);

    // the other half keeps its transcript, and the tab it was left on
    await userEvent.click(canvas.getByRole("tab", { name: "Speech → Text" }));
    const upload = await waitFor(() => {
      const found = canvasElement.querySelector<HTMLInputElement>(
        'input[type="file"][accept="audio/*"]',
      );
      expect(found).not.toBeNull();
      return found as HTMLInputElement;
    });
    await userEvent.upload(upload, new File(["audio"], "clip.wav", { type: "audio/wav" }));
    await canvas.findByText("The transcript the gateway heard.");

    await awayAndBack(canvas, "Chat", "Audio", () =>
      canvas.queryByRole("tab", { name: "Speech → Text" }),
    );
    await expect(await canvas.findByText("The transcript the gateway heard.")).toBeVisible();
    await expect(canvas.getByRole("tab", { name: "Speech → Text" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    await expect(spoken.calls.filter((c) => c.url.includes(TRANSCRIBE_PATH))).toHaveLength(1);
  },
};

/**
 * A column's thread is the column's: removing the first of two leaves the
 * second one's thread under its own model, not the first's (#2062).
 */
export const RemovingAColumnKeepsTheOthersThread: Story = {
  render: () => (
    <Screen
      fetchStub={deployment(async () => json(minted()), undefined, undefined, {
        chat: (request) => {
          const last = request.messages[request.messages.length - 1];
          return completion(`echo ${String(last.content)}`);
        },
      })}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await readyComposer(canvas);
    await userEvent.click(canvas.getByRole("button", { name: "Add model" }));

    const [first, second] = await canvas.findAllByRole("textbox", { name: /^Message to / });
    const [sendFirst, sendSecond] = canvas.getAllByRole("button", { name: SEND });
    await userEvent.type(first, "alpha");
    await userEvent.click(sendFirst);
    await canvas.findByText("echo alpha");
    await userEvent.type(second, "beta");
    await userEvent.click(sendSecond);
    await canvas.findByText("echo beta");

    await userEvent.click(canvas.getAllByRole("button", { name: /^Remove column / })[0]);
    await waitFor(() => expect(canvas.queryByText("echo alpha")).toBeNull());
    await expect(canvas.getByText("echo beta")).toBeVisible();
    await expect(canvas.getByText("beta")).toBeVisible();
    await expect(canvas.queryByText("alpha")).toBeNull();
  },
};

/**
 * Every field on the other modes has a name of its own, not only a placeholder
 * that goes when it is typed into (#2062): each embedding row by its number, the
 * image prompt, the text to speak and the realtime frame.
 */
export const EveryFieldHasAName: Story = {
  render: () => <Screen fetchStub={deployment(async () => json(minted()))} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await readyComposer(canvas);

    await userEvent.click(canvas.getByRole("tab", { name: "Embeddings" }));
    for (const n of [1, 2, 3, 4, 5, 6]) {
      await canvas.findByRole("textbox", { name: `Text ${n}` });
    }

    await userEvent.click(canvas.getByRole("tab", { name: "Image" }));
    await canvas.findByRole("textbox", { name: "Image prompt" });

    await userEvent.click(canvas.getByRole("tab", { name: "Audio" }));
    await canvas.findByRole("textbox", { name: "Text to synthesize" });

    await userEvent.click(canvas.getByRole("tab", { name: "Realtime" }));
    await canvas.findByRole("textbox", { name: "Text frame to send" });
  },
};

/**
 * A socket that opens on its own and counts the closes it is asked for, so a
 * story can see what leaving the tab did to it without a gateway to dial.
 */
const sockets: { url: string; closes: number }[] = [];

function fakeRealtime() {
  const real = window.WebSocket;
  sockets.length = 0;
  class FakeSocket {
    onopen: ((ev: Event) => void) | null = null;
    onmessage: ((ev: MessageEvent) => void) | null = null;
    onerror: ((ev: Event) => void) | null = null;
    onclose: ((ev: CloseEvent) => void) | null = null;
    closes = 0;
    constructor(public url: string) {
      sockets.push(this);
      setTimeout(() => this.onopen?.(new Event("open")), 0);
    }
    send() {}
    close() {
      this.closes += 1;
      this.onclose?.(new CloseEvent("close"));
    }
  }
  window.WebSocket = FakeSocket as unknown as typeof WebSocket;
  return () => {
    window.WebSocket = real;
  };
}

/**
 * Realtime is the one mode that starts over on a tab switch (#2329): a mounted
 * one would hold its WebSocket open under a tab nobody is looking at. So the
 * socket is closed on the way out, the log and the session are gone on the way
 * back, nothing is dialled again by coming back, and the tab says so up front.
 */
export const RealtimeStartsOverOnATabSwitch: Story = {
  beforeEach: fakeRealtime,
  render: () => <Screen fetchStub={deployment(async () => json(minted()))} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const note = en.pages.playground.realtimeResets;
    await readyComposer(canvas);
    await userEvent.click(canvas.getByRole("tab", { name: "Realtime" }));
    // said before anything is started, not after the log has been lost
    await expect(await canvas.findByText(note)).toBeVisible();
    await expect(canvas.getByText("no events yet")).toBeVisible();

    await clickWhenEnabled(canvasElement, "Start session");
    await canvas.findByText("● connected");
    await expect(canvas.getByRole("button", { name: "Stop session" })).toBeVisible();
    await expect(sockets).toHaveLength(1);
    await expect(sockets[0].closes).toBe(0);

    await userEvent.click(canvas.getByRole("tab", { name: "Chat" }));
    // closed on the way out, not left open under another tab
    await waitFor(() => expect(sockets[0].closes).toBe(1));
    await waitFor(() => expect(canvas.queryByText(note)).toBeNull());

    await userEvent.click(canvas.getByRole("tab", { name: "Realtime" }));
    await expect(await canvas.findByText("no events yet")).toBeVisible();
    await expect(canvas.queryByText("● connected")).toBeNull();
    await expect(canvas.getByRole("button", { name: "Start session" })).toBeVisible();
    await expect(canvas.getByText(note)).toBeVisible();
    // coming back dialled nothing
    await expect(sockets).toHaveLength(1);
  },
};

// the model picker row and the composer wrap on a phone (#1242)
export const Mobile: Story = {
  ...atMobile,
  render: () => <Screen fetchStub={deployment(async () => json(minted()))} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvasElement.querySelector('[role="combobox"]')).toBeTruthy());
    void canvas;
    await expectNoHorizontalOverflow();
    // the strip scrolls and hides its scrollbar, so it says there is more: the
    // last tab ("Realtime") was clipped with nothing to show it (#2004)
    const strip = canvas.getByRole("tablist");
    await expect(strip.scrollWidth).toBeGreaterThan(strip.clientWidth);
    await expect(strip).toHaveAttribute("data-more-end", "true");
  },
};

/**
 * Sets the control plane's injected documentation base for one story and puts
 * it back afterwards, so the two states below cannot leak into each other.
 */
function withDocsBase(base: string | undefined) {
  return () => {
    const before = window.__ROLTER_CONFIG__;
    window.__ROLTER_CONFIG__ = base === undefined ? {} : { ...before, docsBaseUrl: base };
    return () => {
      window.__ROLTER_CONFIG__ = before;
    };
  };
}

/** The paste field's hint links into `security/which-key` when docs exist (#1164). */
export const KeyHintLinksToTheDocs: Story = {
  beforeEach: withDocsBase("https://docs.example.com"),
  render: () => <Screen fetchStub={deployment(async () => json(minted()))} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "Use a specific key" }));
    const link = await canvas.findByRole("link", { name: /Which key do I need/ });
    await expect(link).toHaveAttribute("href", "https://docs.example.com/security/which-key");
  },
};

/** The air-gapped default: the hint stands alone, with nothing to click. */
export const KeyHintHasNoLinkWithoutADocsHost: Story = {
  beforeEach: withDocsBase(undefined),
  render: () => <Screen fetchStub={deployment(async () => json(minted()))} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "Use a specific key" }));
    await canvas.findByText(/A rolter virtual key, minted on the Virtual Keys screen/);
    await expect(canvas.queryByRole("link", { name: /Which key do I need/ })).toBeNull();
  },
};

/**
 * Playground reports when it became usable (#1730).
 *
 * It is the screen an evaluator spends the most time in and the last one still
 * missing `useScreenReady`, so its time-to-interactive is the number most worth
 * having. The catalog is what the screen waits on — until it lands there is
 * nothing to send a request to — and the gateway probe only counts once there
 * is a key to make it with, otherwise a query that is `enabled: false` stays
 * pending forever and the event never fires.
 */
const interactive = recording(deployment(async () => json(minted())));

export const ReportsTimeToInteractive: Story = {
  render: () => (
    <UxScreenProvider screen="playground">
      <Screen fetchStub={interactive.stub} />
    </UxScreenProvider>
  ),
  play: async ({ canvasElement }) => {
    await within(canvasElement).findByRole("combobox", { name: "Model" });
    const event = await expectUxEvent("time_to_interactive");
    await expect(event.screen).toBe("playground");
    await expect(typeof event.duration_ms).toBe("number");
  },
};
