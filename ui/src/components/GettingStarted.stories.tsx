import type { Meta, StoryObj } from "@storybook/react-vite";
import { MemoryRouter } from "react-router";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { GettingStarted } from "./GettingStarted";
import {
  Harness,
  effectiveFor,
  expectEmptyState,
  expectGateAnswered,
  expectLoadError,
  expectRefused,
  expectSkeleton,
  json,
  matrixFixture,
  pending,
  recording,
  routes,
  scoped,
  type FetchStub,
  type Recorder,
  type StoryRole,
} from "@/pages/story-harness";
import { CapabilityProvider } from "@/lib/can";
import en from "@/lib/i18n/locales/en.json";

// The first-run checklist (#1585). Its whole job is to say which screen comes
// next while the deployment is still empty, so the states that matter are the
// ones where it is wrong to show it at all: after a dismissal, once the
// deployment is configured and serving, and for a caller who can act on none
// of its steps (#1848).

const PROVIDER = {
  id: "prov-1",
  org_id: "org-1",
  name: "OpenAI",
  slug: "openai",
  kind: "openai",
  enabled: true,
  created_at: "2026-01-01T00:00:00Z",
};

const ROUTE = {
  id: "route-1",
  project_id: "project-1",
  model: "gpt-4o",
  strategy: "weighted",
  created_at: "2026-01-01T00:00:00Z",
};

const KEY = {
  id: "vk-1",
  project_id: "project-1",
  name: "prod",
  prefix: "rk_live_",
  created_at: "2026-01-01T00:00:00Z",
};

/** a deployment where nothing has been created yet */
const empty: FetchStub = routes([
  ["/providers", () => []],
  ["/routes", () => []],
  ["/virtual-keys", () => []],
]);

/** provider, route and key all exist — three of four steps done */
const configured: FetchStub = routes([
  ["/providers", () => [PROVIDER]],
  ["/routes", () => [ROUTE]],
  ["/virtual-keys", () => [KEY]],
]);

const broken: FetchStub = scoped(async (input) =>
  String(input).includes("/providers") ? json({ error: "boom" }, 500) : json([]),
);

// the scope chain answers, the three lists never do: the checklist cannot say
// what is done yet
const slow: FetchStub = pending;

/** nothing under the scope chain, so there is no project to read state from */
const noProject: FetchStub = async (input) => {
  const path = new URL(String(input), "http://localhost").pathname;
  if (path === "/api/v1/orgs") return json([]);
  return json([]);
};

function render(stub: FetchStub, props: { requests?: number } = {}, role?: StoryRole) {
  return (
    <MemoryRouter>
      <Harness fetchStub={stub} role={role}>
        <GettingStarted {...props} />
      </Harness>
    </MemoryRouter>
  );
}

const meta = {
  title: "Components/GettingStarted",
  component: GettingStarted,
  parameters: { layout: "padded" },
  // the dismissal is persisted, so one story's click would otherwise decide
  // what the next story renders
  beforeEach: () => {
    localStorage.removeItem("rolter.getting-started.dismissed");
    return () => localStorage.removeItem("rolter.getting-started.dismissed");
  },
} satisfies Meta<typeof GettingStarted>;

export default meta;
type Story = StoryObj<typeof meta>;

const state = (canvasElement: HTMLElement, step: string) =>
  within(canvasElement).getByTestId(`getting-started-state-${step}`).textContent;

/**
 * An empty deployment: four steps, none of them done, and the curl a client
 * would send once there is a key to send it with.
 */
export const Loaded: Story = {
  render: () => render(empty),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(en.pages.gettingStarted.title)).toBeVisible();
    await waitFor(() =>
      expect(state(canvasElement, "provider")).toBe(en.pages.gettingStarted.todo),
    );
    await expect(state(canvasElement, "route")).toBe(en.pages.gettingStarted.todo);
    await expect(state(canvasElement, "key")).toBe(en.pages.gettingStarted.todo);
    // the CTAs are links to the screens that own those forms, not a second copy
    await expect(
      canvas.getByRole("link", { name: new RegExp(en.pages.gettingStarted.steps.provider.action) }),
    ).toHaveAttribute("href", "/providers");
    await expect(canvasElement.textContent ?? "").toContain("/v1/chat/completions");
  },
};

/**
 * The state is the rows, not a local checklist: with a provider, a route and a
 * key in the store, three steps read as done without anything having been
 * ticked here. This is what "another admin did the work" looks like on reload.
 */
export const StepsFollowTheRealRows: Story = {
  render: () => render(configured),
  play: async ({ canvasElement }) => {
    await waitFor(() =>
      expect(state(canvasElement, "provider")).toBe(en.pages.gettingStarted.done),
    );
    await expect(state(canvasElement, "route")).toBe(en.pages.gettingStarted.done);
    await expect(state(canvasElement, "key")).toBe(en.pages.gettingStarted.done);
    // no traffic yet, so the one step the operator has not done is still open
    await expect(state(canvasElement, "call")).toBe(en.pages.gettingStarted.todo);
  },
};

