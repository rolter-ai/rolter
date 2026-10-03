import type { Meta, StoryObj } from "@storybook/react-vite";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { ModelSheet, type ModelSheetMode } from "./ModelSheet";
import {
  Harness,
  ORG,
  Toasted,
  PROJECT,
  expectClosesWithoutPrompting,
  expectSheetClosed,
  expectToast,
  json,
  openOptions,
  pickOption,
  recording,
  sheet,
  answerDiscardPrompt,
  type FetchStub,
} from "@/pages/story-harness";
import type { EffectiveModelDto, ProviderRow, RouteRow, RouteTargetRow } from "@/lib/api";
import ru from "@/lib/i18n/locales/ru.json";
import { atMobile, expectInViewport, expectNoHorizontalOverflow } from "@/lib/story-viewport";

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
  { model: "llama-70b", strategy: "cache_aware", targets: 3, source: "db" },
];

/**
 * A `cache_aware` route over three vLLM replicas (#1979). The sheet used to
 * show target 0 alone and rewrite it on save; every line is on screen now, and
 * a save touches only the lines that changed.
 */
const FLEET_ROUTE: RouteRow = {
  ...ROUTE,
  id: "route-2",
  model: "llama-70b",
  strategy: "cache_aware",
  params: {},
};

const FLEET_TARGETS: RouteTargetRow[] = [
  {
    id: "fleet-1",
    route_id: "route-2",
    provider_id: "prov-2",
    upstream_model: "meta-llama/Llama-3.1-70B",
    weight: 1,
    created_at: "2026-02-01T00:00:00Z",
  },
  {
    id: "fleet-2",
    route_id: "route-2",
    provider_id: "prov-2",
    upstream_model: "meta-llama/Llama-3.1-70B",
    weight: 1,
    created_at: "2026-02-01T00:00:00Z",
  },
  {
    id: "fleet-3",
    route_id: "route-2",
    provider_id: "prov-1",
    upstream_model: null,
    weight: 1,
    created_at: "2026-02-01T00:00:00Z",
  },
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
  if (url.includes("/routes/route-2/targets")) return json(FLEET_TARGETS);
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
  configTargets,
  stub = backing,
  providers = PROVIDERS,
  toasts = false,
}: {
  mode: ModelSheetMode;
  route?: RouteRow | null;
  configModel?: EffectiveModelDto | null;
  configTargets?: React.ComponentProps<typeof ModelSheet>["configTargets"];
  stub?: FetchStub;
  providers?: ProviderRow[];
  /** mount the toast queue, for a story that asserts what the save announced */
  toasts?: boolean;
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
  const Toasting = toasts ? Toasted : React.Fragment;
  return (
    <Harness fetchStub={recorder.current.stub}>
      <Toasting>
        <ModelSheet
          open={open}
          mode={mode}
          onOpenChange={setOpen}
          projectId={PROJECT.id}
          orgId={ORG.id}
          providers={providers}
          route={route}
          configModel={configModel}
          configTargets={configTargets}
          models={MODELS}
          routes={[ROUTE, FLEET_ROUTE]}
          onDone={() => {}}
        />
      </Toasting>
    </Harness>
  );
}

// the sheet seeds its draft in an effect, so every play function waits for the
// seed before it types: an edit landing first would be overwritten by it
async function seeded(dialog: ReturnType<typeof within>): Promise<void> {
  // the combobox shows the provider's name; `prov-1` is what goes on the wire
  await waitFor(() =>
    expect(dialog.getByLabelText("Target 1 provider")).toHaveValue("openai-prod"),
  );
}

