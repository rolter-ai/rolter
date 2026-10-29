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
  expectAllowed,
  expectRefused,
  answerDiscardPrompt,
  withDocsBase,
  type FetchStub,
  type StoryRole,
} from "@/pages/story-harness";
import type {
  EffectiveModelDto,
  ModelPriceRow,
  ProviderRow,
  ProviderTestResult,
  RouteRow,
  RouteTargetRow,
} from "@/lib/api";
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
    limits: { timeout_secs: 30, retries: 1 },
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
  { model: "gpt-4o", strategy: "round_robin", targets: 1, source: "db", org_id: ORG.id },
  { model: "fake-llm", strategy: "round_robin", targets: 1, source: "config" },
  { model: "llama-70b", strategy: "cache_aware", targets: 3, source: "db", org_id: ORG.id },
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

/** the upstream model ids each provider's catalogue lists, as the control plane relays them */
const LISTED: Record<string, string[]> = {
  "prov-1": ["gpt-4o", "gpt-4o-mini"],
  "prov-2": [
    "meta-llama/Llama-3.1-70B",
    "meta-llama/Llama-3.1-8B",
    "mistralai/Mistral-7B-Instruct-v0.3",
  ],
};

/** everything the sheet reads on open; the rbac chip sources are best-effort */
const backing: FetchStub = async (input) => {
  const url = String(input);
  const catalogue = /\/providers\/([^/]+)\/models/.exec(url);
  if (catalogue) return json({ models: LISTED[catalogue[1]] ?? [] });
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
  models = MODELS,
  toasts = false,
  role,
}: {
  mode: ModelSheetMode;
  route?: RouteRow | null;
  configModel?: EffectiveModelDto | null;
  configTargets?: React.ComponentProps<typeof ModelSheet>["configTargets"];
  stub?: FetchStub;
  providers?: ProviderRow[];
  /** the catalog the name check reads */
  models?: EffectiveModelDto[];
  /** mount the toast queue, for a story that asserts what the save announced */
  toasts?: boolean;
  /** mount the capability gate as this role, for a story that asserts it */
  role?: StoryRole;
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
    <Harness fetchStub={recorder.current.stub} role={role}>
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
          models={models}
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

/**
 * The seed of a new route: its first target line is there, on no provider while
 * the org has several (#2810). The line appearing is the seed having run.
 */
async function seededBlank(dialog: ReturnType<typeof within>): Promise<void> {
  await expect(await dialog.findByLabelText("Target 1 provider")).toHaveValue("");
}

/** the bodies of every request matching `method` and `fragment`, in order */
function sentBodies<T>(method: string, fragment: string): T[] {
  return calls.calls
    .filter((c) => c.method === method && c.url.includes(fragment) && c.body)
    .map((c) => JSON.parse(c.body as string) as T);
}

/** the body of the one `PUT /routes/{id}` (not its `/params` or `/advanced` siblings) */
async function routeUpdate(id: string): Promise<unknown> {
  const sent = () =>
    calls.calls.find((c) => c.method === "PUT" && c.url.endsWith(`/routes/${id}`) && c.body);
  await waitFor(() => expect(sent()).toBeDefined());
  return JSON.parse(sent()!.body as string);
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
 * A blank draft opens with nothing wrong on it (#2810).
 *
 * The required name is not an error before anybody has had the chance to fill
 * it: it used to be stated three times, under the field, in a summary and
 * beside the buttons, on a form nothing had been typed into. The first target
 * line is there and points at no provider while the org has several, since the
 * first one alphabetically is a provider the route's traffic should not reach
 * unless it was chosen.
 */
export const Add: Story = {
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await waitFor(() => expect(dialog.getByRole("heading", { name: "Add route" })).toBeVisible());
    await expect(dialog.getByText("Name the route clients call", { exact: false })).toBeVisible();
    // the draft has no target line until the seed effect adds one; asserting
    // before the seed raced it (#1500)
    await seededBlank(dialog);
    const provider = dialog.getByLabelText("Target 1 provider");
    await expect(provider).toHaveAttribute("placeholder", "Pick a provider");
    // no field is marked, and nothing is announced
    const name = dialog.getByLabelText("Route name");
    await expect(name).not.toHaveAttribute("aria-invalid");
    await expect(provider).not.toHaveAttribute("aria-invalid");
    await expect(name).toHaveAccessibleDescription(/The name clients call/);
    await expect(dialog.queryByText(/Enter the name clients will send/)).not.toBeInTheDocument();
    await expect(dialog.queryByText(/Pick a provider for every target/)).not.toBeInTheDocument();
    await expect(dialog.queryByText(/needs? attention/)).not.toBeInTheDocument();
    await expect(dialog.queryByRole("alert")).not.toBeInTheDocument();
    // the primary action is there to be pressed: pressing it is what asks for
    // the errors, so it is never greyed out by them
    await expect(dialog.getByRole("button", { name: "Add route" })).toBeEnabled();
    // a new route is not created `round_robin` behind the operator's back: the
    // strategy is a field, starting at the first one offered (#1979)
    await expect(dialog.getByLabelText("Strategy")).toHaveValue("round_robin");
    await expect(dialog.getByLabelText("Strategy")).toBeEnabled();
    // "duplicate from" is offered only where there is something to duplicate
    await expect(dialog.getByLabelText("Duplicate from")).toBeVisible();
  },
};

/**
 * A field that was visited and left blank says so, and only that field does.
 *
 * Focus coming in and going out is what makes a required field's error fair;
 * the summary and every other field wait for a refused save (#2810).
 */
export const TouchedNameShowsItsError: Story = {
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await seededBlank(dialog);
    const name = dialog.getByLabelText("Route name");
    await userEvent.click(name);
    // on the way in, the field is not wrong yet
    await expect(name).not.toHaveAttribute("aria-invalid");
    await userEvent.tab();
    await waitFor(() => expect(name).toHaveAttribute("aria-invalid", "true"));
    await expect(name).toHaveAccessibleDescription(/Enter the name clients will send/);
    // said once, at the field
    await expect(dialog.getAllByText(/Enter the name clients will send/)).toHaveLength(1);
    // the untouched provider line and the footer stay as they were
    await expect(dialog.getByLabelText("Target 1 provider")).not.toHaveAttribute("aria-invalid");
    await expect(dialog.queryByRole("status")).not.toBeInTheDocument();
    await expect(dialog.getByRole("button", { name: "Add route" })).toBeEnabled();
  },
};

/**
 * A save with problems puts each one at its own field, once, and counts them in
 * one line above the buttons (#2810).
 *
 * The button stays enabled and is the press that asks; focus goes to the first
 * problem, and nothing is sent.
 */
export const RefusedSaveStatesEachProblemOnce: Story = {
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await seededBlank(dialog);
    const save = dialog.getByRole("button", { name: "Add route" });
    await userEvent.click(save);

    const name = dialog.getByLabelText("Route name");
    const provider = dialog.getByLabelText("Target 1 provider");
    await waitFor(() => expect(name).toHaveAttribute("aria-invalid", "true"));
    await expect(provider).toHaveAttribute("aria-invalid", "true");
    // one message per field, each read through its own control's description
    await expect(dialog.getAllByText(/Enter the name clients will send/)).toHaveLength(1);
    await expect(dialog.getAllByText(/Pick a provider for every target/)).toHaveLength(1);
    await expect(name).toHaveAccessibleDescription(/Enter the name clients will send/);
    await expect(provider).toHaveAccessibleDescription(/Pick a provider for every target/);
    // and one summary, which counts the problems instead of repeating them
    await expect(dialog.getByRole("status")).toHaveTextContent(
      "2 fields need attention before this route can be saved.",
    );
    // focus is on the first problem, so the operator is not left in the footer
    await waitFor(() => expect(name).toHaveFocus());
    // nothing went out, and the button is still there to press
    calls.expectNotSent("POST", `/projects/${PROJECT.id}/routes`);
    await expect(save).toBeEnabled();
  },
};

/**
 * Fixing what a refused save named takes the summary and the field errors away
 * as each is fixed, and the same press then goes through.
 */
export const FixingTheProblemsLetsTheSaveThrough: Story = {
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await seededBlank(dialog);
    const save = dialog.getByRole("button", { name: "Add route" });
    await userEvent.click(save);
    const name = dialog.getByLabelText("Route name");
    const provider = dialog.getByLabelText("Target 1 provider");
    await expect(await dialog.findByRole("status")).toHaveTextContent("2 fields need attention");

    await userEvent.type(name, "llama-3.1-8b");
    await waitFor(() => expect(name).not.toHaveAttribute("aria-invalid"));
    // one problem left, and the count follows it
    await expect(dialog.getByRole("status")).toHaveTextContent(
      "1 field needs attention before this route can be saved.",
    );
    await pickOption(provider, "vllm-cluster");
    await waitFor(() => expect(dialog.queryByRole("status")).not.toBeInTheDocument());
    await expect(provider).not.toHaveAttribute("aria-invalid");
    await expect(dialog.queryByText(/Pick a provider for every target/)).not.toBeInTheDocument();

    await userEvent.click(save);
    await calls.expectSent("POST", `/projects/${PROJECT.id}/routes`);
    await calls.expectSent("POST", "/routes/route-new/targets");
  },
};

/**
 * Emptying the target list says a target is needed, once, at the list — and the
 * "add a target" button points at it.
 */
export const RemovingTheLastTargetSaysATargetIsNeeded: Story = {
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await seededBlank(dialog);
    await userEvent.click(dialog.getByRole("button", { name: "Remove target 1" }));
    const addTarget = dialog.getByRole("button", { name: "Add target" });
    await waitFor(() => expect(addTarget).toHaveAccessibleDescription(/at least one target/));
    await expect(dialog.getAllByText(/at least one target/)).toHaveLength(1);
    // the provider error was about the line that is gone
    await expect(dialog.queryByText(/Pick a provider for every target/)).not.toBeInTheDocument();
    await expect(dialog.getAllByRole("alert")).toHaveLength(1);
    // adding one back clears it, and the new line starts on no provider too
    await userEvent.click(addTarget);
    await expect(await dialog.findByLabelText("Target 1 provider")).toHaveValue("");
    await waitFor(() => expect(dialog.queryByText(/at least one target/)).not.toBeInTheDocument());
  },
};

/**
 * With exactly one provider there is nothing to choose between, so the line
 * starts on it — and so does every line added after it (#2810).
 */
export const OneProviderIsTheDefaultTarget: Story = {
  render: () => <Stage mode="add" providers={[PROVIDERS[0]]} />,
  play: async () => {
    const dialog = within(sheet());
    await waitFor(() =>
      expect(dialog.getByLabelText("Target 1 provider")).toHaveValue("openai-prod"),
    );
    // nothing to fix about it
    await expect(dialog.getByLabelText("Target 1 provider")).not.toHaveAttribute("aria-invalid");
    await expect(dialog.queryByText(/Pick a provider for every target/)).not.toBeInTheDocument();
    await userEvent.click(dialog.getByRole("button", { name: "Add target" }));
    await expect(await dialog.findByLabelText("Target 2 provider")).toHaveValue("openai-prod");

    await userEvent.click(dialog.getByRole("button", { name: "Remove target 2" }));
    await userEvent.type(dialog.getByLabelText("Route name"), "gpt-4o-eu");
    await userEvent.click(dialog.getByRole("button", { name: "Add route" }));
    const target = (await calls.expectSentBody("POST", "/routes/route-new/targets")) as {
      provider_id: string;
    };
    await expect(target.provider_id).toBe("prov-1");
  },
};

/**
 * With several providers a new target line starts on none, and so does every
 * line added after it — the first one alphabetically is a guess that sent a
 * `llama-3.1-8b` route to `anthropic-direct` (#2810).
 */
export const SeveralProvidersAreNotDefaulted: Story = {
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await seededBlank(dialog);
    await userEvent.click(dialog.getByRole("button", { name: "Add target" }));
    await expect(await dialog.findByLabelText("Target 2 provider")).toHaveValue("");
    // both lines offer every provider, and neither holds one
    await expect(dialog.getByLabelText("Target 1 provider")).toHaveValue("");
    const listbox = await openOptions(dialog.getByLabelText("Target 2 provider"));
    await expect(
      within(listbox)
        .getAllByRole("option")
        .map((o) => o.textContent),
    ).toEqual(["openai-prod", "vllm-cluster"]);
  },
};

/**
 * A provider that is not an org-wide one nor this project's is not offered, so
 * the only one left is the default (#1919, #2810): the count of providers the
 * sheet may use decides it, not the count the org has.
 */
