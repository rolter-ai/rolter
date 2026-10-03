import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import Providers from "./Providers";
import {
  Harness,
  cancelConfirmation,
  clickWhenEnabled,
  confirmDestructive,
  expectEmptyState,
  expectNoUxEvent,
  expectSheetClosed,
  expectUxEvent,
  expectLoadError,
  expectListStateInViewport,
  expectListTable,
  expectNoFalseEmpty,
  expectSkeleton,
  json,
  pending,
  recordUxEvents,
  recording,
  routes,
  scoped,
  Toasted,
  expectToast,
  uxEvents,
  type Recorder,
} from "./story-harness";
import type { LabelRow, ProviderGroupRow, ProviderRow, ProviderTestResult } from "@/lib/api";
import { atMobile, atTablet, expectNoHorizontalOverflow } from "@/lib/story-viewport";
import { UxScreenProvider } from "@/lib/ux-react";

const PROVIDERS: ProviderRow[] = [
  {
    id: "p-1",
    org_id: "org-1",
    name: "openai-prod",
    slug: "openai-prod",
    kind: "openai",
    api_base: "https://api.openai.com/v1",
    api_key_env: "OPENAI_API_KEY",
    egress_proxies: [],
    created_at: "2026-01-02T00:00:00Z",
  },
  {
    id: "p-2",
    org_id: "org-1",
    name: "anthropic-eu",
    slug: "anthropic-eu",
    kind: "anthropic",
    api_base: "https://api.anthropic.com",
    api_key_env: "ANTHROPIC_API_KEY",
    egress_proxies: [],
    created_at: "2026-01-09T00:00:00Z",
  },
];

const loaded = routes([
  ["/providers", () => PROVIDERS],
  ["/config/problems", () => ({ problems: [] })],
]);

const meta = {
  title: "Screens/Providers",
  component: Providers,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof Providers>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <Providers />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getAllByText("openai-prod").length).toBeGreaterThan(0));
    await expect(canvas.getAllByText("anthropic-eu").length).toBeGreaterThan(0);
    await expectListTable(canvasElement, "Model Providers");
  },
};

// the rows are skeletons inside the real table, so the column headers stay put
// and the list does not jump a row-height when the data lands
export const Loading: Story = {
  render: () => (
    <Harness fetchStub={pending}>
      <Providers />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
    await expectNoFalseEmpty(canvasElement, /No providers yet/);
  },
};

// a fresh install: the CTA is the whole point of the screen
export const Empty: Story = {
  render: () => (
    <Harness fetchStub={routes([["/providers", () => []]])}>
      <Providers />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectEmptyState(canvasElement, /No providers yet/, /Add provider/);
  },
};

// the empty state and the skeleton sit in the part of the table a phone shows,
// not in the row's 760px floor past the right edge of the card (#2362)
export const EmptyIsOnScreenAtPhoneWidth: Story = {
  ...atMobile,
  render: () => (
    <Harness fetchStub={routes([["/providers", () => []]])}>
      <Providers />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectListStateInViewport(canvasElement, "Model Providers", {
      says: /No providers yet/,
      cta: /Add provider/,
    });
    await expectNoHorizontalOverflow();
  },
};

export const LoadingIsOnScreenAtPhoneWidth: Story = {
  ...atMobile,
  render: () => (
    <Harness fetchStub={pending}>
      <Providers />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectListStateInViewport(canvasElement, "Model Providers");
    await expectNoHorizontalOverflow();
  },
};

// a search that matched nothing is not the same answer as an empty org: the
// copy blames the query and offers to clear it rather than to create a row
export const NoSearchMatch: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <Providers />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getAllByText("openai-prod").length).toBeGreaterThan(0));
    await userEvent.type(canvas.getByLabelText("Search providers"), "cohere");
    await waitFor(() => expect(canvas.getByText(/No providers match/)).toBeVisible());
    await expect(canvas.getByRole("button", { name: /Clear search/i })).toBeInTheDocument();
  },
};

export const Error_: Story = {
  name: "Error",
  render: () => (
    <Harness fetchStub={scoped(async () => json({ error: { message: "boom" } }, 500))}>
      <Providers />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /failed to return providers/i);
    await expectNoFalseEmpty(canvasElement, /No providers yet/);
  },
};