/** the bodies of every request matching `method` and `fragment`, in order */
function sentBodies<T>(method: string, fragment: string): T[] {
  return calls.calls
    .filter((c) => c.method === method && c.url.includes(fragment) && c.body)
    .map((c) => JSON.parse(c.body as string) as T);
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
 * A blank draft: one target on the first provider is already there, so the
 * model name is what is missing, and removing the last target makes "add a
 * target" a reason of its own.
 */
export const Add: Story = {
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await expect(dialog.getByRole("heading", { name: "Add model" })).toBeVisible();
    // the draft starts with no target until the seed effect adds one on the
    // first provider; asserting before the seed raced it (#1500)
    await seeded(dialog);
    // each error is read through its own field's description, so it is the
    // field that is invalid rather than some text somewhere on the sheet (#1527)
    const name = dialog.getByLabelText("Model name");
    await expect(name).toHaveAttribute("aria-invalid", "true");
    await expect(name).toHaveAccessibleDescription(/Enter the name clients will send/);
    // the primary action keeps its place and greys out while the draft is
    // incomplete (#1265), with the first blocking reason beside it
    const save = dialog.getByRole("button", { name: "Add model" });
    await expect(save).toBeDisabled();
    await expect(save).toHaveAccessibleDescription(/Enter the name clients will send/);
    // a new model is not created `round_robin` behind the operator's back: the
    // strategy is a field, starting at the first one offered (#1979)
    await expect(dialog.getByLabelText("Strategy")).toHaveValue("round_robin");
    await expect(dialog.getByLabelText("Strategy")).toBeEnabled();

    await userEvent.type(name, "qwen-72b");
    await userEvent.click(dialog.getByRole("button", { name: "Remove target 1" }));
    const addTarget = dialog.getByRole("button", { name: "Add target" });
    await waitFor(() => expect(addTarget).toHaveAccessibleDescription(/at least one target/));
    await expect(save).toHaveAccessibleDescription(/at least one target/);
    // `getAll`: the sheet states each error under its field *and* repeats the
    // set in a summary above the footer
    await expect(dialog.getAllByText(/at least one target/).length).toBeGreaterThan(1);
    // "duplicate from" is offered only where there is something to duplicate
    await expect(dialog.getByLabelText("Duplicate from")).toBeVisible();
  },
};

/**
 * The footer on a phone, in the longer of the two catalogs (#2003).
 *
 * The sheet is the whole screen below `sm` and cannot be scrolled sideways, so
 * a button past the right edge cannot be pressed at all. The blocking reason
 * used to sit beside Cancel and the primary action and squeeze them past the
 * gutter. Now the pair takes the bottom line to itself with the primary action
 * last and widest, and the reason stays what the button says it waits for —
 * the summary above the buttons already shows it, so it is not printed twice.
 */
export const AddOnAPhoneInRussian: Story = {
  ...atMobile,
  globals: { ...atMobile.globals, locale: "ru" },
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await expect(
      await dialog.findByRole("heading", { name: ru.modelSheet.titleAdd }),
    ).toBeVisible();
    await waitFor(() =>
      expect(
        dialog.getByLabelText(ru.modelSheet.targets.providerAria.replace("{{n}}", "1")),
      ).toHaveValue("openai-prod"),
    );
    const save = dialog.getByRole("button", { name: ru.modelSheet.ctaAdd });
    const cancel = dialog.getByRole("button", { name: ru.common.cancel });
    // greyed out in place (#1265), still naming why
    await expect(save).toBeDisabled();
    await expect(save).toHaveAccessibleDescription(ru.modelSheet.errors.name);
    await expectInViewport(save);
    await expectInViewport(cancel);
    // on the sheet, not only on the screen: level with the header's close
    // button rather than pushed into the gutter by the reason beside it
    const close = dialog.getByRole("button", { name: ru.common.close });
    const [saveBox, cancelBox] = [save.getBoundingClientRect(), cancel.getBoundingClientRect()];
    await expect(saveBox.right).toBeLessThanOrEqual(close.getBoundingClientRect().right);
    // one line, primary last and taking the room Cancel leaves
    await expect(saveBox.top).toBe(cancelBox.top);
    await expect(saveBox.left).toBeGreaterThan(cancelBox.right);
    await expect(saveBox.width).toBeGreaterThan(cancelBox.width);
    await expectNoHorizontalOverflow();
  },
};

