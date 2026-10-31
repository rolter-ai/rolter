import type { Meta, StoryObj } from "@storybook/react-vite";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import Limits from "./Limits";
import {
  Harness,
  Toasted,
  adminOfProject,
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
  openOptions,
  pickOption,
  pending,
  recordUxEvents,
  recording,
  routes,
  scoped,
  sheet,
  answerDiscardPrompt,
  uxEvents,
  type Recorder,
} from "./story-harness";
import { AuthProvider } from "@/lib/auth";
import type { BudgetRow, BudgetSpendReport, RateLimitRow, VirtualKeyRow } from "@/lib/api";
import en from "@/lib/i18n/locales/en.json";
import ru from "@/lib/i18n/locales/ru.json";
import { expectFieldHints } from "@/lib/story-hints";
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

// the gateway's counters, as the control plane reads them (#2091): the first
// budget is comfortably inside its cap, the second is close to it
const SPEND: BudgetSpendReport = {
  status: "ok",
  budgets: [
    { id: "budget-1", spent_usd: "212.5000", resets_at: "2026-08-01T00:00:00Z" },
    { id: "budget-2", spent_usd: "46.2500", resets_at: "2026-07-03T00:00:00Z" },
  ],
};

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

/**
 * The gateway's counters for whichever budgets a story serves (#3027). The
 * screen reads `/budgets/spend` beside the list, so a stub that answers
 * `/budgets` alone renders the "spend unavailable" note instead of the state
 * the story names. A budget the shared fixture has no counter for reads as
 * comfortably inside its cap.
 */
function spendFor(budgets: BudgetRow[]): BudgetSpendReport {
  return {
    status: "ok",
    budgets: budgets.map(
      (budget) =>
        SPEND.budgets.find((entry) => entry.id === budget.id) ?? {
          id: budget.id,
          spent_usd: "10.0000",
          resets_at: "2026-08-01T00:00:00Z",
        },
    ),
  };
}

/** answers a `url.includes("/budgets")` branch: the counters for the spend path, the rows otherwise */
function budgetRead(url: string, budgets: BudgetRow[]) {
  return json(url.includes("/budgets/spend") ? spendFor(budgets) : budgets);
}

// the screen resolves virtual keys for its scope picker before either list, so
// the longer fragment has to be matched ahead of the shared prefix
const loaded = routes([
  ["/virtual-keys", () => KEYS],
  ["/budgets/spend", () => SPEND],
  ["/budgets", () => BUDGETS],
  ["/rate-limits", () => RATE_LIMITS],
]);
const empty = routes([
  ["/virtual-keys", () => KEYS],
  ["/budgets/spend", () => spendFor([])],
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

// a link from another screen, such as a team's detail sheet, opens the page on
// the scope it names rather than on the sidebar's project
const linked = recording(
  routes([
    ["/virtual-keys", () => KEYS],
    ["/budgets/spend", () => spendFor([])],
    ["/budgets", () => []],
    ["/rate-limits", () => []],
  ]),
);
export const OpensOnTheScopeALinkNames: Story = {
  render: () => (
    <Harness fetchStub={linked.stub} route="/budgets?scope_type=team&scope_id=team-7">
      <Limits />
    </Harness>
  ),
  play: async () => {
    await linked.expectSent("GET", "/budgets?scope_type=team&scope_id=team-7");
    await linked.expectSent("GET", "/rate-limits?scope_type=team&scope_id=team-7");
    linked.expectNotSent("GET", "scope_type=project");
  },
};

function withSpend(spend: BudgetSpendReport) {
  return (
    <Harness
      fetchStub={routes([
        ["/virtual-keys", () => KEYS],
        ["/budgets/spend", () => spend],
        ["/budgets", () => BUDGETS],
        ["/rate-limits", () => RATE_LIMITS],
      ])}
    >
      <Limits />
    </Harness>
  );
}

/**
 * #2091: the card says how much of the cap is spent, how much is left and when
 * the counter resets, with the figures also in text so the bar is not the only
 * carrier. Close to the cap the bar takes the warning hue.
 */
export const SpendNearTheCap: Story = {
  render: () => withSpend(SPEND),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(/\$212\.50 spent · \$287\.50 left · 43%/)).toBeVisible();
    await expect(canvas.getByText(/\$46\.25 spent · \$3\.75 left · 93%/)).toBeVisible();
    await expect(canvasElement.querySelectorAll("[data-spend=warning]")).toHaveLength(1);
    await expect(canvasElement.querySelectorAll("[data-spend=ok]")).toHaveLength(1);
    await expect(canvas.getAllByText(/^Resets /)).toHaveLength(2);
    await expect(canvas.getAllByRole("progressbar")).toHaveLength(2);
  },
};

export const SpendExhausted: Story = {
  render: () =>
    withSpend({
      status: "ok",
      budgets: [
        { id: "budget-1", spent_usd: "500.0000", resets_at: "2026-08-01T00:00:00Z" },
        { id: "budget-2", spent_usd: "61.0000", resets_at: "2026-07-03T00:00:00Z" },
      ],
    }),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(/\$500\.00 spent · cap reached \(100%\)/)).toBeVisible();
    await expect(canvas.getByText(/\$61\.00 spent · cap reached \(122%\)/)).toBeVisible();
    await expect(canvasElement.querySelectorAll("[data-spend=danger]")).toHaveLength(2);
  },
};

