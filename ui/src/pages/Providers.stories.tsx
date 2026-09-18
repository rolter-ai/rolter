import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import Providers from "./Providers";
import {
  Harness,
  expectEmptyState,
  expectLoadError,
  expectSkeleton,
  json,
  pending,
  routes,
  scoped,
  Toasted,
  expectToast,
} from "./story-harness";
import type { LabelRow, ProviderRow } from "@/lib/api";
import { atMobile, atTablet, expectNoHorizontalOverflow } from "@/lib/story-viewport";

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
  ["/config/problems", () => []],
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
  render: () => (
    <Harness
      fetchStub={scoped(async (input, init) =>
        init?.method === "DELETE"
          ? json({ error: { message: "openai-prod is the target of 4 live routes" } }, 409)
          : loaded(input, init),
      )}
    >
      <Toasted>
        <Providers />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", { name: "Delete provider openai-prod" }),
    );
    const dialog = within(await within(document.body).findByRole("dialog"));
    await userEvent.click(dialog.getByRole("button", { name: "Delete" }));

    await expectToast(canvasElement, /target of 4 live routes/, "error");
    await waitFor(() => expect(dialog.getByText(/target of 4 live routes/)).toBeVisible());
    await expect(within(document.body).getByRole("dialog")).toBeInTheDocument();
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
    await expect(
      canvas.getByLabelText("tier=observed-gold, automatic label"),
    ).toBeVisible();
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
    await expect(
      panel.queryByRole("button", { name: "Remove tier=observed-gold" }),
    ).toBeNull();
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
  const canvas = within(canvasElement);
  await userEvent.click(await canvas.findByRole("button", { name: "+ Add provider" }));
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
