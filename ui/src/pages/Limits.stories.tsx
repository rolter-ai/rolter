import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, waitFor, within } from "storybook/test";

import Limits from "./Limits";
import {
  Harness,
  clickWhenEnabled,
  expectClosesWithoutPrompting,
  expectSheetClosed,
  expectSkeleton,
  json,
  pending,
  routes,
  scoped,
  sheet,
  withConfirm,
} from "./story-harness";
import type { BudgetRow, RateLimitRow, VirtualKeyRow } from "@/lib/api";
import { atMobile, expectNoHorizontalOverflow } from "@/lib/story-viewport";

const BUDGETS: BudgetRow[] = [
  {
    id: "budget-1",
    scope_type: "project",
    scope_id: "project-1",
    limit_usd: "500.00",
    period: "30d",
    // inherits the deployment-wide unpriced policy, which is the shape of
    // every budget created before #996
    unpriced_policy: null,
    created_at: "2026-07-01T00:00:00Z",
  },
  // this one refuses traffic it cannot account for, whatever the deployment
  // setting says — an override can tighten, never loosen
  {
    id: "budget-2",
    scope_type: "project",
    scope_id: "project-1",
    limit_usd: "50.00",
    period: "1d",
    unpriced_policy: "block",
    created_at: "2026-07-02T00:00:00Z",
  },
];

const RATE_LIMITS: RateLimitRow[] = [
  {
    id: "rl-1",
    scope_type: "project",
    scope_id: "project-1",
    rpm: 600,
    tpm: 150000,
    created_at: "2026-07-01T00:00:00Z",
  },
  {
    id: "rl-2",
    scope_type: "project",
    scope_id: "project-1",
    rpm: null,
    tpm: 20000,
    created_at: "2026-07-02T00:00:00Z",
  },
];

const KEYS: VirtualKeyRow[] = [
  {
    id: "vk-1",
    project_id: "project-1",
    key_hash: "hash",
    key_prefix: "sk-rolter-backend",
    name: "backend service",
    models: [],
    providers: [],
    created_by: null,
    business_unit_id: null,
    customer_id: null,
    disabled: false,
    created_at: "2026-07-01T00:00:00Z",
  },
];

// the screen resolves virtual keys for its scope picker before either list, so
// the longer fragment has to be matched ahead of the shared prefix
const loaded = routes([
  ["/virtual-keys", () => KEYS],
  ["/budgets", () => BUDGETS],
  ["/rate-limits", () => RATE_LIMITS],
]);
const empty = routes([
  ["/virtual-keys", () => KEYS],
  ["/budgets", () => []],
  ["/rate-limits", () => []],
]);

const meta = {
  title: "Screens/Limits",
  component: Limits,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof Limits>;
export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <Limits />
    </Harness>
  ),
};

export const Loading: Story = {
  render: () => (
    <Harness fetchStub={pending}>
      <Limits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
  },
};

export const Empty: Story = {
  render: () => (
    <Harness fetchStub={empty}>
      <Limits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(/no budgets for this scope/i)).toBeInTheDocument();
    await expect(canvas.getByText(/no rate limits for this scope/i)).toBeInTheDocument();
  },
};

export const Forbidden: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input) =>
        String(input).includes("/virtual-keys")
          ? json(KEYS)
          : json({ error: { message: "forbidden" } }, 403),
      )}
    >
      <Limits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // both panels report the permission problem, not a load failure (#962)
    await expect(await canvas.findByText(/do not have access to budgets/i)).toBeInTheDocument();
    await expect(canvas.getByText(/do not have access to rate limits/i)).toBeInTheDocument();
  },
};

export const CreatesABudget: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input, init) => {
        const url = String(input);
        if (init?.method === "POST") return json(BUDGETS[0], 201);
        if (url.includes("/virtual-keys")) return json(KEYS);
        if (url.includes("/budgets")) return json(BUDGETS);
        return json(RATE_LIMITS);
      })}
    >
      <Limits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /add budget/i);
    const form = sheet();
    const limit = within(form).getByLabelText("Limit (USD)");
    await userEvent.clear(limit);
    await userEvent.type(limit, "250");
    await userEvent.click(within(form).getByRole("button", { name: "Create" }));
    await expectSheetClosed();
  },
};

/**
 * The seeded-defaults case #868 introduced and #879 called out by name.
 *
 * `Add budget` opens pre-filled with `100` / `30d` rather than blank, so its
 * dirty flag is "differs from the seed", not "is non-empty". Getting that
 * backwards makes an untouched form prompt on every close — and a typecheck
 * cannot tell the two apart.
 */
export const AnUntouchedSeededBudgetFormClosesWithoutPrompting: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <Limits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /add budget/i);
    // the seed itself, which is what makes this case interesting
    await expect(within(sheet()).getByLabelText("Limit (USD)")).toHaveValue(100);
    await expect(within(sheet()).getByLabelText("Period")).toHaveValue("30d");
    await expectClosesWithoutPrompting();
  },
};

/**
 * #996: a budget may carry its own answer to unpriced traffic. The card says so
 * only when there is an override — a budget that inherits the deployment-wide
 * setting has nothing of its own to report — and the form defaults to inherit,
 * so opening it and closing it cannot silently narrow what the gateway serves.
 */
export const ABudgetShowsItsUnpricedOverride: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <Limits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("unpriced: Refuse")).toBeVisible());
    // exactly one of the two budgets carries an override
    await expect(canvas.getAllByText(/^unpriced: /)).toHaveLength(1);
  },
};

export const TheUnpricedOverrideDefaultsToInherit: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <Limits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /add budget/i);
    const select = within(sheet()).getByLabelText("Unpriced traffic");
    await expect(select).toHaveValue("");
    // picking an override makes the form dirty, so closing it prompts rather
    // than dropping a choice that changes what the gateway will serve
    await userEvent.selectOptions(select, "block");
    await expect(select).toHaveValue("block");
  },
};

export const AnEditedBudgetFormPromptsBeforeDiscarding: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <Limits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /add budget/i);
    const form = sheet();
    const limit = within(form).getByLabelText("Limit (USD)");
    await userEvent.clear(limit);
    await userEvent.type(limit, "999");

    await withConfirm(false, async () => {
      await userEvent.click(within(form).getByRole("button", { name: "Cancel" }));
      await expect(within(document.body).getByRole("dialog")).toBeInTheDocument();
      await expect(within(form).getByLabelText("Limit (USD)")).toHaveValue(999);
    });
  },
};

export const CreatesARateLimit: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input, init) => {
        const url = String(input);
        if (init?.method === "POST") return json(RATE_LIMITS[0], 201);
        if (url.includes("/virtual-keys")) return json(KEYS);
        if (url.includes("/budgets")) return json(BUDGETS);
        return json(RATE_LIMITS);
      })}
    >
      <Limits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /add rate limit/i);
    const form = sheet();
    await userEvent.type(within(form).getByLabelText("Requests per minute (optional)"), "300");
    await userEvent.click(within(form).getByRole("button", { name: "Create" }));
    await expectSheetClosed();
  },
};

// the toolbars and budget headers wrap instead of pushing the page sideways (#1242)
export const Mobile: Story = {
  ...atMobile,
  render: () => (
    <Harness fetchStub={loaded}>
      <Limits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByRole("button", { name: /add budget/i });
    await expectNoHorizontalOverflow();
  },
};
