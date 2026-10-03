import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { MemoryRouter, Route, Routes, useLocation, useNavigate } from "react-router";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { BusinessUnits, Customers } from "./CostAttribution";
import {
  cancelConfirmation,
  confirmDestructive,
  expectInStatusRegion,
  expectLoadError,
  expectNoFalseEmpty,
  expectRefused,
  expectSheetClosed,
  expectSkeleton,
  expectToast,
  Harness as ScreenHarness,
  json,
  pickOption,
  recording,
  sheet,
  Toasted,
  type Recorder,
  type StoryRole,
} from "./story-harness";
import type { AttributionSpendRow, BusinessUnitRow, CustomerRow } from "@/lib/api";
import { formattersFor } from "@/lib/i18n/format";
import { TIME_WINDOW_STORAGE_KEY } from "@/lib/time-window";

// the formatter the screen itself uses, so a story asserts the house format
// rather than a second copy of it
const fmt = formattersFor("en");

const spendRow = (id: string, cost: number, requests: number): AttributionSpendRow => ({
  id,
  requests,
  tokens: requests * 900,
  prompt_tokens: requests * 600,
  completion_tokens: requests * 300,
  cost_usd: cost,
  errors: 0,
});

const ORG = "11111111-1111-1111-1111-111111111111";

const UNITS: BusinessUnitRow[] = [
  {
    id: "aaaaaaaa-0000-0000-0000-000000000001",
    org_id: ORG,
    name: "Platform Engineering",
    slug: "platform-engineering",
    retired_at: null,
    created_at: "2026-01-05T10:00:00Z",
  },
  {
    id: "aaaaaaaa-0000-0000-0000-000000000002",
    org_id: ORG,
    name: "Legacy Research",
    slug: "legacy-research",
    retired_at: "2026-06-01T10:00:00Z",
    created_at: "2025-03-05T10:00:00Z",
  },
];

const CUSTOMERS: CustomerRow[] = [
  {
    id: "bbbbbbbb-0000-0000-0000-000000000001",
    org_id: ORG,
    business_unit_id: UNITS[0].id,
    name: "Acme Corp",
    slug: "acme-corp",
    retired_at: null,
    created_at: "2026-02-05T10:00:00Z",
  },
  {
    id: "bbbbbbbb-0000-0000-0000-000000000002",
    org_id: ORG,
    business_unit_id: null,
    name: "Globex",
    slug: "globex",
    retired_at: null,
    created_at: "2026-04-05T10:00:00Z",
  },
];

// `""` is the unattributed bucket the control plane returns when a key never
// named a unit or a customer — the hole in an otherwise tidy chargeback report
const UNIT_SPEND: AttributionSpendRow[] = [
  spendRow(UNITS[0].id, 128.5, 4200),
  // a retired unit still carries its history: that is the whole reason
  // retiring exists rather than deleting
  spendRow(UNITS[1].id, 20, 700),
  spendRow("", 41.5, 1300),
];

const CUSTOMER_SPEND: AttributionSpendRow[] = [
  spendRow(CUSTOMERS[0].id, 90, 3000),
  spendRow(CUSTOMERS[1].id, 25, 800),
  spendRow("", 60, 2000),
];