/** The edit variant's longer "Сохранить модель", with nothing blocking it. */
export const EditOnAPhoneInRussian: Story = {
  ...atMobile,
  globals: { ...atMobile.globals, locale: "ru" },
  render: () => <Stage mode="edit" route={ROUTE} />,
  play: async () => {
    const dialog = within(sheet());
    await expect(
      await dialog.findByRole("heading", { name: ru.modelSheet.titleEdit }),
    ).toBeVisible();
    // the target line stacks below `sm`: the provider on a line of its own,
    // the upstream model, weight and remove beneath it, all inside the sheet
    const upstream = await dialog.findByLabelText(
      ru.modelSheet.targets.upstreamAria.replace("{{n}}", "1"),
    );
    const remove = dialog.getByRole("button", {
      name: ru.modelSheet.targets.removeAria.replace("{{n}}", "1"),
    });
    remove.scrollIntoView({ block: "center" });
    await expectInViewport(upstream);
    await expectInViewport(remove);
    const save = dialog.getByRole("button", { name: ru.modelSheet.ctaSave });
    await waitFor(() => expect(save).toBeEnabled());
    await expectInViewport(save);
    await expectInViewport(dialog.getByRole("button", { name: ru.common.cancel }));
    await expect(save.getBoundingClientRect().right).toBeLessThanOrEqual(
      dialog.getByRole("button", { name: ru.common.close }).getBoundingClientRect().right,
    );
    await expectNoHorizontalOverflow();
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
    await expect(dialog.getByLabelText("Model name")).toHaveValue("gpt-4o");
    // renaming is not supported yet, and the field says so rather than
    // accepting an edit the control plane would drop
    await expect(dialog.getByLabelText("Model name")).toBeDisabled();
    // the strategy is shown as stored, and said to be fixed: the control plane
    // takes it at creation and has no call that changes it (#1979)
    const strategy = dialog.getByLabelText("Strategy");
    await expect(strategy).toHaveValue("round_robin");
    await expect(strategy).toBeDisabled();
    await expect(strategy).toHaveAccessibleDescription(/set when a model is created/);
  },
};

/**
 * A config-owned model. It is always present and cannot be edited — the
 * control plane answers 409 — so the sheet is a reference view that says why
 * rather than a form that fails on save. Its strategy and targets are the ones
 * `rolter.toml` declares, not the blank draft's (#1979).
 */
export const ViewConfigModel: Story = {
  render: () => (
    <Stage
      mode="view"
      configModel={{ model: "fake-llm", strategy: "weighted", targets: 2, source: "config" }}
      configTargets={[
        { provider: "sim-a", upstream: "fake-llm", weight: 3 },
        { provider: "sim-b", upstream: "fake-llm-canary", weight: 1 },
      ]}
    />
  ),
  play: async () => {
    const dialog = within(sheet());
    await expect(dialog.getByText("Model details")).toBeVisible();
    await expect(dialog.getByText(/Read-only config model/)).toBeVisible();
    await expect(dialog.queryByRole("button", { name: "Save model" })).not.toBeInTheDocument();
    await waitFor(() => expect(dialog.getByLabelText("Strategy")).toHaveValue("weighted"));
    await expect(dialog.getByLabelText("Strategy")).toBeDisabled();
    // the targets are a list to read, not an editor with nothing behind it
    const list = within(dialog.getByRole("list", { name: "Targets of fake-llm" }));
    const lines = await list.findAllByRole("listitem");
    await expect(lines).toHaveLength(2);
    await expect(lines[1]).toHaveTextContent(/sim-b.*fake-llm-canary.*weight 1.*25% of traffic/);
    await expect(dialog.queryByRole("button", { name: "Add target" })).not.toBeInTheDocument();
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
    const name = dialog.getByLabelText("Model name");
    await userEvent.type(name, "gpt-4o");
    await waitFor(() => expect(name).toHaveAccessibleDescription(/already exists/));
    await expect(name).toHaveAttribute("aria-invalid", "true");
    await expect(dialog.getByRole("button", { name: "Add model" })).toBeDisabled();
  },
};

