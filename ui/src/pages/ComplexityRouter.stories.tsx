import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import ComplexityRouter from "./ComplexityRouter";
import {
  Harness,
  expectEmptyState,
  expectLoadError,
  expectNoFalseEmpty,
  expectRefused,
  expectSkeleton,
  json,
  pending,
  recording,
  routes,
  scoped,
  scopeResponse,
} from "./story-harness";
import type { FetchStub, Recorder } from "./story-harness";
import type { ComplexityPolicy, ComplexityTier, RouteRow } from "@/lib/api";

const route = (over: Partial<RouteRow> = {}): RouteRow => ({
  id: "r-1",
  project_id: "project-1",
  model: "gpt-4o",
  strategy: "weighted",
  enabled: true,
  advanced: {},
  params: {},
  param_policy: {},
  created_at: "2026-02-01T00:00:00Z",
  ...over,
});

const ROUTES = [route(), route({ id: "r-2", model: "claude-sonnet" })];

const SMALL: ComplexityPolicy = {
  tiers: [{ name: "small", max_input_bytes: 4096, route: "gpt-4o-mini" }],
};

/** `GET /routes/{id}/complexity`, the read the editor makes when it opens */
const PER_ROUTE_POLICY = /\/routes\/([^/?]+)\/complexity/;

/**
 * The control plane as the screen sees it (#3046): the route list embeds each
 * route's policy, and the editor reads one route's policy when it opens.
 * `policyFor` answers with a policy, or `null` for a route the list returns
 * without one, as a control plane that predates the field does.
 */
const serving =
  (
    policyFor: (routeId: string) => ComplexityPolicy | null,
    rest: FetchStub = async () => json([]),
  ): FetchStub =>
  async (input, init) => {
    const url = String(input);
    const scope = scopeResponse(url);
    if (scope) return scope;
    const single = PER_ROUTE_POLICY.exec(url);
    if (single) {
      return init?.method === "PUT" ? json(route()) : json(policyFor(single[1]) ?? { tiers: [] });
    }
    if (url.includes("/routes")) {
      return json(
        ROUTES.map((r) => {
          const complexity = policyFor(r.id);
          return complexity ? { ...r, complexity } : r;
        }),
      );
    }
    return rest(input, init);
  };

const loaded = serving(() => SMALL);

const meta = {
  title: "Screens/ComplexityRouter",
  component: ComplexityRouter,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof ComplexityRouter>;

export default meta;
type Story = StoryObj<typeof meta>;

let finishDisable: (() => void) | undefined;
let disableCalls = 0;
let disabledPolicy = false;
export const DisablesPolicyAfterConfirmation: Story = {
  render: () => {
    disableCalls = 0;
    disabledPolicy = false;
    finishDisable = undefined;
    const stub: FetchStub = (input, init) => {
      if (String(input).includes("/routes/r-1/complexity") && init?.method === "DELETE") {
        disableCalls++;
        return new Promise<Response>((resolve) => {
          finishDisable = () => {
            disabledPolicy = true;
            resolve(json(route()));
          };
        });
      }
      return serving((id) => (id === "r-1" && !disabledPolicy ? SMALL : { tiers: [] }))(
        input,
        init,
      );
    };
    return (
      <Harness fetchStub={stub}>
        <ComplexityRouter />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "Disable policy for gpt-4o" }));
    const dialog = within(await within(document.body).findByRole("dialog"));
    await waitFor(() =>
      expect(dialog.getByRole("button", { name: "Disable policy" })).toBeVisible(),
    );
    await expect(disableCalls).toBe(0);
    await userEvent.click(dialog.getByRole("button", { name: "Disable policy" }));
    await waitFor(() => expect(disableCalls).toBe(1));
    await waitFor(() =>
      expect(dialog.getByRole("button", { name: "Disable policy" })).toBeDisabled(),
    );
    finishDisable?.();
    await waitFor(() => expect(within(document.body).queryByRole("dialog")).toBeNull());
    await waitFor(() =>
      expect(canvas.queryByRole("button", { name: "Disable policy for gpt-4o" })).toBeNull(),
    );
  },
};