export const Loading: Story = {
  render: () => render(slow),
  play: async ({ canvasElement }) => expectSkeleton(canvasElement),
};

/**
 * A failed list is a failure, not an empty deployment: without this branch the
 * card would read "nothing configured yet" about a control plane that never
 * answered, and send the operator to create a provider they may already have.
 */
export const LoadFailed: Story = {
  render: () => render(broken),
  play: async ({ canvasElement }) =>
    expectLoadError(canvasElement, new RegExp(en.errors.resources.gettingStarted)),
};

/** no project in scope, so there is nothing whose real state could be reflected */
export const NoProjectSelected: Story = {
  render: () => render(noProject),
  play: async ({ canvasElement }) =>
    expectEmptyState(canvasElement, new RegExp(en.pages.gettingStarted.noScopeTitle)),
};

/**
 * Dismissing it leaves the way back. A one-way dismissal would make the only
 * page that names the order unreachable for anyone who clicked it once.
 */
export const DismissedAndReopened: Story = {
  render: () => render(empty),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(en.pages.gettingStarted.title)).toBeVisible();
    await userEvent.click(canvas.getByRole("button", { name: en.pages.gettingStarted.dismiss }));
    // the subtitle, not the title: the way back carries the same two words, and
    // asserting on the title would pass over a card that never went away
    await waitFor(() => expect(canvas.queryByText(en.pages.gettingStarted.subtitle)).toBeNull());
    await userEvent.click(canvas.getByRole("button", { name: en.pages.gettingStarted.reopen }));
    await expect(await canvas.findByText(en.pages.gettingStarted.subtitle)).toBeVisible();
  },
};

/**
 * Traffic alone is not enough to retire it — one `fake-llm` call in the
 * Playground is step one of four — but traffic on a deployment that has a
 * provider is a deployment past its first run, and the card leaves.
 */
export const RetiresOnceConfiguredAndServing: Story = {
  render: () => render(configured, { requests: 132 }),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.queryByText(en.pages.gettingStarted.subtitle)).toBeNull());
    await expect(
      canvas.queryByRole("button", { name: en.pages.gettingStarted.dismiss }),
    ).toBeNull();
    // and no way back either: there is nothing left to come back to
    await expect(canvas.queryByRole("button", { name: en.pages.gettingStarted.reopen })).toBeNull();
  },
};

export const StaysWhileOnlyTheBuiltInModelWasCalled: Story = {
  render: () => render(empty, { requests: 3 }),
  play: async ({ canvasElement }) => {
    await waitFor(() => expect(state(canvasElement, "call")).toBe(en.pages.gettingStarted.done));
    await expect(state(canvasElement, "provider")).toBe(en.pages.gettingStarted.todo);
  },
};

/**
 * What a caller below admin got back from the three lists on the deployment
 * #1848 was found on. A fresh recorder per story, since each asserts on its own
 * calls.
 */
function refusedLists() {
  return recording(
    scoped(async (input) => {
      const path = new URL(String(input), "http://localhost").pathname;
      if (/\/(providers|routes|virtual-keys)$/.test(path)) {
        return json({ error: { message: "forbidden" } }, 403);
      }
      return json([]);
    }),
  );
}

/** the absence below means something only once the gate has answered */
async function expectHidden(canvasElement: HTMLElement, lists: Recorder) {
  const canvas = within(canvasElement);
  await expectGateAnswered();
  // the title, not the subtitle: the way back after a dismissal carries the
  // same words, and a member should not get that either
  await expect(canvas.queryByText(en.pages.gettingStarted.title)).toBeNull();
  await expect(canvas.queryByRole("alert")).toBeNull();
  // nor are the lists sent only to be refused
  for (const fragment of ["/providers", "/routes", "/virtual-keys"]) {
    lists.expectNotSent("GET", fragment);
  }
}

const asMember = refusedLists();

/**
 * Every setup step is an admin task, and a member can act on none of them, so
 * the card is not theirs. It used to render anyway, and the lists behind it
 * came back 403: the first screen every engineer opened said "You do not have
 * access to the setup checklist" (#1848).
 */
export const HiddenFromAMember: Story = {
  render: () => render(asMember.stub, {}, "member"),
  play: async ({ canvasElement }) => expectHidden(canvasElement, asMember),
};

const asViewer = refusedLists();

/** the same for a viewer, who can create nothing at all */
export const HiddenFromAViewer: Story = {
  render: () => render(asViewer.stub, {}, "viewer"),
  play: async ({ canvasElement }) => expectHidden(canvasElement, asViewer),
};

