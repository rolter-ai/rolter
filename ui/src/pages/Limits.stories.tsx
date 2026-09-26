import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, waitFor, within } from "storybook/test";

import Limits from "./Limits";
import {
  Harness,
  Toasted,
  clickWhenEnabled,
  expectAllowed,
  expectClosesWithoutPrompting,
  expectRefused,
  expectSheetClosed,
  expectSkeleton,
  expectToast,
  json,
  pickOption,
  pending,
  recording,
  routes,
  scoped,
  sheet,
  answerDiscardPrompt,
  type Recorder,
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
 * The budget is refused (#1607).
 *
 * The sheet closes on success, so a `CreatesABudget` that also passed when the
 * write failed would be indistinguishable — this pins the other branch: the
 * refusal is announced and the sheet stays with the amount that was typed.
 */
export const BudgetCreateRejectedByTheServer: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input, init) => {
        const url = String(input);
        if (init?.method === "POST") {
          return json({ error: { message: "this project already has a 30d budget" } }, 409);
        }
        if (url.includes("/virtual-keys")) return json(KEYS);
        if (url.includes("/budgets")) return json(BUDGETS);
        return json(RATE_LIMITS);
      })}
    >
      <Toasted>
        <Limits />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /add budget/i);
    const form = sheet();
    const limit = within(form).getByLabelText("Limit (USD)");
    await userEvent.clear(limit);
    await userEvent.type(limit, "250");
    await userEvent.click(within(form).getByRole("button", { name: "Create" }));

    await expectToast(canvasElement, /already has a 30d budget/, "error");
    await waitFor(() => expect(within(document.body).getByRole("dialog")).toBeInTheDocument());
    await expect(within(form).getByLabelText("Limit (USD)")).toHaveValue(250);
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
    // a combobox reads as the option's label; "" is the value behind "inherit"
    const picker = within(sheet()).getByLabelText("Unpriced traffic");
    await expect(picker).toHaveValue("Inherit deployment setting");
    // picking an override makes the form dirty, so closing it prompts rather
    // than dropping a choice that changes what the gateway will serve
    await pickOption(picker, "Refuse");
    await expect(picker).toHaveValue("Refuse");
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

    await userEvent.click(within(form).getByRole("button", { name: "Cancel" }));
    await answerDiscardPrompt(false);
    await expect(within(document.body).getByRole("dialog")).toBeInTheDocument();
    await expect(within(form).getByLabelText("Limit (USD)")).toHaveValue(999);
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

/**
 * The edits #1285 exists for. Each answers a PATCH with the row it would have
 * written and records what left, so a story can assert the *body*: an edit
 * that sent every field, or that went out as a delete and a create, would
 * pass any assertion about the card that comes back.
 */
function editable(): Recorder {
  return recording(
    scoped(async (input, init) => {
      const url = String(input);
      if (init?.method === "PATCH" && url.includes("/budgets/")) {
        return json({ ...BUDGETS[0], limit_usd: "750.0000" });
      }
      if (init?.method === "PATCH") return json({ ...RATE_LIMITS[0], rpm: null });
      if (url.includes("/virtual-keys")) return json(KEYS);
      if (url.includes("/budgets")) return json(BUDGETS);
      return json(RATE_LIMITS);
    }),
  );
}

let budgetEdit: Recorder;
export const EditsABudgetInPlace: Story = {
  render: () => {
    budgetEdit = editable();
    return (
      <Harness fetchStub={budgetEdit.stub}>
        <Toasted>
          <Limits />
        </Toasted>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /Edit the 30d budget/);
    const form = sheet();
    await expect(within(form).getByRole("heading", { name: "Edit budget" })).toBeVisible();
    // the form opens on the row as it stands, not on the create defaults
    await expect(within(form).getByLabelText("Limit (USD)")).toHaveValue(500);
    await expect(within(form).getByLabelText("Period")).toHaveValue("30d");
    // the workaround the sheet used to advertise is gone
    await expect(within(form).queryByText(/delete and recreate/i)).toBeNull();
    // nothing moved yet, so there is nothing to save
    await expect(within(form).getByRole("button", { name: "Save" })).toBeDisabled();

    const limit = within(form).getByLabelText("Limit (USD)");
    await userEvent.clear(limit);
    await userEvent.type(limit, "750");
    await userEvent.click(within(form).getByRole("button", { name: "Save" }));

    // only the field that moved goes on the wire, to the row's own id
    await expect(await budgetEdit.expectSentBody("PATCH", "/budgets/budget-1")).toEqual({
      limit_usd: "750",
    });
    await expectToast(canvasElement, /budget updated/i);
    await expectSheetClosed();
    budgetEdit.expectNotSent("DELETE", "/budgets/");
    budgetEdit.expectNotSent("POST", "/budgets");
  },
};

