import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, waitFor, within } from "storybook/test";
import { useTranslation } from "react-i18next";

import { AlertChannels } from "./Alerting";
import Keys from "./Keys";
import Pricing from "./Pricing";
import Providers from "./Providers";
import Security from "./Security";
import {
  Harness,
  expectForbidden,
  json,
  routes,
  scoped,
  type StoryRole,
} from "./story-harness";
import { NavSidebar, type NavItem } from "@/components/ui/nav-sidebar";
import type {
  AlertChannelRow,
  ModelPriceRow,
  ProviderRow,
  SecuritySettingsDto,
  VirtualKeyRow,
} from "@/lib/api";
import { useCan } from "@/lib/can";
import { visibleNav, type NavDef } from "@/lib/nav";

// What each role sees of the same three screens (#1183).
//
// The dashboard used to render every control for every account and let the 403
// explain afterwards. These stories are the four answers the control plane
// gives — a viewer, a member, an admin and a superadmin — against the screens
// where the difference is most expensive: the keys anyone can try to mint, the
// providers anyone can try to add, and the deployment-wide security policy that
// is the admin token's alone.

const SECURITY: SecuritySettingsDto = {
  virtual_key_required: true,
  allowed_origins: ["https://app.example.com"],
  allowed_headers: ["x-request-id"],
  required_headers: {},
  auth_bypass_routes: ["/healthz"],
  dashboard_auth_enabled: true,
  dashboard_credential_ref: "ROLTER_DASHBOARD_SECRET",
  dashboard_secret_configured: true,
  updated_at: "2026-08-01T09:00:00Z",
};

// the screens under test only need to get *somewhere* renderable: what these
// stories assert is the control, not the list behind it
const empty = routes([]);
const securityStub = scoped(async () => json(SECURITY));

const ADD_KEY = /add virtual key/i;
const ADD_PROVIDER = /add provider/i;

/** Assert the create control is refused, and says what it would take. */
async function expectGatedOut(canvasElement: HTMLElement, name: RegExp) {
  const canvas = within(canvasElement);
  const button = await canvas.findByRole("button", { name });
  await waitFor(() => expect(button).toBeDisabled());
  // "disabled" alone is the same non-answer the 403 was: the control has to
  // name the role that would make it work
  await expect(button).toHaveAttribute("title", "Requires the Admin role");
}

/** Assert the create control is offered. */
async function expectOffered(canvasElement: HTMLElement, name: RegExp) {
  const canvas = within(canvasElement);
  const button = await canvas.findByRole("button", { name });
  await waitFor(() => expect(button).toBeEnabled());
}

// One row's worth of each list, so the stories below have a row to gate. The
// controls are what is under test, not the data, so these are the smallest
// fixtures that render one.
const KEY: VirtualKeyRow = {
  id: "vk-1",
  project_id: "project-1",
  key_hash: "hash-1",
  key_prefix: "sk-rolter-backend",
  name: "backend service",
  models: [],
  providers: [],
  created_by: null,
  business_unit_id: null,
  customer_id: null,
  disabled: false,
  expires_at: null,
  cache_enabled: null,
  created_at: "2026-07-01T00:00:00Z",
};

const PROVIDER: ProviderRow = {
  id: "p-1",
  org_id: "org-1",
  name: "openai-prod",
  slug: "openai-prod",
  kind: "openai",
  api_base: "https://api.openai.com/v1",
  api_key_env: "OPENAI_API_KEY",
  egress_proxies: [],
  created_at: "2026-01-02T00:00:00Z",
};

const CHANNEL: AlertChannelRow = {
  id: "chan-1",
  name: "ops-slack",
  kind: "webhook",
  endpoint: "https://hooks.slack.com/services/T000/B000/xxx",
  enabled: true,
  secret_configured: true,
  created_at: "2026-05-01T00:00:00Z",
  updated_at: "2026-05-01T00:00:00Z",
};

const PRICE: ModelPriceRow = {
  id: "price-1",
  model: "gpt-4o",
  input_per_mtok: "2.50",
  output_per_mtok: "10.00",
  cached_input_per_mtok: null,
  currency: "USD",
  created_at: "2026-07-01T00:00:00Z",
};

const oneKey = routes([["/virtual-keys", () => [KEY]]]);
const oneProvider = routes([["/providers", () => [PROVIDER]]]);
const oneChannel = routes([["/alert-channels", () => [CHANNEL]]]);
const onePrice = routes([
  ["/api/v1/currency", () => ({ base: "USD", codes: ["USD"], rates: { USD: 1 } })],
  ["/model-prices", () => [PRICE]],
]);

const NEEDS_ADMIN = "Requires the Admin role";
const NEEDS_SUPERADMIN = "Requires a superadmin";

/**
 * Assert a per-row control is refused, and says what it would take (#1258).
 *
 * Looked up by its accessible name rather than by position, which is what
 * naming the row in every label bought (#1214): "the edit button on the
 * backend service key", not "the third button on the screen".
 */
