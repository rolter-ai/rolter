import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { BusinessUnits, Customers } from "./CostAttribution";
import {
  cancelConfirmation,
  confirmDestructive,
  expectSkeleton,
  recording,
} from "./story-harness";
import type {
  AttributionSpendRow,
  BusinessUnitRow,
  CustomerRow,
} from "@/lib/api";
import { formattersFor } from "@/lib/i18n/format";

// the formatter the screen itself uses, so a story asserts the house format
// rather than a second copy of it
const fmt = formattersFor("en");

const spendRow = (
  id: string,
  cost: number,
  requests: number,
): AttributionSpendRow => ({
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

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });

// route by path so useScope's /api/v1/orgs call is served alongside the
// screen's own collection
function router(
  handlers: Partial<
    Record<"units" | "customers" | "spend", (init?: RequestInit) => Response>
  >,
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

// the stub is installed during render, not in an effect: child effects run
// before the parent's, so an effect would let the first real fetch through
function Harness({
  fetchStub,
  children,
}: {
  fetchStub: FetchStub;
  children: React.ReactNode;
}) {
  const original = React.useRef<typeof globalThis.fetch | null>(null);
  const client = React.useMemo(() => {
    original.current ??= globalThis.fetch;
    globalThis.fetch = fetchStub as typeof globalThis.fetch;
    return new QueryClient({ defaultOptions: { queries: { retry: false } } });
  }, [fetchStub]);
  React.useEffect(
    () => () => {
      if (original.current) globalThis.fetch = original.current;
    },
    [],
  );
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}

const meta = {
  title: "Screens/CostAttribution",
  component: BusinessUnits,
  parameters: { layout: "fullscreen" },
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
    await waitFor(() =>
      expect(canvas.getByText("No business units yet")).toBeVisible(),
    );
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
      expect(
        canvas.getByText(/You do not have access to business units/),
      ).toBeVisible(),
    );
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
    await userEvent.click(
      canvas.getByRole("button", { name: "Edit Platform Engineering" }),
    );
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
const unitDeletes = recording(async (input, init) => {
  if (init?.method === "DELETE") return json({}, 204);
  return router({})(input, init);
});

export const ConfirmsBeforeDeletingABusinessUnit: Story = {
  render: () => (
    <Harness fetchStub={unitDeletes.stub}>
      <BusinessUnits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("Platform Engineering")).toBeVisible());
    const del = () =>
      canvas.getByRole("button", { name: "Delete Platform Engineering" });

    await userEvent.click(del());
    await cancelConfirmation();
    unitDeletes.expectNotSent("DELETE", `/business-units/${UNITS[0].id}`);

    await userEvent.click(del());
    // the copy points at the reversible alternative rather than only warning
    await confirmDestructive(/Platform Engineering/, "Delete");
    await unitDeletes.expectSent("DELETE", `/business-units/${UNITS[0].id}`);
  },
};

const customerDeletes = recording(async (input, init) => {
  if (init?.method === "DELETE") return json({}, 204);
  return router({})(input, init);
});

export const ConfirmsBeforeDeletingACustomer: Story = {
  render: () => (
    <Harness fetchStub={customerDeletes.stub}>
      <Customers />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("Acme Corp")).toBeVisible());
    await userEvent.click(canvas.getByRole("button", { name: "Delete Acme Corp" }));
    await confirmDestructive(/Acme Corp/, "Delete");
    await customerDeletes.expectSent("DELETE", `/customers/${CUSTOMERS[0].id}`);
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
    await waitFor(() =>
      expect(canvas.getByText(fmt.currency(190, "USD"))).toBeVisible(),
    );
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
    await waitFor(() =>
      expect(canvas.getByText(fmt.currency(190, "EUR"))).toBeVisible(),
    );
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
    await waitFor(() =>
      expect(canvas.getByText(fmt.currency(175, "USD"))).toBeVisible(),
    );
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
    await expectSkeleton(canvasElement);
  },
};

/**
 * Analytics is optional; governance is not. A deployment with no ClickHouse
 * gets the Dashboard's calm note where the spend strip would be, and keeps the
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
    await waitFor(() =>
      expect(canvas.getByText("Analytics not configured")).toBeVisible(),
    );
    await expect(canvas.getByText("Platform Engineering")).toBeVisible();
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
  },
};