export const Forbidden: Story = {
  render: () => (
    <Harness fetchStub={scoped(async () => json({ error: { message: "forbidden" } }, 403))}>
      <Providers />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /You do not have access to providers/);
    await expectNoFalseEmpty(canvasElement, /No providers yet/);
  },
};

/**
 * The provider list is six columns wide and stays six columns wide: below `md`
 * it scrolls inside its own border instead of dragging the page sideways under
 * the shell (#1203).
 */
export const Mobile: Story = {
  ...atMobile,
  render: () => (
    <Harness fetchStub={loaded}>
      <Providers />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getAllByText("openai-prod").length).toBeGreaterThan(0));
    await expectNoHorizontalOverflow();
  },
};

export const Tablet: Story = {
  ...atTablet,
  render: () => (
    <Harness fetchStub={loaded}>
      <Providers />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getAllByText("openai-prod").length).toBeGreaterThan(0));
    await expectNoHorizontalOverflow();
  },
};

/**
 * The delete is refused (#1607).
 *
 * Deleting a provider strands every route that targets it, so a control plane
 * that refuses has a reason worth reading — the dialog stays open carrying it
 * rather than closing over a provider that is still registered.
 */
export const DeleteRejectedByTheServer: Story = {
  beforeEach: recordUxEvents,
  render: () => (
    <Harness
      // answered in the same tick, so the pending render and the refusal land
      // in one notify batch. the dialog reads the failure off the press rather
      // than off a pending edge that never renders (#1761)
      fetchStub={scoped(async (input, init) =>
        init?.method === "DELETE"
          ? json({ error: { message: "openai-prod is the target of 4 live routes" } }, 409)
          : loaded(input, init),
      )}
    >
      <UxScreenProvider screen="providers">
        <Toasted>
          <Providers />
        </Toasted>
      </UxScreenProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", { name: "Delete provider openai-prod" }),
    );
    const dialog = within(await within(document.body).findByRole("dialog"));
    await userEvent.click(dialog.getByRole("button", { name: "Delete provider" }));
    // the press and the refusal are two rows under the same key, read before the
    // toast waits so the queue's 5s flush timer cannot drain them first
    await waitFor(() =>
      expect(
        uxEvents()
          .filter((e) => e.action === "form_submit" && e.target === "provider-delete")
          .map((e) => e.outcome),
      ).toEqual(["ok", "error"]),
    );

    await expectToast(canvasElement, /target of 4 live routes/, "error");
    await waitFor(() => expect(dialog.getByText(/target of 4 live routes/)).toBeVisible());
    await expect(within(document.body).getByRole("dialog")).toBeInTheDocument();
  },
};

/**
 * The delete confirms through the shared `ConfirmDialog` (#1738), which names
 * the provider, sends nothing when cancelled, and reports both outcomes to the
 * UX stream under the `provider-delete` key the hand-rolled dialog used — so
 * the series reads on across the swap.
 */
let deleted: Recorder;
export const DeleteIsConfirmedAndReported: Story = {
  beforeEach: recordUxEvents,
  render: () => {
    deleted = recording(
      scoped(async (input, init) =>
        init?.method === "DELETE" ? new Response(null, { status: 204 }) : loaded(input, init),
      ),
    );
    return (
      <Harness fetchStub={deleted.stub}>
        <UxScreenProvider screen="providers">
          <Providers />
        </UxScreenProvider>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "Delete provider openai-prod");
    await expect(
      await within(document.body).findByRole("heading", { name: "Delete provider openai-prod?" }),
    ).toBeInTheDocument();
    await cancelConfirmation();
    deleted.expectNotSent("DELETE", "/providers/");
    const abandon = await expectUxEvent("form_abandon", "provider-delete");
    await expect(abandon.screen).toBe("providers");
    await expect(abandon.outcome).toBe("cancelled");
    expectNoUxEvent("form_submit", "provider-delete");

    await clickWhenEnabled(canvasElement, "Delete provider openai-prod");
    await confirmDestructive("Delete provider openai-prod?", "Delete provider");
    await deleted.expectSent("DELETE", "/providers/p-1");
    const submit = await expectUxEvent("form_submit", "provider-delete");
    await expect(submit.outcome).toBe("ok");
    await expectSheetClosed();
    // the confirmed delete is not also an abandon on its way out
    await expect(
      uxEvents().filter((e) => e.action === "form_abandon" && e.target === "provider-delete"),
    ).toHaveLength(1);
  },
};

