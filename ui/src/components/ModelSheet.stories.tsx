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
import type {
  EffectiveModelDto,
  ModelPriceRow,
  ProviderRow,
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
    await expect(dialog.getByRole("heading", { name: "Add route" })).toBeVisible();
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
    await expect(dialog.queryByRole("alert")).not.toBeInTheDocument();
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
    await expect(dialog.getByRole("alert")).toHaveTextContent(
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
    await expect(await dialog.findByRole("alert")).toHaveTextContent("2 fields need attention");

    await userEvent.type(name, "llama-3.1-8b");
    await waitFor(() => expect(name).not.toHaveAttribute("aria-invalid"));
    // one problem left, and the count follows it
    await expect(dialog.getByRole("alert")).toHaveTextContent(
      "1 field needs attention before this route can be saved.",
    );
    await pickOption(provider, "vllm-cluster");
    await waitFor(() => expect(dialog.queryByRole("alert")).not.toBeInTheDocument());
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
    await expect(dialog.queryByRole("alert")).not.toBeInTheDocument();
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
    await expect(
      dialog.getByText("1 provider scoped to another project is not offered here."),
    ).toBeVisible();
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
    await expect(
      await dialog.findByRole("heading", { name: ru.modelSheet.titleAdd }),
    ).toBeVisible();
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
    const summary = await dialog.findByRole("alert");
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
    await expect(dialog.getByLabelText("Route name")).toHaveValue("gpt-4o");
    // renaming is not supported yet, and the field says so rather than
    // accepting an edit the control plane would drop
    await expect(dialog.getByLabelText("Route name")).toBeDisabled();
    // the strategy is shown as stored, and said to be fixed: the control plane
    // takes it at creation and has no call that changes it (#1979)
    const strategy = dialog.getByLabelText("Strategy");
    await expect(strategy).toHaveValue("round_robin");
    await expect(strategy).toBeDisabled();
    await expect(strategy).toHaveAccessibleDescription(/set when a route is created/);
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
    await expect(dialog.getByText("Route details")).toBeVisible();
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
    await expect(await dialog.findByRole("alert")).toHaveTextContent(
      "1 field needs attention before this route can be saved.",
    );
    await waitFor(() => expect(name).toHaveFocus());
    calls.expectNotSent("POST", `/projects/${PROJECT.id}/routes`);
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
    await seededBlank(dialog);
    await userEvent.type(dialog.getByLabelText("Route name"), "qwen-72b");
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

    await userEvent.click(dialog.getByRole("button", { name: "Add route" }));
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
    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));

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
    await seededBlank(dialog);
    await pickOption(dialog.getByLabelText("Duplicate from"), "llama-70b");
    await waitFor(() => expect(dialog.getByLabelText("Strategy")).toHaveValue("cache_aware"));
    await expect(await dialog.findByLabelText("Target 3 upstream model")).toHaveValue("llama-70b");
    // the copy carries the source's name, which is taken, until it is renamed
    await expect(dialog.getByLabelText("Route name")).toHaveAccessibleDescription(/already exists/);
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
    await expect(dialog.getByLabelText("Max retries")).toBeVisible();
    await expect(dialog.queryByText("Allow additional fields")).not.toBeInTheDocument();
  },
};

/**
 * A route that carries keys the gateway never read (#2924): a per-route
 * endpoint, an insecure-TLS switch, flat image and audio prices, and four
 * limits. The sheet offers none of them, and a save sheds them from the stored
 * blob instead of copying them through, so the routes API stops reading back a
 * setting that looks like policy and does nothing. What the gateway does apply
 * (timeout, retries, headers, the guardrail selection) is kept.
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
    // the three limits that apply are there, and none of what does not
    await expect(dialog.getByLabelText("Timeout (ms)")).toHaveValue(30000);
    await expect(dialog.getByLabelText("Max retries")).toHaveValue(1);
    await expect(dialog.getByLabelText("Max output tokens")).toHaveValue(4096);
    for (const gone of [
      "Base URL override",
      "Requests / min",
      "Tokens / min",
      "Max concurrency",
      "Context window",
    ]) {
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
    await expect(body.advanced.limits).toEqual({
      timeout_secs: 30,
      retries: 1,
      output_tokens: 4096,
    });
    await expect(body.advanced.headers).toEqual({ "X-Tenant": "beta" });
    await expect(body.advanced.guardrails).toEqual({ rules: ["pii-out"] });
  },
};

/**
 * A model type billed per image or per minute has no rate to set: cost is
 * computed from tokens, and the flat price this section used to offer never
 * reached a cost path (#2924). The section says so, rather than showing a
 * currency picker with nothing to apply it to.
 */
export const PricesTokensOnly: Story = {
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await seededBlank(dialog);
    await pickOption(dialog.getByLabelText("Model type"), "image");
    await userEvent.click(dialog.getByRole("button", { name: "Pricing override" }));
    await expect(await dialog.findByText(/Rolter prices by tokens/)).toBeVisible();
    await expect(dialog.queryByLabelText(/Flat price per/)).not.toBeInTheDocument();
    await expect(dialog.queryByLabelText("Currency")).not.toBeInTheDocument();
    // and a chat model still gets its rates
    await pickOption(dialog.getByLabelText("Model type"), "chat");
    await expect(await dialog.findByLabelText(/Input USD\/Mtok/)).toBeVisible();
    await expect(dialog.queryByText(/Rolter prices by tokens/)).not.toBeInTheDocument();
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
    await seededBlank(dialog);
    await pickOption(dialog.getByLabelText("Target 1 provider"), "vllm-cluster");
    await userEvent.type(dialog.getByLabelText("Route name"), "llama-3.1-70b");
    // a complete draft, the state an operator would have tested from: this is
    // an absent control, not a footer that has not painted its actions yet
    await expect(dialog.getByRole("button", { name: "Add route" })).toBeEnabled();
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
    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
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

/**
 * The price table with one row for `gpt-4o`, carrying the two cache-write rates
 * as the control plane returns them: decimal text, six places, or null for no
 * rate.
 */
function pricedAt(cacheWrite: string | null, cacheWrite1h: string | null = null): FetchStub {
  const row: ModelPriceRow = {
    id: "price-1",
    model: "gpt-4o",
    input_per_mtok: "2.500000",
    output_per_mtok: "10.000000",
    cached_input_per_mtok: "1.250000",
    cache_write_per_mtok: cacheWrite,
    cache_write_1h_per_mtok: cacheWrite1h,
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
    await expect(dialog.getByRole("alert")).toHaveTextContent(
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
    await expect(dialog.getByRole("alert")).toHaveTextContent(
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