/** A base URL that is not a URL is refused before it reaches a provider. */
export const InvalidBaseUrl: Story = {
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    const baseUrl = dialog.getByLabelText("Base URL override");
    await userEvent.type(baseUrl, "vllm.internal:8000");
    await waitFor(() => expect(baseUrl).toHaveAttribute("aria-invalid", "true"));
    await expect(baseUrl).toHaveAccessibleDescription(/must start with http/);
    await expect(dialog.getByRole("button", { name: "Add model" })).toBeDisabled();
  },
};

/**
 * Adding a model is a route plus a target, in that order: the target needs the
 * id the route creation returns. A target that names no upstream model of its
 * own sends the public name through, and says so by sending no model at all.
 */
export const AddsAModel: Story = {
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await userEvent.type(dialog.getByLabelText("Model name"), "llama-3.1-70b");
    await pickOption(dialog.getByLabelText("Target 1 provider"), "vllm-cluster");
    await userEvent.click(dialog.getByRole("button", { name: "Add model" }));
    const route = (await calls.expectSentBody("POST", `/projects/${PROJECT.id}/routes`)) as {
      model: string;
      strategy: string;
    };
    await expect(route).toEqual({ model: "llama-3.1-70b", strategy: "round_robin" });
    const target = (await calls.expectSentBody("POST", "/routes/route-new/targets")) as {
      provider_id: string;
      upstream_model?: string;
      weight: number;
    };
    await expect(target).toEqual({ provider_id: "prov-2", weight: 1 });
  },
};

/**
 * A `cache_aware` model over two replicas, created from the sheet (#1979).
 *
 * The strategy the operator picks is the one the route is created with, and
 * each target line becomes its own target, in the order listed. `cache_aware`
 * does not read weights, and the sheet says so once there is more than one.
 */
export const AddsACacheAwareFleet: Story = {
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await userEvent.type(dialog.getByLabelText("Model name"), "qwen-72b");
    await pickOption(dialog.getByLabelText("Strategy"), "cache_aware");
    await pickOption(dialog.getByLabelText("Target 1 provider"), "vllm-cluster");
    await userEvent.type(
      dialog.getByLabelText("Target 1 upstream model"),
      "meta-llama/Llama-3.1-70B",
    );
    await userEvent.click(dialog.getByRole("button", { name: "Add target" }));
    await pickOption(await dialog.findByLabelText("Target 2 provider"), "openai-prod");
    const weight = dialog.getByLabelText("Target 2 weight");
    await userEvent.clear(weight);
    await userEvent.type(weight, "3");
    await expect(dialog.getByText(/does not read weights/)).toHaveTextContent(/^cache_aware/);

    await userEvent.click(dialog.getByRole("button", { name: "Add model" }));
    const route = (await calls.expectSentBody("POST", `/projects/${PROJECT.id}/routes`)) as {
      strategy: string;
    };
    await expect(route.strategy).toBe("cache_aware");
    await waitFor(() => expect(sentBodies("POST", "/routes/route-new/targets")).toHaveLength(2));
    await expect(sentBodies("POST", "/routes/route-new/targets")).toEqual([
      { provider_id: "prov-2", upstream_model: "meta-llama/Llama-3.1-70B", weight: 1 },
      { provider_id: "prov-1", weight: 3 },
    ]);
  },
};

/** A weight is a whole number from 1; the control plane refuses anything else. */
export const RefusesAWeightBelowOne: Story = {
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await userEvent.type(dialog.getByLabelText("Model name"), "qwen-72b");
    const weight = dialog.getByLabelText("Target 1 weight");
    await userEvent.clear(weight);
    await userEvent.type(weight, "0");
    await waitFor(() => expect(weight).toHaveAttribute("aria-invalid", "true"));
    await expect(weight).toHaveAccessibleDescription(/whole numbers, 1 or more/);
    await expect(dialog.getByRole("button", { name: "Add model" })).toBeDisabled();
  },
};