// ---------------------------------------------------------------- labels (#1329)

const label = (over: Partial<LabelRow> & { id: string; key: string }): LabelRow => ({
  subject_type: "provider",
  subject_id: "p-1",
  source: "custom",
  value: null,
  created_at: "2026-01-02T00:00:00Z",
  updated_at: "2026-01-02T00:00:00Z",
  ...over,
});

// the same key on the same provider from both sources, which the API allows on
// purpose and which a screen that renders them alike shows as a duplicate
const LABELS: LabelRow[] = [
  label({ id: "l-1", key: "tier", value: "gold" }),
  label({
    id: "l-2",
    key: "tier",
    value: "observed-gold",
    source: "auto",
    observed_at: "2026-03-01T09:00:00Z",
    observation: "priced from the last 24h of traffic",
  }),
  label({ id: "l-3", subject_id: "p-2", key: "region", value: "eu" }),
];

// the label endpoint narrows on `subject_id`, and the sheet depends on it: a
// stub that answers every request with the whole org would show one provider
// the labels of another
const withLabels = scoped(async (input) => {
  const url = new URL(String(input), "http://localhost");
  if (url.pathname.endsWith("/labels")) {
    const subject = url.searchParams.get("subject_id");
    return json(subject ? LABELS.filter((l) => l.subject_id === subject) : LABELS);
  }
  if (url.pathname.endsWith("/providers")) return json(PROVIDERS);
  return json([]);
});

/**
 * Both sources on one provider, under one key. They differ by tone, by the icon
 * in front of them and by the word in the accessible name, so neither a
 * colour-blind reader nor a screen reader has to take the colour's word for it.
 */
export const Labelled: Story = {
  render: () => (
    <Harness fetchStub={withLabels}>
      <Providers />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("tier=gold")).toBeVisible());
    await expect(canvas.getAllByTestId("label-custom")).toHaveLength(2);
    await expect(canvas.getAllByTestId("label-auto")).toHaveLength(1);
    // the two on `openai-prod` are told apart by name, not by colour
    await expect(canvas.getByLabelText("tier=gold, your label")).toBeVisible();
    await expect(canvas.getByLabelText("tier=observed-gold, automatic label")).toBeVisible();
  },
};

/** the list narrows to the subjects carrying the chosen label */
export const FilteredByLabel: Story = {
  render: () => (
    <Harness fetchStub={withLabels}>
      <Providers />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getAllByText("anthropic-eu").length).toBeGreaterThan(0));
    await userEvent.click(canvas.getByRole("combobox", { name: /Filter by label/i }));
    await userEvent.click(await within(document.body).findByRole("option", { name: "region=eu" }));
    await waitFor(() => expect(canvas.queryByText("openai-prod")).toBeNull());
    await expect(canvas.getAllByText("anthropic-eu").length).toBeGreaterThan(0);
  },
};

/**
 * A label filter that matches nothing is a filter, not an empty organisation:
 * the copy blames the narrowing and offers to clear it rather than offering to
 * add the first provider to an org that already has two.
 */
export const NoLabelMatch: Story = {
  render: () => (
    <Harness fetchStub={withLabels}>
      <Providers />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("tier=gold")).toBeVisible());
    await userEvent.type(canvas.getByLabelText("Search providers"), "cohere");
    await userEvent.click(canvas.getByRole("combobox", { name: /Filter by label/i }));
    await userEvent.click(await within(document.body).findByRole("option", { name: "region=eu" }));
    await waitFor(() => expect(canvas.getByText(/No providers match/)).toBeVisible());
    // clearing puts both back, so the button really cleared both narrowings
    await userEvent.click(canvas.getByRole("button", { name: /Clear search/i }));
    await waitFor(() => expect(canvas.getAllByText("openai-prod").length).toBeGreaterThan(0));
  },
};