export const TheOnlyUsableProviderIsTheDefault: Story = {
  render: () => (
    <Stage
      mode="add"
      providers={[
        PROVIDERS[0],
        { ...PROVIDERS[1], id: "prov-other", name: "elsewhere", project_id: "project-2" },
      ]}
    />
  ),
  play: async () => {
    const dialog = within(sheet());
    await waitFor(() =>
      expect(dialog.getByLabelText("Target 1 provider")).toHaveValue("openai-prod"),
    );
    await waitFor(() =>
      expect(
        dialog.getByText("1 provider scoped to another project is not offered here."),
      ).toBeVisible(),
    );
  },
};

/**
 * The footer on a phone, in the longer of the two catalogs (#2003).
 *
 * The sheet is the whole screen below `sm` and cannot be scrolled sideways, so
 * a button past the right edge cannot be pressed at all. A reason beside Cancel
 * and the primary action used to squeeze them past the gutter. Now the pair
 * takes the bottom line to itself with the primary action last and widest, and
 * what a refused save has to say is one line above them (#2810).
 */
export const AddOnAPhoneInRussian: Story = {
  ...atMobile,
  globals: { ...atMobile.globals, locale: "ru" },
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await waitFor(() =>
      expect(dialog.getByRole("heading", { name: ru.modelSheet.titleAdd })).toBeVisible(),
    );
    await expect(
      await dialog.findByLabelText(ru.modelSheet.targets.providerAria.replace("{{n}}", "1")),
    ).toHaveValue("");
    const save = dialog.getByRole("button", { name: ru.modelSheet.ctaAdd });
    const cancel = dialog.getByRole("button", { name: ru.common.cancel });
    // nothing is wrong yet, so nothing is said, and the action is live
    await expect(dialog.queryByRole("alert")).not.toBeInTheDocument();
    await expect(save).toBeEnabled();
    await userEvent.click(save);
    // two problems, in the plural form `ru` gives two
    const summary = await dialog.findByRole("status");
    await expect(summary).toHaveTextContent(
      ru.modelSheet.errors.summary_few.replace("{{count}}", "2"),
    );
    await expectInViewport(summary);
    await expectInViewport(save);
    await expectInViewport(cancel);
    // on the sheet, not only on the screen: level with the header's close
    // button rather than pushed into the gutter
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

/** The edit variant's longer "Сохранить маршрут", with nothing blocking it. */
export const EditOnAPhoneInRussian: Story = {
  ...atMobile,
  globals: { ...atMobile.globals, locale: "ru" },
  render: () => <Stage mode="edit" route={ROUTE} />,
  play: async () => {
    const dialog = within(sheet());
    await waitFor(() =>
      expect(dialog.getByRole("heading", { name: ru.modelSheet.titleEdit })).toBeVisible(),
    );
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
    await expect(dialog.getByLabelText("Route name")).toHaveValue("gpt-4o");
    // renaming is not supported yet, and the field says so rather than
    // accepting an edit the control plane would drop
    await expect(dialog.getByLabelText("Route name")).toBeDisabled();
    // the strategy is shown as stored and can be changed: the route's update
    // call takes it (#2979)
    const strategy = dialog.getByLabelText("Strategy");
    await expect(strategy).toHaveValue("round_robin");
    await expect(strategy).toBeEnabled();
    await expect(dialog.queryByText(/set when a route is created/)).not.toBeInTheDocument();
    // the stored providers' catalogues are not asked for just because the sheet
    // opened: an upstream call waits until somebody reaches for its models
    calls.expectNotSent("GET", "/providers/prov-1/models");
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
    await waitFor(() => expect(dialog.getByText("Route details")).toBeVisible());
    await expect(dialog.getByText(/Read-only config route/)).toBeVisible();
    await expect(dialog.queryByRole("button", { name: "Save route" })).not.toBeInTheDocument();
    await waitFor(() => expect(dialog.getByLabelText("Strategy")).toHaveValue("weighted"));
    await expect(dialog.getByLabelText("Strategy")).toBeDisabled();
    // the targets are a list to read, not an editor with nothing behind it
    const list = within(dialog.getByRole("list", { name: "Targets of fake-llm" }));
    const lines = await list.findAllByRole("listitem");
    await expect(lines).toHaveLength(2);
    await expect(lines[1]).toHaveTextContent(/sim-b.*fake-llm-canary.*weight 1.*25% of traffic/);
    await expect(dialog.queryByRole("button", { name: "Add target" })).not.toBeInTheDocument();
    // what the control plane reports is all it shows: the source, and none of
    // the blank draft's defaults dressed up as this route's config (#1982)
    await expect(dialog.getByText("Defined in rolter.toml.")).toBeVisible();
    for (const absent of [
      "Model type",
      "Enabled",
      "Default parameters",
      "Pricing override",
      "Config preview",
    ]) {
      await expect(dialog.queryByText(absent)).toBeNull();
    }
  },
};

/**
 * #1982: with no provider yet a target has nothing to point at, and the sheet
 * cannot make one, so it links to the screen that can instead of leaving the
 * operator at a disabled button.
 */
export const NoProvidersLinksToAddingOne: Story = {
  render: () => <Stage mode="add" providers={[]} />,
  play: async () => {
    const dialog = within(sheet());
    const link = await dialog.findByRole("link", { name: /Add one under Model Providers/ });
    await expect(link).toHaveAttribute("href", "/providers");
    // story-wait-allow: disabled by its own prop from the first paint, with no provider to add
    await expect(dialog.getByRole("button", { name: "Add target" })).toBeDisabled();
  },
};

/**
 * A public name already in the catalog. Two routes answering the same name is
 * ambiguous, so it is caught here rather than by whichever one the gateway
 * happens to resolve first. The conflict is stated when the field is left, and a
 * save with it standing is refused (#2810).
 */
export const NameConflict: Story = {
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await seededBlank(dialog);
    await pickOption(dialog.getByLabelText("Target 1 provider"), "vllm-cluster");
    const name = dialog.getByLabelText("Route name");
    await userEvent.type(name, "gpt-4o");
    // still typing: "gpt-4" on the way to "gpt-4o-mini" is not a mistake yet
    await expect(name).not.toHaveAttribute("aria-invalid");
    await userEvent.tab();
    await waitFor(() => expect(name).toHaveAccessibleDescription(/already exists/));
    await expect(name).toHaveAttribute("aria-invalid", "true");
    await expect(dialog.getAllByText(/already exists/)).toHaveLength(1);

    await userEvent.click(dialog.getByRole("button", { name: "Add route" }));
    await expect(await dialog.findByRole("status")).toHaveTextContent(
      "1 field needs attention before this route can be saved.",
    );
    await waitFor(() => expect(name).toHaveFocus());
    calls.expectNotSent("POST", `/projects/${PROJECT.id}/routes`);
  },
};

/**
 * #3460: another organization's `gpt-4o` is no conflict. Names are unique per
 * organization (#1857), and the control plane accepts the route, so the sheet
 * does not refuse it on a name only another organization uses.
 */
export const AnotherOrganizationsNameIsNoConflict: Story = {
  render: () => (
    <Stage mode="add" models={[{ ...MODELS[0], org_id: "org-2" }, MODELS[1], MODELS[2]]} />
  ),
  play: async () => {
    const dialog = within(sheet());
    await seededBlank(dialog);
    await pickOption(dialog.getByLabelText("Target 1 provider"), "vllm-cluster");
    const name = dialog.getByLabelText("Route name");
    await userEvent.type(name, "gpt-4o");
    await userEvent.tab();
    await userEvent.click(dialog.getByRole("button", { name: "Add route" }));
    await calls.expectSent("POST", `/projects/${PROJECT.id}/routes`);
    await expect(name).not.toHaveAttribute("aria-invalid");
    await expect(dialog.queryByText(/already exists/)).toBeNull();
  },
};

/**
 * A retry budget past the ceiling is refused before it reaches the control
 * plane — said when the field is left, not on the way to a valid number
 * (#2810). Zero is a value, not a blank: it turns retries off for the route.
 */
export const RetriesPastTheCeilingAreRefused: Story = {
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await seededBlank(dialog);
    await userEvent.click(dialog.getByRole("button", { name: "Limits & network" }));
    const retries = dialog.getByLabelText("Max retries");
    await userEvent.type(retries, "11");
    await expect(retries).not.toHaveAttribute("aria-invalid");
    await userEvent.tab();
    await waitFor(() => expect(retries).toHaveAttribute("aria-invalid", "true"));
    await expect(retries).toHaveAccessibleDescription(/whole number from 0 to 10/);
    await expect(dialog.getAllByText(/whole number from 0 to 10/)).toHaveLength(1);
    // zero is a budget of its own, so it clears the error
    await userEvent.clear(retries);
    await userEvent.type(retries, "0");
    await userEvent.tab();
    await waitFor(() => expect(retries).not.toHaveAttribute("aria-invalid"));
    // and the save refuses again once the field is back out of range
    await userEvent.clear(retries);
    await userEvent.type(retries, "11");
    await userEvent.type(dialog.getByLabelText("Route name"), "qwen-72b");
    await pickOption(dialog.getByLabelText("Target 1 provider"), "vllm-cluster");
    await userEvent.click(dialog.getByRole("button", { name: "Add route" }));
    await waitFor(() => expect(retries).toHaveFocus());
    calls.expectNotSent("POST", `/projects/${PROJECT.id}/routes`);
  },
};

/**
 * Adding a route is a route plus a target, in that order: the target needs the
 * id the route creation returns. A target that names no upstream model of its
 * own sends the public name through, and says so by sending no model at all.
 */
export const AddsAModel: Story = {
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await seededBlank(dialog);
    await userEvent.type(dialog.getByLabelText("Route name"), "llama-3.1-70b");
    await pickOption(dialog.getByLabelText("Target 1 provider"), "vllm-cluster");
    await userEvent.click(dialog.getByRole("button", { name: "Add route" }));
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
    await expect(target).toEqual({ provider_id: "prov-2", weight: 1, takes_over: [] });
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
    await seededBlank(dialog);
    await userEvent.type(dialog.getByLabelText("Route name"), "qwen-72b");
    await pickOption(dialog.getByLabelText("Strategy"), /^cache_aware\b/);
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

    await userEvent.click(dialog.getByRole("button", { name: "Add route" }));
    const route = (await calls.expectSentBody("POST", `/projects/${PROJECT.id}/routes`)) as {
      strategy: string;
    };
    await expect(route.strategy).toBe("cache_aware");
    await waitFor(() => expect(sentBodies("POST", "/routes/route-new/targets")).toHaveLength(2));
    await expect(sentBodies("POST", "/routes/route-new/targets")).toEqual([
      {
        provider_id: "prov-2",
        upstream_model: "meta-llama/Llama-3.1-70B",
        weight: 1,
        takes_over: [],
      },
      { provider_id: "prov-1", weight: 3, takes_over: [] },
    ]);
  },
};

/**
 * A fallback target is created after the ordinary ones (#3319): the control
 * plane refuses a fallback on a route with no ordinary target yet, and a
 * fallback is never dialled first, so the order moves no traffic. Its classes
 * go out with it.
 */
export const AddsAModelWithAFallbackTarget: Story = {
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await seededBlank(dialog);
    await userEvent.type(dialog.getByLabelText("Route name"), "gpt-burst");
    await pickOption(dialog.getByLabelText("Target 1 provider"), "vllm-cluster");
    const first = within(dialog.getByRole("group", { name: "Fallback classes of target 1" }));
    await userEvent.click(first.getByRole("button", { name: "Ordinary target" }));
    await userEvent.click(first.getByRole("button", { name: "throttled" }));
    await expect(first.getByTestId("fallback-badge")).toBeVisible();
    await userEvent.click(dialog.getByRole("button", { name: "Add target" }));
    await pickOption(await dialog.findByLabelText("Target 2 provider"), "openai-prod");

    await userEvent.click(dialog.getByRole("button", { name: "Add route" }));
    await waitFor(() => expect(sentBodies("POST", "/routes/route-new/targets")).toHaveLength(2));
    await expect(sentBodies("POST", "/routes/route-new/targets")).toEqual([
      { provider_id: "prov-1", weight: 1, takes_over: [] },
      { provider_id: "prov-2", weight: 1, takes_over: ["throttled"] },
    ]);
  },
};

/** A sheet whose every target is a fallback cannot be saved, and says why at the list. */
export const RefusesAModelOfOnlyFallbacks: Story = {
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await seededBlank(dialog);
    await userEvent.type(dialog.getByLabelText("Route name"), "gpt-stranded");
    await pickOption(dialog.getByLabelText("Target 1 provider"), "vllm-cluster");
    const first = within(dialog.getByRole("group", { name: "Fallback classes of target 1" }));
    await userEvent.click(first.getByRole("button", { name: "Ordinary target" }));
    await userEvent.click(first.getByRole("button", { name: "refusal" }));
    await userEvent.click(dialog.getByRole("button", { name: "Add route" }));
    await waitFor(() => expect(dialog.getByText(/Every target is a fallback/)).toBeVisible());
    calls.expectNotSent("POST", `/projects/${PROJECT.id}/routes`);
  },
};

