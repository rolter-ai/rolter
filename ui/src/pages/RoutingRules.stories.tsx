import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, waitFor, within } from "storybook/test";

import RoutingRules from "./RoutingRules";
import {
  Harness,
  Toasted,
  cancelConfirmation,
  confirmDestructive,
  expectSkeleton,
  expectToast,
  json,
  pending,
  recording,
  scoped,
  type FetchStub,
} from "./story-harness";
import type { ProviderRow, RouteRow, RouteTargetRow } from "@/lib/api";

const PROVIDERS: ProviderRow[] = [
  {
    id: "prov-1",
    org_id: "org-1",
    name: "openai-prod",
    slug: "openai-prod",
    kind: "openai",
    api_base: "https://api.openai.com/v1",
    egress_proxies: [],
    created_at: "2026-05-01T00:00:00Z",
  },
  {
    id: "prov-2",
    org_id: "org-1",
    name: "azure-west",
    slug: "azure-west",
    kind: "azure_openai",
    api_base: "https://west.openai.azure.com",
    egress_proxies: [],
    created_at: "2026-05-01T00:00:00Z",
  },
];

const ROUTES: RouteRow[] = [
  {
    id: "route-1",
    project_id: "project-1",
    model: "gpt-4o",
    strategy: "weighted",
    enabled: true,
    params: {},
    param_policy: {},
    advanced: {},
    created_at: "2026-05-01T00:00:00Z",
  },
  // a disabled route still renders: a route nobody can see is a route nobody
  // remembers to turn back on
  {
    id: "route-2",
    project_id: "project-1",
    model: "claude-sonnet",
    strategy: "least_load",
    enabled: false,
    params: {},
    param_policy: {},
    advanced: {},
    created_at: "2026-05-02T00:00:00Z",
  },
];

const TARGETS: Record<string, RouteTargetRow[]> = {
  "route-1": [
    {
      id: "rt-1",
      route_id: "route-1",
      provider_id: "prov-1",
      upstream_model: "gpt-4o-2024-08-06",
      weight: 80,
      created_at: "2026-05-01T00:00:00Z",
    },
    {
      id: "rt-2",
      route_id: "route-1",
      provider_id: "prov-2",
      upstream_model: null,
      weight: 20,
      created_at: "2026-05-01T00:00:00Z",
    },
  ],
  "route-2": [],
};

const answer = (routes: RouteRow[], status = 200): FetchStub =>
  scoped(async (input) => {
    const url = String(input);
    if (url.includes("/providers")) return json(PROVIDERS);
    const targets = /\/routes\/([^/]+)\/targets/.exec(url);
    if (targets) return json(TARGETS[targets[1]] ?? []);
    return json(status === 200 ? routes : { error: { message: "forbidden" } }, status);
  });

const meta = {
  title: "Screens/RoutingRules",
  component: RoutingRules,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof RoutingRules>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = {
  render: () => (
    <Harness fetchStub={answer(ROUTES)}>
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("gpt-4o")).toBeInTheDocument();
    // weights are rendered as shares of the route's total, so 80/100 is 80%
    await waitFor(() => expect(canvas.getByText("80%")).toBeInTheDocument());
    await expect(canvas.getByText("disabled")).toBeInTheDocument();
  },
};

export const Loading: Story = {
  render: () => (
    <Harness fetchStub={pending}>
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
  },
};

export const Empty: Story = {
  render: () => (
    <Harness fetchStub={answer([])}>
      <RoutingRules />
    </Harness>
  ),
};

export const Forbidden: Story = {
  render: () => (
    <Harness fetchStub={answer([], 403)}>
      <RoutingRules />
    </Harness>
  ),
};

// deleting a route silently breaks every client calling that public model name,
// so it asks by name first (#1179)
const deletes = recording(
  scoped(async (input, init) => {
    if (init?.method === "DELETE") return json({}, 204);
    return answer(ROUTES)(input, init);
  }),
);

export const ConfirmsBeforeDeletingARoute: Story = {
  render: () => (
    <Harness fetchStub={deletes.stub}>
      <Toasted>
        <RoutingRules />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("gpt-4o")).toBeInTheDocument();
    // by name, not by index: every row control names the route it acts on,
    // so the story asserts on the control it means (#1214)
    const button = canvas.getByRole("button", { name: "Delete route gpt-4o" });

    // cancelling must leave the route alone — the half a manual click-through
    // never checks
    await userEvent.click(button);
    await cancelConfirmation();
    deletes.expectNotSent("DELETE", "/api/v1/routes/route-1");

    await userEvent.click(button);
    await confirmDestructive(/gpt-4o/, /delete route/i);
    await deletes.expectSent("DELETE", "/api/v1/routes/route-1");
    // the confirmation closes on success, so the outcome is announced where it
    // outlives the dialog (#1197)
    await expectToast(canvasElement, /gpt-4o deleted/);
  },
};

// the request is on the wire and the button says so, rather than looking like
// the click was dropped
const hangs = scoped(async (input, init) => {
  if (init?.method === "DELETE") return new Promise<Response>(() => {});
  return answer(ROUTES)(input, init);
});

export const DeletingARoute: Story = {
  render: () => (
    <Harness fetchStub={hangs}>
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("gpt-4o")).toBeInTheDocument();
    await userEvent.click(canvas.getByRole("button", { name: "Delete route gpt-4o" }));
    await confirmDestructive(/gpt-4o/, /delete route/i);
    const dialog = within(document.body).getByRole("dialog");
    await waitFor(() =>
      expect(within(dialog).getByRole("button", { name: /delete route/i })).toBeDisabled(),
    );
  },
};

// a delete the server refuses leaves the dialog open with the reason, instead
// of closing on an action that did not happen
export const DeleteFails: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input, init) => {
        if (init?.method === "DELETE") {
          return json({ error: { message: "route is referenced by 2 virtual keys" } }, 409);
        }
        return answer(ROUTES)(input, init);
      })}
    >
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("gpt-4o")).toBeInTheDocument();
    await userEvent.click(canvas.getByRole("button", { name: "Delete route gpt-4o" }));
    await confirmDestructive(/gpt-4o/, /delete route/i);

    const dialog = within(document.body).getByRole("dialog");
    await waitFor(() =>
      expect(within(dialog).getByRole("alert")).toHaveTextContent(
        /referenced by 2 virtual keys/,
      ),
    );
  },
};
