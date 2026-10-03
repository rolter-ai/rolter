import type { Meta, StoryObj } from "@storybook/react-vite";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { SavedViews } from "./SavedViews";
import {
  Harness,
  expectEmptyState,
  expectLoadError,
  expectNoFalseEmpty,
  expectSkeleton,
  json,
  pending,
  recording,
  scoped,
  type FetchStub,
} from "@/pages/story-harness";
import type { SavedView, SavedViewFilters } from "@/lib/api";

const view = (over: Partial<SavedView> & Pick<SavedView, "id" | "name">): SavedView => ({
  surface: "llm_logs",
  filters: {},
  effective_filters: {},
  unavailable: [],
  created_at: "2026-09-01T09:00:00Z",
  updated_at: "2026-09-01T09:00:00Z",
  ...over,
});

const ERRORS_ON_GPT = view({
  id: "11111111-1111-4111-8111-111111111111",
  name: "Errors on gpt-4o this week",
  filters: { window: "7d", status: "error", model: "gpt-4o" },
  effective_filters: { window: "7d", status: "error", model: "gpt-4o" },
});

const KEY = "22222222-2222-4222-8222-222222222222";
const UNIT = "33333333-3333-4333-8333-333333333333";
const CUSTOMER_A = "44444444-4444-4444-8444-444444444444";
const CUSTOMER_B = "55555555-5555-4555-8555-555555555555";

/** a view that names a key and two customers the account can no longer read */
const OUTLIVED_ACCESS = view({
  id: "66666666-6666-4666-8666-666666666666",
  name: "Platform spend",
  filters: {
    window: "30d",
    key: KEY,
    business_unit: [UNIT],
    customer: [CUSTOMER_A, CUSTOMER_B],
  },
  effective_filters: { window: "30d", business_unit: [UNIT] },
  unavailable: [
    { filter: "key", id: KEY },
    { filter: "customer", id: CUSTOMER_A },
    { filter: "customer", id: CUSTOMER_B },
  ],
});

/** the control plane's answers, over a list that the stub keeps */
function savedViewsApi(initial: SavedView[], onWrite?: (method: string) => Response | null) {
  let list = [...initial];
  return recording(
    scoped(async (input, init) => {
      const url = new URL(String(input), "http://localhost");
      if (!url.pathname.startsWith("/api/v1/me/saved-views")) return json([]);
      const method = (init?.method ?? "GET").toUpperCase();
      const refused = method === "GET" ? null : (onWrite?.(method) ?? null);
      if (refused) return refused;
      const id = url.pathname.split("/")[5];
      if (method === "POST") {
        const body = JSON.parse(String(init?.body));
        const created = view({ id: "77777777-7777-4777-8777-777777777777", ...body });
        created.effective_filters = body.filters;
        list = [...list, created];
        return json(created, 201);
      }
      if (method === "PATCH") {
        const body = JSON.parse(String(init?.body));
        list = list.map((v) => (v.id === id ? { ...v, ...body } : v));
        return json(list.find((v) => v.id === id));
      }
      if (method === "DELETE") {
        list = list.filter((v) => v.id !== id);
        return json(null, 204);
      }
      return json(list);
    }),
  );
}

/** the screen's side: the filters it has applied, and what an apply handed it */
function Host({
  stub,
  current = { window: "7d", status: "error", model: "gpt-4o" },
}: {
  stub: FetchStub;
  current?: SavedViewFilters;
}) {
  const [applied, setApplied] = React.useState<SavedViewFilters | null>(null);
  return (
    <Harness fetchStub={stub}>
      <SavedViews surface="llm_logs" current={current} onApply={setApplied} />
      <span hidden data-testid="applied">
        {applied ? JSON.stringify(applied) : "none"}
      </span>
    </Harness>
  );
}

const meta = {
  title: "Overlays/SavedViews",
  component: SavedViews,
  parameters: { layout: "centered" },
} satisfies Meta<typeof SavedViews>;
export default meta;
type Story = StoryObj<typeof meta>;

// the sheet portals onto the body
const screen = () => within(document.body);

const openSheet = async (canvasElement: HTMLElement) => {
  await userEvent.click(await within(canvasElement).findByRole("button", { name: "Saved views" }));
  return screen().findByRole("dialog", { name: "Saved views" });
};

const base = savedViewsApi([ERRORS_ON_GPT]);

