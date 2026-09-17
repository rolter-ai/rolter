import type { Meta, StoryObj } from "@storybook/react-vite";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { ModelSheet, type ModelSheetMode } from "./ModelSheet";
import {
  Harness,
  ORG,
  PROJECT,
  expectClosesWithoutPrompting,
  expectSheetClosed,
  json,
  recording,
  sheet,
  withConfirm,
  type FetchStub,
} from "@/pages/story-harness";
import type { EffectiveModelDto, ProviderRow, RouteRow } from "@/lib/api";

const PROVIDERS: ProviderRow[] = [
  {
    id: "prov-1",
    org_id: ORG.id,
    name: "openai-prod",
    slug: "openai-prod",
    kind: "openai",
    api_base: "https://api.openai.com",
    egress_proxies: [],
    created_at: "2026-01-01T00:00:00Z",
  },
  {
    id: "prov-2",
    org_id: ORG.id,
    name: "vllm-cluster",
    slug: "vllm-cluster",
    kind: "openai_compatible",
    api_base: "http://vllm.internal:8000",
    egress_proxies: [],
    created_at: "2026-01-01T00:00:00Z",
  },
];

const ROUTE: RouteRow = {
  id: "route-1",
  project_id: PROJECT.id,
  model: "gpt-4o",
  strategy: "round_robin",
  enabled: true,
  params: { temperature: 0.7 },
  param_policy: { mode: "allow", allow: [], deny: [] },
  advanced: {},
  created_at: "2026-02-01T00:00:00Z",
};

/**
 * The same route with a populated `advanced` blob. `guardrails` is a field the
 * sheet has no editor for: it rides along to prove a save carries it rather
 * than resetting it to the backend's serde default.
 */
const ADVANCED_ROUTE: RouteRow = {
  ...ROUTE,
  advanced: {
    model_type: "chat",
    capabilities: ["streaming", "tools", "json"],
    description: "prod chat traffic",
    base_url: "https://api.openai.com/v1",
    limits: { rpm: 600, timeout_secs: 30 },
    insecure_tls: false,
    headers: { "X-Tenant": "acme" },
    locked_headers: ["X-Tenant"],
    visibility: {
      minimum_role: "member",
      allowed_team_ids: [],
      allowed_key_ids: [],
      allowed_user_ids: [],
    },
    guardrails: { rules: ["pii-out"] },
  },
};

const MODELS: EffectiveModelDto[] = [
  { model: "gpt-4o", strategy: "round_robin", targets: 1, source: "db" },
  { model: "fake-llm", strategy: "round_robin", targets: 1, source: "config" },
];

const TARGETS = [
  {
    id: "tgt-1",
    route_id: ROUTE.id,
    provider_id: "prov-1",
    upstream_model: null,
    weight: 1,
    position: 0,
  },
];

/** everything the sheet reads on open; the rbac chip sources are best-effort */
const backing: FetchStub = async (input) => {
  const url = String(input);
  if (url.includes("/routes/route-1/targets")) return json(TARGETS);
  if (url.includes("/model-prices")) return json([]);
  if (url.includes("/currency")) return json({ settlement: "USD", codes: ["USD", "EUR"] });
  if (url.includes("/teams")) return json([]);
  if (url.includes("/virtual-keys")) return json([]);
  if (url.includes("/users")) return json([]);
  return json({ id: "route-new", model: "new" });
};

/** the recorder the story under way installed, read back by its play function */
let calls: ReturnType<typeof recording>;

function Stage({
  mode,
  route,
  configModel,
  stub = backing,
}: {
  mode: ModelSheetMode;
  route?: RouteRow | null;
  configModel?: EffectiveModelDto | null;
  stub?: FetchStub;
}) {
  const [open, setOpen] = React.useState(true);
  // a ref, not `useMemo`: a story that passes an inline stub changes its
  // identity on every render, and a memo keyed on it would hand each render a
  // fresh recorder — with the requests the last one saw thrown away
  const recorder = React.useRef<ReturnType<typeof recording> | null>(null);
  if (!recorder.current) {
    recorder.current = recording(stub);
    calls = recorder.current;
  }
  return (
    <Harness fetchStub={recorder.current.stub}>
      <ModelSheet
        open={open}
        mode={mode}
        onOpenChange={setOpen}
        projectId={PROJECT.id}
        orgId={ORG.id}
        providers={PROVIDERS}
        route={route}
        configModel={configModel}
        models={MODELS}
        routes={[ROUTE]}
        onDone={() => {}}
      />
    </Harness>
  );
}

// the sheet seeds its draft in an effect, so every play function waits for the
// seed before it types: an edit landing first would be overwritten by it
async function seeded(dialog: ReturnType<typeof within>): Promise<void> {
  await waitFor(() => expect(dialog.getByLabelText("Provider")).toHaveValue("prov-1"));
}