async function expectRowRefused(
  canvasElement: HTMLElement,
  role: "button" | "switch" | "combobox",
  name: string,
  requirement: string,
) {
  const canvas = within(canvasElement);
  const control = await canvas.findByRole(role, { name });
  await waitFor(() => expect(control).toBeDisabled());
  await expect(control).toHaveAttribute("title", requirement);
}

/** Assert a per-row control is offered. */
async function expectRowOffered(
  canvasElement: HTMLElement,
  role: "button" | "switch" | "combobox",
  name: string,
) {
  const canvas = within(canvasElement);
  const control = await canvas.findByRole(role, { name });
  await waitFor(() => expect(control).toBeEnabled());
}

const meta = {
  title: "Screens/Capability gating",
  parameters: { layout: "fullscreen" },
} satisfies Meta;

export default meta;
type Story = StoryObj<typeof meta>;

export const KeysAsViewer: Story = {
  render: () => (
    <Harness fetchStub={empty} role="viewer">
      <Keys />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectGatedOut(canvasElement, ADD_KEY);
  },
};

// a member may mint a key for themself on the Account screen, but minting one
// for a project is still an admin's — the two are different capabilities, and
// the button on this screen is the second one
export const KeysAsMember: Story = {
  render: () => (
    <Harness fetchStub={empty} role="member">
      <Keys />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectGatedOut(canvasElement, ADD_KEY);
  },
};

export const KeysAsAdmin: Story = {
  render: () => (
    <Harness fetchStub={empty} role="admin">
      <Keys />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectOffered(canvasElement, ADD_KEY);
  },
};

export const KeysAsSuperadmin: Story = {
  render: () => (
    <Harness fetchStub={empty} role="superadmin">
      <Keys />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectOffered(canvasElement, ADD_KEY);
  },
};

export const ProvidersAsViewer: Story = {
  render: () => (
    <Harness fetchStub={empty} role="viewer">
      <Providers />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectGatedOut(canvasElement, ADD_PROVIDER);
  },
};

export const ProvidersAsMember: Story = {
  render: () => (
    <Harness fetchStub={empty} role="member">
      <Providers />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectGatedOut(canvasElement, ADD_PROVIDER);
  },
};

export const ProvidersAsAdmin: Story = {
  render: () => (
    <Harness fetchStub={empty} role="admin">
      <Providers />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectOffered(canvasElement, ADD_PROVIDER);
  },
};

// the deployment-wide policy screens are the superadmin's alone: an admin is
// refused up front, before the request that would have been refused anyway
export const SecurityAsAdmin: Story = {
  render: () => (
    <Harness fetchStub={securityStub} role="admin">
      <Security />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectForbidden(canvasElement);
    // and it is not a failure a retry can change
    await expect(
      within(canvasElement).queryByRole("button", { name: /try again/i }),
    ).toBeNull();
  },
};

export const SecurityAsViewer: Story = {
  render: () => (
    <Harness fetchStub={securityStub} role="viewer">
      <Security />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectForbidden(canvasElement);
  },
};

export const SecurityAsSuperadmin: Story = {
  render: () => (
    <Harness fetchStub={securityStub} role="superadmin">
      <Security />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() =>
      expect(canvas.getByText("Password protect the dashboard")).toBeVisible(),
    );
  },
};

// the rail, built exactly as the shell builds it: `visibleNav` over the same
// `useCan` the screens ask
function GatedNav() {
  const { t } = useTranslation();
  const can = useCan();
  const toItem = (def: NavDef): NavItem => ({
    key: def.key,
    label: t(`nav.${def.key}`),
    icon: def.icon,
    children: def.children?.map(toItem),
  });
  return (
    <div className="flex h-[600px]">
      <NavSidebar
        brand="rolter"
        groups={[{ items: visibleNav(can).map(toItem) }]}
        activeKey="dashboard"
        searchable
      />
    </div>
  );
}

export const NavAsViewer: Story = {
  render: () => (
    <Harness fetchStub={empty} role="viewer">
      <GatedNav />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // a group whose every leaf is deployment-scoped is gone, not disabled: a
    // rail full of entries that all open the same refusal is a worse map of
    // the product than a shorter rail
    await waitFor(() => expect(canvas.queryByText("Alerting")).toBeNull());
    await expect(canvas.queryByText("Guardrails")).toBeNull();
    await expect(canvas.queryByText("Cluster Config")).toBeNull();
    // what a viewer may read is still there
    await expect(canvas.getByText("Governance")).toBeVisible();
    await expect(canvas.getByText("Playground")).toBeVisible();
  },
};

export const NavAsSuperadmin: Story = {
  render: () => (
    <Harness fetchStub={empty} role="superadmin">
      <GatedNav />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("Alerting")).toBeVisible());
    await expect(canvas.getByText("Guardrails")).toBeVisible();
    await expect(canvas.getByText("Cluster Config")).toBeVisible();
  },
};

// no provider above and no answer yet are the same case, and both render the
// dashboard as it always was: enabled, with the 403 as the backstop
export const NavUngated: Story = {
  render: () => (
    <Harness fetchStub={empty}>
      <GatedNav />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByText("Alerting")).toBeVisible();
  },
};