/**
 * The upstream model field suggests what the picked provider lists (#2810).
 *
 * The id is chosen from the provider's own catalogue instead of retyped to the
 * letter. The catalogue is asked for when the operator reaches for it, and only
 * for the provider that line points at.
 */
export const SuggestsTheProvidersModels: Story = {
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await seededBlank(dialog);
    const upstream = dialog.getByLabelText("Target 1 upstream model");

    // with no provider there is nothing to suggest, and the field says why
    // rather than claiming the list is empty
    const none = await openOptions(upstream);
    await expect(none.parentElement).toHaveTextContent(/Pick a provider to see its models/);
    await userEvent.keyboard("{Escape}");
    calls.expectNotSent("GET", "/providers/");

    await pickOption(dialog.getByLabelText("Target 1 provider"), "vllm-cluster");
    const listbox = await openOptions(upstream);
    await waitFor(() =>
      expect(
        within(listbox)
          .getAllByRole("option")
          .map((o) => o.textContent),
      ).toEqual([
        "meta-llama/Llama-3.1-70B",
        "meta-llama/Llama-3.1-8B",
        "mistralai/Mistral-7B-Instruct-v0.3",
      ]),
    );
    // the one provider asked about is the one that line points at
    await calls.expectSent("GET", "/providers/prov-2/models");
    calls.expectNotSent("GET", "/providers/prov-1/models");
  },
};

/**
 * Typing narrows the suggestions, and picking one is what the target sends.
 */
export const PickingASuggestionSendsItUpstream: Story = {
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await seededBlank(dialog);
    await userEvent.type(dialog.getByLabelText("Route name"), "llama-3.1-8b");
    await pickOption(dialog.getByLabelText("Target 1 provider"), "vllm-cluster");
    const upstream = dialog.getByLabelText("Target 1 upstream model");
    const listbox = await openOptions(upstream);
    await waitFor(() => expect(within(listbox).getAllByRole("option")).toHaveLength(3));
    // typing narrows the list, as it does everywhere a combobox is used
    await userEvent.type(upstream, "8b");
    await waitFor(() =>
      expect(
        within(listbox)
          .getAllByRole("option")
          .map((o) => o.textContent),
      ).toEqual(["meta-llama/Llama-3.1-8B", "Use “8b”"]),
    );
    await userEvent.click(within(listbox).getByRole("option", { name: "meta-llama/Llama-3.1-8B" }));
    await expect(upstream).toHaveValue("meta-llama/Llama-3.1-8B");

    await userEvent.click(dialog.getByRole("button", { name: "Add route" }));
    const target = (await calls.expectSentBody("POST", "/routes/route-new/targets")) as {
      provider_id: string;
      upstream_model?: string;
    };
    await expect(target).toEqual({
      provider_id: "prov-2",
      upstream_model: "meta-llama/Llama-3.1-8B",
      weight: 1,
      takes_over: [],
    });
  },
};

/**
 * On a phone the suggestions are as readable as on the sheet: the field is a
 * third of the line, and ids that share a long prefix are told apart in a list
 * that takes the width the screen has instead of the field's.
 */
export const SuggestionsFitOnAPhone: Story = {
  ...atMobile,
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await seededBlank(dialog);
    await pickOption(dialog.getByLabelText("Target 1 provider"), "vllm-cluster");
    const upstream = dialog.getByLabelText("Target 1 upstream model");
    upstream.scrollIntoView({ block: "center" });
    const listbox = await openOptions(upstream);
    await waitFor(() => expect(within(listbox).getAllByRole("option")).toHaveLength(3));
    const popup = listbox.parentElement as HTMLElement;
    await expectInViewport(popup);
    // wide enough to read the id that tells the 70B from the 8B
    await expect(popup.getBoundingClientRect().width).toBeGreaterThan(250);
    await expectNoHorizontalOverflow();
  },
};

/**
 * The list only suggests: an id the provider does not list is typed and kept.
 *
 * A fine-tune, a model added since the list was cached or a provider that lists
 * nothing all still need their id to be said, and leaving the field is what
 * keeps it, with no "Use …" row to find first.
 */
export const KeepsAnUpstreamModelTheProviderDoesNotList: Story = {
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await seededBlank(dialog);
    await userEvent.type(dialog.getByLabelText("Route name"), "support-bot");
    await pickOption(dialog.getByLabelText("Target 1 provider"), "vllm-cluster");
    const upstream = dialog.getByLabelText("Target 1 upstream model");
    await userEvent.type(upstream, "acme/support-ft-v2");
    // leaving the field, not pressing Enter on a row
    await userEvent.tab();
    await waitFor(() => expect(upstream).toHaveValue("acme/support-ft-v2"));

    await userEvent.click(dialog.getByRole("button", { name: "Add route" }));
    const target = (await calls.expectSentBody("POST", "/routes/route-new/targets")) as {
      upstream_model?: string;
    };
    await expect(target.upstream_model).toBe("acme/support-ft-v2");
  },
};

/**
 * A provider whose upstream listed nothing says so, and the field is the text
 * box it always was. The control plane answers an upstream that is down, or that
 * is not a catalogue, with the same empty list, so a failure there never blocks
 * typing the id.
 */
export const SaysWhenAProviderListsNoModels: Story = {
  render: () => (
    <Stage
      mode="add"
      stub={async (input, init) =>
        String(input).includes("/providers/prov-2/models")
          ? json({ models: [] })
          : backing(input, init)
      }
    />
  ),
  play: async () => {
    const dialog = within(sheet());
    await seededBlank(dialog);
    await pickOption(dialog.getByLabelText("Target 1 provider"), "vllm-cluster");
    const upstream = dialog.getByLabelText("Target 1 upstream model");
    const listbox = await openOptions(upstream);
    await waitFor(() =>
      expect(listbox.parentElement).toHaveTextContent(/This provider listed no models/),
    );
    await userEvent.type(upstream, "meta-llama/Llama-3.1-8B");
    await userEvent.tab();
    await waitFor(() => expect(upstream).toHaveValue("meta-llama/Llama-3.1-8B"));
    // nothing on the sheet is marked as an error for it
    await expect(upstream).not.toHaveAttribute("aria-invalid");
    await expect(dialog.queryByRole("alert")).not.toBeInTheDocument();
  },
};

/**
 * A refused listing degrades quietly (#2810).
 *
 * The listing spends the provider's credential, so it takes the permission the
 * connection test takes, and a role that may build a route but not test its
 * provider is refused — `403`, or `404` for a provider it cannot see. No
 * suggestions come of it, nothing on the sheet reads as a failure of the form,
 * and the id is typed and kept as it is for any provider.
 */
function refusedListing(status: number, message: string): Story {
  return {
    render: () => (
      <Stage
        mode="add"
        toasts
        stub={async (input, init) =>
          String(input).includes("/providers/prov-2/models")
            ? json({ error: { message } }, status)
            : backing(input, init)
        }
      />
    ),
    play: async ({ canvasElement }) => {
      const dialog = within(sheet());
      await seededBlank(dialog);
      await userEvent.type(dialog.getByLabelText("Route name"), "llama-3.1-8b");
      await pickOption(dialog.getByLabelText("Target 1 provider"), "vllm-cluster");
      const upstream = dialog.getByLabelText("Target 1 upstream model");
      const listbox = await openOptions(upstream);
      await calls.expectSent("GET", "/providers/prov-2/models");
      await waitFor(() =>
        expect(listbox.parentElement).toHaveTextContent(/Suggestions are not available/),
      );
      await expect(within(listbox).queryAllByRole("option")).toHaveLength(0);
      // the refusal is not an error of the form: no alert on the sheet, no toast
      // over it, no field marked
      await expect(dialog.queryByRole("alert")).not.toBeInTheDocument();
      await expect(within(canvasElement).queryByText(message)).not.toBeInTheDocument();
      await expect(within(document.body).queryByText(message)).not.toBeInTheDocument();
      await expect(upstream).not.toHaveAttribute("aria-invalid");

      // and the id is typed as for any provider, and what is typed is sent
      await userEvent.type(upstream, "meta-llama/Llama-3.1-8B");
      await userEvent.tab();
      await waitFor(() => expect(upstream).toHaveValue("meta-llama/Llama-3.1-8B"));
      await userEvent.click(dialog.getByRole("button", { name: "Add route" }));
      const target = (await calls.expectSentBody("POST", "/routes/route-new/targets")) as {
        provider_id: string;
        upstream_model?: string;
      };
      await expect(target).toEqual({
        provider_id: "prov-2",
        upstream_model: "meta-llama/Llama-3.1-8B",
        weight: 1,
        takes_over: [],
      });
    },
  };
}

export const StaysQuietWhenTheListingIsForbidden: Story = refusedListing(
  403,
  "insufficient role for this resource",
);

export const StaysQuietWhenTheProviderIsNotVisible: Story = refusedListing(404, "provider prov-2");

/** While the catalogue is on its way, the field says it is loading and stays typeable. */
export const SaysWhileAProvidersModelsLoad: Story = {
  render: () => (
    <Stage
      mode="add"
      stub={(input, init) =>
        String(input).includes("/providers/prov-2/models")
          ? new Promise<Response>(() => {})
          : backing(input, init)
      }
    />
  ),
  play: async () => {
    const dialog = within(sheet());
    await seededBlank(dialog);
    await pickOption(dialog.getByLabelText("Target 1 provider"), "vllm-cluster");
    const upstream = dialog.getByLabelText("Target 1 upstream model");
    const listbox = await openOptions(upstream);
    await waitFor(() =>
      expect(listbox.parentElement).toHaveTextContent(/Loading this provider's models/),
    );
    await userEvent.type(upstream, "meta-llama/Llama-3.1-8B");
    await userEvent.tab();
    await waitFor(() => expect(upstream).toHaveValue("meta-llama/Llama-3.1-8B"));
  },
};

/**
 * A weight is a whole number from 1; the control plane refuses anything else.
 * The field says so when it is left, and a save with it standing is refused.
 */
export const RefusesAWeightBelowOne: Story = {
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await seededBlank(dialog);
    await userEvent.type(dialog.getByLabelText("Route name"), "qwen-72b");
    await pickOption(dialog.getByLabelText("Target 1 provider"), "vllm-cluster");
    const weight = dialog.getByLabelText("Target 1 weight");
    await userEvent.clear(weight);
    await userEvent.type(weight, "0");
    await expect(weight).not.toHaveAttribute("aria-invalid");
    await userEvent.tab();
    await waitFor(() => expect(weight).toHaveAttribute("aria-invalid", "true"));
    await expect(weight).toHaveAccessibleDescription(/whole numbers, 1 or more/);
    await userEvent.click(dialog.getByRole("button", { name: "Add route" }));
    await waitFor(() => expect(weight).toHaveFocus());
    calls.expectNotSent("POST", `/projects/${PROJECT.id}/routes`);
  },
};

/**
 * Editing a multi-target route shows every target and saves them in one call.
 *
 * The whole list goes to `PUT /routes/{id}` and the control plane keeps each
 * target it can match by (provider, upstream model), so a weight edit keeps the
 * target's id: nothing is created or deleted line by line. A line left out of
 * the list is the one removed.
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

    const weight = dialog.getByLabelText("Target 3 weight");
    await userEvent.clear(weight);
    await userEvent.type(weight, "2");
    await userEvent.click(dialog.getByRole("button", { name: "Remove target 2" }));
    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));

    const body = await routeUpdate("route-2");
    await expect(body).toEqual({
      targets: [
        {
          provider_id: "prov-2",
          upstream_model: "meta-llama/Llama-3.1-70B",
          weight: 1,
          takes_over: [],
        },
        { provider_id: "prov-1", upstream_model: null, weight: 2, takes_over: [] },
      ],
    });
    calls.expectNotSent("POST", "/routes/route-2/targets");
    calls.expectNotSent("DELETE", "/route-targets/");
  },
};

/**
 * A weight edit keeps the target. A stored target whose upstream model spells
 * out the route's own name goes back exactly as stored: the sheet's usual
 * "equal to the route name means absent" would send a different key, and the
 * control plane would replace the target with a new one (#2979).
 */