let overrideCleared: Recorder;
/**
 * Dropping an override is an edit of its own: the budget goes back to
 * inheriting the deployment setting, which the API spells `null`. An editor
 * that omitted the field instead would leave the override in force.
 */
export const ClearingAnUnpricedOverrideSendsNull: Story = {
  render: () => {
    overrideCleared = editable();
    return (
      <Harness fetchStub={overrideCleared.stub}>
        <Limits />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /Edit the 1d budget/);
    const picker = within(sheet()).getByLabelText("Unpriced traffic");
    await expect(picker).toHaveValue("Refuse");
    await pickOption(picker, "Inherit deployment setting");
    await userEvent.click(within(sheet()).getByRole("button", { name: "Save" }));

    await expect(await overrideCleared.expectSentBody("PATCH", "/budgets/budget-2")).toEqual({
      unpriced_policy: null,
    });
  },
};

export const AnUntouchedBudgetEditClosesWithoutPrompting: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <Limits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /Edit the 30d budget/);
    await expectClosesWithoutPrompting();
  },
};

let rateLimitEdit: Recorder;
/**
 * Blanking a cap lifts it. The field is blank for "uncapped" on both forms, so
 * on an edit a cap that was set and is now blank has to leave as `null`, while
 * the cap nobody touched stays out of the body.
 */
export const EditsARateLimitInPlace: Story = {
  render: () => {
    rateLimitEdit = editable();
    return (
      <Harness fetchStub={rateLimitEdit.stub}>
        <Limits />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /Edit the 600 rpm · 150000 tpm rate limit/);
    const form = sheet();
    await expect(within(form).getByRole("heading", { name: "Edit rate limit" })).toBeVisible();
    const rpm = within(form).getByLabelText("Requests per minute (optional)");
    await expect(rpm).toHaveValue(600);
    await userEvent.clear(rpm);
    await userEvent.click(within(form).getByRole("button", { name: "Save" }));

    await expect(await rateLimitEdit.expectSentBody("PATCH", "/rate-limits/rl-1")).toEqual({
      rpm: null,
    });
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

// Budgets and rate limits are separate resources with separate capabilities,
// and both are admin (#1606). Each list carries its own create control and its
// own hand-gated delete, so this screen has four gates that can drift apart.
export const RefusedToAViewer: Story = {
  render: () => (
    <Harness fetchStub={loaded} role="viewer">
      <Limits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, "Add budget");
    await expectRefused(canvasElement, "Add rate limit");
    await expectRefused(canvasElement, /Delete the 30d budget/);
    await expectRefused(canvasElement, /Delete the 600 rpm · 150000 tpm rate limit/);
    // editing in place is `update`, a capability of its own (#1285)
    await expectRefused(canvasElement, /Edit the 30d budget/);
    await expectRefused(canvasElement, /Edit the 600 rpm · 150000 tpm rate limit/);
  },
};

export const EditableByAnAdmin: Story = {
  render: () => (
    <Harness fetchStub={loaded} role="admin">
      <Limits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectAllowed(canvasElement, /Edit the 30d budget/);
    await expectAllowed(canvasElement, /Edit the 600 rpm · 150000 tpm rate limit/);
  },
};

export const RefusedToAMember: Story = {
  render: () => (
    <Harness fetchStub={loaded} role="member">
      <Limits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, "Add budget");
    await expectRefused(canvasElement, "Add rate limit");
    await expectRefused(canvasElement, /Edit the 30d budget/);
  },
};