/** an auto label carries what was observed and when, and offers no way to edit it */
export const AutoLabelsAreReadOnly: Story = {
  render: () => (
    <Harness fetchStub={withLabels}>
      <Providers />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getAllByText("openai-prod").length).toBeGreaterThan(0));
    await userEvent.click(canvas.getByRole("button", { name: "Labels on openai-prod" }));
    const panel = within(await within(document.body).findByRole("dialog"));
    await expect(await panel.findByText(/priced from the last 24h/)).toBeVisible();
    // the custom one can go; the observation cannot
    await expect(panel.getByRole("button", { name: "Remove tier=gold" })).toBeVisible();
    // and only this provider's labels: `region=eu` belongs to the other row
    await expect(panel.queryByRole("button", { name: "Remove region=eu" })).toBeNull();
    await expect(panel.queryByRole("button", { name: "Remove tier=observed-gold" })).toBeNull();
  },
};

/** the API's 409 is named before it is sent: that key is already set here */
export const DuplicateKeyIsRefusedBeforeSending: Story = {
  render: () => (
    <Harness fetchStub={withLabels}>
      <Providers />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getAllByText("openai-prod").length).toBeGreaterThan(0));
    await userEvent.click(canvas.getByRole("button", { name: "Labels on openai-prod" }));
    const panel = within(await within(document.body).findByRole("dialog"));
    await userEvent.type(await panel.findByLabelText("Key"), "tier");
    await waitFor(() => expect(panel.getByText(/already set here/)).toBeVisible());
    await expect(panel.getByRole("button", { name: "Add label" })).toBeDisabled();
  },
};

/**
 * Labels are an addition to this screen, not its subject: a caller who may not
 * read them still gets the providers, with no error panel over the list.
 */
export const LabelsUnavailable: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input) => {
        const url = String(input);
        if (url.includes("/labels")) return json({ error: { message: "forbidden" } }, 403);
        if (url.includes("/providers")) return json(PROVIDERS);
        return json([]);
      })}
    >
      <Providers />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getAllByText("openai-prod").length).toBeGreaterThan(0));
    await expect(canvas.queryByRole("alert")).toBeNull();
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

/**
 * Open the add-provider sheet, where the provider-key field explains which of
 * the three credentials it wants (#943).
 */
async function openTheProviderKeyField(canvasElement: HTMLElement) {
  // the button is disabled by its own prop until the scope resolves an org
  await clickWhenEnabled(canvasElement, "+ Add provider");
  return within(await within(document.body).findByRole("dialog"));
}

/** The hint carries a link into `security/which-key` when docs exist (#1164). */
export const ProviderKeyHintLinksToTheDocs: Story = {
  beforeEach: withDocsBase("https://docs.example.com"),
  render: () => (
    <Harness fetchStub={loaded}>
      <Providers />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const sheet = await openTheProviderKeyField(canvasElement);
    const link = await sheet.findByRole("link", { name: /Which key do I need/ });
    await expect(link).toHaveAttribute("href", "https://docs.example.com/security/which-key");
  },
};

/**
 * The air-gapped default: no documentation host, so the hint stands alone and
 * there is no link to click into nothing.
 */
export const ProviderKeyHintHasNoLinkWithoutADocsHost: Story = {
  beforeEach: withDocsBase(undefined),
  render: () => (
    <Harness fetchStub={loaded}>
      <Providers />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const sheet = await openTheProviderKeyField(canvasElement);
    // the hint itself is still there — only the link is suppressed
    await sheet.findByText(/the credential this provider issued to rolter/);
    await expect(sheet.queryByRole("link", { name: /Which key do I need/ })).toBeNull();
  },
};

// ------------------------------------------------------------------ sorting (#2143)

const SPARE: ProviderRow = {
  id: "p-3",
  org_id: "org-1",
  name: "mistral-fr",
  slug: "mistral-fr",
  kind: "mistral",
  api_base: "https://api.mistral.ai/v1",
  api_key_env: null,
  egress_proxies: [],
  created_at: "2026-01-12T00:00:00Z",
};

const THREE = routes([
  ["/providers", () => [...PROVIDERS, SPARE]],
  ["/config/problems", () => ({ problems: [] })],
]);

/** the first cell of every body row, in the order the list shows them */
const firstCells = (canvas: ReturnType<typeof within>) =>
  canvas
    .getAllByRole("row")
    .slice(1)
    .map((row: HTMLElement) => within(row).getAllByRole("cell")[0]?.textContent);