/**
 * Editing a multi-target route shows every target and writes only what moved.
 *
 * There is no update endpoint for a target, so a changed line is created anew
 * and its old row deleted — and every create goes out before any delete, so
 * the gateway never sees the route with nothing to send to part-way through.
 * An untouched line is left alone.
 */
export const EditsEveryTargetOfAFleet: Story = {
  render: () => <Stage mode="edit" route={FLEET_ROUTE} />,
  play: async () => {
    const dialog = within(sheet());
    await waitFor(() =>
      expect(dialog.getByLabelText("Target 3 provider")).toHaveValue("openai-prod"),
    );
    await expect(dialog.getByLabelText("Target 1 upstream model")).toHaveValue(
      "meta-llama/Llama-3.1-70B",
    );
    await expect(dialog.getByLabelText("Strategy")).toHaveValue("cache_aware");

    const weight = dialog.getByLabelText("Target 2 weight");
    await userEvent.clear(weight);
    await userEvent.type(weight, "2");
    await userEvent.click(dialog.getByRole("button", { name: "Remove target 3" }));
    await userEvent.click(dialog.getByRole("button", { name: "Save model" }));

    await calls.expectSent("DELETE", "/route-targets/fleet-3");
    await calls.expectSent("DELETE", "/route-targets/fleet-2");
    await expect(sentBodies("POST", "/routes/route-2/targets")).toEqual([
      { provider_id: "prov-2", upstream_model: "meta-llama/Llama-3.1-70B", weight: 2 },
    ]);
    calls.expectNotSent("DELETE", "/route-targets/fleet-1");
    const writes = calls.calls.filter((c) => c.method !== "GET").map((c) => `${c.method} ${c.url}`);
    const creates = writes.filter((w) => w.includes("/routes/route-2/targets"));
    const lastCreate = writes.lastIndexOf(creates[creates.length - 1]);
    const firstDelete = writes.findIndex((w) => w.startsWith("DELETE"));
    await expect(lastCreate).toBeLessThan(firstDelete);
  },
};

/**
 * Duplicating a route starts from its strategy and targets. A target that sent
 * the source's name through keeps sending that model once the copy is renamed.
 */
export const DuplicatesStrategyAndTargets: Story = {
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await pickOption(dialog.getByLabelText("Duplicate from"), "llama-70b");
    await waitFor(() => expect(dialog.getByLabelText("Strategy")).toHaveValue("cache_aware"));
    await expect(await dialog.findByLabelText("Target 3 upstream model")).toHaveValue("llama-70b");
    // the copy carries the source's name, which is taken, until it is renamed
    await expect(dialog.getByLabelText("Model name")).toHaveAccessibleDescription(/already exists/);
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
    // and the key the backend dropped is not written back: `set_route_advanced`
    // persists the raw body, so the sheet copying it through was the one path
    // keeping it alive in the stored blob (#1665, #1710)
    await expect(body.advanced).not.toHaveProperty("additional_fields");
  },
};

/**
 * "Allow additional fields" is gone from "Limits & network" (#1271).
 *
 * It was draft state with nothing behind it: `seedAdvanced` derived it from
 * whether the stored `additional_fields` map had any keys, so turning it on for
 * a route with an empty map saved nothing and it read as off again on reopen,
 * while turning it off cleared a map the operator was never shown the contents
 * of. The map itself was written by the control plane and read by nothing, and
 * has since been dropped from `AdvancedModelConfig` outright (#1665) — the
 * sheet no longer copies it back either (#1710), so the stored blob sheds the
 * key on the next save instead of carrying it forever.
 */