/** A counter the control plane could not read is unknown, never a zero. */
export const SpendUnavailable: Story = {
  render: () =>
    withSpend({
      status: "unavailable",
      budgets: [
        { id: "budget-1", spent_usd: null, resets_at: "2026-08-01T00:00:00Z" },
        { id: "budget-2", spent_usd: null, resets_at: "2026-07-03T00:00:00Z" },
      ],
    }),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findAllByText(/spend so far is unavailable/i)).toHaveLength(2);
    await expect(canvas.queryByRole("progressbar")).toBeNull();
    await expect(canvas.queryByText(/\$0\.00 spent/)).toBeNull();
  },
};

export const SpendNeedsRedis: Story = {
  render: () =>
    withSpend({
      status: "not_configured",
      budgets: [
        { id: "budget-1", spent_usd: null, resets_at: null },
        { id: "budget-2", spent_usd: null, resets_at: null },
      ],
    }),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect((await canvas.findAllByText(/ROLTER_REDIS_URL/)).length).toBe(2);
    await expect(canvas.queryByRole("progressbar")).toBeNull();
  },
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
        if (url.includes("/budgets")) return budgetRead(url, BUDGETS);
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
        if (url.includes("/budgets")) return budgetRead(url, BUDGETS);
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
 * `Add budget` opens pre-filled with `100` / monthly rather than blank, so its
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
    await expect(within(sheet()).getByLabelText("Period")).toHaveValue("Monthly");
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

let periodPicked: Recorder;
/**
 * #1902: the period is picked from the windows the gateway has, never typed.
 * The free-text field suggested `7d`, which the gateway read as monthly, so
 * this pins what is offered, that typing cannot add another, that the keyboard
 * reaches the list, and what is sent.
 */
export const ThePeriodIsPickedFromTheWindowsTheGatewayHas: Story = {
  render: () => {
    periodPicked = recording(
      scoped(async (input, init) => {
        const url = String(input);
        if (init?.method === "POST") return json({ ...BUDGETS[1], id: "budget-3" }, 201);
        if (url.includes("/virtual-keys")) return json(KEYS);
        if (url.includes("/budgets")) return budgetRead(url, BUDGETS);
        return json(RATE_LIMITS);
      }),
    );
    return (
      <Harness fetchStub={periodPicked.stub}>
        <Limits />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /add budget/i);
    const form = sheet();
    const picker = await within(form).findByRole("combobox", { name: "Period" });
    // the old hint offered `7d` as an example; this one says there is no such window
    await waitFor(() =>
      expect(within(form).getByText(/There are no rolling windows/)).toBeVisible(),
    );
    await expect(within(form).queryByText(/e\.g\./)).toBeNull();

    const listbox = await openOptions(picker);
    const options = within(listbox).getAllByRole("option");
    await expect(options.map((option) => option.textContent)).toEqual([
      "DailyResets every day at 00:00 UTC",
      "MonthlyResets on the 1st of each month at 00:00 UTC",
      "LifetimeNever resets",
    ]);
    await expect(within(listbox).getByRole("option", { name: /^Monthly/ })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    await userEvent.click(picker);

    // typing filters the list, it does not add to it: `7d` matches nothing, and
    // leaving the field puts the picked window back
    await userEvent.clear(picker);
    await userEvent.type(picker, "7d");
    await expect(within(document.body).queryAllByRole("option")).toHaveLength(0);
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(picker).toHaveValue("Monthly"));

    // arrows name the active option while focus stays on the input
    picker.focus();
    await userEvent.keyboard("{ArrowDown}");
    await expect(picker).toHaveAttribute("aria-expanded", "true");
    await userEvent.keyboard("{Home}");
    const active = picker.getAttribute("aria-activedescendant");
    await expect(document.getElementById(active ?? "")).toHaveTextContent(/^Daily/);
    await expect(picker).toHaveFocus();
    await userEvent.keyboard("{Enter}");
    await expect(picker).toHaveValue("Daily");

    await userEvent.click(within(form).getByRole("button", { name: "Create" }));
    await expect(await periodPicked.expectSentBody("POST", "/budgets")).toMatchObject({
      period: "daily",
    });
    await expectSheetClosed();
  },
};

const MISREAD: BudgetRow = {
  id: "budget-7d",
  scope_type: "project",
  scope_id: "project-1",
  limit_usd: "70.0000",
  period: "7d",
  unpriced_policy: null,
  created_at: "2026-06-01T00:00:00Z",
};

let misreadFixed: Recorder;
/**
 * #1902: a budget stored as `7d` before the control plane checked the period.
 * Nothing rewrites it, and the gateway enforces it as monthly, so the card has
 * to say so rather than wear a badge that reads as a weekly cap. The edit sheet
 * opens on no period at all and cannot be saved until a real window is picked,
 * so the operator decides what replaces it.
 */
