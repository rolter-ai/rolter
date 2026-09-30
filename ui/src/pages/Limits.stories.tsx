import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, waitFor, within } from "storybook/test";

import Limits from "./Limits";
import {
  Harness,
  Toasted,
  cancelConfirmation,
  clickWhenEnabled,
  confirmDestructive,
  confirmation,
  expectAllowed,
  expectClosesWithoutPrompting,
  expectLoadError,
  expectNoFalseEmpty,
  expectNoUxEvent,
  expectRefused,
  expectSheetClosed,
  expectSkeleton,
  expectToast,
  expectUxEvent,
  json,
  pickOption,
  pending,
  recordUxEvents,
  recording,
  routes,
  scoped,
  sheet,
  answerDiscardPrompt,
  type Recorder,
} from "./story-harness";
import type { BudgetRow, RateLimitRow, VirtualKeyRow } from "@/lib/api";
import en from "@/lib/i18n/locales/en.json";
import ru from "@/lib/i18n/locales/ru.json";
import { atMobile, expectNoHorizontalOverflow } from "@/lib/story-viewport";
import { UxScreenProvider } from "@/lib/ux-react";

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
    await clickWhenEnabled(canvasElement, /Edit the monthly budget/);
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
    await clickWhenEnabled(canvasElement, /Edit the daily budget/);
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
    await clickWhenEnabled(canvasElement, /Edit the monthly budget/);
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
    await clickWhenEnabled(canvasElement, /Edit the 600 rpm · 150,000 tpm rate limit/);
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

let refusedEdit: Recorder;
/**
 * The control plane refuses an edit. An edit meets refusals a create never did,
 * a cap below 1 among them, and every other edit story answers the PATCH with a
 * 200. So this one pins the other path: the sheet stays open on the draft with
 * the server's own reason, the failure is announced, and nothing claims the
 * limit was updated.
 */
export const ARefusedEditKeepsTheSheetOpen: Story = {
  render: () => {
    refusedEdit = recording(
      scoped(async (input, init) => {
        const url = String(input);
        if (init?.method === "PATCH") {
          return json(
            { error: { message: "config error: tpm must be at least 1, or null to lift the cap" } },
            400,
          );
        }
        if (url.includes("/virtual-keys")) return json(KEYS);
        if (url.includes("/budgets")) return json(BUDGETS);
        return json(RATE_LIMITS);
      }),
    );
    return (
      <Harness fetchStub={refusedEdit.stub}>
        <Toasted>
          <Limits />
        </Toasted>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /Edit the 600 rpm · 150,000 tpm rate limit/);
    const form = sheet();
    const rpm = within(form).getByLabelText("Requests per minute (optional)");
    const tpm = within(form).getByLabelText("Tokens per minute (optional)");
    await userEvent.clear(rpm);
    await userEvent.clear(tpm);
    await userEvent.type(tpm, "0");
    await userEvent.click(within(form).getByRole("button", { name: "Save" }));

    await expect(await refusedEdit.expectSentBody("PATCH", "/rate-limits/rl-1")).toEqual({
      rpm: null,
      tpm: 0,
    });
    // the reason is shown in the sheet, which still holds the draft
    await waitFor(() => expect(within(form).getByText(/tpm must be at least 1/)).toBeVisible());
    await expect(within(document.body).getByRole("dialog")).toBeInTheDocument();
    await expect(tpm).toHaveValue(0);
    await expectToast(canvasElement, /could not save the rate limit/i, "error");
    await expect(within(canvasElement).queryByText(/rate limit updated/i)).toBeNull();
  },
};

let budgetDeletes: Recorder;
let budgetDeleted = false;
let releaseBudgetDelete: () => void = () => {};
/**
 * #1904: a budget used to be deleted on the first click, leaving its scope
 * uncapped after a misclick on the bin beside the edit pencil. The delete now
 * confirms through `ConfirmDialog`: the title names the cap, a cancel sends
 * nothing and is an abandon, and a confirm holds both buttons while the DELETE
 * is out, then closes, drops the card and lands as `save_confirmed`.
 */