type FetchStub = (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;

// route by path so useScope's /api/v1/orgs call is served alongside the
// screen's own collection
function router(
  handlers: Partial<Record<"units" | "customers" | "spend", (init?: RequestInit) => Response>>,
): FetchStub {
  return async (input, init) => {
    const url = String(input);
    if (url.endsWith("/orgs")) return json([{ id: ORG, name: "Acme", slug: "acme" }]);
    // checked before the collections: the rollup's own query string carries
    // `dimension=customer`, and a looser match would answer it with the roster
    if (url.includes("/analytics/by-attribution")) {
      return (
        handlers.spend?.(init) ??
        json({
          data: url.includes("dimension=customer") ? CUSTOMER_SPEND : UNIT_SPEND,
        })
      );
    }
    if (url.includes("/currency")) return json({ base: "USD", codes: ["USD"], rates: {} });
    if (url.includes("/business-units") || url.includes("/business-units/")) {
      return handlers.units?.(init) ?? json(UNITS);
    }
    if (url.includes("/customers")) {
      return handlers.customers?.(init) ?? json(CUSTOMERS);
    }
    // teams/projects, fetched by useScope on the way to an org id
    return json([]);
  };
}

/**
 * The screen under the shared fetch-stub harness, with a role to render as.
 *
 * `role` is what a story needs to mount a `CapabilityProvider` at all: with no
 * provider above it `can()` answers "unknown" and every gated control renders
 * enabled, so a story without one can never see a control refused (#1606).
 */
function Harness({
  fetchStub,
  role,
  children,
}: {
  fetchStub: FetchStub;
  role?: StoryRole;
  children: React.ReactNode;
}) {
  return (
    <ScreenHarness fetchStub={fetchStub} role={role}>
      {children}
    </ScreenHarness>
  );
}

const meta = {
  title: "Screens/CostAttribution",
  component: BusinessUnits,
  parameters: { layout: "fullscreen" },
  // the spend window lives in the address (#2107), so every story supplies a
  // router the way main.tsx does; `parameters.address` is where it starts
  decorators: [
    (Story, { parameters }) => (
      <MemoryRouter initialEntries={parameters.address ? [parameters.address] : undefined}>
        <Story />
      </MemoryRouter>
    ),
  ],
  // the window picked last is carried in session storage, which outlives a
  // story: one that picked "last month" would hand it to the next one
  beforeEach: () => {
    sessionStorage.removeItem(TIME_WINDOW_STORAGE_KEY);
    return () => sessionStorage.removeItem(TIME_WINDOW_STORAGE_KEY);
  },
} satisfies Meta<typeof BusinessUnits>;

export default meta;
type Story = StoryObj<typeof meta>;

export const BusinessUnitsLoaded: Story = {
  render: () => (
    <Harness fetchStub={router({})}>
      <BusinessUnits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("Platform Engineering")).toBeVisible());
    // a retired unit keeps its history rather than disappearing
    await expect(canvas.getByText("RETIRED")).toBeVisible();
  },
};

export const BusinessUnitsLoading: Story = {
  // only the collection hangs: the scope chain still has to resolve, or the
  // query never becomes enabled and the screen shows an empty list instead
  render: () => (
    <Harness
      fetchStub={async (input, init) =>
        String(input).includes("/business-units")
          ? new Promise<Response>(() => {})
          : router({})(input, init)
      }
    >
      <BusinessUnits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
    await expectNoFalseEmpty(canvasElement, /No business units yet/);
  },
};

export const BusinessUnitsEmpty: Story = {
  render: () => (
    <Harness fetchStub={router({ units: () => json([]) })}>
      <BusinessUnits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("No business units yet")).toBeVisible());
  },
};

export const BusinessUnitsForbidden: Story = {
  render: () => (
    <Harness
      fetchStub={router({
        units: () => json({ error: { message: "forbidden" } }, 403),
      })}
    >
      <BusinessUnits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() =>
      expect(canvas.getByText(/You do not have access to business units/)).toBeVisible(),
    );
    await expectNoFalseEmpty(canvasElement, /No business units yet/);
  },
};

// the slug is the identity spend is attributed by, so renaming it is gated
// behind an explicit confirmation rather than silently accepted and rejected
// by the server
export const SlugRenameNeedsConfirmation: Story = {
  render: () => (
    <Harness fetchStub={router({})}>
      <BusinessUnits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("Platform Engineering")).toBeVisible());
    // by name, not by index: each row control names its own unit (#1214)
    await userEvent.click(canvas.getByRole("button", { name: "Edit Platform Engineering" }));
    // the sheet portals to document.body, so query outside the canvas root
    const sheet = within(document.body);
    const slug = await sheet.findByLabelText("Slug");
    await userEvent.clear(slug);
    await userEvent.type(slug, "platform-eng");
    await waitFor(() =>
      expect(sheet.getByText(/Renaming the slug breaks attribution/)).toBeVisible(),
    );
    await expect(sheet.getByRole("button", { name: "Save" })).toBeDisabled();
    await userEvent.click(sheet.getByLabelText("Allow slug change"));
    await expect(sheet.getByRole("button", { name: "Save" })).toBeEnabled();
  },
};