export const KeepsAStoredUpstreamEqualToTheRouteName: Story = {
  render: () => (
    <Stage
      mode="edit"
      route={ROUTE}
      stub={(input, init) =>
        String(input).includes("/routes/route-1/targets")
          ? Promise.resolve(json([{ ...TARGETS[0], upstream_model: "gpt-4o", weight: 1 }]))
          : backing(input, init)
      }
    />
  ),
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await expect(dialog.getByLabelText("Target 1 upstream model")).toHaveValue("gpt-4o");
    // the preview is the list the save sends, stored model included
    await userEvent.click(dialog.getByRole("button", { name: "Config preview" }));
    await waitFor(() => expect(sheet()).toHaveTextContent(/"upstream_model": "gpt-4o"/));
    const weight = dialog.getByLabelText("Target 1 weight");
    await userEvent.clear(weight);
    await userEvent.type(weight, "4");
    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    await expect(await routeUpdate("route-1")).toEqual({
      targets: [{ provider_id: "prov-1", upstream_model: "gpt-4o", weight: 4, takes_over: [] }],
    });
    calls.expectNotSent("POST", "/routes/route-1/targets");
    calls.expectNotSent("DELETE", "/route-targets/");
  },
};

/** The strategy is editable on a stored route and travels in the same call as the targets. */
export const ChangesTheStrategy: Story = {
  render: () => <Stage mode="edit" route={ROUTE} />,
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await pickOption(dialog.getByLabelText("Strategy"), /^weighted\b/);
    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    await expect(await routeUpdate("route-1")).toEqual({
      strategy: "weighted",
      targets: [{ provider_id: "prov-1", upstream_model: null, weight: 1, takes_over: [] }],
    });
  },
};

/** route-1 read with one variant whose only target takes over `takesOver` */
const withVariant =
  (takesOver: string[]): FetchStub =>
  (input, init) =>
    String(input).includes("/routes/route-1/variants")
      ? Promise.resolve(
          json([
            {
              id: "var-1",
              route_id: "route-1",
              name: "canary",
              weight: 1,
              position: 0,
              params: {},
              created_at: "2026-05-01T00:00:00Z",
              targets: [
                {
                  id: "vt-1",
                  variant_id: "var-1",
                  provider_id: "prov-2",
                  upstream_model: null,
                  weight: 1,
                  position: 0,
                  takes_over: takesOver,
                },
              ],
            },
          ]),
        )
      : backing(input, init);

/**
 * A route with variants dials only the variants' targets (#3487): a variant
 * with an ordinary target lets every target of the list be a fallback, and the
 * sheet saves it.
 */
export const AVariantsOrdinaryTargetAllowsAListOfFallbacks: Story = {
  render: () => <Stage mode="edit" route={ROUTE} stub={withVariant([])} />,
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    const first = within(dialog.getByRole("group", { name: "Fallback classes of target 1" }));
    await userEvent.click(first.getByRole("button", { name: "Ordinary target" }));
    await userEvent.click(first.getByRole("button", { name: "timeout" }));
    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    await expect(await routeUpdate("route-1")).toEqual({
      targets: [
        { provider_id: "prov-1", upstream_model: null, weight: 1, takes_over: ["timeout"] },
      ],
    });
  },
};

/** With every variant target a fallback too, the route could dial nothing: refused in the sheet. */
export const VariantFallbacksCountTowardsAllFallbacks: Story = {
  render: () => <Stage mode="edit" route={ROUTE} stub={withVariant(["throttled"])} />,
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    const first = within(dialog.getByRole("group", { name: "Fallback classes of target 1" }));
    await userEvent.click(first.getByRole("button", { name: "Ordinary target" }));
    await userEvent.click(first.getByRole("button", { name: "timeout" }));
    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    await waitFor(() => expect(dialog.getByText(/Every target is a fallback/)).toBeVisible());
    calls.expectNotSent("PUT", "/routes/route-1");
  },
};

/**
 * The converse (#3487): variants that hold only fallbacks strand the route even
 * over an ordinary list, since the list is not dialled while they exist. The
 * sheet refuses the save and sends nothing.
 */
export const FallbackOnlyVariantsOverAnOrdinaryListAreRefused: Story = {
  render: () => <Stage mode="edit" route={ROUTE} stub={withVariant(["throttled"])} />,
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    const weight = dialog.getByLabelText("Target 1 weight");
    await userEvent.clear(weight);
    await userEvent.type(weight, "2");
    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    // the variants strand it, so the message sits beside them rather than under
    // the list, and focus moves to where they are edited (#3510)
    const edit = dialog.getByRole("link", { name: "Edit the variants on Routing Rules" });
    await waitFor(() => expect(edit).toHaveFocus());
    await expect(edit).toHaveAccessibleDescription(/Variants.*every one of this route's/);
    await expect(dialog.getAllByText(/Every target is a fallback/)).toHaveLength(1);
    await expect(dialog.getByLabelText("Target 1 weight")).toBeValid();
    calls.expectNotSent("PUT", "/routes/route-1");
  },
};

/**
 * A refusal the control plane pins to a target (`targets[1].weight`) is said at
 * that line, the sheet stays open and nothing is announced as saved.
 */
export const MapsARefusedSaveToItsRow: Story = {
  render: () => (
    <Stage
      mode="edit"
      route={FLEET_ROUTE}
      stub={(input, init) =>
        init?.method === "PUT" && String(input).endsWith("/routes/route-2")
          ? Promise.resolve(
              json(
                {
                  error: {
                    code: "invalid_field",
                    message: "weight must be at least 1",
                    field: "targets[1].weight",
                  },
                },
                400,
              ),
            )
          : backing(input, init)
      }
    />
  ),
  play: async () => {
    const dialog = within(sheet());
    await waitFor(() =>
      expect(dialog.getByLabelText("Target 3 provider")).toHaveValue("openai-prod"),
    );
    const weight = dialog.getByLabelText("Target 2 weight");
    await userEvent.clear(weight);
    await userEvent.type(weight, "5");
    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    await waitFor(() => expect(dialog.getByLabelText("Target 2 weight")).toBeInvalid());
    await expect(dialog.getByLabelText("Target 1 weight")).toBeValid();
    await waitFor(() => expect(dialog.getByText(/refused the weight of target 2/)).toBeVisible());
    await expect(sheet()).toBeVisible();
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
    await seededBlank(dialog);
    await pickOption(dialog.getByLabelText("Duplicate from"), "llama-70b");
    await waitFor(() => expect(dialog.getByLabelText("Strategy")).toHaveValue("cache_aware"));
    await expect(await dialog.findByLabelText("Target 3 upstream model")).toHaveValue("llama-70b");
    // the copy carries the source's name, which is taken, until it is renamed
    await expect(dialog.getByLabelText("Route name")).toHaveAccessibleDescription(/already exists/);
  },
};

/** the effective config as the sheet reads it for the rules an override can name */
const POLICY_DOC = {
  feature_flags: { guardrails: true },
  guardrails: {
    enabled: true,
    streaming_post_call: "reject",
    rules: [
      {
        name: "email",
        builtin: "email",
        stage: "pre_call",
        action: "redact",
        include_system: false,
      },
      {
        name: "phone",
        builtin: "phone",
        stage: "pre_call",
        action: "block",
        include_system: false,
      },
    ],
  },
  routes: [
    { model: "fake-llm", advanced: { guardrails: { disable: ["phone"], enable: ["gone"] } } },
  ],
};

const withPolicy: FetchStub = async (input, init) =>
  /\/api\/v1\/config(\?|$)/.test(String(input)) ? json(POLICY_DOC) : backing(input, init);

/** a route that switches `email` off and names a rule that no longer exists */
const GUARDED_ROUTE: RouteRow = {
  ...ROUTE,
  advanced: {
    ...ADVANCED_ROUTE.advanced,
    guardrails: { disable: ["email", "gone"], rules: ["pii-out"] },
  },
};

/**
 * A route's guardrail overrides are edited here (#2307). The rules on offer are
 * the ones in force, a stored name that matches none stays a marked chip so it
 * can be removed, and the saved `advanced` body carries the edited lists next to
 * the keys the sheet does not model.
 */