export const ABudgetDeleteIsConfirmedFirst: Story = {
  beforeEach: recordUxEvents,
  render: () => {
    budgetDeleted = false;
    budgetDeletes = recording(
      scoped(async (input, init) => {
        const url = String(input);
        if (init?.method === "DELETE") {
          // held until the play has seen the pending state
          return new Promise<Response>((resolve) => {
            releaseBudgetDelete = () => {
              budgetDeleted = true;
              resolve(new Response(null, { status: 204 }));
            };
          });
        }
        if (url.includes("/virtual-keys")) return json(KEYS);
        if (url.includes("/budgets")) return json(budgetDeleted ? BUDGETS.slice(1) : BUDGETS);
        return json(RATE_LIMITS);
      }),
    );
    return (
      <Harness fetchStub={budgetDeletes.stub}>
        <UxScreenProvider screen="limits">
          <Toasted>
            <Limits />
          </Toasted>
        </UxScreenProvider>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await clickWhenEnabled(canvasElement, /Delete the monthly budget/);
    const title = await within(document.body).findByRole("heading", {
      name: /^Delete the monthly budget of .*500\.00\?$/,
    });
    await expect(title).toBeInTheDocument();
    // the body names the scope by its name and type, never by its uuid
    await waitFor(() =>
      expect(
        within(document.body).getByText(/^Project Gateway is left without this budget/),
      ).toBeVisible(),
    );
    await cancelConfirmation();
    budgetDeletes.expectNotSent("DELETE", "/budgets/");
    const abandon = await expectUxEvent("form_abandon", "budget-delete");
    await expect(abandon.screen).toBe("limits");
    await expect(abandon.outcome).toBe("cancelled");
    expectNoUxEvent("form_submit", "budget-delete");

    await clickWhenEnabled(canvasElement, /Delete the monthly budget/);
    await confirmDestructive(/monthly budget/, "Delete budget");
    await budgetDeletes.expectSent("DELETE", "/budgets/budget-1");
    // the request is on the wire: the confirm spins and neither button
    // pretends it can call it back
    const dialog = within(await confirmation());
    await waitFor(() =>
      expect(dialog.getByRole("button", { name: "Delete budget" })).toBeDisabled(),
    );
    await expect(dialog.getByRole("button", { name: "Cancel" })).toBeDisabled();
    expectNoUxEvent("save_confirmed", "budget-delete");

    releaseBudgetDelete();
    await expectSheetClosed();
    await expectToast(canvasElement, /budget deleted/i);
    await waitFor(() =>
      expect(canvas.queryByRole("button", { name: /Delete the monthly budget/ })).toBeNull(),
    );
    const submit = await expectUxEvent("form_submit", "budget-delete");
    await expect(submit.outcome).toBe("ok");
    await expectUxEvent("save_confirmed", "budget-delete");
  },
};

let rateLimitDeletes: Recorder;
let rateLimitDeleted = false;
/**
 * #1904: the rate-limit delete confirms the same way, the title carrying the
 * caps, since they are all that tells two limits on one scope apart.
 */
export const ARateLimitDeleteIsConfirmedFirst: Story = {
  beforeEach: recordUxEvents,
  render: () => {
    rateLimitDeleted = false;
    rateLimitDeletes = recording(
      scoped(async (input, init) => {
        const url = String(input);
        if (init?.method === "DELETE") {
          rateLimitDeleted = true;
          return new Response(null, { status: 204 });
        }
        if (url.includes("/virtual-keys")) return json(KEYS);
        if (url.includes("/budgets")) return json(BUDGETS);
        return json(rateLimitDeleted ? RATE_LIMITS.slice(1) : RATE_LIMITS);
      }),
    );
    return (
      <Harness fetchStub={rateLimitDeletes.stub}>
        <UxScreenProvider screen="limits">
          <Toasted>
            <Limits />
          </Toasted>
        </UxScreenProvider>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await clickWhenEnabled(canvasElement, /Delete the 600 rpm · 150,000 tpm rate limit/);
    await expect(
      await within(document.body).findByRole("heading", {
        name: "Delete the 600 rpm · 150,000 tpm rate limit?",
      }),
    ).toBeInTheDocument();
    await waitFor(() =>
      expect(
        within(document.body).getByText(/^Project Gateway is left without this rate limit/),
      ).toBeVisible(),
    );
    await cancelConfirmation();
    rateLimitDeletes.expectNotSent("DELETE", "/rate-limits/");
    await expect((await expectUxEvent("form_abandon", "rate-limit-delete")).outcome).toBe(
      "cancelled",
    );

    await clickWhenEnabled(canvasElement, /Delete the 600 rpm · 150,000 tpm rate limit/);
    await confirmDestructive(/600 rpm · 150,000 tpm/, "Delete rate limit");
    await rateLimitDeletes.expectSent("DELETE", "/rate-limits/rl-1");
    await expectSheetClosed();
    await expectToast(canvasElement, /rate limit deleted/i);
    await waitFor(() =>
      expect(
        canvas.queryByRole("button", { name: /Delete the 600 rpm · 150,000 tpm rate limit/ }),
      ).toBeNull(),
    );
    await expectUxEvent("save_confirmed", "rate-limit-delete");
  },
};

/**
 * A refused delete keeps the dialog open with the control plane's reason, so
 * the operator is not left guessing whether the cap is still there.
 */
export const ARefusedBudgetDeleteKeepsTheDialogOpen: Story = {
  beforeEach: recordUxEvents,
  render: () => (
    <Harness
      fetchStub={scoped(async (input, init) => {
        const url = String(input);
        if (init?.method === "DELETE") {
          return json({ error: { message: "the store is read-only while a restore runs" } }, 409);
        }
        if (url.includes("/virtual-keys")) return json(KEYS);
        if (url.includes("/budgets")) return json(BUDGETS);
        return json(RATE_LIMITS);
      })}
    >
      <UxScreenProvider screen="limits">
        <Toasted>
          <Limits />
        </Toasted>
      </UxScreenProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /Delete the daily budget/);
    await confirmDestructive(/daily budget/, "Delete budget");
    const dialog = within(await confirmation());
    await waitFor(() =>
      expect(dialog.getByRole("alert")).toHaveTextContent("read-only while a restore runs"),
    );
    expectNoUxEvent("save_confirmed", "budget-delete");
    await expect(
      within(canvasElement).getByRole("button", { name: /Delete the daily budget/, hidden: true }),
    ).toBeInTheDocument();
  },
};

/**
 * A card's controls, measured from the top of the card that holds them.
 *
 * Measured against the card rather than the page so the same read holds when
 * the grid is one column wide. The card is the nearest column-flex ancestor,
 * which is how `LimitCard` lays itself out.
 */
const offsetInCard = (control: HTMLElement) => {
  const card = control.closest("div.flex-col") as HTMLElement;
  return control.getBoundingClientRect().top - card.getBoundingClientRect().top;
};

/** The edit controls of two cards sit at the same height, whatever else wraps. */
async function expectControlsShareAHeight(canvasElement: HTMLElement) {
  const canvas = within(canvasElement);
  const edits = (name: RegExp) => canvas.getAllByRole("button", { name });
  for (const group of [edits(/^Edit the .* budget of /), edits(/^Edit the .* rate limit/)]) {
    await expect(group).toHaveLength(2);
    await expect(Math.abs(offsetInCard(group[0]) - offsetInCard(group[1]))).toBeLessThan(1);
  }
}

/**
 * #2095: the budget form asked for dollars whatever the deployment settles in,
 * while the card beside it formatted the same amount in the settlement
 * currency, so on a EUR deployment the form asked for dollars and saved euros.
 */
export const TheBudgetFormNamesTheSettlementCurrency: Story = {
  render: () => (
    <Harness
      fetchStub={routes([
        ["/api/v1/currency", () => ({ base: "EUR", codes: ["EUR"], rates: {} })],
        ["/virtual-keys", () => KEYS],
        ["/budgets", () => BUDGETS],
        ["/rate-limits", () => RATE_LIMITS],
      ])}
    >
      <Limits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    // the card and the form agree on the currency once the settings have answered
    await expect(await within(canvasElement).findByText(/€500\.00/)).toBeVisible();
    await clickWhenEnabled(canvasElement, /add budget/i);
    await expect(await within(sheet()).findByLabelText("Limit (EUR)")).toHaveValue(100);
    await expect(within(sheet()).queryByLabelText("Limit (USD)")).toBeNull();
  },
};

/**
 * #2095: the scope badge printed `project` (the stored enum, untranslated in
 * `ru`) beside the scope's uuid, on every card, repeating what the picker above
 * already names. The cards now say nothing about it, and the controls and the
 * confirmation name the scope the way the picker does.
 */
export const TheScopeIsNamedNotShownAsAnId: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <Limits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(
      await canvas.findByRole("button", {
        name: "Delete the monthly budget of $500.00 for Gateway",
      }),
    ).toBeInTheDocument();
    await expect(
      canvas.getByRole("button", {
        name: "Delete the 600 rpm · 150,000 tpm rate limit for Gateway",
      }),
    ).toBeInTheDocument();
    await expect(canvas.queryByText("project")).toBeNull();
    await expect(canvas.queryByText("project-1")).toBeNull();

    // the forms call the scope the same thing, not `project:project-1`
    await clickWhenEnabled(canvasElement, /add budget/i);
    await expect(await within(sheet()).findByText("Spend cap for Gateway")).toBeInTheDocument();
    await expectClosesWithoutPrompting();
    await clickWhenEnabled(canvasElement, /add rate limit/i);
    await expect(
      await within(sheet()).findByText(/^Throughput caps for Gateway/),
    ).toBeInTheDocument();
    await expect(within(sheet()).queryByText(/project:/)).toBeNull();
  },
};