const ROLES: StoryRole[] = ["viewer", "member", "admin", "superadmin"];

// one frame with all four answers side by side, for the visual review the play
// functions cannot do
export const EveryRole: Story = {
  render: () => (
    <div className="flex flex-col gap-6">
      {ROLES.map((role) => (
        <Harness key={role} fetchStub={empty} role={role}>
          <Providers />
        </Harness>
      ))}
    </div>
  ),
};

// The row controls, which #1183 deliberately left out and #1258 finished.
//
// A viewer used to get a live Edit, Delete and Enabled toggle on every row and
// learn it was refused only from the 403 that came back after the click — the
// exact failure the create buttons above no longer have.

export const KeyRowsAsViewer: Story = {
  render: () => (
    <Harness fetchStub={oneKey} role="viewer">
      <Keys />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRowRefused(canvasElement, "button", "Edit key backend service", NEEDS_ADMIN);
    await expectRowRefused(canvasElement, "button", "Delete key backend service", NEEDS_ADMIN);
    // the toggle and the cache select are updates too, and a switch that flips
    // and snaps back is a worse answer than one that never moved
    await expectRowRefused(canvasElement, "switch", "Enable backend service", NEEDS_ADMIN);
    await expectRowRefused(
      canvasElement,
      "combobox",
      "Response cache policy for backend service",
      NEEDS_ADMIN,
    );
  },
};

// a member may mint a key for themself, and still may not touch a project's
export const KeyRowsAsMember: Story = {
  render: () => (
    <Harness fetchStub={oneKey} role="member">
      <Keys />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRowRefused(canvasElement, "button", "Edit key backend service", NEEDS_ADMIN);
    await expectRowRefused(canvasElement, "button", "Delete key backend service", NEEDS_ADMIN);
    await expectRowRefused(canvasElement, "switch", "Enable backend service", NEEDS_ADMIN);
  },
};

export const KeyRowsAsAdmin: Story = {
  render: () => (
    <Harness fetchStub={oneKey} role="admin">
      <Keys />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRowOffered(canvasElement, "button", "Edit key backend service");
    await expectRowOffered(canvasElement, "button", "Delete key backend service");
    await expectRowOffered(canvasElement, "switch", "Enable backend service");
  },
};

export const ProviderRowsAsViewer: Story = {
  render: () => (
    <Harness fetchStub={oneProvider} role="viewer">
      <Providers />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRowRefused(canvasElement, "button", "Edit provider openai-prod", NEEDS_ADMIN);
    await expectRowRefused(canvasElement, "button", "Delete provider openai-prod", NEEDS_ADMIN);
  },
};

export const ProviderRowsAsMember: Story = {
  render: () => (
    <Harness fetchStub={oneProvider} role="member">
      <Providers />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRowRefused(canvasElement, "button", "Edit provider openai-prod", NEEDS_ADMIN);
    await expectRowRefused(canvasElement, "button", "Delete provider openai-prod", NEEDS_ADMIN);
  },
};

export const ProviderRowsAsAdmin: Story = {
  render: () => (
    <Harness fetchStub={oneProvider} role="admin">
      <Providers />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRowOffered(canvasElement, "button", "Edit provider openai-prod");
    await expectRowOffered(canvasElement, "button", "Delete provider openai-prod");
  },
};

// alerting has no tenancy scope to be an admin of, so its rows are the
// superadmin's — an admin never reaches them at all, because the screen refuses
// itself before it mounts
export const AlertingRowsAsAdmin: Story = {
  render: () => (
    <Harness fetchStub={oneChannel} role="admin">
      <AlertChannels />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectForbidden(canvasElement);
    await expect(
      within(canvasElement).queryByRole("button", { name: "Delete channel ops-slack" }),
    ).toBeNull();
  },
};

export const AlertingRowsAsSuperadmin: Story = {
  render: () => (
    <Harness fetchStub={oneChannel} role="superadmin">
      <AlertChannels />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRowOffered(canvasElement, "button", "Delete channel ops-slack");
    await expectRowOffered(canvasElement, "switch", "Enable ops-slack");
  },
};

// pricing is the case where both halves are visible at once: the rows load
// for an admin — a price is a deployment-wide read — and every control on them
// names the authority it would take, which is not a role anyone can be granted
export const PriceRowsAsAdmin: Story = {
  render: () => (
    <Harness fetchStub={onePrice} role="admin">
      <Pricing />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRowRefused(
      canvasElement,
      "button",
      "Edit the price for gpt-4o",
      NEEDS_SUPERADMIN,
    );
    await expectRowRefused(
      canvasElement,
      "button",
      "Delete the price for gpt-4o",
      NEEDS_SUPERADMIN,
    );
  },
};

export const PriceRowsAsSuperadmin: Story = {
  render: () => (
    <Harness fetchStub={onePrice} role="superadmin">
      <Pricing />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRowOffered(canvasElement, "button", "Edit the price for gpt-4o");
    await expectRowOffered(canvasElement, "button", "Delete the price for gpt-4o");
  },
};