export const OffersNoAdditionalFieldsSwitch: Story = {
  render: () => <Stage mode="edit" route={ADVANCED_ROUTE} />,
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await userEvent.click(dialog.getByRole("button", { name: "Limits & network" }));
    // the section did open, so this is an absent control and not an unopened
    // section standing in for one
    await expect(dialog.getByLabelText("Requests / min")).toBeVisible();
    await expect(dialog.queryByText("Allow additional fields")).not.toBeInTheDocument();
  },
};

/**
 * The sheet offers no connection check (#1972).
 *
 * Its "Test connection" button ran a timer and then showed "Connection OK" for
 * any draft, an empty provider and upstream included, without a request. The
 * one probe the control plane has, the provider test, says whether a provider
 * answers and not whether it serves this upstream model, so nothing here claims
 * a result until a probe can (#2008, #2009).
 */
export const OffersNoFakeConnectionTest: Story = {
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await pickOption(dialog.getByLabelText("Target 1 provider"), "vllm-cluster");
    await userEvent.type(dialog.getByLabelText("Model name"), "llama-3.1-70b");
    // a complete draft, the state an operator would have tested from, with the
    // footer past validation: this is an absent control, not a footer that has
    // not painted its actions yet
    await waitFor(() => expect(dialog.getByRole("button", { name: "Add model" })).toBeEnabled());
    await expect(
      dialog.queryByRole("button", { name: /test connection/i }),
    ).not.toBeInTheDocument();
    await expect(dialog.queryByText(/connection ok/i)).not.toBeInTheDocument();
    calls.expectNotSent("POST", "/test");
  },
};

/**
 * A save that touched nothing in the advanced editor does not rewrite the blob
 * — the params PUT still goes, the advanced PUT does not — and the targets are
 * not deleted and recreated either.
 */
export const LeavesTheAdvancedBlobAloneWhenUntouched: Story = {
  render: () => <Stage mode="edit" route={ADVANCED_ROUTE} />,
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await userEvent.click(dialog.getByRole("button", { name: "Save model" }));
    await calls.expectSent("PUT", "/routes/route-1/params");
    calls.expectNotSent("PUT", "/routes/route-1/advanced");
    // nor are the targets rewritten: an unchanged line is left as it is
    calls.expectNotSent("POST", "/routes/route-1/targets");
    calls.expectNotSent("DELETE", "/route-targets/");
  },
};

/**
 * A route is visible to the whole organization until an admin narrows it to
 * the project it lives in (#1844). The pin travels as
 * `visibility.project_only`, and the gateway then refuses keys minted in the
 * organization's other projects.
 */
export const PinsARouteToItsProject: Story = {
  render: () => <Stage mode="edit" route={ADVANCED_ROUTE} />,
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await userEvent.click(dialog.getByRole("button", { name: "Access & permissions" }));
    const visibility = within(dialog.getByRole("radiogroup", { name: "Visibility" }));
    await expect(visibility.getByRole("radio", { name: "Whole organization" })).toBeChecked();
    await userEvent.click(visibility.getByRole("radio", { name: "This project" }));

    await userEvent.click(dialog.getByRole("button", { name: "Save model" }));
    const body = (await calls.expectSentBody("PUT", "/routes/route-1/advanced")) as {
      advanced: { visibility: Record<string, unknown> };
    };
    await expect(body.advanced.visibility).toEqual({
      minimum_role: "member",
      allowed_team_ids: [],
      allowed_key_ids: [],
      allowed_user_ids: [],
      project_only: true,
    });
  },
};

/**
 * A pinned route reopens pinned, and opening it back up to the organization
 * drops the flag rather than writing `false` over it.
 */
