import type { Meta, StoryObj } from "@storybook/react-vite";
import * as React from "react";
import { expect, userEvent, within } from "storybook/test";

import {
  AttributionBadges,
  KeyAttributionFields,
  KeyProvidersField,
  UNATTRIBUTED,
} from "./KeyAttributionFields";
import type { BusinessUnitRow, CustomerRow, ProviderRow } from "@/lib/api";
import { openOptions, pickOption } from "@/pages/story-harness";

const unit = (id: string, name: string, retired = false): BusinessUnitRow => ({
  id,
  org_id: "org-1",
  name,
  slug: name.toLowerCase(),
  retired_at: retired ? "2026-05-01T00:00:00Z" : null,
  created_at: "2026-01-01T00:00:00Z",
});

const customer = (
  id: string,
  name: string,
  business_unit_id: string | null,
  retired = false,
): CustomerRow => ({
  id,
  org_id: "org-1",
  business_unit_id,
  name,
  slug: name.toLowerCase(),
  retired_at: retired ? "2026-05-01T00:00:00Z" : null,
  created_at: "2026-01-01T00:00:00Z",
});

const UNITS: BusinessUnitRow[] = [
  unit("bu-1", "Platform"),
  unit("bu-2", "Research"),
  unit("bu-3", "Archived", true),
];

const CUSTOMERS: CustomerRow[] = [
  customer("cu-1", "Acme", "bu-1"),
  customer("cu-2", "Globex", "bu-2"),
  customer("cu-3", "Unowned", null),
  customer("cu-4", "Lapsed", "bu-1", true),
];

const PROVIDERS: ProviderRow[] = [
  {
    id: "p-1",
    org_id: "org-1",
    name: "openai-prod",
    slug: "openai-prod",
    kind: "openai",
    api_base: "https://api.openai.com",
    egress_proxies: [],
    created_at: "2026-01-01T00:00:00Z",
  },
  {
    id: "p-2",
    org_id: "org-1",
    name: "vllm-cluster",
    slug: "vllm-cluster",
    kind: "openai_compatible",
    api_base: "http://vllm.internal:8000",
    egress_proxies: [],
    created_at: "2026-01-01T00:00:00Z",
  },
];

function Attribution({
  unitId = UNATTRIBUTED,
  customerId = UNATTRIBUTED,
}: {
  unitId?: string;
  customerId?: string;
}) {
  const [pair, setPair] = React.useState({ unitId, customerId });
  return (
    <div className="max-w-md space-y-4">
      <KeyAttributionFields
        units={UNITS}
        customers={CUSTOMERS}
        businessUnitId={pair.unitId}
        customerId={pair.customerId}
        onChange={(nextUnit, nextCustomer) =>
          setPair({ unitId: nextUnit, customerId: nextCustomer })
        }
      />
    </div>
  );
}

const meta = {
  title: "Components/KeyAttributionFields",
  component: KeyAttributionFields,
  parameters: { layout: "padded" },
  args: {
    units: UNITS,
    customers: CUSTOMERS,
    businessUnitId: UNATTRIBUTED,
    customerId: UNATTRIBUTED,
    onChange: () => {},
  },
} satisfies Meta<typeof KeyAttributionFields>;

export default meta;
type Story = StoryObj<typeof meta>;

/** Attribution is optional: unattributed is a legitimate resting state. */
export const Unattributed: Story = {
  render: () => <Attribution />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // a combobox reads as the option's label; UNATTRIBUTED is the value behind it
    await expect(canvas.getByLabelText("Business unit")).toHaveValue("Unattributed");
    await expect(canvas.getByLabelText("Customer")).toHaveValue("Unattributed");
  },
};

export const Attributed: Story = {
  render: () => <Attribution unitId="bu-1" customerId="cu-1" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByLabelText("Customer")).toHaveValue("Acme");
  },
};