// a slug that cannot round-trip through the server's rule is refused up front
export const RejectsAnInvalidSlug: Story = {
  render: () => (
    <Harness fetchStub={router({})}>
      <BusinessUnits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("Platform Engineering")).toBeVisible());
    await userEvent.click(canvas.getAllByRole("button", { name: "+ New business unit" })[0]);
    const sheet = within(document.body);
    await userEvent.type(await sheet.findByLabelText("Name"), "Ops");
    const slug = sheet.getByLabelText("Slug");
    await userEvent.clear(slug);
    await userEvent.type(slug, "Not A Slug");
    await waitFor(() =>
      expect(sheet.getByText(/Slug must be lowercase alphanumerics/)).toBeVisible(),
    );
  },
};

/**
 * The unit is refused (#1607).
 *
 * The refusal is reported twice — an assertive toast and the inline message
 * beside the toolbar — and both are asserted, because either could stop
 * reporting on its own.
 *
 * The sheet survives it, with the typed draft intact. It did not: `onSubmit`
 * called `setOpen(false)` before the mutation had answered, so a refused save
 * threw away the name, the slug and the slug-change acknowledgement, leaving
 * nothing on screen that could bring them back (#1626).
 */
export const CreateRejectedByTheServer: Story = {
  render: () => (
    <Harness
      fetchStub={router({
        units: (init) =>
          init?.method === "POST"
            ? json({ error: { message: "slug ops is already taken" } }, 409)
            : json(UNITS),
      })}
    >
      <Toasted>
        <BusinessUnits />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("Platform Engineering")).toBeVisible());
    await userEvent.click(canvas.getAllByRole("button", { name: "+ New business unit" })[0]);
    const panel = within(document.body);
    await userEvent.type(await panel.findByLabelText("Name"), "Ops");
    await userEvent.click(panel.getByRole("button", { name: "Create" }));

    await expectToast(canvasElement, /already taken/, "error");
    // and the inline copy beside the toolbar, which is what is left on screen
    // once the toast has gone — matched by excluding the toast's own live
    // region, since both carry the same words
    await waitFor(() =>
      expect(
        canvas
          .getAllByText(/already taken/)
          .some((node) => node.closest('[role="alert"]') === null),
      ).toBe(true),
    );
    // and the draft is still there to correct: the sheet stands, the name field
    // holds what was typed, and the inline refusal is inside the sheet
    await expect(panel.getByRole("button", { name: "Create" })).toBeVisible();
    await expect(await panel.findByLabelText("Name")).toHaveValue("Ops");
    await expect(within(sheet()).getByText(/already taken/)).toBeVisible();
  },
};

export const CustomersLoaded: Story = {
  render: () => (
    <Harness fetchStub={router({})}>
      <Customers />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // an assigned customer names its unit; an unassigned one says so
    await waitFor(() => expect(canvas.getByText("Platform Engineering")).toBeVisible());
    await expect(canvas.getByText("unassigned")).toBeVisible();
  },
};

export const CustomersEmpty: Story = {
  render: () => (
    <Harness fetchStub={router({ customers: () => json([]) })}>
      <Customers />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("No customers yet")).toBeVisible());
  },
};

// Retire and Delete sit one click apart on the same row, and only one of them
// is reversible. The confirmation is what separates them (#1179).
//
// the roster shrinks once the DELETE lands, so the story can assert what the
// delete left behind rather than that the request went out. A stub answering
// the full list forever passes even when the mutation took its `onError` path,
// which is how a 204 fixture that threw went unnoticed (#1260)
let unitDeleted = false;
const unitDeletes = recording(async (input, init) => {
  if (init?.method === "DELETE") {
    unitDeleted = true;
    return json({}, 204);
  }
  return router({
    units: () => json(unitDeleted ? UNITS.filter((row) => row.id !== UNITS[0].id) : UNITS),
  })(input, init);
});

export const ConfirmsBeforeDeletingABusinessUnit: Story = {
  render: () => {
    unitDeleted = false;
    return (
      <Harness fetchStub={unitDeletes.stub}>
        <Toasted>
          <BusinessUnits />
        </Toasted>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("Platform Engineering")).toBeVisible());
    const del = () => canvas.getByRole("button", { name: "Delete Platform Engineering" });

    await userEvent.click(del());
    await cancelConfirmation();
    unitDeletes.expectNotSent("DELETE", `/business-units/${UNITS[0].id}`);

    await userEvent.click(del());
    // the copy points at the reversible alternative rather than only warning
    await confirmDestructive(/Platform Engineering/, "Delete");
    await unitDeletes.expectSent("DELETE", `/business-units/${UNITS[0].id}`);

    // the outcome, not only the request: the confirmation closes, the queue
    // announces it, and the card is gone from the roster
    await expectSheetClosed();
    await expectToast(canvasElement, /Platform Engineering deleted/);
    await waitFor(() => expect(canvas.queryByText("Platform Engineering")).not.toBeInTheDocument());
  },
};