/**
 * A scope that is not a project is named by its own label, and the
 * confirmation says what kind of scope it is leaving uncapped.
 */
export const AVirtualKeyScopeIsNamedByItsKeyName: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <Limits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await pickOption(await canvas.findByLabelText("Scope type"), "Virtual key");
    await pickOption(await canvas.findByLabelText("Scope"), "backend service");
    await clickWhenEnabled(
      canvasElement,
      "Delete the monthly budget of $500.00 for backend service",
    );
    await waitFor(() =>
      expect(
        within(document.body).getByText(/^Virtual key backend service is left without this budget/),
      ).toBeVisible(),
    );
    await cancelConfirmation();
  },
};

// the periods the dashboard can name, and one it cannot
const PERIOD_BUDGETS: BudgetRow[] = [
  { ...BUDGETS[0], id: "period-1", period: "30d", limit_usd: "500.00" },
  { ...BUDGETS[0], id: "period-2", period: "1d", limit_usd: "50.00" },
  { ...BUDGETS[0], id: "period-3", period: "total", limit_usd: "1000.00" },
  { ...BUDGETS[0], id: "period-4", period: "7d", limit_usd: "120.00" },
];

/**
 * #2095: the period badge printed the stored text. A known period is named in
 * words, and one the dashboard has no name for is kept exactly as stored, so
 * the operator reads what the row holds. #1902 owns what the field accepts.
 */