/**
 * The control plane rejects a customer owned by a different business unit, so
 * a customer that cannot be paired with the chosen unit is not offered at all
 * — learning that rule from a 400 after saving is learning it too late.
 */
export const OnlyCustomersThatFitTheUnit: Story = {
  render: () => <Attribution unitId="bu-1" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const customers = within(await openOptions(canvas.getByLabelText("Customer")));
    await expect(customers.getByRole("option", { name: "Acme" })).toBeInTheDocument();
    // owned by Research, and an unowned customer any unit may claim
    await expect(customers.queryByRole("option", { name: "Globex" })).not.toBeInTheDocument();
    await expect(customers.getByRole("option", { name: "Unowned" })).toBeInTheDocument();
  },
};

/**
 * Moving the unit can strand the customer under it. The pairing rule lives in
 * the component rather than in each caller, so both editors drop the stranded
 * pick the same way instead of one of them saving an invalid pair.
 */
export const MovingTheUnitDropsAStrandedCustomer: Story = {
  render: () => <Attribution unitId="bu-1" customerId="cu-1" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await pickOption(canvas.getByLabelText("Business unit"), "Research");
    await expect(canvas.getByLabelText("Customer")).toHaveValue("Unattributed");
  },
};

/**
 * Retired entities keep their history and stop taking new attribution — except
 * where one *is* the selection being edited, which must stay visible or the
 * form would silently re-point an existing key.
 */
export const RetiredAreNotOffered: Story = {
  render: () => <Attribution />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const units = within(await openOptions(canvas.getByLabelText("Business unit")));
    await expect(units.queryByRole("option", { name: "Archived" })).not.toBeInTheDocument();
    await userEvent.keyboard("{Escape}");
    const customers = within(await openOptions(canvas.getByLabelText("Customer")));
    await expect(customers.queryByRole("option", { name: "Lapsed" })).not.toBeInTheDocument();
  },
};

export const RetiredStaysWhenItIsTheSelection: Story = {
  render: () => <Attribution unitId="bu-3" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const units = canvas.getByLabelText("Business unit");
    await expect(units).toHaveValue("Archived");
    await expect(
      within(await openOptions(units)).getByRole("option", { name: "Archived" }),
    ).toBeInTheDocument();
  },
};

/**
 * An empty provider selection is the permissive default the server documents,
 * so the hint says it out loud: ticking nothing has not locked the key out of
 * everything, and the opposite reading is the expensive one.
 */
export const ProvidersEmptyMeansAll: Story = {
  render: () => {
    const [selected, setSelected] = React.useState<string[]>([]);
    return (
      <div className="max-w-md">
        <KeyProvidersField providers={PROVIDERS} selected={selected} onChange={setSelected} />
      </div>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByText(/may reach every provider/)).toBeVisible();
    await userEvent.click(canvas.getByRole("checkbox", { name: "openai-prod" }));
    await expect(canvas.getByText("1 provider selected")).toBeVisible();
  },
};

/** No providers configured at all: the field renders nothing rather than an empty box. */
export const ProvidersFieldHidesWhenThereAreNone: Story = {
  render: () => <KeyProvidersField providers={[]} selected={[]} onChange={() => {}} />,
  play: async ({ canvasElement }) => {
    await expect(canvasElement.textContent?.trim()).toBe("");
  },
};

/**
 * On a list row, an unattributed key renders a dash rather than an
 * "unattributed" chip: badging the absence on every row would read as a
 * warning about a state most deployments are legitimately in.
 */
export const BadgesOnARow: Story = {
  render: () => (
    <div className="space-y-2 text-sm">
      <AttributionBadges unit="Platform" customer="Acme" />
      <AttributionBadges unit="Platform" />
      <AttributionBadges />
    </div>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getAllByText("Platform")).toHaveLength(2);
    await expect(canvas.getByText("Acme")).toBeVisible();
    await expect(canvas.getByTitle("Unattributed")).toBeVisible();
  },
};
