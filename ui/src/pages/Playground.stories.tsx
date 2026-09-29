import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { MemoryRouter } from "react-router";
import { expect, userEvent, waitFor, within } from "storybook/test";

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
  type FetchStub,
  type StoryRole,
} from "./story-harness";
import { setKeyPropagationForTests, setPlaygroundKey } from "@/lib/gateway";
import en from "@/lib/i18n/locales/en.json";
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

/** Every `Authorization` the screen sent to the gateway, in order. */
interface Sent {
  keys: string[];
}

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
  } = upstream;
  let gatewayCalls = 0;
  return async (input, init) => {
    const url = String(input);
    const path = new URL(url, "http://localhost").pathname;
    if (/^\/api\/v1\/teams\/[^/]+\/projects$/.test(path)) return json(projects);
    const scope = scopeResponse(url);
    if (scope) return scope;
    if (url.includes(MINT_PATH)) return mint();
    if (path === "/api/v1/config/problems") return problems();
    if (url.includes("/gw/v1/models")) {
      const auth = new Headers(init?.headers).get("Authorization");
      if (!auth) return json({ error: { message: "missing key" } }, 401);
      sent.keys.push(auth.replace("Bearer ", ""));
      return gateway(gatewayCalls++);
    }
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
      const send = canvas.getByRole("button", { name: "Send" });
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
      const send = canvas.getByRole("button", { name: "Send" });
      expect(send).toBeDisabled();
      expect(send).toHaveAttribute("title", SEND_NEEDS_KEY);
    });

    await userEvent.type(field, "sk-rolter-given");
    await userEvent.click(canvas.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(canvas.getByText("Pasted")).toBeVisible());
    await expectAllowed(canvasElement, "Send");
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

    const send = canvas.getByRole("button", { name: "Send" });
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
      const send = canvas.getByRole("button", { name: "Send" });
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
    await waitFor(() => expect(canvas.getByRole("button", { name: "Send" })).toBeEnabled());
    await expect(canvas.getByRole("button", { name: "Send" })).not.toHaveAttribute("title");
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
    await waitFor(() => expect(canvas.getByRole("button", { name: "Send" })).toBeEnabled());
    // the list is the gateway's own, so there is no fallback to explain
    await expect(canvas.queryByText(/Showing configured routes/)).toBeNull();
    await expect(canvas.queryByText(/Pick a project to mint a key against/)).toBeNull();
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
    await expect(canvas.getByRole("button", { name: "Send" })).toHaveAttribute(
      "title",
      SEND_WAITING,
    );

    // the same key, asked again until the gateway took it
    await waitFor(() => expect(sent.keys.length).toBe(4));
    await expect(new Set(sent.keys)).toEqual(new Set(["sk-rolter-minted"]));

    // the notice clears once the gateway's own list is in
    await waitFor(() =>
      expect(canvas.queryByText(/Asking the gateway which models this key can use/)).toBeNull(),
    );
    await expect(canvas.queryByText(/Could not read the gateway's model list/)).toBeNull();
    await waitFor(() => expect(canvas.getByRole("button", { name: "Send" })).toBeEnabled());

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

// the model picker row and the composer wrap on a phone (#1242)
export const Mobile: Story = {
  ...atMobile,
  render: () => <Screen fetchStub={deployment(async () => json(minted()))} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvasElement.querySelector('[role="combobox"]')).toBeTruthy());
    void canvas;
    await expectNoHorizontalOverflow();
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