export const PeriodsAreNamedAndAnUnknownOneIsKeptAsStored: Story = {
  render: () => (
    <Harness
      fetchStub={routes([
        ["/virtual-keys", () => KEYS],
        ["/budgets", () => PERIOD_BUDGETS],
        ["/rate-limits", () => RATE_LIMITS],
      ])}
    >
      <Limits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("monthly")).toBeVisible();
    await expect(canvas.getByText("daily")).toBeVisible();
    await expect(canvas.getByText("lifetime")).toBeVisible();
    await expect(canvas.getByText("7d")).toBeVisible();
    await expect(canvas.queryByText("30d")).toBeNull();
    await expect(canvas.queryByText("1d")).toBeNull();
    await expect(canvas.queryByText("total")).toBeNull();
  },
};

/**
 * #2095: the caps were 10px badge text, below the detector's floor, on the card
 * whose whole point is those numbers. They are set the way the budget's amount
 * is, and they group digits through the dashboard's locale.
 */
export const RateLimitCapsAreTheFigureOfTheCard: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <Limits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const amount = await canvas.findByText("$500.00");
    const cap = canvas.getByText("150,000");
    const type = (node: HTMLElement) => {
      const { fontSize, fontFamily, fontWeight } = getComputedStyle(node);
      return { fontSize, fontFamily, fontWeight };
    };
    await expect(type(cap)).toEqual(type(amount));
    await expect(parseFloat(type(cap).fontSize)).toBeGreaterThan(11);
    await expect(canvas.queryByText(/150000/)).toBeNull();
    // the unit stays beside its number, once per cap that is set
    await expect(canvas.getAllByText("tpm")).toHaveLength(2);
    await expect(canvas.getAllByText("rpm")).toHaveLength(1);
  },
};