export const Listed: Story = {
  args: { surface: "llm_logs", current: {}, onApply: () => {} },
  render: () => <Host stub={base.stub} />,
  play: async ({ canvasElement }) => {
    const sheet = within(await openSheet(canvasElement));
    const list = await sheet.findByRole("list", { name: "Your saved views" });
    await expect(within(list).getByText("Errors on gpt-4o this week")).toBeVisible();
    await expect(
      sheet.getByRole("button", { name: "Rename Errors on gpt-4o this week" }),
    ).toBeVisible();
    await expect(
      sheet.getByRole("button", { name: "Delete view Errors on gpt-4o this week" }),
    ).toBeVisible();
  },
};

const saving = savedViewsApi([]);

/** saving sends the filters applied right now, under the name typed */
export const SavesTheCurrentFilters: Story = {
  args: { surface: "llm_logs", current: {}, onApply: () => {} },
  render: () => <Host stub={saving.stub} />,
  play: async ({ canvasElement }) => {
    const sheet = within(await openSheet(canvasElement));
    await userEvent.type(await sheet.findByLabelText("Save the current filters as"), "Mine");
    await userEvent.click(sheet.getByRole("button", { name: "Save view" }));
    const body = await saving.expectSentBody<{
      surface: string;
      name: string;
      filters: Record<string, unknown>;
    }>("POST", "/api/v1/me/saved-views");
    await expect(body).toEqual({
      surface: "llm_logs",
      name: "Mine",
      filters: { window: "7d", status: "error", model: "gpt-4o" },
    });
    // the new view is in the list, and the field is ready for the next name
    await expect(await sheet.findByRole("list", { name: "Your saved views" })).toBeVisible();
    await waitFor(() =>
      expect(sheet.getByLabelText("Save the current filters as")).toHaveValue(""),
    );
  },
};

const applying = savedViewsApi([OUTLIVED_ACCESS]);

/** applying hands over `effective_filters`; the stored `filters` never leave the list */
export const AppliesTheEffectiveFilters: Story = {
  args: { surface: "llm_logs", current: {}, onApply: () => {} },
  render: () => <Host stub={applying.stub} />,
  play: async ({ canvasElement }) => {
    const sheet = within(await openSheet(canvasElement));
    await userEvent.click(await sheet.findByRole("button", { name: "Apply Platform spend" }));
    const applied = within(canvasElement).getByTestId("applied");
    await waitFor(() =>
      expect(JSON.parse(applied.textContent ?? "")).toEqual({
        window: "30d",
        business_unit: [UNIT],
      }),
    );
    // the sheet closes once the view is on the screen
    await waitFor(() => expect(screen().queryByRole("dialog")).toBeNull());
  },
};

/**
 * What the control plane left out is counted, never named: the ids are no longer
 * readable, so there is no name to show.
 */
export const SaysWhatItAppliedWithout: Story = {
  args: { surface: "llm_logs", current: {}, onApply: () => {} },
  render: () => <Host stub={applying.stub} />,
  play: async ({ canvasElement }) => {
    const sheet = within(await openSheet(canvasElement));
    await userEvent.click(await sheet.findByRole("button", { name: "Apply Platform spend" }));
    const notice = await within(canvasElement).findByRole("status");
    await expect(notice).toHaveTextContent("Applied without: 1 key, 2 customers.");
    await expect(notice).not.toHaveTextContent(CUSTOMER_A);
    await userEvent.click(within(notice).getByRole("button", { name: "Dismiss" }));
    await waitFor(() => expect(within(canvasElement).queryByRole("status")).toBeNull());
  },
};

const noNotice = savedViewsApi([ERRORS_ON_GPT]);

export const AViewWithNothingDroppedSaysNothing: Story = {
  args: { surface: "llm_logs", current: {}, onApply: () => {} },
  render: () => <Host stub={noNotice.stub} />,
  play: async ({ canvasElement }) => {
    const sheet = within(await openSheet(canvasElement));
    await userEvent.click(
      await sheet.findByRole("button", { name: "Apply Errors on gpt-4o this week" }),
    );
    await waitFor(() =>
      expect(within(canvasElement).getByTestId("applied")).toHaveTextContent('"status":"error"'),
    );
    await expect(within(canvasElement).queryByRole("status")).toBeNull();
  },
};

const duplicate = savedViewsApi([ERRORS_ON_GPT], (method) =>
  method === "POST"
    ? json({ error: { message: "a saved view with this name already exists" } }, 409)
    : null,
);

export const ADuplicateNameIsRefused: Story = {
  args: { surface: "llm_logs", current: {}, onApply: () => {} },
  render: () => <Host stub={duplicate.stub} />,
  play: async ({ canvasElement }) => {
    const sheet = within(await openSheet(canvasElement));
    const field = await sheet.findByLabelText("Save the current filters as");
    await userEvent.type(field, "Errors on gpt-4o this week");
    await userEvent.click(sheet.getByRole("button", { name: "Save view" }));
    const message = await sheet.findByText(/A view with this name already exists/);
    await expect(message).toBeVisible();
    await expect(field).toHaveAttribute("aria-invalid", "true");
    // the typed name stays for the correction, and editing it clears the refusal
    await expect(field).toHaveValue("Errors on gpt-4o this week");
    await userEvent.type(field, " 2");
    await waitFor(() =>
      expect(sheet.queryByText(/A view with this name already exists/)).toBeNull(),
    );
  },
};