/**
 * A dismissal is no way back in for a caller the card is not for: the
 * "Getting started" button it leaves behind would open onto three refusals.
 */
export const HiddenFromAMemberWhoDismissedIt: Story = {
  beforeEach: () => {
    localStorage.setItem("rolter.getting-started.dismissed", "1");
  },
  render: () => render(refusedLists().stub, {}, "member"),
  play: async ({ canvasElement }) => {
    await expectGateAnswered();
    await expect(
      within(canvasElement).queryByRole("button", { name: en.pages.gettingStarted.reopen }),
    ).toBeNull();
  },
};

// a member whose custom role adds routes and keys, but not providers. The
// harness's `role` answers from the built-in table alone, so the provider is
// mounted by hand and this stub has the last word on the gate
const partialGrant: FetchStub = async (input, init) => {
  const path = new URL(String(input), "http://localhost").pathname;
  if (path === "/api/v1/rbac/effective") {
    const member = effectiveFor("member");
    return json({ ...member, allowed: [...member.allowed, "route:create", "virtual_key:create"] });
  }
  if (path === "/api/v1/rbac/matrix") return json(matrixFixture());
  return empty(input, init);
};

/**
 * One setup step within reach is enough for the card to stay, and the steps
 * that are not are shown with the role they take, on a disabled control — not
 * a link into a form that answers 403 after it has been filled in.
 */
export const RefusedStepsNameTheirRole: Story = {
  render: () => (
    <MemoryRouter>
      <Harness fetchStub={partialGrant}>
        <CapabilityProvider>
          <GettingStarted />
        </CapabilityProvider>
      </Harness>
    </MemoryRouter>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectRefused(canvasElement, en.pages.gettingStarted.steps.provider.action);
    for (const [step, href] of [
      ["route", "/routing-rules"],
      ["key", "/virtual-keys"],
      // the Playground needs no capability at all, so that one is always a link
      ["call", "/playground"],
    ] as const) {
      const action = en.pages.gettingStarted.steps[step].action;
      await waitFor(() =>
        expect(canvas.getByRole("link", { name: new RegExp(action) })).toHaveAttribute(
          "href",
          href,
        ),
      );
    }
  },
};

const unanswered = recording(
  scoped(async (input) => {
    const path = new URL(String(input), "http://localhost").pathname;
    if (path === "/api/v1/rbac/effective") return json({ error: "boom" }, 500);
    if (path === "/api/v1/rbac/matrix") return json(matrixFixture());
    return json([]);
  }),
);

/**
 * Only an explicit "no" hides the card. A control plane that cannot answer the
 * question — an older one, or one whose RBAC route failed — still shows it, and
 * leaves the 403 to say the rest.
 */
export const ShownWhenTheGateCannotAnswer: Story = {
  render: () => (
    <MemoryRouter>
      <Harness fetchStub={unanswered.stub}>
        <CapabilityProvider>
          <GettingStarted />
        </CapabilityProvider>
      </Harness>
    </MemoryRouter>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await unanswered.expectSent("GET", "/api/v1/rbac/effective");
    await expect(await canvas.findByText(en.pages.gettingStarted.subtitle)).toBeVisible();
    await waitFor(() =>
      expect(
        canvas.getByRole("link", {
          name: new RegExp(en.pages.gettingStarted.steps.provider.action),
        }),
      ).toHaveAttribute("href", "/providers"),
    );
  },
};

const forbidden = recording(
  scoped(async (input) =>
    String(input).includes("/providers")
      ? json({ error: { message: "forbidden" } }, 403)
      : json([]),
  ),
);

/**
 * A list the caller may not read is an answer, not an outage. The gate cannot
 * always say it first — a role held below the org the provider list is read
 * at, or no gate at all — and the 403 that says it instead must not become the
 * error card #1848 was filed over. A 500 still does; that is `LoadFailed`.
 */
export const RefusedListIsNotALoadError: Story = {
  render: () => render(forbidden.stub),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await forbidden.expectSent("GET", "/providers");
    // the card is up, in its skeleton, until the 403 lands, so this waits on
    // the answer rather than passing before it
    await waitFor(() => {
      expect(canvas.queryByText(en.pages.gettingStarted.subtitle)).toBeNull();
      expect(canvas.queryByRole("alert")).toBeNull();
    });
  },
};

/** an admin gets all four, as links */
export const AdminSeesEveryCta: Story = {
  render: () => render(empty, {}, "admin"),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    for (const [step, href] of [
      ["provider", "/providers"],
      ["route", "/routing-rules"],
      ["key", "/virtual-keys"],
    ] as const) {
      const action = en.pages.gettingStarted.steps[step].action;
      await waitFor(() =>
        expect(canvas.getByRole("link", { name: new RegExp(action) })).toHaveAttribute(
          "href",
          href,
        ),
      );
    }
  },
};