export const EditsGuardrailOverrides: Story = {
  render: () => <Stage mode="edit" route={GUARDED_ROUTE} stub={withPolicy} />,
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await userEvent.click(dialog.getByRole("button", { name: "Guardrails" }));
    const off = within(await dialog.findByRole("group", { name: "Switched off on this route" }));
    const on = within(dialog.getByRole("group", { name: "Forced on for this route" }));
    await expect(off.getByRole("button", { name: "email" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    await expect(off.getByRole("button", { name: "phone" })).toHaveAttribute(
      "aria-pressed",
      "false",
    );
    // the stale name is shown as what it is, and pressing it removes it
    const stale = off.getByRole("button", { name: "gone (no such rule)" });
    await expect(stale).toHaveAttribute("aria-pressed", "true");
    await userEvent.click(stale);
    await userEvent.click(off.getByRole("button", { name: "email" }));
    await userEvent.click(off.getByRole("button", { name: "phone" }));
    await userEvent.click(on.getByRole("button", { name: "email" }));
    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    const body = (await calls.expectSentBody("PUT", "/routes/route-1/advanced")) as {
      advanced: { guardrails: unknown };
    };
    await expect(body.advanced.guardrails).toEqual({
      disable: ["phone"],
      enable: ["email"],
      rules: ["pii-out"],
    });
  },
};

/** Switching the last override off sends no `guardrails` object at all. */
export const ClearingEveryGuardrailOverrideSendsNone: Story = {
  render: () => (
    <Stage
      mode="edit"
      route={{
        ...ROUTE,
        advanced: { ...ADVANCED_ROUTE.advanced, guardrails: { disable: ["email"] } },
      }}
      stub={withPolicy}
    />
  ),
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await userEvent.click(dialog.getByRole("button", { name: "Guardrails" }));
    const off = within(await dialog.findByRole("group", { name: "Switched off on this route" }));
    await userEvent.click(off.getByRole("button", { name: "email" }));
    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    const body = (await calls.expectSentBody("PUT", "/routes/route-1/advanced")) as {
      advanced: Record<string, unknown>;
    };
    await expect(body.advanced).not.toHaveProperty("guardrails");
  },
};

/** The same section in Russian, with the stale name marked in the operator's language. */
export const GuardrailOverridesInRussian: Story = {
  globals: { locale: "ru" },
  render: () => <Stage mode="edit" route={GUARDED_ROUTE} stub={withPolicy} />,
  play: async () => {
    const dialog = within(sheet());
    await userEvent.click(
      await dialog.findByRole("button", { name: ru.modelSheet.sections.guardrails }),
    );
    const off = within(
      await dialog.findByRole("group", { name: ru.modelSheet.guardrails.disable }),
    );
    await expect(
      off.getByRole("button", {
        name: ru.modelSheet.guardrails.unknownRule.replace("{{name}}", "gone"),
      }),
    ).toHaveAttribute("aria-pressed", "true");
  },
};

/**
 * A config-file route shows the overrides `rolter.toml` gives it and cannot be
 * edited from here (#2307).
 */
export const ShowsAConfigRoutesGuardrailOverridesReadOnly: Story = {
  render: () => (
    <Stage
      mode="view"
      configModel={{ model: "fake-llm", strategy: "weighted", targets: 1, source: "config" }}
      stub={withPolicy}
    />
  ),
  play: async () => {
    const dialog = within(sheet());
    await userEvent.click(await dialog.findByRole("button", { name: "Guardrails" }));
    await waitFor(() => expect(dialog.getByText(/declared in the config file/)).toBeVisible());
    const off = within(await dialog.findByRole("group", { name: "Switched off on this route" }));
    const phone = await off.findByRole("button", { name: "phone" });
    await expect(phone).toHaveAttribute("aria-pressed", "true");
    await expect(phone).toBeDisabled();
  },
};

/**
 * #3460: an organization's route of the same name comes first in the effective
 * config and switches `email` off. The config-file route's sheet shows the
 * file route's own overrides, never the organization's namesake's.
 */
export const AConfigRoutesOverridesAreNotItsNamesakes: Story = {
  render: () => (
    <Stage
      mode="view"
      configModel={{ model: "fake-llm", strategy: "weighted", targets: 1, source: "config" }}
      stub={async (input, init) =>
        /\/api\/v1\/config(\?|$)/.test(String(input))
          ? json({
              ...POLICY_DOC,
              routes: [
                {
                  model: "fake-llm",
                  tenancy: { org_id: "org-2", project_id: "project-9" },
                  advanced: { guardrails: { disable: ["email"] } },
                },
                ...POLICY_DOC.routes,
              ],
            })
          : backing(input, init)
      }
    />
  ),
  play: async () => {
    const dialog = within(sheet());
    await userEvent.click(await dialog.findByRole("button", { name: "Guardrails" }));
    const off = within(await dialog.findByRole("group", { name: "Switched off on this route" }));
    await expect(await off.findByRole("button", { name: "phone" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    await expect(off.getByRole("button", { name: "email" })).toHaveAttribute(
      "aria-pressed",
      "false",
    );
  },
};

/**
 * A read-only sheet has nothing to discard, so its footer offers a single
 * Close rather than a Cancel beside a Close that do the same (#2046).
 */
export const ReadOnlyFooterHasOneAction: Story = {
  render: () => (
    <Stage
      mode="view"
      configModel={{ model: "fake-llm", strategy: "weighted", targets: 1, source: "config" }}
    />
  ),
  play: async () => {
    const dialog = within(sheet());
    // the header's X is labelled Close too; the footer's is the one with text
    const closes = await dialog.findAllByRole("button", { name: "Close" });
    const close = closes.find((b) => b.textContent === "Close");
    await expect(close).toBeDefined();
    const footer = within(close!.parentElement as HTMLElement);
    await expect(footer.getAllByRole("button")).toHaveLength(1);
    await expect(dialog.queryByRole("button", { name: "Cancel" })).toBeNull();
    await userEvent.click(close!);
    await expectSheetClosed();
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
    const retries = dialog.getByLabelText("Max retries");
    await expect(retries).toHaveValue(1);
    // zero turns retries off for the route, so it is sent and not left blank
    await userEvent.clear(retries);
    await userEvent.type(retries, "0");

    await userEvent.click(dialog.getByRole("button", { name: "Custom request headers" }));
    const headerValue = dialog.getByLabelText("Header value");
    await expect(headerValue).toHaveValue("acme");
    await userEvent.clear(headerValue);
    await userEvent.type(headerValue, "beta");

    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    const body = (await calls.expectSentBody("PUT", "/routes/route-1/advanced")) as {
      advanced: {
        limits: { retries: number; timeout_secs: number };
        headers: Record<string, string>;
        locked_headers: string[];
        guardrails: unknown;
      };
    };
    await expect(body.advanced.limits.retries).toBe(0);
    // milliseconds on screen, whole seconds on the wire
    await expect(body.advanced.limits.timeout_secs).toBe(30);
    await expect(body.advanced.headers).toEqual({ "X-Tenant": "beta" });
    await expect(body.advanced.locked_headers).toEqual(["X-Tenant"]);
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
    await waitFor(() => expect(dialog.getByLabelText("Max retries")).toBeVisible());
    await expect(dialog.queryByText("Allow additional fields")).not.toBeInTheDocument();
  },
};

/**
 * A route that carries keys the gateway never read (#2924): a per-route
 * endpoint, an insecure-TLS switch, flat image and audio prices, and a context
 * window. The sheet offers none of them, and a save sheds them from the stored
 * blob instead of copying them through, so the routes API stops reading back a
 * setting that looks like policy and does nothing. What the gateway does apply
 * (timeout, retries, the throughput caps of #2936, headers, the guardrail
 * selection) is kept.
 */
const LEGACY_ROUTE: RouteRow = {
  ...ADVANCED_ROUTE,
  advanced: {
    ...ADVANCED_ROUTE.advanced,
    base_url: "https://models.internal/v1",
    insecure_tls: true,
    pricing: { image_per_unit: 0.04, audio_input_per_minute: 0.006 },
    limits: {
      rpm: 600,
      tpm: 90000,
      concurrency: 4,
      context_window: 128000,
      timeout_secs: 30,
      retries: 1,
      output_tokens: 4096,
    },
  },
};

export const ShedsTheSettingsTheGatewayNeverRead: Story = {
  render: () => <Stage mode="edit" route={LEGACY_ROUTE} />,
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await userEvent.click(dialog.getByRole("button", { name: "Limits & network" }));
    // the limits that apply are there, and none of what does not
    await expect(dialog.getByLabelText("Timeout (ms)")).toHaveValue(30000);
    await expect(dialog.getByLabelText("Max retries")).toHaveValue(1);
    await expect(dialog.getByLabelText("Max output tokens")).toHaveValue(4096);
    await expect(dialog.getByLabelText("Requests per minute")).toHaveValue(600);
    await expect(dialog.getByLabelText("Tokens per minute")).toHaveValue(90000);
    await expect(dialog.getByLabelText("Concurrent requests")).toHaveValue(4);
    for (const gone of ["Base URL override", "Context window"]) {
      await expect(dialog.queryByLabelText(gone)).not.toBeInTheDocument();
    }
    await expect(dialog.queryByText("Allow insecure TLS")).not.toBeInTheDocument();

    // a change on the advanced half, so the blob is written and can be read
    await userEvent.click(dialog.getByRole("button", { name: "Custom request headers" }));
    const headerValue = dialog.getByLabelText("Header value");
    await userEvent.clear(headerValue);
    await userEvent.type(headerValue, "beta");
    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    const body = (await calls.expectSentBody("PUT", "/routes/route-1/advanced")) as {
      advanced: Record<string, unknown>;
    };
    for (const key of ["base_url", "insecure_tls", "pricing"]) {
      await expect(body.advanced).not.toHaveProperty(key);
    }
    // the caps survive an edit of something else: re-saving must not reset a
    // limit the operator set, and `context_window` is the one that is shed
    await expect(body.advanced.limits).toEqual({
      timeout_secs: 30,
      retries: 1,
      output_tokens: 4096,
      rpm: 600,
      tpm: 90000,
      concurrency: 4,
    });
    await expect(body.advanced.headers).toEqual({ "X-Tenant": "beta" });
    await expect(body.advanced.guardrails).toEqual({ rules: ["pii-out"] });
  },
};

/**
 * A route's throughput caps are typed in the sheet and travel in the advanced
 * blob (#2936). Blank is no cap and is not sent; a cleared field drops the cap
 * from the stored blob.
 */
export const SavesTheThroughputCaps: Story = {
  render: () => <Stage mode="edit" route={LEGACY_ROUTE} />,
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await userEvent.click(dialog.getByRole("button", { name: "Limits & network" }));
    const rpm = dialog.getByLabelText("Requests per minute");
    await userEvent.clear(rpm);
    await userEvent.type(rpm, "120");
    // clearing a cap removes it rather than sending a zero the backend refuses
    await userEvent.clear(dialog.getByLabelText("Tokens per minute"));
    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    const body = (await calls.expectSentBody("PUT", "/routes/route-1/advanced")) as {
      advanced: { limits: Record<string, unknown> };
    };
    await expect(body.advanced.limits.rpm).toBe(120);
    await expect(body.advanced.limits).not.toHaveProperty("tpm");
    await expect(body.advanced.limits.concurrency).toBe(4);
  },
};

/**
 * A route's context window is typed in the sheet and travels in the advanced
 * blob as `context_length` (#3265): the stored value seeds the row, an edit is
 * sent as a number, and a cleared field drops it rather than sending a zero.
 */
export const EditsTheContextLength: Story = {
  render: () => (
    <Stage
      mode="edit"
      route={{ ...LEGACY_ROUTE, advanced: { ...LEGACY_ROUTE.advanced, context_length: 200000 } }}
    />
  ),
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await userEvent.click(dialog.getByRole("button", { name: "Limits & network" }));
    const field = await dialog.findByLabelText("Context length (tokens)");
    await expect(field).toHaveValue(200000);
    await expect(field).toHaveAccessibleDescription(/served on GET \/v1\/models/);
    await userEvent.clear(field);
    await userEvent.type(field, "128000");
    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    const body = (await calls.expectSentBody("PUT", "/routes/route-1/advanced")) as {
      advanced: Record<string, unknown>;
    };
    await expect(body.advanced.context_length).toBe(128000);
  },
};

export const ClearingTheContextLengthDropsIt: Story = {
  render: () => (
    <Stage
      mode="edit"
      route={{ ...LEGACY_ROUTE, advanced: { ...LEGACY_ROUTE.advanced, context_length: 200000 } }}
    />
  ),
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await userEvent.click(dialog.getByRole("button", { name: "Limits & network" }));
    await userEvent.clear(await dialog.findByLabelText("Context length (tokens)"));
    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    const body = (await calls.expectSentBody("PUT", "/routes/route-1/advanced")) as {
      advanced: Record<string, unknown>;
    };
    await expect(body.advanced).not.toHaveProperty("context_length");
  },
};

export const ContextLengthOfZeroIsRefused: Story = {
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await seededBlank(dialog);
    await userEvent.click(dialog.getByRole("button", { name: "Limits & network" }));
    const field = await dialog.findByLabelText("Context length (tokens)");
    await userEvent.type(field, "0");
    await userEvent.tab();
    await waitFor(() => expect(field).toHaveAttribute("aria-invalid", "true"));
    await expect(field).toHaveAccessibleDescription(/whole number of tokens, 1 or more/);
  },
};

/**
 * `GET /v1/models` serves a route's context length whatever its model type, and
 * an embedding model's input limit is the one clients most need to size
 * against (#3406). The row is in Limits & network, which every type shows, so an
 * embedding route seeds it, edits it and saves it like a chat one, with no
 * Capabilities section in the sheet at all.
 */
export const EditsTheContextLengthOfAnEmbeddingRoute: Story = {
  render: () => (
    <Stage
      mode="edit"
      route={{
        ...LEGACY_ROUTE,
        advanced: { ...LEGACY_ROUTE.advanced, model_type: "embedding", context_length: 8192 },
      }}
    />
  ),
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await expect(dialog.queryByRole("button", { name: "Capabilities" })).toBeNull();
    await userEvent.click(dialog.getByRole("button", { name: "Limits & network" }));
    const field = await dialog.findByLabelText("Context length (tokens)");
    await expect(field).toHaveValue(8192);
    await userEvent.clear(field);
    await userEvent.type(field, "32768");
    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    const body = (await calls.expectSentBody("PUT", "/routes/route-1/advanced")) as {
      advanced: Record<string, unknown>;
    };
    await expect(body.advanced.context_length).toBe(32768);
    await expect(body.advanced.model_type).toBe("embedding");
  },
};

/**
 * A cap the control plane would refuse is said at its field before the save
 * (#2936): zero would turn every request away, and blank is the way to say
 * "no cap". Each message is stated once and the save puts focus on the field.
 */
export const ThroughputCapsOutOfRangeAreRefused: Story = {
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await seededBlank(dialog);
    await userEvent.click(dialog.getByRole("button", { name: "Limits & network" }));
    const rpm = dialog.getByLabelText("Requests per minute");
    await userEvent.type(rpm, "0");
    await expect(rpm).not.toHaveAttribute("aria-invalid");
    await userEvent.tab();
    await waitFor(() => expect(rpm).toHaveAttribute("aria-invalid", "true"));
    await expect(rpm).toHaveAccessibleDescription(/whole number from 1 to 10000000/);
    await expect(dialog.getAllByText(/whole number from 1 to 10000000/)).toHaveLength(1);
    await userEvent.clear(rpm);
    await userEvent.type(rpm, "10");
    await userEvent.tab();
    await waitFor(() => expect(rpm).not.toHaveAttribute("aria-invalid"));

    const concurrency = dialog.getByLabelText("Concurrent requests");
    await userEvent.type(concurrency, "1.5");
    await userEvent.tab();
    await waitFor(() => expect(concurrency).toHaveAttribute("aria-invalid", "true"));
    await userEvent.type(dialog.getByLabelText("Route name"), "qwen-72b");
    await pickOption(dialog.getByLabelText("Target 1 provider"), "vllm-cluster");
    await userEvent.click(dialog.getByRole("button", { name: "Add route" }));
    await waitFor(() => expect(concurrency).toHaveFocus());
    calls.expectNotSent("POST", `/projects/${PROJECT.id}/routes`);
  },
};

/**
 * A model type billed per image or per minute is priced by that unit (#2937),
 * not by tokens: the sheet offers the unit rates for it instead of the token
 * ones, and a chat model keeps its token rates.
 */