/**
 * Every column sorts, as Provider Groups does. A click sorts ascending, a
 * second descending and a third goes back to the order the control plane sent,
 * and the direction is the header's `aria-sort`, not only an arrow.
 */
export const EveryColumnSorts: Story = {
  render: () => (
    <Harness fetchStub={THREE}>
      <Providers />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getAllByText("mistral-fr").length).toBeGreaterThan(0));
    await expectListTable(canvasElement, "Model Providers");
    for (const name of ["Name", "Type", "API base", "Slug", "Key env"]) {
      const header = canvas.getByRole("columnheader", { name });
      await expect(within(header).getByRole("button")).toBeVisible();
      await expect(header).toHaveAttribute("aria-sort", "none");
    }
    await expect(firstCells(canvas)).toEqual(["openai-prod", "anthropic-eu", "mistral-fr"]);

    const header = canvas.getByRole("columnheader", { name: "Name" });
    const button = within(header).getByRole("button");
    await userEvent.click(button);
    await expect(header).toHaveAttribute("aria-sort", "ascending");
    await expect(firstCells(canvas)).toEqual(["anthropic-eu", "mistral-fr", "openai-prod"]);
    await userEvent.click(button);
    await expect(header).toHaveAttribute("aria-sort", "descending");
    await expect(firstCells(canvas)).toEqual(["openai-prod", "mistral-fr", "anthropic-eu"]);
    await userEvent.click(button);
    await expect(header).toHaveAttribute("aria-sort", "none");
    await expect(firstCells(canvas)).toEqual(["openai-prod", "anthropic-eu", "mistral-fr"]);

    // another column takes over the sort: type reads anthropic, mistral, openai
    const type = canvas.getByRole("columnheader", { name: "Type" });
    await userEvent.click(within(type).getByRole("button"));
    await expect(type).toHaveAttribute("aria-sort", "ascending");
    await expect(firstCells(canvas)).toEqual(["anthropic-eu", "mistral-fr", "openai-prod"]);
  },
};

// ------------------------------------------------ test right after a create (#2142)

const CREATED: ProviderRow = {
  id: "p-new",
  org_id: "org-1",
  name: "vllm-eu",
  slug: "vllm-eu",
  kind: "openai",
  api_base: "http://vllm.internal:8000",
  api_key_env: null,
  egress_proxies: [],
  created_at: "2026-09-30T10:00:00Z",
};

const PROBE: ProviderTestResult = {
  reachable: true,
  probed_url: "http://vllm.internal:8000/v1/models",
  status: 200,
  latency_ms: 38,
  credential: "none",
  models_found: 2,
  error: null,
};

let created: Recorder;

/**
 * Create leaves the sheet open on the new provider with the connection test
 * one click away, instead of closing over it and sending the operator to find
 * the row and open Edit. The list behind already holds the row, and the test is
 * not run until it is asked for.
 */