export const DisableRefusedStaysOpen: Story = {
  render: () => (
    <Harness
      fetchStub={(input, init) =>
        init?.method === "DELETE"
          ? Promise.resolve(json({ error: { message: "refused" } }, 403))
          : loaded(input, init)
      }
    >
      <ComplexityRouter />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "Disable policy for gpt-4o" }));
    const dialog = within(await within(document.body).findByRole("dialog"));
    await waitFor(() =>
      expect(dialog.getByRole("button", { name: "Disable policy" })).toBeVisible(),
    );
    await userEvent.click(dialog.getByRole("button", { name: "Disable policy" }));
    await expect(await dialog.findByRole("alert")).toBeVisible();
    await expect(within(document.body).getByRole("dialog")).toBeVisible();
  },
};

export const Loaded: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <ComplexityRouter />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getAllByText("gpt-4o").length).toBeGreaterThan(0));
    await waitFor(() => expect(canvas.getAllByText("small").length).toBeGreaterThan(0));
    // a policy with no catch-all says where the larger requests go
    await expect(
      canvas.getAllByText("Requests larger than 4 KiB match no tier and stay on gpt-4o.").length,
    ).toBeGreaterThan(0);
  },
};

// the group labels are headings, so a screen reader can jump between them
export const GroupsAreHeadings: Story = {
  render: () => (
    <Harness fetchStub={unconfigured}>
      <ComplexityRouter />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByRole("heading", { name: "No policy yet" })).toBeVisible();
  },
};

export const Loading: Story = {
  render: () => (
    <Harness fetchStub={pending}>
      <ComplexityRouter />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
    await expectNoFalseEmpty(canvasElement, /No routes to give a policy/);
  },
};

// a complexity policy hangs off a route, so with no routes there is nothing
// this screen can create — the CTA points at the screen that can
export const Empty: Story = {
  render: () => (
    <Harness fetchStub={routes([["/routes", () => []]])}>
      <ComplexityRouter />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectEmptyState(canvasElement, /No routes to give a policy/);
    await expect(canvas.getByRole("link", { name: /Open routing rules/ })).toBeInTheDocument();
  },
};

// the screen had no error state at all before #1180: a failed route read left
// the page looking like a project with no routes
export const Error_: Story = {
  name: "Error",
  render: () => (
    <Harness fetchStub={scoped(async () => json({ error: { message: "boom" } }, 500))}>
      <ComplexityRouter />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /failed to return routes/i);
    await expectNoFalseEmpty(canvasElement, /No routes to give a policy/);
    // no "0 of 0 routes" while the list failed to arrive
    await expect(within(canvasElement).queryByText(/of 0 routes/)).not.toBeInTheDocument();
  },
};

export const Forbidden: Story = {
  render: () => (
    <Harness fetchStub={scoped(async () => json({ error: { message: "forbidden" } }, 403))}>
      <ComplexityRouter />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /You do not have access to routes/);
    await expectNoFalseEmpty(canvasElement, /No routes to give a policy/);
  },
};

// a route with no policy at all, so the "add a policy" entry point renders
const unconfigured = serving(() => ({ tiers: [] }));

// A complexity policy is stored on its route, so both entry points are
// `route:update` — admin (#1606).
export const RefusedToAViewer: Story = {
  render: () => (
    <Harness fetchStub={loaded} role="viewer">
      <ComplexityRouter />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, "Edit the complexity policy for gpt-4o");
    await expectRefused(canvasElement, "Disable policy for gpt-4o");
  },
};

// the second entry point is a bare button rather than a `GatedButton`, so it
// carries the gate by hand and can lose it without the first one noticing
export const RefusedToAMemberWithNoPolicyYet: Story = {
  render: () => (
    <Harness fetchStub={unconfigured} role="member">
      <ComplexityRouter />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, "Add a complexity policy for gpt-4o");
  },
};