export const OpensAPinnedRouteToTheOrganization: Story = {
  render: () => (
    <Stage
      mode="edit"
      route={{
        ...ADVANCED_ROUTE,
        advanced: {
          ...ADVANCED_ROUTE.advanced,
          visibility: {
            minimum_role: "member",
            allowed_team_ids: [],
            allowed_key_ids: [],
            allowed_user_ids: [],
            project_only: true,
          },
        },
      }}
    />
  ),
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await userEvent.click(dialog.getByRole("button", { name: "Access & permissions" }));
    const visibility = within(dialog.getByRole("radiogroup", { name: "Visibility" }));
    await expect(visibility.getByRole("radio", { name: "This project" })).toBeChecked();
    await userEvent.click(visibility.getByRole("radio", { name: "Whole organization" }));

    await userEvent.click(dialog.getByRole("button", { name: "Save model" }));
    const body = (await calls.expectSentBody("PUT", "/routes/route-1/advanced")) as {
      advanced: { visibility: Record<string, unknown> };
    };
    await expect(body.advanced.visibility).not.toHaveProperty("project_only");
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
    await userEvent.type(dialog.getByLabelText("Model name"), "llama-3.1-70b");
    await userEvent.click(dialog.getByRole("button", { name: /close/i }));
    await answerDiscardPrompt(false);
    await expect(dialog.getByLabelText("Model name")).toHaveValue("llama-3.1-70b");
  },
};

/** And "discard" closes it. */
export const DiscardGuardThrowsItAway: Story = {
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await userEvent.type(dialog.getByLabelText("Model name"), "llama-3.1-70b");
    await userEvent.click(dialog.getByRole("button", { name: /close/i }));
    await answerDiscardPrompt(true);
    await expectSheetClosed();
  },
};

// providers scoped to a project (#1919): one of this route's project, one of
// another. the first is the route's to use, the second is not
const SCOPED_PROVIDERS: ProviderRow[] = [
  ...PROVIDERS,
  {
    ...PROVIDERS[0],
    id: "prov-3",
    name: "gateway-private",
    slug: "gateway-private",
    project_id: PROJECT.id,
  },
  {
    ...PROVIDERS[0],
    id: "prov-4",
    name: "search-private",
    slug: "search-private",
    project_id: "project-2",
  },
];

/**
 * A route in project P is offered P's providers and the org-wide ones, and says
 * how many it is leaving out, so a missing provider is not a mystery.
 */
export const OffersOnlyTheProjectsOwnAndOrgWideProviders: Story = {
  render: () => <Stage mode="add" providers={SCOPED_PROVIDERS} />,
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    const listbox = await openOptions(dialog.getByLabelText("Target 1 provider"));
    const names = within(listbox)
      .getAllByRole("option")
      .map((o) => o.textContent);
    await expect(names).toEqual(["openai-prod", "vllm-cluster", "gateway-private"]);
    await userEvent.keyboard("{Escape}");
    await expect(
      dialog.getByText("1 provider scoped to another project is not offered here."),
    ).toBeVisible();
  },
};

/** With every provider usable there is nothing to explain. */
export const SaysNothingWhenNoProviderIsLeftOut: Story = {
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await expect(dialog.queryByText(/not offered here/)).toBeNull();
  },
};

/**
 * The control plane has the last word: a target it refuses on scope (a provider
 * another tab just scoped away) reaches the operator as its own sentence.
 */
export const TargetRefusedByTheProvidersScope: Story = {
  render: () => (
    <Stage
      mode="add"
      toasts
      stub={async (input, init) => {
        if (String(input).includes("/targets") && init?.method === "POST") {
          return json(
            {
              error: {
                message:
                  "this route cannot use provider 'search-private': scoped to a different project; use org-wide providers or ones scoped to the same project",
              },
            },
            409,
          );
        }
        return backing(input, init);
      }}
    />
  ),
  play: async ({ canvasElement }) => {
    const dialog = within(sheet());
    await seeded(dialog);
    await userEvent.type(dialog.getByLabelText("Model name"), "llama-3.1-70b");
    await userEvent.click(dialog.getByRole("button", { name: "Add model" }));
    await expectToast(canvasElement, /cannot use provider 'search-private'/, "error");
    // the sheet stays open on the draft
    await expect(sheet()).toBeVisible();
  },
};