export const CreateThenTest: Story = {
  render: () => {
    let stored = [...PROVIDERS];
    created = recording(
      scoped(async (input, init) => {
        const url = String(input);
        if (init?.method === "POST" && url.endsWith("/test")) return json(PROBE);
        if (init?.method === "POST") {
          stored = [...stored, CREATED];
          return json(CREATED);
        }
        if (url.includes("/config/problems")) return json({ problems: [] });
        if (url.includes("/providers")) return json(stored);
        return json([]);
      }),
    );
    return (
      <Harness fetchStub={created.stub}>
        <Providers />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await clickWhenEnabled(canvasElement, "+ Add provider");
    const dialog = within(await within(document.body).findByRole("dialog"));
    await userEvent.type(await dialog.findByLabelText("Name"), "vllm-eu");
    await userEvent.type(dialog.getByLabelText("API base"), "http://vllm.internal:8000");
    await userEvent.click(dialog.getByRole("button", { name: "Create provider" }));

    await dialog.findByText(/vllm-eu is created but not tested yet/);
    await waitFor(() => expect(canvas.getAllByText("vllm-eu").length).toBeGreaterThan(0));
    created.expectNotSent("POST", "/test");

    await userEvent.click(dialog.getByRole("button", { name: "Test connection" }));
    await created.expectSent("POST", "/providers/p-new/test");
    await waitFor(() => expect(dialog.getByText(/Reachable · 2 models/)).toBeVisible());

    // Done closes it, with nothing unsaved to confirm
    await userEvent.click(dialog.getByRole("button", { name: "Done" }));
    await expectSheetClosed();
  },
};

/**
 * A probe belongs to the provider it ran against. The sheet stays mounted on
 * this screen, so a result that outlived the closing would greet the next
 * provider opened and read as that one's health.
 */
export const ATestResultDoesNotFollowToTheNextProvider: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input, init) =>
        init?.method === "POST" && String(input).endsWith("/test")
          ? json(PROBE)
          : loaded(input, init),
      )}
    >
      <Providers />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "Edit provider openai-prod");
    const first = within(await within(document.body).findByRole("dialog"));
    await userEvent.click(await first.findByRole("button", { name: "Test connection" }));
    await waitFor(() => expect(first.getByText(/Reachable · 2 models/)).toBeVisible());
    await userEvent.click(first.getByRole("button", { name: "Cancel" }));
    await expectSheetClosed();

    await clickWhenEnabled(canvasElement, "Edit provider anthropic-eu");
    const second = within(await within(document.body).findByRole("dialog"));
    await second.findByRole("button", { name: "Test connection" });
    await expect(second.queryByText(/Reachable/)).toBeNull();
  },
};

// ------------------------------------------- what uses the provider being deleted (#2143)

const GROUPS: ProviderGroupRow[] = [
  {
    id: "g-1",
    org_id: "org-1",
    name: "eu-fleet",
    slug: "eu-fleet",
    strategy: "round_robin",
    created_at: "2026-01-20T00:00:00Z",
    members: [
      {
        group_id: "g-1",
        provider_id: "p-1",
        provider_name: "openai-prod",
        weight: 1,
        position: 0,
      },
      {
        group_id: "g-1",
        provider_id: "p-2",
        provider_name: "anthropic-eu",
        weight: 1,
        position: 1,
      },
    ],
  },
];

const target = (provider: string) => ({ provider, weight: 1 });

const EFFECTIVE = {
  providers: [],
  virtual_keys: [],
  routes: [
    { model: "gpt-4o", strategy: "weighted", targets: [target("openai-prod")] },
    {
      model: "chat",
      strategy: "weighted",
      targets: [target("openai-prod"), target("anthropic-eu")],
    },
    { model: "claude", strategy: "weighted", targets: [target("anthropic-eu")] },
  ],
};

/** the screen's reads plus the two the delete confirm makes, each overridable */
const withUsage = (over: { config?: () => Promise<Response> } = {}) =>
  scoped(async (input, init) => {
    const url = new URL(String(input), "http://localhost");
    if (url.pathname.endsWith("/config/problems")) return json({ problems: [] });
    if (url.pathname.endsWith("/config")) return over.config ? over.config() : json(EFFECTIVE);
    if (url.pathname.endsWith("/provider-groups")) return json(GROUPS);
    if (url.pathname.endsWith("/providers")) return json([...PROVIDERS, SPARE]);
    return loaded(input, init);
  });

const openDeleteFor = async (canvasElement: HTMLElement, name: string) => {
  await clickWhenEnabled(canvasElement, `Delete provider ${name}`);
  return within(await within(document.body).findByRole("dialog"));
};

/**
 * The confirm names what still points at the provider: the routes that target
 * it, flagging the one it is the only target of, and the groups it belongs to.
 * It also says what the delete does to a client addressing it directly, and
 * leaves the confirm pressable, since the control plane has the last word.
 */
export const DeleteNamesTheRoutesAndGroupsThatUseTheProvider: Story = {
  render: () => (
    <Harness fetchStub={withUsage()}>
      <Providers />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const dialog = await openDeleteFor(canvasElement, "openai-prod");
    await dialog.findByText("Target of 2 routes");
    await expect(dialog.getByText("gpt-4o")).toBeVisible();
    await expect(dialog.getByText("chat")).toBeVisible();
    // the route the provider is the whole of is marked, the other is not
    await expect(dialog.getAllByText("only target")).toHaveLength(1);
    await expect(dialog.getByText("gpt-4o").parentElement).toHaveTextContent("only target");
    await expect(dialog.queryByText("claude")).toBeNull();
    await expect(dialog.getByText("Member of 1 group")).toBeVisible();
    await expect(dialog.getByText("eu-fleet")).toBeVisible();
    await expect(dialog.getByText(/take it out of those first/)).toBeVisible();
    await expect(dialog.getByText("openai-prod/model")).toBeVisible();
    await expect(dialog.getByRole("button", { name: "Delete provider" })).toBeEnabled();
  },
};