let customerDeleted = false;
const customerDeletes = recording(async (input, init) => {
  if (init?.method === "DELETE") {
    customerDeleted = true;
    return json({}, 204);
  }
  return router({
    customers: () =>
      json(customerDeleted ? CUSTOMERS.filter((row) => row.id !== CUSTOMERS[0].id) : CUSTOMERS),
  })(input, init);
});

export const ConfirmsBeforeDeletingACustomer: Story = {
  render: () => {
    customerDeleted = false;
    return (
      <Harness fetchStub={customerDeletes.stub}>
        <Toasted>
          <Customers />
        </Toasted>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("Acme Corp")).toBeVisible());
    await userEvent.click(canvas.getByRole("button", { name: "Delete Acme Corp" }));
    await confirmDestructive(/Acme Corp/, "Delete");
    await customerDeletes.expectSent("DELETE", `/customers/${CUSTOMERS[0].id}`);

    await expectSheetClosed();
    await expectToast(canvasElement, /Acme Corp deleted/);
    await waitFor(() => expect(canvas.queryByText("Acme Corp")).not.toBeInTheDocument());
  },
};

/**
 * #1193: the screens listed units and customers but could not say what any of
 * them cost, which made the whole feature write-only. Spend for the window now
 * sits on every card, with the totals above the grid.
 */
export const BusinessUnitsShowWindowSpend: Story = {
  render: () => (
    <Harness fetchStub={router({})}>
      <BusinessUnits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText(fmt.currency(190, "USD"))).toBeVisible());
    // attributed and unattributed are shown side by side: a report that lists
    // five units and quietly omits a quarter of the spend is the failure mode
    await expect(canvas.getByText(fmt.currency(148.5, "USD"))).toBeVisible();
    await expect(canvas.getByText(fmt.currency(128.5, "USD"))).toBeVisible();
    await expect(canvas.getByText(fmt.currency(41.5, "USD"))).toBeVisible();
    await expect(canvas.getByText("Unattributed")).toBeVisible();
    await expect(canvas.getByText(/of the window/)).toBeVisible();
  },
};

/** amounts follow the deployment's settlement currency, never a literal `$` */
export const SpendFollowsTheDeploymentCurrency: Story = {
  render: () => (
    <Harness
      fetchStub={async (input, init) =>
        String(input).includes("/currency")
          ? json({ base: "EUR", codes: ["EUR"], rates: {} })
          : router({})(input, init)
      }
    >
      <BusinessUnits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText(fmt.currency(190, "EUR"))).toBeVisible());
  },
};

/** the customer screen reads the same rollup on its own dimension */
export const CustomersShowWindowSpend: Story = {
  render: () => (
    <Harness fetchStub={router({})}>
      <Customers />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // total, attributed and each card's own figure are all distinct here, so
    // the assertions cannot pass by coincidence
    await waitFor(() => expect(canvas.getByText(fmt.currency(175, "USD"))).toBeVisible());
    await expect(canvas.getByText(fmt.currency(115, "USD"))).toBeVisible();
    await expect(canvas.getByText(fmt.currency(90, "USD"))).toBeVisible();
    await expect(canvas.getByText(fmt.currency(25, "USD"))).toBeVisible();
  },
};

/** a unit with no traffic in the window says so rather than printing a zero */
export const AUnitWithNoTrafficSaysSo: Story = {
  render: () => (
    <Harness fetchStub={router({ spend: () => json({ data: [] }) })}>
      <BusinessUnits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() =>
      expect(canvas.getAllByText("No spend in this window").length).toBeGreaterThan(0),
    );
  },
};

export const SpendLoading: Story = {
  render: () => (
    <Harness
      fetchStub={async (input, init) =>
        String(input).includes("/analytics/by-attribution")
          ? new Promise<Response>(() => {})
          : router({})(input, init)
      }
    >
      <BusinessUnits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // the roster first: until the cards exist, "no card says no spend" is true
    // of a screen that has not rendered them
    await waitFor(() => expect(canvas.getByText("Platform Engineering")).toBeVisible());
    await expectInStatusRegion(canvasElement, "spend-loading");
    // every card holds its figure's place rather than claiming no spend (#2105)
    await expect(canvas.getAllByTestId("card-spend-loading")).toHaveLength(UNITS.length);
    await expectNoFalseEmpty(canvasElement, /No spend in this window/);
  },
};