const meta = {
  title: "Overlays/ModelSheet",
  component: ModelSheet,
  parameters: { layout: "fullscreen" },
  // every story renders through `Stage`, which owns the props; these satisfy
  // the required-prop contract for the docs page
  args: {
    open: true,
    mode: "add" as const,
    onOpenChange: () => {},
    projectId: PROJECT.id,
    orgId: ORG.id,
    providers: PROVIDERS,
    models: MODELS,
    routes: [ROUTE],
    onDone: () => {},
  },
} satisfies Meta<typeof ModelSheet>;

export default meta;
type Story = StoryObj<typeof meta>;

/**
 * A blank draft: the sheet preselects the first provider, so the model id is
 * what is missing, and clearing the provider makes that the first reason.
 */
export const Add: Story = {
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await expect(dialog.getByRole("heading", { name: "Add model" })).toBeVisible();
    // the draft starts with no provider until the seed effect picks the first
    // one; asserting before the seed raced it and read whichever state won
    // (#1500)
    await seeded(dialog);
    // the primary action keeps its place and greys out while the draft is
    // incomplete (#1265), with the first blocking reason beside it — read
    // through the button's own description so no sibling field hint can match
    const save = dialog.getByRole("button", { name: "Add model" });
    await expect(save).toBeDisabled();
    await expect(save).toHaveAccessibleDescription(/Enter the model id exactly/);
    await userEvent.selectOptions(dialog.getByLabelText("Provider"), "");
    await waitFor(() =>
      expect(save).toHaveAccessibleDescription(/Pick the upstream provider/),
    );
    // `getAll`: the sheet states each error under its field *and* repeats the
    // set in a summary above the footer
    await expect(dialog.getAllByText(/Pick the upstream provider/).length).toBeGreaterThan(1);
    // "duplicate from" is offered only where there is something to duplicate
    await expect(dialog.getByLabelText("Duplicate from")).toBeVisible();
  },
};

/**
 * Edit waits for the route's target and the price table before it seeds the
 * draft — seeding early would show an empty provider on a model that has one.
 */
export const EditLoading: Story = {
  render: () => <Stage mode="edit" route={ROUTE} stub={() => new Promise<Response>(() => {})} />,
  play: async () => {
    const dialog = within(sheet());
    await waitFor(() => expect(dialog.getAllByRole("status").length).toBeGreaterThan(0));
  },
};

export const Edit: Story = {
  render: () => <Stage mode="edit" route={ROUTE} />,
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await expect(dialog.getByLabelText("Upstream model name")).toHaveValue("gpt-4o");
    // renaming is not supported yet, and the field says so rather than
    // accepting an edit the control plane would drop
    await expect(dialog.getByLabelText("Upstream model name")).toBeDisabled();
  },
};

/**
 * A config-owned model. It is always present and cannot be edited — the
 * control plane answers 409 — so the sheet is a reference view that says why
 * rather than a form that fails on save.
 */
export const ViewConfigModel: Story = {
  render: () => <Stage mode="view" configModel={MODELS[1]} />,
  play: async () => {
    const dialog = within(sheet());
    await expect(dialog.getByText("Model details")).toBeVisible();
    await expect(dialog.getByText(/Read-only config model/)).toBeVisible();
    await expect(dialog.queryByRole("button", { name: "Save model" })).not.toBeInTheDocument();
    await expect(dialog.getByLabelText("Provider")).toBeDisabled();
  },
};

/**
 * A public name already in the catalog. Two routes answering the same name is
 * ambiguous, so it is caught here rather than by whichever one the gateway
 * happens to resolve first.
 */
export const NameConflict: Story = {
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await userEvent.type(dialog.getByLabelText("Upstream model name"), "gpt-4o");
    await expect(dialog.getAllByText(/already exists/).length).toBeGreaterThan(0);
    await expect(dialog.getByRole("button", { name: "Add model" })).toBeDisabled();
  },
};

/** A base URL that is not a URL is refused before it reaches a provider. */
export const InvalidBaseUrl: Story = {
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await userEvent.type(dialog.getByLabelText("Base URL override"), "vllm.internal:8000");
    await expect(dialog.getAllByText(/must start with http/).length).toBeGreaterThan(0);
    await expect(dialog.getByRole("button", { name: "Add model" })).toBeDisabled();
  },
};

/**
 * Adding a model is a route plus a target, in that order: the target needs the
 * id the route creation returns.
 */
export const AddsAModel: Story = {
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await userEvent.selectOptions(dialog.getByLabelText("Provider"), "prov-2");
    await userEvent.type(dialog.getByLabelText("Upstream model name"), "llama-3.1-70b");
    await userEvent.click(dialog.getByRole("button", { name: "Add model" }));
    const route = (await calls.expectSentBody("POST", `/projects/${PROJECT.id}/routes`)) as {
      model: string;
    };
    await expect(route.model).toBe("llama-3.1-70b");
    const target = (await calls.expectSentBody("POST", "/routes/route-new/targets")) as {
      provider_id: string;
    };
    await expect(target.provider_id).toBe("prov-2");
  },
};

/**
 * The advanced half of the form is written back.
 *
 * Every field under "Limits & network", "Custom request headers" and "Access &
 * permissions" was local draft state that the sheet threw away on close
 * (#1189). Saving now sends the route's `advanced` blob, and the story asserts
 * the body: the edited limit and header, and the `guardrails` the sheet cannot
 * edit but must not reset.
 */
