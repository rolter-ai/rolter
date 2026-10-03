import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import Preferences from "./Preferences";
import {
  Harness,
  Toasted,
  expectLoadError,
  expectSkeleton,
  expectToast,
  json,
  pending,
  recording,
  scoped,
  type FetchStub,
} from "./story-harness";
import type { PreferencesResponse } from "@/lib/api";
import en from "@/lib/i18n/locales/en.json";

// the screen over `/api/v1/me/preferences`. PUT replaces the whole document and
// refuses the computed `effective_default_scope`, so the stories that save
// assert the body: all six keys, nothing else

const DOCUMENT: PreferencesResponse = {
  language: null,
  default_org_id: "org-1",
  default_team_id: "team-1",
  default_project_id: "project-1",
  default_playground_model: "gpt-4o",
  chart_time_zone: "Europe/Berlin",
  effective_default_scope: { org_id: "org-1", team_id: "team-1", project_id: "project-1" },
};

const KEYS = [
  "chart_time_zone",
  "default_org_id",
  "default_playground_model",
  "default_project_id",
  "default_team_id",
  "language",
];

const answer =
  (doc: PreferencesResponse): FetchStub =>
  async () =>
    json(doc);

function Screen({ fetchStub }: { fetchStub: FetchStub }) {
  return (
    <Harness fetchStub={fetchStub}>
      <Toasted>
        <Preferences />
      </Toasted>
    </Harness>
  );
}

const meta = {
  title: "Screens/Preferences",
  component: Preferences,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof Preferences>;

export default meta;
type Story = StoryObj<typeof meta>;

/** What the account saved, on the screen. Save waits for an edit. */
export const Loaded: Story = {
  render: () => <Screen fetchStub={scoped(answer(DOCUMENT))} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() =>
      expect(canvas.getByLabelText(en.pages.preferences.playground.label)).toHaveValue("gpt-4o"),
    );
    await expect(canvas.getByRole("radio", { name: "Browser default" })).toBeChecked();
    await expect(canvas.getByLabelText(en.pages.preferences.zone.label)).toHaveValue(
      "Europe/Berlin",
    );
    await expect(canvas.getByRole("button", { name: en.common.saveChanges })).toBeDisabled();
  },
};

/** Nothing saved: every key is the default, which is a real answer and not an empty state. */
export const Defaults: Story = {
  render: () => (
    <Screen
      fetchStub={scoped(
        answer({
          language: null,
          default_org_id: null,
          default_team_id: null,
          default_project_id: null,
          default_playground_model: null,
          chart_time_zone: null,
          effective_default_scope: null,
        }),
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() =>
      expect(canvas.getByLabelText(en.pages.preferences.playground.label)).toHaveValue(""),
    );
    await expect(canvas.queryByText(en.pages.preferences.scope.lost)).toBeNull();
  },
};

/**
 * A save sends the document as the server holds it with the edit laid over it:
 * all six keys, and not the computed scope, which the server refuses.
 */
export const SavesTheMergedDocument: Story = {
  render: () => {
    const recorder = recording(
      scoped(async (_input, init) =>
        json(
          init?.method === "PUT"
            ? { ...DOCUMENT, default_playground_model: "claude-sonnet-5" }
            : DOCUMENT,
        ),
      ),
    );
    return <Screen fetchStub={recorder.stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const model = await canvas.findByLabelText(en.pages.preferences.playground.label);
    await waitFor(() => expect(model).toHaveValue("gpt-4o"));
    await userEvent.clear(model);
    await userEvent.type(model, "claude-sonnet-5");
    await userEvent.click(canvas.getByRole("button", { name: en.common.saveChanges }));
    await expectToast(canvasElement, new RegExp(en.toast.saved));
    // the form now holds what the server answered, so there is nothing left to save
    await waitFor(() =>
      expect(canvas.getByRole("button", { name: en.common.saveChanges })).toBeDisabled(),
    );
  },
};

/** The body of that save, asserted on its own so a failure names the key that was wrong. */
export const SendsEverySavedKey: Story = {
  render: () => {
    // the first read is what the form loads; the document changes under it
    // before the save, and the save must lay its edit over the *newer* one
    let reads = 0;
    const recorder = recording(
      scoped(async (_input, init) => {
        if (init?.method === "PUT") return json(DOCUMENT);
        reads += 1;
        return json(reads === 1 ? DOCUMENT : { ...DOCUMENT, chart_time_zone: "Asia/Tokyo" });
      }),
    );
    (globalThis as { __recorder?: typeof recorder }).__recorder = recorder;
    return <Screen fetchStub={recorder.stub} />;
  },
  play: async ({ canvasElement }) => {
    const recorder = (globalThis as { __recorder?: ReturnType<typeof recording> }).__recorder!;
    const canvas = within(canvasElement);
    const model = await canvas.findByLabelText(en.pages.preferences.playground.label);
    await waitFor(() => expect(model).toHaveValue("gpt-4o"));
    await userEvent.type(model, "-mini");
    await userEvent.click(canvas.getByRole("button", { name: en.common.saveChanges }));
    const body = await recorder.expectSentBody<Record<string, unknown>>("PUT", "/me/preferences");
    await expect(Object.keys(body).sort()).toEqual(KEYS);
    await expect(body).not.toHaveProperty("effective_default_scope");
    await expect(body.default_playground_model).toBe("gpt-4o-mini");
    // the zone another tab set meanwhile survives the save
    await expect(body.chart_time_zone).toBe("Asia/Tokyo");
  },
};

/** Picking an org clears the team and project under it, so a mismatched pair is never saved. */
export const ScopeCascades: Story = {
  render: () => <Screen fetchStub={scoped(answer(DOCUMENT))} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const org = await canvas.findByLabelText("Org");
    await waitFor(() => expect(org).toHaveValue("Rolter"));
    const project = canvas.getByLabelText("Project");
    await waitFor(() => expect(project).toHaveValue("Gateway"));
    // clearing the org leaves no team to hold a project under
    await userEvent.click(canvas.getAllByRole("button", { name: /clear/i })[0]);
    await waitFor(() => expect(canvas.getByLabelText("Team")).toBeDisabled());
    await expect(canvas.getByLabelText("Project")).toBeDisabled();
  },
};

/**
 * The saved default names a scope the account lost access to: the server
 * computed no effective scope while a raw id is still stored, and the screen
 * says the dashboard will not open there rather than showing the dead pick as
 * if it were live.
 */
export const LostDefaultScope: Story = {
  render: () => (
    <Screen fetchStub={scoped(answer({ ...DOCUMENT, effective_default_scope: null }))} />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(en.pages.preferences.scope.lost)).toBeVisible();
  },
};