/**
 * Analytics is optional; governance is not. A deployment with no ClickHouse
 * gets the `noAnalytics` load error where the spend strip would be — the
 * setting it lacks, and no retry that cannot help (#1270) — and keeps the
 * postgres-backed roster it can still serve.
 */
export const SpendUnavailableKeepsTheRoster: Story = {
  render: () => (
    <Harness
      fetchStub={router({
        spend: () => json({ error: { message: "analytics is not configured" } }, 503),
      })}
    >
      <BusinessUnits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectLoadError(canvasElement, /Analytics are not configured[\s\S]*attribution spend/);
    await expect(canvas.queryByRole("button", { name: /try again/i })).toBeNull();
    await expect(canvas.getByText("Platform Engineering")).toBeVisible();
    await expect(canvas.queryAllByTestId("card-spend-loading")).toHaveLength(0);
    await expectNoFalseEmpty(canvasElement, /No spend in this window/);
  },
};

/** a failed rollup is a failure, not a quiet day: it says so and offers a retry */
export const SpendFailed: Story = {
  render: () => (
    <Harness
      fetchStub={router({
        spend: () => json({ error: { message: "analytics query failed" } }, 502),
      })}
    >
      <BusinessUnits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() =>
      expect(
        canvas.getAllByRole("alert").some((a) => /attribution spend/.test(a.textContent ?? "")),
      ).toBe(true),
    );
    // the strip's alert is the one place that says why; the cards say nothing
    // about spend at all, neither a figure nor "no spend" (#2105)
    await expect(canvas.getByText("Platform Engineering")).toBeVisible();
    await expect(canvas.queryByText(fmt.currency(128.5, "USD"))).not.toBeInTheDocument();
    await expect(canvas.queryAllByTestId("card-spend-loading")).toHaveLength(0);
    await expectNoFalseEmpty(canvasElement, /No spend in this window/);
  },
};

// One screen, two resources (#1606). A business unit is `business_unit` and a
// customer is `customer`, both admin at every action, and the screen picks the
// capability from the `kind` it was rendered with — so each half has to be
// asserted separately or a swapped pair passes.
export const BusinessUnitsRefusedToAViewer: Story = {
  render: () => (
    <Harness fetchStub={router({})} role="viewer">
      <BusinessUnits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, /new business unit/i);
    await expectRefused(canvasElement, "Edit Platform Engineering");
    await expectRefused(canvasElement, "Retire Platform Engineering");
    await expectRefused(canvasElement, "Delete Platform Engineering");
  },
};

export const CustomersRefusedToAMember: Story = {
  render: () => (
    <Harness fetchStub={router({})} role="member">
      <Customers />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, /new customer/i);
    await expectRefused(canvasElement, "Edit Acme Corp");
    await expectRefused(canvasElement, "Delete Acme Corp");
  },
};

// --- the spend window (#2107) ----------------------------------------------

/**
 * The router's search string, published so a play can read what the screen
 * wrote to the address. It rides on a data attribute with no text, so no
 * `getByText` can match it.
 */
function AddressProbe() {
  const { search } = useLocation();
  return <span data-testid="address" data-search={search} hidden />;
}

const addressOf = (canvasElement: HTMLElement) =>
  new URLSearchParams(within(canvasElement).getByTestId("address").dataset.search ?? "");

/** the query string of every spend rollup the screen asked for, oldest first */
const spendQueries = (recorder: Recorder) =>
  recorder.calls
    .filter((c) => c.url.includes("/analytics/by-attribution"))
    .map((c) => new URL(c.url, "http://localhost").searchParams);

/** the spend rollup the screen asked for last */
const lastSpendQuery = (recorder: Recorder) => {
  const all = spendQueries(recorder);
  return all[all.length - 1];
};

/** how far back a rolling window's `since` reached, in hours */
const hoursBack = (query: URLSearchParams | undefined) =>
  (Date.now() - Date.parse(query?.get("since") ?? "")) / 3_600_000;

/**
 * Local midnight on the first of the month `offset` months from this one:
 * the viewer's calendar, which is where a chargeback month starts. Written out
 * here rather than borrowed from the screen, so a wrong boundary there cannot
 * agree with itself.
 */