export const PricesPerUnitModels: Story = {
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await seededBlank(dialog);
    await pickOption(dialog.getByLabelText("Model type"), "image");
    await userEvent.click(dialog.getByRole("button", { name: "Pricing override" }));
    await waitFor(() => expect(dialog.getByLabelText(IMAGE_LABEL)).toBeVisible());
    await expect(dialog.queryByLabelText(/Input USD\/Mtok/)).not.toBeInTheDocument();
    await pickOption(dialog.getByLabelText("Model type"), "audio");
    await expect(await dialog.findByLabelText(AUDIO_LABEL)).toBeVisible();
    await expect(dialog.getByLabelText(SPEECH_LABEL)).toBeVisible();
    await expect(dialog.queryByLabelText(IMAGE_LABEL)).not.toBeInTheDocument();
    // an audio model's audio tokens are priced too (#3145)
    await expect(dialog.getByLabelText(AUDIO_IN_TOKENS_LABEL)).toBeVisible();
    await expect(dialog.getByLabelText(AUDIO_OUT_TOKENS_LABEL)).toBeVisible();
    await expect(dialog.getByLabelText(CACHED_AUDIO_LABEL)).toBeVisible();
    // and a chat model still gets its rates
    await pickOption(dialog.getByLabelText("Model type"), "chat");
    await expect(await dialog.findByLabelText(/Input USD\/Mtok/)).toBeVisible();
    await expect(dialog.queryByLabelText(AUDIO_LABEL)).not.toBeInTheDocument();
    await expect(dialog.queryByLabelText(AUDIO_IN_TOKENS_LABEL)).not.toBeInTheDocument();
  },
};

const PROBE: ProviderTestResult = {
  reachable: true,
  probed_url: "https://api.openai.com/v1/models",
  status: 200,
  latency_ms: 84,
  credential: "stored",
  models_found: 2,
  model_listed: true,
  error: null,
};

/** the sheet's stub, answering the provider test with `answer` */
function probing(answer: (url: string) => Response | Promise<Response>): FetchStub {
  return async (input, init) =>
    String(input).includes("/providers/prov-1/test") ? answer(String(input)) : backing(input, init);
}

/** a draft that names a provider and an upstream model, ready to be checked */
async function draftToCheck(dialog: ReturnType<typeof within>): Promise<HTMLElement> {
  await seeded(dialog);
  const upstream = dialog.getByLabelText("Target 1 upstream model");
  await userEvent.type(upstream, "gpt-4o");
  await userEvent.tab();
  return dialog.getByRole("button", { name: "Check target 1 upstream model with its provider" });
}

/**
 * The check asks the provider whether it lists the upstream model (#2009).
 *
 * The probe is given the model the line would send, and "listed" says what was
 * verified: the provider answered, and its catalogue has that name.
 */
export const ChecksThatTheProviderListsTheModel: Story = {
  render: () => <Stage mode="edit" route={ROUTE} stub={probing(() => json(PROBE))} />,
  play: async () => {
    const dialog = within(sheet());
    const check = await draftToCheck(dialog);
    await userEvent.click(check);
    await calls.expectSent("POST", "/providers/prov-1/test?model=gpt-4o");
    await waitFor(() =>
      expect(dialog.getByText("openai-prod answered and lists gpt-4o.")).toBeVisible(),
    );
  },
};

/** a model the provider answered without listing is a warning, not an OK */
export const SaysWhenTheProviderDoesNotListTheModel: Story = {
  render: () => (
    <Stage
      mode="edit"
      route={ROUTE}
      stub={probing(() => json({ ...PROBE, model_listed: false }))}
    />
  ),
  play: async () => {
    const dialog = within(sheet());
    await userEvent.click(await draftToCheck(dialog));
    await waitFor(() =>
      expect(dialog.getByText(/its model list does not include gpt-4o/)).toBeVisible(),
    );
    await expect(dialog.queryByText(/answered and lists/)).not.toBeInTheDocument();
  },
};

/** a provider with no catalogue cannot confirm the model, and says so */
export const SaysWhenTheProviderHasNoCatalogueToCheck: Story = {
  render: () => (
    <Stage
      mode="edit"
      route={ROUTE}
      stub={probing(() => json({ ...PROBE, models_found: null, model_listed: null }))}
    />
  ),
  play: async () => {
    const dialog = within(sheet());
    await userEvent.click(await draftToCheck(dialog));
    await waitFor(() =>
      expect(dialog.getByText(/has no model list to look gpt-4o up in/)).toBeVisible(),
    );
    await expect(dialog.queryByText(/answered and lists/)).not.toBeInTheDocument();
  },
};

/**
 * A failed probe shows the server's reason and the URL it tried, and nothing
 * on the line reads as a pass (#2009).
 */
export const ShowsWhyTheCheckFailedAndNeverReadsAsOk: Story = {
  render: () => (
    <Stage
      mode="edit"
      route={ROUTE}
      stub={probing(() =>
        json({
          ...PROBE,
          reachable: false,
          status: 401,
          models_found: null,
          model_listed: null,
          error: "401: the upstream rejected the credential (resolved from: stored)",
        }),
      )}
    />
  ),
  play: async () => {
    const dialog = within(sheet());
    await userEvent.click(await draftToCheck(dialog));
    await waitFor(() => expect(dialog.getByText("Could not reach this provider")).toBeVisible());
    await expect(dialog.getByText(/the upstream rejected the credential/)).toBeVisible();
    await expect(dialog.getByText("https://api.openai.com/v1/models")).toBeVisible();
    await expect(dialog.queryByText(/ok\b/i)).not.toBeInTheDocument();
    await expect(dialog.queryByText(/lists gpt-4o/)).not.toBeInTheDocument();
  },
};

/** a refused request (the role may not spend the credential) is shown as the error it is */
export const ShowsARefusedCheck: Story = {
  render: () => (
    <Stage
      mode="edit"
      route={ROUTE}
      stub={probing(() =>
        json({ error: { message: "provider:update needs the admin role" } }, 403),
      )}
    />
  ),
  play: async () => {
    const dialog = within(sheet());
    await userEvent.click(await draftToCheck(dialog));
    await waitFor(() => expect(dialog.getByRole("alert")).toBeVisible());
    await expect(dialog.queryByText(/answered and lists/)).not.toBeInTheDocument();
  },
};

/** while the probe is out the button is busy and the line says what it is waiting on */
export const ShowsThatTheCheckIsPending: Story = {
  render: () => <Stage mode="edit" route={ROUTE} stub={probing(() => new Promise(() => {}))} />,
  play: async () => {
    const dialog = within(sheet());
    const check = await draftToCheck(dialog);
    await userEvent.click(check);
    await waitFor(() =>
      expect(dialog.getByText("Asking openai-prod for its models…")).toBeVisible(),
    );
    await expect(check).toBeDisabled();
    await expect(dialog.queryByText(/answered and lists/)).not.toBeInTheDocument();
  },
};

/** an answer is about one provider and one name, so editing either clears it */
export const ClearsTheAnswerWhenTheNameChanges: Story = {
  render: () => <Stage mode="edit" route={ROUTE} stub={probing(() => json(PROBE))} />,
  play: async () => {
    const dialog = within(sheet());
    await userEvent.click(await draftToCheck(dialog));
    await dialog.findByText("openai-prod answered and lists gpt-4o.");
    await userEvent.type(dialog.getByLabelText("Target 1 upstream model"), "-mini");
    await userEvent.tab();
    await waitFor(() => expect(dialog.queryByText(/answered and lists/)).not.toBeInTheDocument());
  },
};

/** nothing to check until a provider and a name are chosen; a blank name checks the public one */
export const TheCheckWaitsForAProviderAndAName: Story = {
  render: () => <Stage mode="add" stub={probing(() => json(PROBE))} />,
  play: async () => {
    const dialog = within(sheet());
    await seededBlank(dialog);
    const check = dialog.getByRole("button", {
      name: "Check target 1 upstream model with its provider",
    });
    await expect(check).toBeDisabled();
    await pickOption(dialog.getByLabelText("Target 1 provider"), "openai-prod");
    // a provider but no name, and no public name to stand in for it
    await expect(check).toBeDisabled();
    await userEvent.type(dialog.getByLabelText("Route name"), "gpt-4o");
    await waitFor(() => expect(check).toBeEnabled());
    await userEvent.click(check);
    await calls.expectSent("POST", "/providers/prov-1/test?model=gpt-4o");
  },
};

/** the probe spends the provider's credential, so it follows `provider:update` */
export const TheCheckIsRefusedWithoutProviderUpdate: Story = {
  render: () => <Stage mode="edit" route={ROUTE} role="viewer" />,
  play: async () => {
    const dialog = sheet();
    await seeded(within(dialog));
    await expectRefused(dialog, "Check target 1 upstream model with its provider", undefined);
  },
};

export const TheCheckIsAllowedForAProviderAdmin: Story = {
  render: () => <Stage mode="edit" route={ROUTE} role="admin" />,
  play: async () => {
    const dialog = sheet();
    await seeded(within(dialog));
    await expectAllowed(dialog, "Check target 1 upstream model with its provider");
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
    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    await calls.expectSent("PUT", "/routes/route-1/params");
    calls.expectNotSent("PUT", "/routes/route-1/advanced");
    // the targets go back unchanged in the one update call, never line by line
    await expect(await routeUpdate("route-1")).toEqual({
      targets: [{ provider_id: "prov-1", upstream_model: null, weight: 1, takes_over: [] }],
    });
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

    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
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

    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    const body = (await calls.expectSentBody("PUT", "/routes/route-1/advanced")) as {
      advanced: { visibility: Record<string, unknown> };
    };
    await expect(body.advanced.visibility).not.toHaveProperty("project_only");
  },
};

/**
 * The control plane refuses a limit of its own accord — `validate_advanced`
 * caps the token limit at ten million. The sheet says which half of the save
 * failed instead of printing the message on its own.
 */
export const AdvancedRejected: Story = {
  render: () => (
    <Stage
      mode="edit"
      route={ADVANCED_ROUTE}
      stub={async (input, init) => {
        const url = String(input);
        if (url.includes("/advanced")) {
          return json({ error: { message: "output_tokens must be between 1 and 10000000" } }, 400);
        }
        return backing(input, init);
      }}
    />
  ),
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await userEvent.click(dialog.getByRole("button", { name: "Limits & network" }));
    const maxOutput = dialog.getByLabelText("Max output tokens");
    await userEvent.clear(maxOutput);
    await userEvent.type(maxOutput, "99999999");
    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    await waitFor(() =>
      expect(dialog.getByRole("alert")).toHaveTextContent(/advanced configuration/),
    );
    await expect(dialog.getByRole("alert")).toHaveTextContent(/output_tokens must be between/);
  },
};

/**
 * Prices are operator-supplied — rolter ships no pricing catalog — so the
 * pricing section points at our own cost docs rather than at a competitor's
 * datasheet presented as their source (#977). The link goes through the
 * configured docs host like every other docs link, never github.com (#1981).
 */
export const PricingLinksToRolterDocs: Story = {
  beforeEach: withDocsBase("https://docs.example.com"),
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await userEvent.click(dialog.getByRole("button", { name: "Pricing override" }));
    const link = dialog.getByRole("link", { name: /How request cost is calculated/ });
    await expect(link).toHaveAttribute(
      "href",
      "https://docs.example.com/observability/logs-and-cost#cost-tracking",
    );
    await expect(link).toHaveAttribute("target", "_blank");
    await expect(link).toHaveAttribute("rel", "noreferrer");
  },
};

/**
 * An air-gapped deployment configures no docs host, and the pricing section
 * then shows no link rather than one that reaches for an unreachable host.
 */
export const PricingWithoutDocsHostHasNoLink: Story = {
  beforeEach: withDocsBase(undefined),
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await userEvent.click(dialog.getByRole("button", { name: "Pricing override" }));
    await waitFor(() =>
      expect(dialog.getByText(/fields shown match the model type/)).toBeVisible(),
    );
    await expect(dialog.queryByRole("link", { name: /How request cost/ })).toBeNull();
  },
};

/**
 * The price table with one row for `gpt-4o`, carrying the two cache-write rates
 * as the control plane returns them: decimal text, six places, or null for no
 * rate.
 */
function pricedAt(
  cacheWrite: string | null,
  cacheWrite1h: string | null = null,
  units: Partial<
    Pick<
      ModelPriceRow,
      | "image_per_unit"
      | "audio_input_per_minute"
      | "speech_per_mchar"
      | "audio_input_per_mtok"
      | "audio_output_per_mtok"
      | "cached_audio_input_per_mtok"
      | "source"
      | "source_imported_at"
    >
  > = {},
): FetchStub {
  const row: ModelPriceRow = {
    id: "price-1",
    model: "gpt-4o",
    input_per_mtok: "2.500000",
    output_per_mtok: "10.000000",
    cached_input_per_mtok: "1.250000",
    cache_write_per_mtok: cacheWrite,
    cache_write_1h_per_mtok: cacheWrite1h,
    image_per_unit: null,
    audio_input_per_minute: null,
    speech_per_mchar: null,
    ...units,
    currency: "USD",
    created_at: "2026-07-01T00:00:00Z",
  };
  return async (input, init) =>
    String(input).includes("/model-prices") && (init?.method ?? "GET") === "GET"
      ? json([row])
      : backing(input, init);
}