export const SavesTheAdvancedEditor: Story = {
  render: () => <Stage mode="edit" route={ADVANCED_ROUTE} />,
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await userEvent.click(dialog.getByRole("button", { name: "Limits & network" }));
    const rpm = dialog.getByLabelText("Requests / min");
    await expect(rpm).toHaveValue(600);
    await userEvent.clear(rpm);
    await userEvent.type(rpm, "900");

    await userEvent.click(dialog.getByRole("button", { name: "Custom request headers" }));
    const headerValue = dialog.getByLabelText("Header value");
    await expect(headerValue).toHaveValue("acme");
    await userEvent.clear(headerValue);
    await userEvent.type(headerValue, "beta");

    await userEvent.click(dialog.getByRole("button", { name: "Save model" }));
    const body = (await calls.expectSentBody("PUT", "/routes/route-1/advanced")) as {
      advanced: {
        base_url: string;
        limits: { rpm: number; timeout_secs: number };
        headers: Record<string, string>;
        locked_headers: string[];
        guardrails: unknown;
      };
    };
    await expect(body.advanced.limits.rpm).toBe(900);
    // milliseconds on screen, whole seconds on the wire
    await expect(body.advanced.limits.timeout_secs).toBe(30);
    await expect(body.advanced.headers).toEqual({ "X-Tenant": "beta" });
    await expect(body.advanced.locked_headers).toEqual(["X-Tenant"]);
    await expect(body.advanced.base_url).toBe("https://api.openai.com/v1");
    await expect(body.advanced.guardrails).toEqual({ rules: ["pii-out"] });
  },
};

/**
 * A save that touched nothing in the advanced editor does not rewrite the blob
 * — the params PUT still goes, the advanced PUT does not.
 */
export const LeavesTheAdvancedBlobAloneWhenUntouched: Story = {
  render: () => <Stage mode="edit" route={ADVANCED_ROUTE} />,
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await userEvent.click(dialog.getByRole("button", { name: "Save model" }));
    await calls.expectSent("PUT", "/routes/route-1/params");
    calls.expectNotSent("PUT", "/routes/route-1/advanced");
  },
};

/**
 * The control plane refuses a limit of its own accord — `validate_advanced`
 * caps every one at ten million. The sheet says which half of the save failed
 * instead of printing the message on its own.
 */
export const AdvancedRejected: Story = {
  render: () => (
    <Stage
      mode="edit"
      route={ADVANCED_ROUTE}
      stub={async (input, init) => {
        const url = String(input);
        if (url.includes("/advanced")) {
          return json({ error: { message: "rpm must be between 1 and 10000000" } }, 400);
        }
        return backing(input, init);
      }}
    />
  ),
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await userEvent.click(dialog.getByRole("button", { name: "Limits & network" }));
    const rpm = dialog.getByLabelText("Requests / min");
    await userEvent.clear(rpm);
    await userEvent.type(rpm, "99999999");
    await userEvent.click(dialog.getByRole("button", { name: "Save model" }));
    await waitFor(() =>
      expect(dialog.getByRole("alert")).toHaveTextContent(/advanced configuration/),
    );
    await expect(dialog.getByRole("alert")).toHaveTextContent(/rpm must be between/);
  },
};

/**
 * Prices are operator-supplied — rolter ships no pricing catalog — so the
 * pricing section points at our own cost docs rather than at a competitor's
 * datasheet presented as their source (#977).
 */
export const PricingLinksToRolterDocs: Story = {
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await userEvent.click(dialog.getByRole("button", { name: "Pricing override" }));
    const link = dialog.getByRole("link", { name: /Rolter docs/ });
    await expect(link.getAttribute("href")).toContain("github.com/rolter-ai/rolter");
    await expect(link).toHaveAttribute("target", "_blank");
    await expect(link).toHaveAttribute("rel", "noreferrer");
  },
};

/** An untouched draft closes without asking whether to discard it. */
export const ClosesCleanWithoutPrompting: Story = {
  render: () => <Stage mode="edit" route={ROUTE} />,
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await expectClosesWithoutPrompting();
  },
};

/** A dirty draft asks first, and "cancel" leaves the edit where it was. */
export const DiscardGuardKeepsTheDraft: Story = {
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await userEvent.type(dialog.getByLabelText("Upstream model name"), "llama-3.1-70b");
    await withConfirm(false, async () => {
      await userEvent.click(dialog.getByRole("button", { name: /close/i }));
    });
    await expect(dialog.getByLabelText("Upstream model name")).toHaveValue("llama-3.1-70b");
  },
};

/** And "discard" closes it. */
export const DiscardGuardThrowsItAway: Story = {
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await userEvent.type(dialog.getByLabelText("Upstream model name"), "llama-3.1-70b");
    await withConfirm(true, async () => {
      await userEvent.click(dialog.getByRole("button", { name: /close/i }));
    });
    await expectSheetClosed();
  },
};