const monthStart = (offset: number) => {
  const now = new Date();
  return new Date(now.getFullYear(), now.getMonth() + offset, 1);
};

const windowPicker = (canvasElement: HTMLElement) =>
  within(canvasElement).getByRole("combobox", { name: "Time window" });

// last month's rollup is unlike the default one on purpose, so the figure on
// screen says which window was read. every amount is distinct, so no
// assertion can pass by matching another figure
const LAST_MONTH_UNIT_SPEND: AttributionSpendRow[] = [
  spendRow(UNITS[0].id, 2480, 81_000),
  spendRow(UNITS[1].id, 40, 1_200),
  spendRow("", 320, 9_000),
];

/** answers a closed window with last month's rollup and an open one as usual */
const byWindow = () =>
  recording(async (input, init) => {
    const url = new URL(String(input), "http://localhost");
    if (url.pathname.endsWith("/analytics/by-attribution") && url.searchParams.has("until")) {
      return json({ data: LAST_MONTH_UNIT_SPEND });
    }
    return router({})(input, init);
  });

const pickedWindow = byWindow();

/**
 * #2107: the screens reported one fixed day and nothing else, so "what did
 * this unit spend last month" had no answer here. Picking the window refetches
 * the rollup over it: last month is the whole previous calendar month, sent
 * with both bounds, and the strip names the window and the dates it covered.
 */
export const PicksTheSpendWindow: Story = {
  render: () => {
    pickedWindow.calls.length = 0;
    return (
      <Harness fetchStub={pickedWindow.stub}>
        <BusinessUnits />
        <AddressProbe />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText(fmt.currency(190, "USD"))).toBeVisible());
    // the default is the last 24 hours, reaching back from the request and
    // left open at the end for the control plane's clock to close
    await expect(canvas.getByText("Spend, last 24 hours")).toBeVisible();
    await expect(windowPicker(canvasElement)).toHaveValue("Last 24 hours");
    await expect(hoursBack(spendQueries(pickedWindow)[0])).toBeCloseTo(24, 1);
    await expect(spendQueries(pickedWindow)[0].has("until")).toBe(false);
    // and the default is not written into the address
    await expect(addressOf(canvasElement).has("window")).toBe(false);

    await pickOption(windowPicker(canvasElement), "Last month");

    await waitFor(() => expect(canvas.getByText(fmt.currency(2840, "USD"))).toBeVisible());
    const sent = lastSpendQuery(pickedWindow);
    await expect(sent?.get("since")).toBe(monthStart(-1).toISOString());
    await expect(sent?.get("until")).toBe(monthStart(0).toISOString());
    await expect(sent?.get("dimension")).toBe("business_unit");
    await expect(canvas.getByText("Spend, last month")).toBeVisible();
    // the caption closes the month on its last day, not on the exclusive
    // bound the request carries
    const lastDay = new Date(monthStart(0).getTime() - 1);
    await expect(
      canvas.getByText(`${fmt.date(monthStart(-1))} – ${fmt.date(lastDay)}`),
    ).toBeVisible();
    // the cards read the same window as the strip
    await expect(canvas.getByText(fmt.currency(2480, "USD"))).toBeVisible();
    await expect(canvas.queryByText(fmt.currency(190, "USD"))).not.toBeInTheDocument();
    await expect(addressOf(canvasElement).get("window")).toBe("last-month");
  },
};

const keyedWindow = byWindow();

/**
 * The picker is the shared `Combobox`, so it is a listbox of the five windows
 * and it is driven from the keyboard like every other dropdown. Going back to
 * the default takes the parameter out of the address rather than writing it.
 */