/**
 * The route as an older release left it: its `advanced.pricing` carries a
 * cache-write rate the sheet used to write there and no cost path ever read.
 */
const STALE_RATE_ROUTE: RouteRow = {
  ...ADVANCED_ROUTE,
  advanced: { ...ADVANCED_ROUTE.advanced, pricing: { cache_write_per_mtok: 9 } },
};

const WRITE_LABEL = "5 minute cache-write USD/Mtok";
const WRITE_1H_LABEL = "1 hour cache-write USD/Mtok";

/**
 * The cache-write inputs are the price row's (#2890). They open on the rates the
 * row holds, never on the route's own `advanced.pricing` copy, which nothing
 * reads and which a stored blob can still carry. Saving without touching them
 * names no rate, so the control plane keeps the ones it holds (#2876, #2902).
 */
export const ReadsTheCacheWriteRateFromThePriceRow: Story = {
  render: () => (
    <Stage mode="edit" route={STALE_RATE_ROUTE} stub={pricedAt("3.750000", "6.000000")} />
  ),
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await userEvent.click(dialog.getByRole("button", { name: "Pricing override" }));
    const write = dialog.getByLabelText(WRITE_LABEL);
    await expect(write).toHaveValue(3.75);
    await expect(write).toHaveAccessibleDescription(
      "Anthropic's 5 minute cache, and any write a provider reports without a cache lifetime. Leave empty to price them at the input rate.",
    );
    // the 1 hour input is the price row's too, and says what empty falls back to
    const hour = dialog.getByLabelText(WRITE_1H_LABEL);
    await expect(hour).toHaveValue(6);
    await expect(hour).toHaveAttribute("placeholder", "5 minute rate");
    await expect(hour).toHaveAccessibleDescription(
      "Anthropic's 1 hour cache only. Leave empty to price those writes at the 5 minute rate, or the input rate when that is empty too.",
    );

    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    const body = (await calls.expectSentBody("PUT", "/api/v1/model-prices")) as Record<
      string,
      unknown
    >;
    await expect(body).toEqual({
      model: "gpt-4o",
      input_per_mtok: "2.500000",
      output_per_mtok: "10.000000",
      cached_input_per_mtok: "1.250000",
      currency: "USD",
    });
  },
};

/**
 * A rate typed into the sheet goes with the price row, and the route's
 * `advanced.pricing` is no longer written: a save that changes the advanced
 * blob sheds the stale copy instead of carrying it on (#2890).
 */
export const SavesTheCacheWriteRateOnThePriceRow: Story = {
  render: () => <Stage mode="edit" route={STALE_RATE_ROUTE} stub={pricedAt(null)} />,
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await userEvent.click(dialog.getByRole("button", { name: "Pricing override" }));
    const write = dialog.getByLabelText(WRITE_LABEL);
    await expect(write).toHaveValue(null);
    await userEvent.type(write, "3.75");
    // a change on the advanced half too, so the blob is written and can be read
    await userEvent.click(dialog.getByRole("button", { name: "Limits & network" }));
    const maxOutput = dialog.getByLabelText("Max output tokens");
    await userEvent.clear(maxOutput);
    await userEvent.type(maxOutput, "900");

    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    const price = (await calls.expectSentBody("PUT", "/api/v1/model-prices")) as Record<
      string,
      unknown
    >;
    await expect(price.cache_write_per_mtok).toBe("3.75");
    const advanced = (await calls.expectSentBody("PUT", "/routes/route-1/advanced")) as {
      advanced: Record<string, unknown>;
    };
    await expect(advanced.advanced.limits).toMatchObject({ output_tokens: 900 });
    await expect(advanced.advanced).not.toHaveProperty("pricing");
  },
};

/** Emptying the input clears the rate, which is `null` on the wire */
export const ClearsTheCacheWriteRateOnThePriceRow: Story = {
  render: () => <Stage mode="edit" route={ROUTE} stub={pricedAt("3.750000")} />,
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await userEvent.click(dialog.getByRole("button", { name: "Pricing override" }));
    const write = dialog.getByLabelText(WRITE_LABEL);
    await expect(write).toHaveValue(3.75);
    await userEvent.clear(write);

    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    const price = (await calls.expectSentBody("PUT", "/api/v1/model-prices")) as Record<
      string,
      unknown
    >;
    await expect(price).toHaveProperty("cache_write_per_mtok", null);
  },
};

/**
 * A rate that is not a number of 0 or more is stated at its own input after a
 * refused save, which opens the section it is in and puts focus there. Nothing
 * is sent, and a number lets the same press through.
 */
export const RefusesAnInvalidCacheWriteRate: Story = {
  render: () => <Stage mode="edit" route={ROUTE} stub={pricedAt(null)} />,
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    const toggle = dialog.getByRole("button", { name: "Pricing override" });
    await userEvent.click(toggle);
    const write = dialog.getByLabelText(WRITE_LABEL);
    await userEvent.type(write, "-1");
    // the sheet says nothing until a save is refused
    await expect(write).not.toHaveAttribute("aria-invalid");
    // folded away, so the refusal has to open it again to be read
    await userEvent.click(toggle);
    await expect(dialog.queryByLabelText(WRITE_LABEL)).not.toBeInTheDocument();

    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    const field = await dialog.findByLabelText(WRITE_LABEL);
    await waitFor(() => expect(field).toHaveAttribute("aria-invalid", "true"));
    await waitFor(() => expect(field).toHaveFocus());
    await expect(field).toHaveAccessibleDescription(/Enter a cache-write rate of 0 or more/);
    await expect(dialog.getByRole("status")).toHaveTextContent(
      "1 field needs attention before this route can be saved.",
    );
    calls.expectNotSent("PUT", "/api/v1/model-prices");

    await userEvent.clear(field);
    await userEvent.type(field, "4.5");
    await waitFor(() => expect(field).not.toHaveAttribute("aria-invalid"));
    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    const price = (await calls.expectSentBody("PUT", "/api/v1/model-prices")) as Record<
      string,
      unknown
    >;
    await expect(price.cache_write_per_mtok).toBe("4.5");
  },
};

/**
 * The rate is saved with the price row, and a row is written once the input or
 * output rate is set. A rate typed with neither would be dropped on save, so the
 * sheet says so at the input rather than losing it.
 */
export const AsksForAnInputRateBesideACacheWriteRate: Story = {
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await seededBlank(dialog);
    await userEvent.type(dialog.getByLabelText("Route name"), "claude-sonnet");
    await pickOption(dialog.getByLabelText("Target 1 provider"), "vllm-cluster");
    await userEvent.click(dialog.getByRole("button", { name: "Pricing override" }));
    const write = dialog.getByLabelText(WRITE_LABEL);
    await userEvent.type(write, "3.75");

    await userEvent.click(dialog.getByRole("button", { name: "Add route" }));
    await waitFor(() => expect(write).toHaveAttribute("aria-invalid", "true"));
    await expect(write).toHaveAccessibleDescription(/Set the input or output rate too/);
    calls.expectNotSent("POST", `/projects/${PROJECT.id}/routes`);

    await userEvent.type(dialog.getByLabelText("Input USD/Mtok"), "3");
    await waitFor(() => expect(write).not.toHaveAttribute("aria-invalid"));
    await userEvent.click(dialog.getByRole("button", { name: "Add route" }));
    const price = (await calls.expectSentBody("PUT", "/api/v1/model-prices")) as Record<
      string,
      unknown
    >;
    await expect(price).toEqual({
      model: "claude-sonnet",
      input_per_mtok: "3",
      output_per_mtok: "0",
      cache_write_per_mtok: "3.75",
      currency: "USD",
    });
  },
};

/** A 1 hour rate typed into the sheet goes with the price row under its own key, and the 5 minute rate is left alone. */
export const SavesTheOneHourCacheWriteRateOnThePriceRow: Story = {
  render: () => <Stage mode="edit" route={ROUTE} stub={pricedAt("3.750000")} />,
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await userEvent.click(dialog.getByRole("button", { name: "Pricing override" }));
    const hour = dialog.getByLabelText(WRITE_1H_LABEL);
    await expect(hour).toHaveValue(null);
    await userEvent.type(hour, "6");

    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    const price = (await calls.expectSentBody("PUT", "/api/v1/model-prices")) as Record<
      string,
      unknown
    >;
    await expect(price.cache_write_1h_per_mtok).toBe("6");
    await expect(price).not.toHaveProperty("cache_write_per_mtok");
  },
};

/**
 * Emptying the 1 hour input is `null` on the wire, which falls back to the 5
 * minute rate; the other input, left alone, names nothing.
 */
export const ClearsTheOneHourCacheWriteRateOnThePriceRow: Story = {
  render: () => <Stage mode="edit" route={ROUTE} stub={pricedAt("3.750000", "6.000000")} />,
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await userEvent.click(dialog.getByRole("button", { name: "Pricing override" }));
    const hour = dialog.getByLabelText(WRITE_1H_LABEL);
    await expect(hour).toHaveValue(6);
    await userEvent.clear(hour);

    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    const price = (await calls.expectSentBody("PUT", "/api/v1/model-prices")) as Record<
      string,
      unknown
    >;
    await expect(price).toHaveProperty("cache_write_1h_per_mtok", null);
    await expect(price).not.toHaveProperty("cache_write_per_mtok");
  },
};

const IMAGE_LABEL = /Per image USD/;
const AUDIO_LABEL = /Per minute of audio USD/;
const SPEECH_LABEL = /Per Mchar of speech USD/;
const IMAGE_ROUTE: RouteRow = { ...ROUTE, advanced: { ...ROUTE.advanced, model_type: "image" } };
const AUDIO_ROUTE: RouteRow = { ...ROUTE, advanced: { ...ROUTE.advanced, model_type: "audio" } };

/** A save that never touched the unit rates names none of them, so a rate set elsewhere survives (#2937). */
export const KeepsTheUnitRatesWhenUntouched: Story = {
  render: () => (
    <Stage
      mode="edit"
      route={AUDIO_ROUTE}
      stub={pricedAt(null, null, {
        audio_input_per_minute: "0.006000",
        speech_per_mchar: "15.000000",
      })}
    />
  ),
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await userEvent.click(dialog.getByRole("button", { name: "Pricing override" }));
    await expect(dialog.getByLabelText(AUDIO_LABEL)).toHaveValue(0.006);
    await expect(dialog.getByLabelText(SPEECH_LABEL)).toHaveValue(15);
    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    const price = (await calls.expectSentBody("PUT", "/api/v1/model-prices")) as Record<
      string,
      unknown
    >;
    await expect(price).not.toHaveProperty("image_per_unit");
    await expect(price).not.toHaveProperty("audio_input_per_minute");
    await expect(price).not.toHaveProperty("speech_per_mchar");
  },
};

const AUDIO_IN_TOKENS_LABEL = "Audio input USD/Mtok";
const AUDIO_OUT_TOKENS_LABEL = "Audio output USD/Mtok";
const CACHED_AUDIO_LABEL = "Cached audio input USD/Mtok";
const AUDIO_TOKEN_KEYS = [
  "audio_input_per_mtok",
  "audio_output_per_mtok",
  "cached_audio_input_per_mtok",
] as const;

/**
 * An audio model opens on the audio token rates its price row holds (#3145),
 * each saying what empty falls back to, and a save that never touched them
 * names none, so a rate set on the Pricing screen survives.
 */
export const KeepsTheAudioTokenRatesWhenUntouched: Story = {
  render: () => (
    <Stage
      mode="edit"
      route={AUDIO_ROUTE}
      stub={pricedAt(null, null, {
        audio_input_per_mtok: "32.000000",
        audio_output_per_mtok: "64.000000",
        cached_audio_input_per_mtok: "0.400000",
      })}
    />
  ),
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await userEvent.click(dialog.getByRole("button", { name: "Pricing override" }));
    const audioIn = await dialog.findByLabelText(AUDIO_IN_TOKENS_LABEL);
    await expect(audioIn).toHaveValue(32);
    await expect(audioIn).toHaveAttribute("placeholder", "input rate");
    await expect(audioIn).toHaveAccessibleDescription(
      "For a realtime or audio model that bills the audio tokens it is sent above its text tokens. Leave empty to price them at the input rate.",
    );
    await expect(dialog.getByLabelText(AUDIO_OUT_TOKENS_LABEL)).toHaveValue(64);
    await expect(dialog.getByLabelText(CACHED_AUDIO_LABEL)).toHaveValue(0.4);
    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    const price = (await calls.expectSentBody("PUT", "/api/v1/model-prices")) as Record<
      string,
      unknown
    >;
    for (const key of AUDIO_TOKEN_KEYS) await expect(price).not.toHaveProperty(key);
  },
};