/**
 * #2095: a budget with an unpriced override wrapped its edit and delete buttons
 * onto a second line, so two cards side by side had their controls at
 * different heights. The controls have a line of their own now.
 */
export const CardControlsShareOneHeight: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <Limits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByRole("button", { name: /^Edit the daily budget/ });
    // exactly one of the two budgets carries the badge that used to push them down
    await expect(canvas.getAllByText(/^unpriced: /)).toHaveLength(1);
    await expectControlsShareAHeight(canvasElement);
  },
};

// a delete that never answers, so the pending state is the one left on screen
const deleteNeverAnswers = scoped(async (input, init) => {
  const url = String(input);
  if (init?.method === "DELETE") return new Promise<Response>(() => {});
  if (url.includes("/virtual-keys")) return json(KEYS);
  if (url.includes("/budgets")) return json(BUDGETS);
  return json(RATE_LIMITS);
});

/**
 * #2095: `deleting` was the mutation's own `isPending`, which every card reads,
 * so one delete put a spinner on every card's bin. Only the row being deleted
 * is held.
 */
export const OnlyTheRowBeingDeletedSpins: Story = {
  render: () => (
    <Harness fetchStub={deleteNeverAnswers}>
      <Limits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    // the dialog is over the cards and makes them inert, so they are read as hidden
    const bin = (period: string) =>
      within(canvasElement).getByRole("button", {
        name: new RegExp(`^Delete the ${period} budget`),
        hidden: true,
      });
    await clickWhenEnabled(canvasElement, /^Delete the monthly budget/);
    await confirmDestructive(/monthly budget/, "Delete budget");
    await waitFor(() => expect(bin("monthly")).toBeDisabled());
    await expect(bin("daily")).toBeEnabled();
  },
};

/** The same holds for the rate limits, which keep their own mutation. */
export const OnlyTheRateLimitBeingDeletedSpins: Story = {
  render: () => (
    <Harness fetchStub={deleteNeverAnswers}>
      <Limits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const bin = (caps: string) =>
      within(canvasElement).getByRole("button", {
        name: new RegExp(`^Delete the ${caps} rate limit`),
        hidden: true,
      });
    await clickWhenEnabled(canvasElement, /^Delete the 600 rpm · 150,000 tpm rate limit/);
    await confirmDestructive(/600 rpm · 150,000 tpm/, "Delete rate limit");
    await waitFor(() => expect(bin("600 rpm · 150,000 tpm")).toBeDisabled());
    await expect(bin("20,000 tpm")).toBeEnabled();
  },
};

/**
 * #2095: the screen read in `ru` showed `project`, the stored enum, and the raw
 * period, beside grouped digits that followed the browser rather than the
 * dashboard. Nothing on a card or in its confirmation is left in English but
 * the `rpm` and `tpm` notation.
 */