export const PicksTheWindowFromTheKeyboard: Story = {
  render: () => {
    keyedWindow.calls.length = 0;
    return (
      <Harness fetchStub={keyedWindow.stub}>
        <BusinessUnits />
        <AddressProbe />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText(fmt.currency(190, "USD"))).toBeVisible());
    const picker = windowPicker(canvasElement);
    picker.focus();
    await userEvent.keyboard("{ArrowDown}");
    const listbox = within(document.getElementById(picker.getAttribute("aria-controls") ?? "")!);
    await expect(listbox.getAllByRole("option").map((o) => o.textContent)).toEqual([
      "Last 24 hours",
      "Last 7 days",
      "Last 30 days",
      "Month to date",
      "Last month",
    ]);
    // the list opens on the window in force, so one step down is the next one
    await userEvent.keyboard("{ArrowDown}{Enter}");

    await waitFor(() => expect(canvas.getByText("Spend, last 7 days")).toBeVisible());
    await waitFor(() => expect(hoursBack(lastSpendQuery(keyedWindow))).toBeCloseTo(24 * 7, 1));
    await expect(lastSpendQuery(keyedWindow)?.has("until")).toBe(false);
    await expect(addressOf(canvasElement).get("window")).toBe("7d");

    await userEvent.keyboard("{ArrowDown}{Home}{Enter}");
    await waitFor(() => expect(canvas.getByText("Spend, last 24 hours")).toBeVisible());
    await expect(addressOf(canvasElement).has("window")).toBe(false);
  },
};

const addressedWindow = byWindow();

/**
 * A link or a reload comes back to the window it was opened on: the address is
 * read before the first request, so month to date is the only rollup asked for.
 */
export const OpensOnTheWindowInTheAddress: Story = {
  parameters: { address: "/customers?window=mtd" },
  render: () => {
    addressedWindow.calls.length = 0;
    return (
      <Harness fetchStub={addressedWindow.stub}>
        <Customers />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("Spend, month to date")).toBeVisible());
    await expect(windowPicker(canvasElement)).toHaveValue("Month to date");
    const sent = spendQueries(addressedWindow);
    await expect(sent).toHaveLength(1);
    await expect(sent[0].get("since")).toBe(monthStart(0).toISOString());
    await expect(sent[0].has("until")).toBe(false);
    await expect(sent[0].get("dimension")).toBe("customer");
  },
};

const unknownWindow = byWindow();

/** an address naming a window the screen does not know reads as the default */
export const AnUnknownWindowIsTheDefault: Story = {
  parameters: { address: "/business-units?window=fortnight" },
  render: () => {
    unknownWindow.calls.length = 0;
    return (
      <Harness fetchStub={unknownWindow.stub}>
        <BusinessUnits />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText(fmt.currency(190, "USD"))).toBeVisible());
    await expect(canvas.getByText("Spend, last 24 hours")).toBeVisible();
    await expect(windowPicker(canvasElement)).toHaveValue("Last 24 hours");
    await expect(hoursBack(spendQueries(unknownWindow)[0])).toBeCloseTo(24, 1);
  },
};

/**
 * A way to the other screen that drops the query string, the way the nav
 * rail's links do.
 */
function OpenScreen({ path }: { path: string }) {
  const navigate = useNavigate();
  return (
    <button type="button" onClick={() => navigate(path)}>
      Open {path}
    </button>
  );
}

const carriedWindow = byWindow();

/**
 * The business units and the customers of one chargeback are read over the
 * same month: the window picked on one screen is the one the other opens on,
 * even through a link that carries no query string, and it is written back
 * into the address there.
 */
export const TheWindowCarriesToTheOtherScreen: Story = {
  parameters: { address: "/business-units" },
  render: () => {
    carriedWindow.calls.length = 0;
    return (
      <Harness fetchStub={carriedWindow.stub}>
        <Routes>
          <Route path="/business-units" element={<BusinessUnits />} />
          <Route path="/customers" element={<Customers />} />
        </Routes>
        <OpenScreen path="/customers" />
        <AddressProbe />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("Platform Engineering")).toBeVisible());
    await pickOption(windowPicker(canvasElement), "Month to date");
    await waitFor(() => expect(canvas.getByText("Spend, month to date")).toBeVisible());

    await userEvent.click(canvas.getByRole("button", { name: "Open /customers" }));

    await waitFor(() => expect(canvas.getByText("Acme Corp")).toBeVisible());
    await waitFor(() => expect(canvas.getByText("Spend, month to date")).toBeVisible());
    await expect(windowPicker(canvasElement)).toHaveValue("Month to date");
    // the customers' first rollup is already month to date: the window is
    // known before the request, not corrected after a day's figures landed
    const customerReads = spendQueries(carriedWindow).filter(
      (q) => q.get("dimension") === "customer",
    );
    await expect(customerReads).toHaveLength(1);
    await expect(customerReads[0].get("since")).toBe(monthStart(0).toISOString());
    await waitFor(() => expect(addressOf(canvasElement).get("window")).toBe("mtd"));
  },
};