/** Audio token rates typed into the sheet go under their own keys, and the one left empty is not named. */
export const SavesTheAudioTokenRates: Story = {
  render: () => <Stage mode="edit" route={AUDIO_ROUTE} stub={pricedAt(null)} />,
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await userEvent.click(dialog.getByRole("button", { name: "Pricing override" }));
    const audioIn = await dialog.findByLabelText(AUDIO_IN_TOKENS_LABEL);
    await expect(audioIn).toHaveValue(null);
    await userEvent.type(audioIn, "40");
    await userEvent.type(dialog.getByLabelText(AUDIO_OUT_TOKENS_LABEL), "80");
    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    const price = (await calls.expectSentBody("PUT", "/api/v1/model-prices")) as Record<
      string,
      unknown
    >;
    await expect(price).toHaveProperty("audio_input_per_mtok", "40");
    await expect(price).toHaveProperty("audio_output_per_mtok", "80");
    await expect(price).not.toHaveProperty("cached_audio_input_per_mtok");
    await expect(price).not.toHaveProperty("audio_input_per_minute");
  },
};

/** Emptying a stored audio token rate is `null` on the wire, and the rates left alone are not named. */
export const ClearsAnAudioTokenRate: Story = {
  render: () => (
    <Stage
      mode="edit"
      route={AUDIO_ROUTE}
      stub={pricedAt(null, null, {
        audio_input_per_mtok: "32.000000",
        cached_audio_input_per_mtok: "0.400000",
      })}
    />
  ),
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await userEvent.click(dialog.getByRole("button", { name: "Pricing override" }));
    const cached = await dialog.findByLabelText(CACHED_AUDIO_LABEL);
    await expect(cached).toHaveValue(0.4);
    await userEvent.clear(cached);
    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    const price = (await calls.expectSentBody("PUT", "/api/v1/model-prices")) as Record<
      string,
      unknown
    >;
    await expect(price).toHaveProperty("cached_audio_input_per_mtok", null);
    await expect(price).not.toHaveProperty("audio_input_per_mtok");
    await expect(price).not.toHaveProperty("audio_output_per_mtok");
  },
};

/**
 * An audio token rate that is not a number of 0 or more is stated at its own
 * input after a refused save, which opens the pricing section and puts focus
 * there; nothing is sent.
 */
export const RefusesAnInvalidAudioTokenRate: Story = {
  render: () => <Stage mode="edit" route={AUDIO_ROUTE} stub={pricedAt(null)} />,
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    const toggle = dialog.getByRole("button", { name: "Pricing override" });
    await userEvent.click(toggle);
    await userEvent.type(await dialog.findByLabelText(AUDIO_OUT_TOKENS_LABEL), "-1");
    await userEvent.click(toggle);
    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    const field = await dialog.findByLabelText(AUDIO_OUT_TOKENS_LABEL);
    await waitFor(() => expect(field).toHaveAttribute("aria-invalid", "true"));
    await waitFor(() => expect(field).toHaveFocus());
    await expect(field).toHaveAccessibleDescription(/Enter a rate of 0 or more/);
    calls.expectNotSent("PUT", "/api/v1/model-prices");
  },
};

/** A rate typed into an image model's sheet goes under its own key and nothing else is named. */
export const SavesTheImageRate: Story = {
  render: () => <Stage mode="edit" route={IMAGE_ROUTE} stub={pricedAt(null)} />,
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await userEvent.click(dialog.getByRole("button", { name: "Pricing override" }));
    await userEvent.type(dialog.getByLabelText(IMAGE_LABEL), "0.04");
    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    const price = (await calls.expectSentBody("PUT", "/api/v1/model-prices")) as Record<
      string,
      unknown
    >;
    await expect(price.image_per_unit).toBe("0.04");
    await expect(price).not.toHaveProperty("audio_input_per_minute");
    await expect(price).not.toHaveProperty("speech_per_mchar");
  },
};

/** Emptying a stored rate is `null` on the wire, which clears it. */
export const ClearsAUnitRate: Story = {
  render: () => (
    <Stage
      mode="edit"
      route={AUDIO_ROUTE}
      stub={pricedAt(null, null, { audio_input_per_minute: "0.006000" })}
    />
  ),
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await userEvent.click(dialog.getByRole("button", { name: "Pricing override" }));
    await userEvent.clear(dialog.getByLabelText(AUDIO_LABEL));
    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    const price = (await calls.expectSentBody("PUT", "/api/v1/model-prices")) as Record<
      string,
      unknown
    >;
    await expect(price).toHaveProperty("audio_input_per_minute", null);
    await expect(price).not.toHaveProperty("speech_per_mchar");
  },
};

/**
 * A 1 hour rate that is not a number of 0 or more is stated at its own input
 * after a refused save, which opens the section it is in and puts focus there.
 * The 5 minute input beside it stays clean, nothing is sent, and a number lets
 * the same press through.
 */
export const RefusesAnInvalidOneHourCacheWriteRate: Story = {
  render: () => <Stage mode="edit" route={ROUTE} stub={pricedAt(null)} />,
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    const toggle = dialog.getByRole("button", { name: "Pricing override" });
    await userEvent.click(toggle);
    const hour = dialog.getByLabelText(WRITE_1H_LABEL);
    await userEvent.type(hour, "-1");
    // the sheet says nothing until a save is refused
    await expect(hour).not.toHaveAttribute("aria-invalid");
    // folded away, so the refusal has to open it again to be read
    await userEvent.click(toggle);
    await expect(dialog.queryByLabelText(WRITE_1H_LABEL)).not.toBeInTheDocument();

    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    const field = await dialog.findByLabelText(WRITE_1H_LABEL);
    await waitFor(() => expect(field).toHaveAttribute("aria-invalid", "true"));
    await waitFor(() => expect(field).toHaveFocus());
    await expect(field).toHaveAccessibleDescription(/Enter a 1 hour cache-write rate of 0 or more/);
    await expect(dialog.getByLabelText(WRITE_LABEL)).not.toHaveAttribute("aria-invalid");
    await expect(dialog.getByRole("status")).toHaveTextContent(
      "1 field needs attention before this route can be saved.",
    );
    calls.expectNotSent("PUT", "/api/v1/model-prices");

    await userEvent.clear(field);
    await userEvent.type(field, "7.5");
    await waitFor(() => expect(field).not.toHaveAttribute("aria-invalid"));
    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    const price = (await calls.expectSentBody("PUT", "/api/v1/model-prices")) as Record<
      string,
      unknown
    >;
    await expect(price.cache_write_1h_per_mtok).toBe("7.5");
  },
};

/** The 1 hour rate is saved with the price row too, so it asks for the input or output rate beside it. */
export const AsksForAnInputRateBesideAOneHourCacheWriteRate: Story = {
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await seededBlank(dialog);
    await userEvent.type(dialog.getByLabelText("Route name"), "claude-sonnet");
    await pickOption(dialog.getByLabelText("Target 1 provider"), "vllm-cluster");
    await userEvent.click(dialog.getByRole("button", { name: "Pricing override" }));
    const hour = dialog.getByLabelText(WRITE_1H_LABEL);
    await userEvent.type(hour, "6");

    await userEvent.click(dialog.getByRole("button", { name: "Add route" }));
    await waitFor(() => expect(hour).toHaveAttribute("aria-invalid", "true"));
    await expect(hour).toHaveAccessibleDescription(/Set the input or output rate too/);
    calls.expectNotSent("POST", `/projects/${PROJECT.id}/routes`);

    await userEvent.type(dialog.getByLabelText("Output USD/Mtok"), "15");
    await waitFor(() => expect(hour).not.toHaveAttribute("aria-invalid"));
    await userEvent.click(dialog.getByRole("button", { name: "Add route" }));
    const price = (await calls.expectSentBody("PUT", "/api/v1/model-prices")) as Record<
      string,
      unknown
    >;
    await expect(price).toEqual({
      model: "claude-sonnet",
      input_per_mtok: "0",
      output_per_mtok: "15",
      cache_write_1h_per_mtok: "6",
      currency: "USD",
    });
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
    await seededBlank(dialog);
    await userEvent.type(dialog.getByLabelText("Route name"), "llama-3.1-70b");
    await userEvent.click(dialog.getByRole("button", { name: /close/i }));
    await answerDiscardPrompt(false);
    await expect(dialog.getByLabelText("Route name")).toHaveValue("llama-3.1-70b");
  },
};

/** And "discard" closes it. */
export const DiscardGuardThrowsItAway: Story = {
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await seededBlank(dialog);
    await userEvent.type(dialog.getByLabelText("Route name"), "llama-3.1-70b");
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
    await seededBlank(dialog);
    const listbox = await openOptions(dialog.getByLabelText("Target 1 provider"));
    const names = within(listbox)
      .getAllByRole("option")
      .map((o) => o.textContent);
    await expect(names).toEqual(["openai-prod", "vllm-cluster", "gateway-private"]);
    await userEvent.keyboard("{Escape}");
    await waitFor(() =>
      expect(
        dialog.getByText("1 provider scoped to another project is not offered here."),
      ).toBeVisible(),
    );
  },
};

/** With every provider usable there is nothing to explain. */
export const SaysNothingWhenNoProviderIsLeftOut: Story = {
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await seededBlank(dialog);
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
    await seededBlank(dialog);
    await userEvent.type(dialog.getByLabelText("Route name"), "llama-3.1-70b");
    await pickOption(dialog.getByLabelText("Target 1 provider"), "vllm-cluster");
    await userEvent.click(dialog.getByRole("button", { name: "Add route" }));
    await expectToast(canvasElement, /cannot use provider 'search-private'/, "error");
    // the sheet stays open on the draft
    await expect(sheet()).toBeVisible();
  },
};

// --- where the model's price came from (#966) ---

const CATALOGUE_PROVENANCE = {
  source: "LiteLLM model_prices_and_context_window",
  source_imported_at: "2026-10-01T00:00:00Z",
};

/** an imported price is named as a catalogue value that can change, with its snapshot */
export const NamesACataloguePriceAsSubjectToChange: Story = {
  render: () => (
    <Stage mode="edit" route={ROUTE} stub={pricedAt(null, null, CATALOGUE_PROVENANCE)} />
  ),
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await userEvent.click(dialog.getByRole("button", { name: "Pricing override" }));
    await expect(
      dialog.getByText(
        "Catalogue price from LiteLLM model_prices_and_context_window, snapshot Oct 1, 2026 — subject to change.",
      ),
    ).toBeInTheDocument();
  },
};

/** saving the route does not write a catalogue price back, which would make it an override */
export const SavingTheRouteKeepsACataloguePriceACataloguePrice: Story = {
  render: () => (
    <Stage mode="edit" route={ROUTE} stub={pricedAt(null, null, CATALOGUE_PROVENANCE)} />
  ),
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await userEvent.click(dialog.getByRole("button", { name: "Pricing override" }));
    await pickOption(dialog.getByLabelText("Strategy"), /^weighted\b/);
    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    await waitFor(() => expect(calls.calls.some((c) => c.method === "PUT")).toBe(true));
    calls.expectNotSent("PUT", "/api/v1/model-prices");
  },
};

/** editing the rates is the operator taking the price over, so it is written */
export const EditingACataloguePriceWritesIt: Story = {
  render: () => (
    <Stage mode="edit" route={ROUTE} stub={pricedAt(null, null, CATALOGUE_PROVENANCE)} />
  ),
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await userEvent.click(dialog.getByRole("button", { name: "Pricing override" }));
    const input = dialog.getByLabelText("Input USD/Mtok");
    await userEvent.clear(input);
    await userEvent.type(input, "3");
    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    await calls.expectSentBody("PUT", "/api/v1/model-prices");
  },
};

export const NamesAnOperatorPriceAsYours: Story = {
  render: () => (
    <Stage
      mode="edit"
      route={ROUTE}
      stub={pricedAt(null, null, { source: null, source_imported_at: null })}
    />
  ),
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await userEvent.click(dialog.getByRole("button", { name: "Pricing override" }));
    await expect(dialog.getByText("Your price (override).")).toBeInTheDocument();
    await expect(dialog.queryByText(/Catalogue price from/)).toBeNull();
  },
};

/** a model with no price says nothing about where it came from */
export const SaysNothingAboutAnUnpricedModel: Story = {
  render: () => <Stage mode="edit" route={ROUTE} />,
  play: async () => {
    const dialog = within(sheet());
    await seeded(dialog);
    await userEvent.click(dialog.getByRole("button", { name: "Pricing override" }));
    await expect(dialog.queryByText(/Catalogue price from/)).toBeNull();
    await expect(dialog.queryByText(/Your price \(override\)/)).toBeNull();
  },
};