// ── policy states (#1461, #3046) ───────────────────────────────────────────────
//
// The policies ride in the route list, so there is no read per route to be
// late or to fail on its own: while the list is out the screen shows its
// skeleton, and when it fails the list's alert says so (both are stories
// above). What stays worth pinning is that a policy which was never delivered
// is not an empty one.

/**
 * A control plane that predates the embedded policy returns routes without it.
 * `claude-sonnet` must not appear under "No policy yet", the group that offers
 * to create one, since an editor seeded from nothing would save a fresh draft
 * over contents nobody has seen.
 */
export const OnePolicyNotDelivered: Story = {
  render: () => (
    <Harness fetchStub={serving((id) => (id === "r-2" ? null : SMALL))}>
      <ComplexityRouter />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // the summary counts the route that answered, not the one that did not
    await waitFor(() =>
      expect(canvas.getByText(/1 of 1 route has a complexity policy/)).toBeVisible(),
    );
    await expect(canvas.getByText(/1 route's policy could not be read/)).toBeVisible();
    await expect(canvas.queryByText("No policy yet")).not.toBeInTheDocument();
    await expect(
      canvas.queryByRole("button", { name: "Add a complexity policy for claude-sonnet" }),
    ).not.toBeInTheDocument();
    await expect(
      canvas.queryByRole("button", { name: "Edit the complexity policy for claude-sonnet" }),
    ).not.toBeInTheDocument();
  },
};

/** The policies are not a request per route: the list is read once, with them. */
export const PoliciesComeWithTheRoutes: Story = {
  render: () => {
    reads = recording(loaded);
    return (
      <Harness fetchStub={reads.stub}>
        <ComplexityRouter />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getAllByText("small")).toHaveLength(ROUTES.length));
    const listReads = reads.calls.filter((c) => /\/projects\/[^/]+\/routes/.test(c.url));
    await expect(listReads).toHaveLength(1);
    await expect(listReads[0].url).toContain("include=complexity");
    await expect(reads.calls.filter((c) => PER_ROUTE_POLICY.test(c.url))).toHaveLength(0);
  },
};

let reads: Recorder;

/**
 * The editor still reads the one policy it is about to overwrite when it opens
 * rather than seeding a draft from whatever the list cached at page load, and
 * that read has to be able to fail. It used to seed the default two-tier draft
 * either way and offer a save that would have replaced a policy nobody had seen.
 */
const editorRead: FetchStub = async (input, init) => {
  if (PER_ROUTE_POLICY.test(String(input)) && init?.method !== "PUT") {
    return json({ error: { message: "policy read blew up" } }, 500);
  }
  return loaded(input, init);
};

export const EditorPolicyReadFails: Story = {
  render: () => (
    <Harness fetchStub={editorRead}>
      <ComplexityRouter />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", { name: "Edit the complexity policy for gpt-4o" }),
    );
    const dialog = within(await within(document.body).findByRole("dialog"));
    await waitFor(() =>
      expect(
        dialog.getAllByRole("alert").some((a) => /complexity policy/i.test(a.textContent ?? "")),
      ).toBe(true),
    );
    // no tier rows to edit and nothing to save: the draft would have gone over
    // contents the read never returned
    await expect(dialog.queryByDisplayValue("small")).not.toBeInTheDocument();
    await expect(dialog.getByRole("button", { name: "Save" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
  },
};

// ── the editor on a loaded policy (#2144) ─────────────────────────────────────

const openEditor = async (canvasElement: HTMLElement) => {
  const canvas = within(canvasElement);
  await userEvent.click(
    await canvas.findByRole("button", { name: "Edit the complexity policy for gpt-4o" }),
  );
  return within(await within(document.body).findByRole("dialog"));
};

/** A saved two-tier policy the editor opens on; `PUT` is recorded. */
const policyRecorder = (tiers: ComplexityTier[]) => recording(serving(() => ({ tiers })));

const TWO_TIERS: ComplexityTier[] = [
  { name: "small", max_input_bytes: 4096, route: "gpt-4o-mini" },
  { name: "large", max_input_bytes: null, route: "gpt-4o" },
];

let saved = policyRecorder(TWO_TIERS);

// every tier row is named: visible column headers, a name per input that says
// which tier it belongs to, and the byte ceiling read back in KiB
export const EditorLabelsTheTierFields: Story = {
  render: () => {
    saved = policyRecorder(TWO_TIERS);
    return (
      <Harness fetchStub={saved.stub}>
        <ComplexityRouter />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const dialog = await openEditor(canvasElement);
    await expect(await dialog.findByText("Max input size (bytes)")).toBeVisible();
    await expect(dialog.getByText("Tier name")).toBeVisible();
    await expect(dialog.getByLabelText("Name of tier 1")).toHaveValue("small");
    const ceiling = dialog.getByLabelText("Maximum input size of tier 1, in bytes");
    await expect(ceiling).toHaveValue(4096);
    await expect(dialog.getByText("= 4 KiB")).toBeVisible();
    await expect(dialog.getByLabelText("Maximum input size of tier 2, in bytes")).toHaveValue(null);
    await expect(dialog.getByText("no ceiling: takes the rest")).toBeVisible();
    await expect(dialog.getByRole("button", { name: "Remove tier small" })).toBeVisible();
  },
};

// edit a ceiling and save: the PUT carries the new number
export const EditsACeilingAndSaves: Story = {
  render: () => {
    saved = policyRecorder(TWO_TIERS);
    return (
      <Harness fetchStub={saved.stub}>
        <ComplexityRouter />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const dialog = await openEditor(canvasElement);
    const ceiling = await dialog.findByLabelText("Maximum input size of tier 1, in bytes");
    await userEvent.clear(ceiling);
    await userEvent.type(ceiling, "8192");
    await expect(dialog.getByText("= 8 KiB")).toBeVisible();
    await userEvent.click(dialog.getByRole("button", { name: "Save" }));
    const body = await saved.expectSentBody<{ tiers: ComplexityTier[] }>("PUT", "/complexity");
    await expect(body.tiers[0].max_input_bytes).toBe(8192);
    await expect(body.tiers[1].max_input_bytes).toBeNull();
  },
};

// descending ceilings and a catch-all that is not last are refused on the row,
// before the round trip
export const OutOfOrderPolicyIsRefusedOnTheRow: Story = {
  render: () => {
    saved = policyRecorder([
      { name: "small", max_input_bytes: 8192, route: "gpt-4o-mini" },
      { name: "medium", max_input_bytes: 4096, route: "gpt-4o" },
      { name: "rest", max_input_bytes: null, route: "gpt-4o" },
    ]);
    return (
      <Harness fetchStub={saved.stub}>
        <ComplexityRouter />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const dialog = await openEditor(canvasElement);
    const second = await dialog.findByLabelText("Maximum input size of tier 2, in bytes");
    await waitFor(() => expect(second).toHaveAttribute("aria-invalid", "true"));
    await expect(second).toHaveAccessibleDescription(
      "The ceiling must be larger than the tier before it.",
    );
    await expect(dialog.getByRole("button", { name: "Save" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    // clearing the middle ceiling makes it a catch-all that is not last
    await userEvent.clear(second);
    const third = dialog.getByLabelText("Maximum input size of tier 3, in bytes");
    await userEvent.clear(third);
    await userEvent.type(third, "100");
    await waitFor(() => expect(second).toHaveAttribute("aria-invalid", "true"));
    await expect(second).toHaveAccessibleDescription(/Only the last tier can have no ceiling/);
    saved.expectNotSent("PUT", "/complexity");
  },
};