export const AnUnrecognisedPeriodIsFlaggedAndHasToBeReplaced: Story = {
  render: () => {
    misreadFixed = recording(
      scoped(async (input, init) => {
        const url = String(input);
        if (init?.method === "PATCH") return json({ ...MISREAD, period: "monthly" });
        if (url.includes("/virtual-keys")) return json(KEYS);
        if (url.includes("/budgets")) return budgetRead(url, [...BUDGETS, MISREAD]);
        return json(RATE_LIMITS);
      }),
    );
    return (
      <Harness fetchStub={misreadFixed.stub}>
        <Limits />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(
      await canvas.findByText(
        /does not recognise the period “7d” and enforces this budget as monthly/,
      ),
    ).toBeVisible();
    // the badge keeps what is stored, and only that row carries the warning: a
    // stored shorthand such as `30d` is a window the gateway reads
    await expect(canvas.getByText("7d")).toBeVisible();
    await expect(canvas.getAllByText(/does not recognise the period/)).toHaveLength(1);
    await expect(canvas.getByText("monthly")).toBeVisible();
    await expect(canvas.getByText("daily")).toBeVisible();

    await clickWhenEnabled(canvasElement, /Edit the 7d budget/);
    const form = sheet();
    const picker = await within(form).findByRole("combobox", { name: "Period" });
    await expect(picker).toHaveValue("");
    await expect(picker).toHaveAttribute("placeholder", "Pick a period");
    await waitFor(() =>
      expect(
        within(form).getByText(/does not recognise “7d” and enforces this budget as monthly/),
      ).toBeVisible(),
    );
    // the stored text is not offered back as a window of its own
    const listbox = await openOptions(picker);
    await expect(
      within(listbox)
        .getAllByRole("option")
        .map((option) => option.textContent),
    ).toEqual([
      "DailyResets every day at 00:00 UTC",
      "MonthlyResets on the 1st of each month at 00:00 UTC",
      "LifetimeNever resets",
    ]);
    await userEvent.click(picker);

    // nothing moved, and then only the cap moved: neither can be saved while
    // the period is still the one the gateway misreads
    const save = within(form).getByRole("button", { name: "Save" });
    await expect(save).toHaveAttribute("aria-disabled", "true");
    const limit = within(form).getByLabelText("Limit (USD)");
    await userEvent.clear(limit);
    await userEvent.type(limit, "80");
    await expect(save).toHaveAttribute("aria-disabled", "true");

    await pickOption(picker, /^Monthly/);
    await expect(picker).toHaveValue("Monthly");
    await waitFor(() => expect(save).toBeEnabled());
    await expect(within(form).queryByText(/does not recognise “7d”/)).toBeNull();
    await userEvent.click(save);

    await expect(await misreadFixed.expectSentBody("PATCH", "/budgets/budget-7d")).toEqual({
      limit_usd: "80",
      period: "monthly",
    });
    await expectSheetClosed();
  },
};

export const CreatesARateLimit: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input, init) => {
        const url = String(input);
        if (init?.method === "POST") return json(RATE_LIMITS[0], 201);
        if (url.includes("/virtual-keys")) return json(KEYS);
        if (url.includes("/budgets")) return budgetRead(url, BUDGETS);
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
 * Either cap will do, so neither field is marked required; a refused submit with
 * both empty says what is missing in the footer (#3242), and Enter submits once
 * one is typed.
 */
let rateLimitBlocked: Recorder;
export const RateLimitSheetNamesWhatIsMissing: Story = {
  render: () => {
    rateLimitBlocked = recording(
      scoped(async (input, init) => {
        const url = String(input);
        if (init?.method === "POST") return json(RATE_LIMITS[0], 201);
        if (url.includes("/virtual-keys")) return json(KEYS);
        if (url.includes("/budgets")) return budgetRead(url, BUDGETS);
        return json(RATE_LIMITS);
      }),
    );
    return (
      <Harness fetchStub={rateLimitBlocked.stub}>
        <Limits />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /add rate limit/i);
    const form = within(sheet());
    const rpm = form.getByLabelText("Requests per minute (optional)");
    await expect(form.queryByRole("status")).toBeNull();

    await userEvent.click(form.getByRole("button", { name: "Create" }));
    await expect(await form.findByRole("status")).toHaveTextContent(
      "Set a requests or a tokens per minute limit.",
    );
    rateLimitBlocked.expectNotSent("POST", "/rate-limits");

    await userEvent.type(rpm, "300{Enter}");
    await expectSheetClosed();
    await expect(await rateLimitBlocked.expectSentBody("POST", "/rate-limits")).toMatchObject({
      rpm: 300,
    });
  },
};

/**
 * The scope type, the budget's limit and both rate-limit caps explain
 * themselves in an (i) (#3285): what the caps belong to, what spend is counted
 * from, and that an empty cap is no cap.
 */
function explainsEveryField(locale: "en" | "ru"): Story {
  const catalog = locale === "ru" ? ru : en;
  const p = catalog.pages.limits;
  return {
    ...(locale === "ru" ? { globals: { locale: "ru" } } : {}),
    render: () => (
      <Harness
        fetchStub={scoped(async (input) => {
          const url = String(input);
          if (url.includes("/virtual-keys")) return json(KEYS);
          if (url.includes("/budgets")) return budgetRead(url, BUDGETS);
          return json(RATE_LIMITS);
        })}
      >
        <Limits />
      </Harness>
    ),
    play: async ({ canvasElement }) => {
      const open = locale === "ru" ? /добавить бюджет/i : /add budget/i;
      const openRate = locale === "ru" ? /добавить лимит скорости/i : /add rate limit/i;
      await clickWhenEnabled(canvasElement, open);
      await expectFieldHints(sheet(), catalog.common.aboutField, [
        [
          p.budgetLimitLabel.replace("{{currency}}", "USD"),
          p.budgetLimitInfo.replace("{{currency}}", "USD"),
        ],
      ]);
      await userEvent.keyboard("{Escape}");
      await expectSheetClosed();
      await expectFieldHints(canvasElement, catalog.common.aboutField, [
        [p.scopeTypeLabel, p.scopeTypeInfo],
      ]);
      await clickWhenEnabled(canvasElement, openRate);
      await expectFieldHints(sheet(), catalog.common.aboutField, [
        [p.rpmLabel, p.rpmInfo],
        [p.tpmLabel, p.tpmInfo],
      ]);
    },
  };
}
export const ExplainsEachFieldInAHint: Story = explainsEveryField("en");
export const ExplainsEachFieldInAHintInRussian: Story = explainsEveryField("ru");

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
      if (url.includes("/budgets")) return budgetRead(url, BUDGETS);
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
    await waitFor(() =>
      expect(within(form).getByRole("heading", { name: "Edit budget" })).toBeVisible(),
    );
    // the form opens on the row as it stands, not on the create defaults: the
    // stored `30d` is the monthly window, and since nobody picked it again the
    // body below carries no period at all
    await expect(within(form).getByLabelText("Limit (USD)")).toHaveValue(500);
    await expect(within(form).getByLabelText("Period")).toHaveValue("Monthly");
    // the workaround the sheet used to advertise is gone
    await expect(within(form).queryByText(/delete and recreate/i)).toBeNull();
    // nothing moved yet, so there is nothing to save
    await expect(within(form).getByRole("button", { name: "Save" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );

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
    await waitFor(() =>
      expect(within(form).getByRole("heading", { name: "Edit rate limit" })).toBeVisible(),
    );
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
        if (url.includes("/budgets")) return budgetRead(url, BUDGETS);
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
        if (url.includes("/budgets"))
          return budgetRead(url, budgetDeleted ? BUDGETS.slice(1) : BUDGETS);
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
        if (url.includes("/budgets")) return budgetRead(url, BUDGETS);
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
        if (url.includes("/budgets")) return budgetRead(url, BUDGETS);
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
        ["/budgets/spend", () => spendFor(BUDGETS)],
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

const KEY_READ_FAILED = { error: { message: "store unavailable" } };
// the key read fails until `keyReadsFail` is cleared, and counts every attempt
let keyReads = 0;
let keyReadsFail = true;
const keyReadFails = scoped(async (input) => {
  const url = String(input);
  if (url.includes("/virtual-keys")) {
    keyReads += 1;
    return keyReadsFail ? json(KEY_READ_FAILED, 500) : json(KEYS);
  }
  if (url.includes("/budgets")) return budgetRead(url, BUDGETS);
  return json(RATE_LIMITS);
});

/**
 * #2642: the virtual-key read only feeds the scope picker, and a failure of it
 * left a bare uuid box with nothing on screen saying why. It is a `LoadError`
 * beside the picker now: it names the keys, quotes the control plane, records
 * one `error_state` under the picker's region, and its retry brings the picker
 * back.
 */
export const AFailedKeyReadSaysSoBesideThePicker: Story = {
  beforeEach: recordUxEvents,
  render: () => {
    keyReads = 0;
    keyReadsFail = true;
    return (
      <Harness fetchStub={keyReadFails}>
        <UxScreenProvider screen="limits">
          <Limits />
        </UxScreenProvider>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // a project scope reads no keys, so nothing has failed yet
    await expect(await canvas.findByText("$500.00")).toBeVisible();
    await expect(canvas.queryAllByRole("alert")).toHaveLength(0);

    await pickOption(await canvas.findByLabelText("Scope type"), "Virtual key");
    await expectLoadError(canvasElement, /failed to return virtual keys/i);
    await expect(canvas.getByText("store unavailable")).toBeVisible();
    const failed = await expectUxEvent("error_state", "scope-virtual-keys");
    await expect(failed.screen).toBe("limits");
    await expect(uxEvents().filter((e) => e.action === "error_state")).toHaveLength(1);

    keyReadsFail = false;
    await userEvent.click(canvas.getByRole("button", { name: "Try again" }));
    await waitFor(() => expect(canvas.queryAllByRole("alert")).toHaveLength(0));
    await pickOption(await canvas.findByLabelText("Scope"), "backend service");
    await expect(keyReads).toBe(2);
    // the read that answered is not a second incident
    await expect(uxEvents().filter((e) => e.action === "error_state")).toHaveLength(1);
  },
};

/**
 * The failed key read keeps its error after another scope type is picked, and
 * that type's picker has nothing to do with keys, so the alert goes with the
 * virtual-key scope (#2642).
 */
export const AFailedKeyReadLeavesWithTheKeyScope: Story = {
  render: () => {
    keyReads = 0;
    keyReadsFail = true;
    return (
      <Harness fetchStub={keyReadFails}>
        <Limits />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const scopeType = await canvas.findByLabelText("Scope type");
    await pickOption(scopeType, "Virtual key");
    await expectLoadError(canvasElement, /failed to return virtual keys/i);

    await pickOption(scopeType, "Team");
    await waitFor(() => expect(canvas.queryAllByRole("alert")).toHaveLength(0));
    await expect(await canvas.findByLabelText("Scope")).toBeVisible();
    await expect(keyReads).toBe(1);
  },
};

// the three reads behind the scope picker, each failing or never answering
const PICKER_READS = [
  { type: "Virtual key", path: "/virtual-keys", noun: /virtual keys/i },
  { type: "Business unit", path: "/business-units", noun: /business units/i },
  { type: "Customer", path: "/customers", noun: /customers/i },
] as const;

const pickerReadStub = (path: string, how: "fail" | "pend") =>
  scoped(async (input) => {
    const url = String(input);
    if (url.includes(path)) {
      return how === "fail"
        ? json({ error: { message: "store unavailable" } }, 500)
        : new Promise<Response>(() => {});
    }
    if (url.includes("/budgets")) return budgetRead(url, BUDGETS);
    return json(RATE_LIMITS);
  });

const failedPickerRead = (i: number): Story => ({
  render: () => (
    <Harness fetchStub={pickerReadStub(PICKER_READS[i].path, "fail")}>
      <Limits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await pickOption(await canvas.findByLabelText("Scope type"), PICKER_READS[i].type);
    await expectLoadError(canvasElement, PICKER_READS[i].noun);
    await expect(canvas.getByText("store unavailable")).toBeVisible();
    await expect(canvas.getByRole("button", { name: "Try again" })).toBeVisible();
  },
});

const pendingPickerRead = (i: number): Story => ({
  render: () => (
    <Harness fetchStub={pickerReadStub(PICKER_READS[i].path, "pend")}>
      <Limits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await pickOption(await canvas.findByLabelText("Scope type"), PICKER_READS[i].type);
    await expectSkeleton(canvasElement);
    // a read that has not answered is not an empty one (#2211)
    await expect(canvas.queryByText(/none are available to pick/i)).toBeNull();
    await expect(canvas.queryByPlaceholderText(/00000000-0000/)).toBeNull();
  },
});

/** #2959: a failed business-unit read says so beside the picker, as the key read does. */
export const AFailedBusinessUnitReadSaysSo: Story = failedPickerRead(1);

/** #2959: a failed customer read says so beside the picker. */
export const AFailedCustomerReadSaysSo: Story = failedPickerRead(2);

/** #2959: while the key read is pending the scope field is a skeleton, not a uuid box. */
export const APendingKeyReadIsNotAnEmptyPicker: Story = pendingPickerRead(0);

/** #2959: the same for business units. */
export const APendingBusinessUnitReadIsNotAnEmptyPicker: Story = pendingPickerRead(1);

/** #2959: the same for customers. */
export const APendingCustomerReadIsNotAnEmptyPicker: Story = pendingPickerRead(2);

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
 * the operator reads what the row holds. The warning beside it, and what the
 * edit sheet accepts instead, are the stories above (#1902).
 */
export const PeriodsAreNamedAndAnUnknownOneIsKeptAsStored: Story = {
  render: () => (
    <Harness
      fetchStub={routes([
        ["/virtual-keys", () => KEYS],
        ["/budgets/spend", () => spendFor(PERIOD_BUDGETS)],
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
  if (url.includes("/budgets")) return budgetRead(url, BUDGETS);
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
    // the period picker reads in Russian too: its value and every window it offers
    const picker = within(sheet()).getByRole("combobox", { name: "Период" });
    await expect(picker).toHaveValue(ru.pages.limits.periodOptions.monthly);
    const listbox = await openOptions(picker);
    await expect(
      within(listbox)
        .getAllByRole("option")
        .map((option) => option.textContent),
    ).toEqual(
      (["daily", "monthly", "total"] as const).map(
        (kind) => ru.pages.limits.periodOptions[kind] + ru.pages.limits.periodDescriptions[kind],
      ),
    );
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
        if (url.includes("/budgets/spend")) return budgetRead(url, BUDGETS);
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

const MIXED_BUDGETS: BudgetRow[] = [
  { ...BUDGETS[0], limit_usd: "500.00" },
  {
    ...BUDGETS[0],
    id: "budget-team",
    scope_type: "team",
    scope_id: "team-1",
    limit_usd: "900.00",
  },
  {
    ...BUDGETS[0],
    id: "budget-org",
    scope_type: "org",
    scope_id: "org-1",
    limit_usd: "2000.00",
  },
];

// a cap is checked at the scope it throttles, so a project admin's answer at
// their project says nothing about the team or the org above it (#2529): the
// project's caps are theirs, the others name the role they take
export const ProjectAdminOnAMixOfProjectTeamAndOrgCaps: Story = {
  render: () => (
    <Harness
      role={adminOfProject("project-1")}
      fetchStub={routes([
        ["/virtual-keys", () => KEYS],
        ["/budgets/spend", () => spendFor(MIXED_BUDGETS)],
        ["/budgets", () => MIXED_BUDGETS],
        [
          "/rate-limits",
          () => [
            { ...RATE_LIMITS[0], rpm: 600, tpm: null },
            {
              ...RATE_LIMITS[0],
              id: "rl-team",
              scope_type: "team",
              scope_id: "team-1",
              rpm: 700,
              tpm: null,
            },
            {
              ...RATE_LIMITS[0],
              id: "rl-org",
              scope_type: "org",
              scope_id: "org-1",
              rpm: 800,
              tpm: null,
            },
          ],
        ],
      ])}
    >
      <Limits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectAllowed(canvasElement, /Edit the .* budget of .*500\.00/);
    await expectAllowed(canvasElement, /Delete the .* budget of .*500\.00/);
    await expectRefused(canvasElement, /Edit the .* budget of .*900\.00/);
    await expectRefused(canvasElement, /Delete the .* budget of .*900\.00/);
    await expectRefused(canvasElement, /Edit the .* budget of .*2,000\.00/);
    await expectRefused(canvasElement, /Delete the .* budget of .*2,000\.00/);
    await expectAllowed(canvasElement, /Edit the 600 rpm rate limit/);
    await expectAllowed(canvasElement, /Delete the 600 rpm rate limit/);
    await expectRefused(canvasElement, /Edit the 700 rpm rate limit/);
    await expectRefused(canvasElement, /Delete the 700 rpm rate limit/);
    await expectRefused(canvasElement, /Edit the 800 rpm rate limit/);
    await expectRefused(canvasElement, /Delete the 800 rpm rate limit/);
  },
};

// caps across the org, as `GET /api/v1/orgs/{org_id}/budgets` answers them: the
// project's own two, plus the team's and the org's above it, a business unit's
// and a virtual key's (#2093)
const ORG_BUDGETS: BudgetRow[] = [
  ...BUDGETS,
  { ...BUDGETS[0], id: "budget-team", scope_type: "team", scope_id: "team-1", limit_usd: "900.00" },
  { ...BUDGETS[0], id: "budget-org", scope_type: "org", scope_id: "org-1", limit_usd: "2000.00" },
  {
    ...BUDGETS[0],
    id: "budget-unit",
    scope_type: "business_unit",
    scope_id: "unit-1",
    limit_usd: "300.00",
  },
  {
    ...BUDGETS[0],
    id: "budget-key",
    scope_type: "virtual_key",
    scope_id: "abcdef0123456789",
    scope_name: "ci runner",
    limit_usd: "20.00",
  },
];
const ORG_RATE_LIMITS: RateLimitRow[] = [
  ...RATE_LIMITS,
  { ...RATE_LIMITS[0], id: "rl-team", scope_type: "team", scope_id: "team-1", rpm: 700, tpm: null },
  { ...RATE_LIMITS[0], id: "rl-org", scope_type: "org", scope_id: "org-1", rpm: 800, tpm: null },
];

const orgWide = (
  budgets: BudgetRow[],
  rateLimits: RateLimitRow[],
  spend: BudgetSpendReport = spendFor(budgets),
) =>
  routes([
    ["/virtual-keys", () => KEYS],
    ["/business-units", () => [{ id: "unit-1", name: "Support", slug: "support" }]],
    // the batch counters (#3128); ahead of the list it shares a prefix with
    ["/orgs/org-1/budgets/spend", () => spend],
    ["/orgs/org-1/budgets", () => budgets],
    ["/orgs/org-1/rate-limits", () => rateLimits],
    ["/budgets/spend", () => spendFor(BUDGETS)],
    ["/budgets", () => BUDGETS],
    ["/rate-limits", () => RATE_LIMITS],
  ]);

/**
 * #2093: with "All scopes" picked the screen lists every cap the caller may
 * read across the org, grouped under the scope each belongs to and named, not
 * shown as a uuid. Scope types come in the picker's order, so the org leads.
 */
export const EveryCapAcrossTheOrgIsListedByScope: Story = {
  render: () => (
    <Harness fetchStub={orgWide(ORG_BUDGETS, ORG_RATE_LIMITS)}>
      <Limits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("$500.00")).toBeVisible();
    await pickOption(await canvas.findByLabelText("Scope type"), "All scopes");
    await expect(await canvas.findByText("$2,000.00")).toBeVisible();
    const headings = canvas.getAllByRole("heading", { level: 3 }).map((h) => h.textContent);
    await expect(headings).toEqual([
      "Organization Rolter",
      "Team Platform",
      "Project Gateway",
      "Virtual key ci runner",
      "Business unit Support",
      "Organization Rolter",
      "Team Platform",
      "Project Gateway",
    ]);
    await expect(canvas.queryByText("org-1")).toBeNull();
    await expect(canvas.queryByText("team-1")).toBeNull();
    await expect(canvas.queryByText("abcdef01")).toBeNull();
    // every card carries the counter the one org-wide read answered (#3128)
    await expect(canvas.queryByText(/spend so far is unavailable/i)).toBeNull();
    await expect(canvas.getAllByRole("progressbar")).toHaveLength(ORG_BUDGETS.length);
    // creating a cap still needs a scope to hang it off
    await expect(canvas.getByRole("button", { name: /add budget/i })).toBeDisabled();
    // each card names its own scope for a screen reader
    await expect(
      canvas.getByRole("button", {
        name: "Delete the monthly budget of $900.00 for Team Platform",
      }),
    ).toBeInTheDocument();
  },
};

/** An org-wide counter that cannot be read is unknown on every card, never zero (#3128). */
export const OrgWideSpendUnavailableIsUnknown: Story = {
  render: () => (
    <Harness
      fetchStub={orgWide(ORG_BUDGETS, ORG_RATE_LIMITS, {
        status: "unavailable",
        budgets: ORG_BUDGETS.map((budget) => ({
          id: budget.id,
          spent_usd: null,
          resets_at: null,
        })),
      })}
    >
      <Limits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("$500.00")).toBeVisible();
    await pickOption(await canvas.findByLabelText("Scope type"), "All scopes");
    await expect(await canvas.findByText("$2,000.00")).toBeVisible();
    await expect(await canvas.findAllByText(/spend so far is unavailable/i)).toHaveLength(
      ORG_BUDGETS.length,
    );
    await expect(canvas.queryByRole("progressbar")).toBeNull();
  },
};

/** The picked type narrows the org-wide list, and a picked scope narrows it to one scope. */
export const PickingATypeAndThenAScopeNarrowsTheList: Story = {
  render: () => (
    <Harness fetchStub={orgWide(ORG_BUDGETS, ORG_RATE_LIMITS)}>
      <Limits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("$500.00")).toBeVisible();
    await pickOption(await canvas.findByLabelText("Scope type"), "Team");
    await expect(await canvas.findByText("$900.00")).toBeVisible();
    await expect(canvas.queryByText("$2,000.00")).toBeNull();
    await expect(canvas.queryByText("$500.00")).toBeNull();
    await expect(canvas.getByText(/Showing every Team cap in the organization/)).toBeVisible();

    // a scope narrows to the per-scope read again, where a cap can be added
    await pickOption(await canvas.findByLabelText("Scope"), "Platform");
    await waitFor(() => expect(canvas.getByRole("button", { name: /add budget/i })).toBeEnabled());
  },
};

/** Nothing in the org is capped: the empty copy says so and offers no add, which needs a scope. */
export const AnOrgWithNoCapsSaysSo: Story = {
  render: () => (
    <Harness fetchStub={orgWide([], [])}>
      <Limits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await pickOption(await canvas.findByLabelText("Scope type"), "All scopes");
    await expect(await canvas.findByText("No budgets in this organization")).toBeVisible();
    await expect(canvas.getByText("No rate limits in this organization")).toBeVisible();
    await expect(
      canvas.queryByRole("button", { name: "Add budget", hidden: false }),
    ).toBeDisabled();
  },
};

/** A type with no caps reads differently from an org with none. */
export const ATypeWithNoCapsNamesTheFilter: Story = {
  render: () => (
    <Harness fetchStub={orgWide(ORG_BUDGETS, ORG_RATE_LIMITS)}>
      <Limits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("$500.00")).toBeVisible();
    await pickOption(await canvas.findByLabelText("Scope type"), "Customer");
    await expect(await canvas.findByText("No budgets for this scope type")).toBeVisible();
    await expect(canvas.getByText("No rate limits for this scope type")).toBeVisible();
  },
};

/** The org-wide read failing is said where the list would be, and retries. */
export const AFailedOrgWideReadSaysSoAndRetries: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input) => {
        const url = String(input);
        if (url.includes("/orgs/org-1/budgets"))
          return json({ error: { message: "store unavailable" } }, 500);
        if (url.includes("/budgets/spend")) return json(spendFor(BUDGETS));
        if (url.includes("/budgets")) return json(BUDGETS);
        if (url.includes("/virtual-keys")) return json(KEYS);
        return json(url.includes("/rate-limits") ? RATE_LIMITS : []);
      })}
    >
      <Limits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("$500.00")).toBeVisible();
    await pickOption(await canvas.findByLabelText("Scope type"), "All scopes");
    await expectLoadError(canvasElement, /failed to return budgets/i);
    await expectNoFalseEmpty(canvasElement, /no budgets/i);
  },
};

const PEOPLE = [
  {
    id: "user-1",
    email: "ada@example.com",
    display_name: "Ada Lovelace",
    is_superadmin: false,
    created_at: "2026-01-01T00:00:00Z",
  },
  {
    id: "user-2",
    email: "grace@example.com",
    display_name: null,
    is_superadmin: false,
    created_at: "2026-01-01T00:00:00Z",
  },
];

const USER_BUDGETS: BudgetRow[] = [
  {
    id: "budget-u1",
    scope_type: "user",
    scope_id: "user-1",
    within_org_id: "org-1",
    limit_usd: "40.00",
    period: "monthly",
    unpriced_policy: null,
    created_at: "2026-07-01T00:00:00Z",
  },
  {
    id: "budget-u2",
    scope_type: "user",
    scope_id: "user-2",
    within_org_id: null,
    within_project_id: null,
    limit_usd: "10.00",
    period: "monthly",
    unpriced_policy: null,
    created_at: "2026-07-01T00:00:00Z",
  },
];

// the rows exist so the screen shows its one header button; the empty state
// repeats it as a call to action
const USER_LIMITS: RateLimitRow[] = [
  {
    id: "limit-u1",
    scope_type: "user",
    scope_id: "user-1",
    within_org_id: "org-1",
    rpm: 30,
    tpm: null,
    created_at: "2026-07-01T00:00:00Z",
  },
  {
    id: "limit-u2",
    scope_type: "user",
    scope_id: "user-2",
    within_org_id: "org-1",
    rpm: 30,
    tpm: null,
    created_at: "2026-07-01T00:00:00Z",
  },
];

const withPeople = recording(
  routes([
    ["/users", () => PEOPLE],
    ["/virtual-keys", () => KEYS],
    ["/budgets/spend", () => spendFor(USER_BUDGETS)],
    ["/budgets", () => USER_BUDGETS],
    ["/rate-limits", () => []],
  ]),
);

/**
 * #3282: a user row names the person, by the name they go by or else the
 * address they sign in with, and says what the cap counts, wherever the scope
 * is named — never the uuid.
 */
export const UserRowsNameThePerson: Story = {
  render: () => (
    <Harness fetchStub={withPeople.stub}>
      <Limits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await pickOption(await canvas.findByLabelText("Scope type"), "All scopes");
    await expect(
      await canvas.findByText("User Ada Lovelace (keys they minted in one organization)"),
    ).toBeVisible();
    await expect(canvas.getByText("User grace@example.com (every key they minted)")).toBeVisible();
    await expect(canvas.queryByText(/user-1/)).not.toBeInTheDocument();
  },
};

const createsUserCaps = recording(
  scoped(async (input, init) => {
    const url = String(input);
    if (init?.method === "POST") return json(USER_BUDGETS[0], 201);
    if (url.includes("/users")) return json(PEOPLE);
    if (url.includes("/virtual-keys")) return json(KEYS);
    if (url.includes("/budgets")) return budgetRead(url, USER_BUDGETS);
    return json(USER_LIMITS);
  }),
);

/**
 * #3282: an admin picks the person, then the org the cap is narrowed to
 * (which decides who may manage it), and the create carries both. A cap that
 * counts every key is offered only to a superadmin.
 */
export const CreatesAUserBudgetNarrowedToAnOrg: Story = {
  render: () => (
    <Harness fetchStub={createsUserCaps.stub}>
      <Limits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await pickOption(await canvas.findByLabelText("Scope type"), "User");
    await pickOption(await canvas.findByLabelText("Scope"), /^Ada Lovelace/);
    await clickWhenEnabled(canvasElement, /add budget/i);
    const form = sheet();
    // the org is already picked, and, with no account behind this open-mode
    // session to refuse it, the un-narrowed reach is on offer
    await waitFor(() => expect(within(form).getByLabelText("Organization")).toHaveValue("Rolter"));
    const reach = await openOptions(within(form).getByLabelText("Counts"));
    await expect(
      within(reach).getByRole("option", { name: "Every key they minted" }),
    ).toBeVisible();
    await userEvent.keyboard("{Escape}");
    const limit = within(form).getByLabelText("Limit (USD)");
    await userEvent.clear(limit);
    await userEvent.type(limit, "40");
    await userEvent.click(within(form).getByRole("button", { name: "Create" }));
    const body = await createsUserCaps.expectSentBody<Record<string, unknown>>(
      "POST",
      "/api/v1/budgets",
    );
    await expect(body).toMatchObject({
      scope_type: "user",
      scope_id: "user-1",
      within_org_id: "org-1",
      limit_usd: "40",
    });
    await expect(body).not.toHaveProperty("within_project_id");
  },
};

const createsUserLimit = recording(
  scoped(async (input, init) => {
    const url = String(input);
    if (init?.method === "POST") return json({}, 201);
    if (url.includes("/users")) return json(PEOPLE);
    if (url.includes("/virtual-keys")) return json(KEYS);
    if (url.includes("/budgets")) return budgetRead(url, USER_BUDGETS);
    return json(USER_LIMITS);
  }),
);

/** #3282: a per-user rate limit narrowed to one project carries only the project. */
export const CreatesAUserRateLimitNarrowedToAProject: Story = {
  render: () => (
    <Harness fetchStub={createsUserLimit.stub}>
      <Limits />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await pickOption(await canvas.findByLabelText("Scope type"), "User");
    await pickOption(await canvas.findByLabelText("Scope"), "grace@example.com");
    await clickWhenEnabled(canvasElement, /add rate limit/i);
    const form = sheet();
    await pickOption(await within(form).findByLabelText("Counts"), "Keys minted in one project");
    // a project cap cannot be created before a project is picked
    await expect(within(form).getByRole("button", { name: "Create" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    await pickOption(within(form).getByLabelText("Project"), "Gateway");
    await userEvent.type(within(form).getByLabelText("Requests per minute (optional)"), "60");
    await userEvent.click(within(form).getByRole("button", { name: "Create" }));
    const body = await createsUserLimit.expectSentBody<Record<string, unknown>>(
      "POST",
      "/api/v1/rate-limits",
    );
    await expect(body).toMatchObject({
      scope_type: "user",
      scope_id: "user-2",
      within_project_id: "project-1",
      rpm: 60,
    });
    await expect(body).not.toHaveProperty("within_org_id");
  },
};

const MEMBER = { id: "u-member", email: "ada@acme.dev", is_superadmin: false };

// signed in the way a login leaves the browser, minus the token, so the
// provider knows the account without asking /auth/me for it
function SignedInMember({ children }: { children: React.ReactNode }) {
  React.useState(() => {
    localStorage.setItem("rolter.session.email", MEMBER.email);
    localStorage.setItem("rolter.session.user", JSON.stringify(MEMBER));
    localStorage.removeItem("rolter.session.token");
  });
  React.useEffect(
    () => () => {
      localStorage.removeItem("rolter.session.email");
      localStorage.removeItem("rolter.session.user");
    },
    [],
  );
  return <AuthProvider>{children}</AuthProvider>;
}

/**
 * #3282: only a superadmin may leave a user cap un-narrowed, so a signed-in
 * account that is not one is offered the org and the project and not the
 * "every key" reach. (An open-mode session has no account and is offered all
 * three, as the story above shows.)
 */
export const ASignedInNonSuperadminIsNotOfferedEveryKey: Story = {
  render: () => (
    <SignedInMember>
      <Harness fetchStub={createsUserCaps.stub}>
        <Limits />
      </Harness>
    </SignedInMember>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await pickOption(await canvas.findByLabelText("Scope type"), "User");
    await pickOption(await canvas.findByLabelText("Scope"), /^Ada Lovelace/);
    await clickWhenEnabled(canvasElement, /add budget/i);
    const form = sheet();
    const reach = await openOptions(await within(form).findByLabelText("Counts"));
    await expect(within(reach).getByRole("option", { name: /one organization/ })).toBeVisible();
    await expect(within(reach).queryByRole("option", { name: "Every key they minted" })).toBeNull();
  },
};