/** The server refuses a field: the reason sits under that field, and the edit clears it. */
export const ValidationError: Story = {
  render: () => (
    <Screen
      fetchStub={scoped(async (_input, init) =>
        init?.method === "PUT"
          ? json(
              {
                error: {
                  message: "default_playground_model must be at most 200 characters",
                  code: "bad_request",
                },
              },
              400,
            )
          : json(DOCUMENT),
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const model = await canvas.findByLabelText(en.pages.preferences.playground.label);
    await waitFor(() => expect(model).toHaveValue("gpt-4o"));
    await userEvent.type(model, "x");
    await userEvent.click(canvas.getByRole("button", { name: en.common.saveChanges }));
    const reason = await canvas.findByText(/must be at most 200 characters/);
    await expect(model).toHaveAttribute("aria-invalid", "true");
    await expect(model.getAttribute("aria-describedby")).toBe(reason.id);
    // the edit stays, so the person can fix it without retyping
    await expect(model).toHaveValue("gpt-4ox");
    await userEvent.type(model, "y");
    await waitFor(() => expect(canvas.queryByText(/must be at most 200 characters/)).toBeNull());
  },
};

/** The read is on its way: a skeleton, and no form to edit with nothing in it. */
export const Loading: Story = {
  render: () => <Screen fetchStub={pending} />,
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
    await expect(
      within(canvasElement).queryByLabelText(en.pages.preferences.playground.label),
    ).toBeNull();
  },
};

/** The control plane failed the read. */
export const LoadFailed: Story = {
  render: () => (
    <Screen fetchStub={scoped(async () => json({ error: { message: "boom" } }, 500))} />
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, new RegExp(en.errors.resources.preferences, "i"));
    await expect(
      within(canvasElement).queryByLabelText(en.pages.preferences.playground.label),
    ).toBeNull();
  },
};

/** Open mode has no accounts, so there is nobody to hold preferences; sign-in would not help. */
export const OpenMode: Story = {
  render: () => (
    <Screen
      fetchStub={scoped(async () =>
        json({ error: { message: "no session", code: "open_mode_no_session" } }, 401),
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /admin token/i);
    await expect(
      within(canvasElement).queryByLabelText(en.pages.preferences.playground.label),
    ).toBeNull();
  },
};

/** The zone list is the engine's own, searchable, and a pick makes the form dirty. */
export const PicksAZone: Story = {
  render: () => <Screen fetchStub={scoped(answer(DOCUMENT))} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const zone = await canvas.findByLabelText(en.pages.preferences.zone.label);
    await waitFor(() => expect(zone).toHaveValue("Europe/Berlin"));
    // the list is the engine's own and long, so it is filtered before picking
    await userEvent.click(zone);
    await userEvent.keyboard("{Control>}a{/Control}Asia/Tokyo");
    await userEvent.click(await within(document.body).findByRole("option", { name: "Asia/Tokyo" }));
    await expect(zone).toHaveValue("Asia/Tokyo");
    await expect(canvas.getByRole("button", { name: en.common.saveChanges })).toBeEnabled();
  },
};