const empty = savedViewsApi([]);

export const Empty: Story = {
  args: { surface: "llm_logs", current: {}, onApply: () => {} },
  render: () => <Host stub={empty.stub} />,
  play: async ({ canvasElement }) => {
    await openSheet(canvasElement);
    await expectEmptyState(document.body, /No saved views yet/, /Name a view/);
    // the call to action leads to the field that creates the first one
    await userEvent.click(screen().getByRole("button", { name: "Name a view" }));
    await waitFor(() =>
      expect(screen().getByLabelText("Save the current filters as")).toHaveFocus(),
    );
  },
};

export const Loading: Story = {
  args: { surface: "llm_logs", current: {}, onApply: () => {} },
  render: () => <Host stub={pending} />,
  play: async ({ canvasElement }) => {
    await openSheet(canvasElement);
    await expectSkeleton(document.body);
    await expectNoFalseEmpty(document.body, /No saved views yet/);
  },
};

const failingRead: FetchStub = scoped(async () =>
  json({ error: { message: "store unreachable" } }, 500),
);

export const Failed: Story = {
  args: { surface: "llm_logs", current: {}, onApply: () => {} },
  render: () => <Host stub={failingRead} />,
  play: async ({ canvasElement }) => {
    await openSheet(canvasElement);
    await expectLoadError(document.body, /your saved views/i);
    await expectNoFalseEmpty(document.body, /No saved views yet/);
  },
};

const noSession = scoped(async () =>
  json({ error: { code: "open_mode_no_session", message: "no session in open mode" } }, 401),
);

/** open mode has no accounts, so the presets cannot be served to anyone */
export const OpenModeHasNoSavedViews: Story = {
  args: { surface: "llm_logs", current: {}, onApply: () => {} },
  render: () => <Host stub={noSession} />,
  play: async ({ canvasElement }) => {
    await openSheet(canvasElement);
    await expectLoadError(document.body, /no admin token|open mode|sign/i);
    await expect(screen().queryByLabelText("Save the current filters as")).toBeNull();
  },
};

const renaming = savedViewsApi([ERRORS_ON_GPT]);

/** a rename sends the name alone: a `filters` value would replace the stored set */
export const RenamesWithoutTouchingTheFilters: Story = {
  args: { surface: "llm_logs", current: {}, onApply: () => {} },
  render: () => <Host stub={renaming.stub} />,
  play: async ({ canvasElement }) => {
    const sheet = within(await openSheet(canvasElement));
    await userEvent.click(
      await sheet.findByRole("button", { name: "Rename Errors on gpt-4o this week" }),
    );
    const field = sheet.getByLabelText("New name for Errors on gpt-4o this week");
    await userEvent.clear(field);
    await userEvent.type(field, "Weekly errors");
    await userEvent.click(sheet.getByRole("button", { name: "Save" }));
    const body = await renaming.expectSentBody<Record<string, unknown>>(
      "PATCH",
      `/api/v1/me/saved-views/${ERRORS_ON_GPT.id}`,
    );
    await expect(body).toEqual({ name: "Weekly errors" });
    await expect(await sheet.findByText("Weekly errors")).toBeVisible();
  },
};

const deleting = savedViewsApi([ERRORS_ON_GPT]);

export const DeletingAsksFirst: Story = {
  args: { surface: "llm_logs", current: {}, onApply: () => {} },
  render: () => <Host stub={deleting.stub} />,
  play: async ({ canvasElement }) => {
    const sheet = within(await openSheet(canvasElement));
    await userEvent.click(
      await sheet.findByRole("button", { name: "Delete view Errors on gpt-4o this week" }),
    );
    const dialog = await screen().findByRole("dialog", { name: /Delete the view/ });
    await expect(
      within(dialog).getByText(/Delete the view Errors on gpt-4o this week\?/),
    ).toBeVisible();
    deleting.expectNotSent("DELETE", "/api/v1/me/saved-views");
    await userEvent.click(within(dialog).getByRole("button", { name: "Delete view" }));
    await deleting.expectSent("DELETE", `/api/v1/me/saved-views/${ERRORS_ON_GPT.id}`);
    await expectEmptyState(document.body, /No saved views yet/);
  },
};