export const ReadsInRussianWithNoRawEnglishEnum: Story = {
  globals: { locale: "ru" },
  render: () => (
    <Harness fetchStub={loaded}>
      <Limits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("ежемесячно")).toBeVisible();
    await expect(canvas.getByText("ежедневно")).toBeVisible();
    await expect(canvas.queryByText("30d")).toBeNull();
    await expect(canvas.queryByText("project")).toBeNull();
    await expect(canvas.getByText(/^150\s000$/)).toBeVisible();

    await clickWhenEnabled(canvasElement, /^Удалить бюджет .*\(ежемесячно\) для Gateway$/);
    await waitFor(() =>
      expect(
        within(document.body).getByRole("heading", {
          name: /^Удалить бюджет 500,00\s\$ \(ежемесячно\)\?$/,
        }),
      ).toBeVisible(),
    );
    await waitFor(() =>
      expect(
        within(document.body).getByText(/^Проект Gateway останется без этого бюджета/),
      ).toBeVisible(),
    );
    await userEvent.click(
      within(await confirmation()).getByRole("button", { name: ru.common.cancel }),
    );
    await expectSheetClosed();

    await clickWhenEnabled(canvasElement, /добавить бюджет/i);
    await expect(await within(sheet()).findByLabelText("Лимит (USD)")).toBeInTheDocument();
    await expect(within(sheet()).getByText("Лимит расходов для Gateway")).toBeInTheDocument();
    await expect(within(sheet()).queryByText(/project/)).toBeNull();
  },
};

let budgetReads: Recorder;
/**
 * #2095: the only failure story was a 403, which withholds the retry. A server
 * error is what offers "Try again", and the retry has to ask the control plane
 * again rather than redraw the failure.
 */
export const AFailedReadCanBeRetried: Story = {
  render: () => {
    let attempts = 0;
    budgetReads = recording(
      scoped(async (input) => {
        const url = String(input);
        if (url.includes("/virtual-keys")) return json(KEYS);
        if (url.includes("/budgets")) {
          attempts += 1;
          return attempts === 1
            ? json({ error: { message: "the budget store is unavailable" } }, 503)
            : json(BUDGETS);
        }
        return json(RATE_LIMITS);
      }),
    );
    return (
      <Harness fetchStub={budgetReads.stub}>
        <Limits />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectLoadError(canvasElement, /budgets/i);
    // a read that failed holds no rows, and the screen claims nothing about them
    await expectNoFalseEmpty(canvasElement, /no budgets for this scope/i);
    await expect(canvas.getByText("the budget store is unavailable")).toBeVisible();

    const reads = () =>
      budgetReads.calls.filter((c) => c.method === "GET" && c.url.includes("/budgets")).length;
    const before = reads();
    await userEvent.click(canvas.getByRole("button", { name: en.errors.load.retry }));
    await waitFor(() => expect(canvas.getByText("$500.00")).toBeVisible());
    await expect(reads()).toBeGreaterThan(before);
    await expect(canvas.queryByRole("alert")).toBeNull();
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
    await canvas.findByRole("button", { name: /^Edit the daily budget/ });
    await expectControlsShareAHeight(canvasElement);
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
    await expectRefused(canvasElement, /Delete the monthly budget/);
    await expectRefused(canvasElement, /Delete the 600 rpm · 150,000 tpm rate limit/);
    // editing in place is `update`, a capability of its own (#1285)
    await expectRefused(canvasElement, /Edit the monthly budget/);
    await expectRefused(canvasElement, /Edit the 600 rpm · 150,000 tpm rate limit/);
  },
};

export const EditableByAnAdmin: Story = {
  render: () => (
    <Harness fetchStub={loaded} role="admin">
      <Limits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectAllowed(canvasElement, /Edit the monthly budget/);
    await expectAllowed(canvasElement, /Edit the 600 rpm · 150,000 tpm rate limit/);
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
    await expectRefused(canvasElement, /Edit the monthly budget/);
  },
};