/** a provider nothing points at says so, in place of a list */
export const DeleteSaysWhenNothingUsesTheProvider: Story = {
  render: () => (
    <Harness fetchStub={withUsage()}>
      <Providers />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const dialog = await openDeleteFor(canvasElement, "mistral-fr");
    await dialog.findByText("No route or group uses it.");
    await expect(dialog.queryByText(/Target of/)).toBeNull();
    await expect(dialog.queryByText(/Member of/)).toBeNull();
  },
};

/**
 * While the reads are out the space is held and nothing is claimed: "no route
 * uses it" on an answer that has not arrived is how a delete gets confirmed on
 * a provider that is still in use.
 */
export const DeleteDoesNotClaimUnusedWhileChecking: Story = {
  render: () => (
    <Harness fetchStub={withUsage({ config: () => new Promise<Response>(() => {}) })}>
      <Providers />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const dialog = await openDeleteFor(canvasElement, "mistral-fr");
    await waitFor(() => expect(dialog.getByRole("status")).toBeVisible());
    await expect(dialog.queryByText("No route or group uses it.")).toBeNull();
    await expect(dialog.getByRole("button", { name: "Delete provider" })).toBeEnabled();
  },
};

/**
 * A failed read says so, with a retry, and does not read as "unused" either.
 * The retry asks again and the answer that follows is the one shown.
 */
export const DeleteDoesNotClaimUnusedWhenTheReadFailed: Story = {
  render: () => {
    let asked = 0;
    return (
      <Harness
        fetchStub={withUsage({
          config: async () =>
            asked++ === 0
              ? json({ error: { message: "config store is down" } }, 500)
              : json(EFFECTIVE),
        })}
      >
        <Providers />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const dialog = await openDeleteFor(canvasElement, "mistral-fr");
    const alert = await dialog.findByRole("alert");
    await expect(alert).toHaveTextContent(/failed to return the routes and groups that use it/);
    await expect(alert).toHaveTextContent(/config store is down/);
    await expect(dialog.queryByText("No route or group uses it.")).toBeNull();
    await expect(dialog.getByRole("button", { name: "Delete provider" })).toBeEnabled();

    await userEvent.click(dialog.getByRole("button", { name: "Try again" }));
    await dialog.findByText("No route or group uses it.");
    await expect(dialog.queryByRole("alert")).toBeNull();
  },
};

/**
 * A provider behind a dozen routes, one of them with a name that never breaks:
 * the confirm lists eight, says how many more there are, and does not push the
 * page sideways on a phone.
 */
export const DeleteUsageHandlesManyAndLongNames: Story = {
  ...atMobile,
  render: () => {
    const long = "Qwen/Qwen2.5-72B-Instruct-GPTQ-Int4-long-context-eu-west-production-primary";
    const many = {
      ...EFFECTIVE,
      routes: [
        { model: long, strategy: "weighted", targets: [target("openai-prod")] },
        ...Array.from({ length: 11 }, (_, i) => ({
          model: `route-${String(i).padStart(2, "0")}`,
          strategy: "weighted",
          targets: [target("openai-prod"), target("anthropic-eu")],
        })),
      ],
    };
    return (
      <Harness fetchStub={withUsage({ config: async () => json(many) })}>
        <Providers />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const dialog = await openDeleteFor(canvasElement, "openai-prod");
    await dialog.findByText("Target of 12 routes");
    await expect(dialog.getByText("and 4 more")).toBeVisible();
    // eight routes and the "more" line, then the one group it belongs to
    await expect(dialog.getAllByRole("listitem")).toHaveLength(10);
    await expectNoHorizontalOverflow();
    const panel = within(document.body).getByRole("dialog");
    await expect(panel.scrollWidth).toBeLessThanOrEqual(panel.clientWidth);
  },
};
