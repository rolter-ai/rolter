import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import ModelSettings from "./ModelSettings";
import {
  expectForbidden,
  expectSkeleton,
  expectToast,
  Harness as ScreenHarness,
  expectRetryRecovers,
  failingThenRecovering,
  json,
  recording,
  Toasted,
  type FetchStub,
  type StoryRole,
} from "./story-harness";
import type { ModelDefaultsDto } from "@/lib/api";
import en from "@/lib/i18n/locales/en.json";
import ru from "@/lib/i18n/locales/ru.json";
import { phoneFits } from "@/lib/story-viewport";

const BASE: ModelDefaultsDto = {
  enabled: false,
  default_model: null,
  default_temperature: null,
  default_top_p: null,
  default_max_tokens: null,
  updated_at: "2026-08-05T12:00:00Z",
};

const CONFIGURED: ModelDefaultsDto = {
  ...BASE,
  enabled: true,
  default_model: "gpt-4o-mini",
  default_temperature: 0.7,
  default_max_tokens: 2048,
};

const MODELS = [
  { model: "gpt-4o-mini", strategy: "round_robin", targets: 2, source: "config" },
  { model: "claude-sonnet", strategy: "least_latency", targets: 1, source: "db" },
  { model: "team-a/gpt-4o", strategy: "round_robin", targets: 3, source: "db" },
];

/** the defaults route answers `defaults`, the model list answers `models` */
const answering =
  (defaults: unknown, models: unknown = MODELS, modelsStatus = 200): FetchStub =>
  async (input, init) => {
    if (String(input).includes("/api/v1/models")) return json(models, modelsStatus);
    if (init?.method === "PUT")
      return json({ ...(defaults as object), ...JSON.parse(String(init.body)) });
    return json(defaults);
  };

/**
 * The screen under the shared fetch-stub harness, with a role to render as.
 *
 * `role` is what a story needs to mount a `CapabilityProvider` at all: with no
 * provider above it `can()` answers "unknown", the `superadminOnly` wrapper
 * never blocks, and a story can only reach the 403 by stubbing one — which
 * tests the screen's own error path rather than the gate (#1606).
 */
function Harness({ fetchStub, role }: { fetchStub: FetchStub; role?: StoryRole }) {
  return (
    <ScreenHarness fetchStub={fetchStub} role={role}>
      <Toasted>
        <ModelSettings />
      </Toasted>
    </ScreenHarness>
  );
}

// every PUT body the screen sent, for the stories that assert on it
const sent: unknown[] = [];
const recordingStub: FetchStub = async (input, init) => {
  if (init?.method === "PUT") sent.push(init.body);
  return answering({ ...BASE, enabled: true })(input, init);
};

const meta = {
  title: "Screens/ModelSettings",
  component: ModelSettings,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof ModelSettings>;

export default meta;
type Story = StoryObj<typeof meta>;

// the shipped default: nothing set, nothing applied
export const Empty: Story = {
  render: () => <Harness fetchStub={answering(BASE)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("inactive")).toBeVisible());
  },
};

export const Configured: Story = {
  render: () => <Harness fetchStub={answering(CONFIGURED)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("active")).toBeVisible());
    await expect(canvas.getByLabelText("Default model")).toHaveValue("gpt-4o-mini");
    // the pointer to Performance Tuning is a link, and the screen says which
    // value wins when a route sets the same parameter
    await expect(canvas.getByRole("link", { name: /Performance Tuning/ })).toHaveAttribute(
      "href",
      "/performance",
    );
    await expect(canvas.getByText(/a route's own params/)).toBeVisible();
  },
};

export const Loading: Story = {
  render: () => <Harness fetchStub={() => new Promise<Response>(() => {})} />,
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
  },
};

// the placeholder says what an empty value does: there is no provider default
// for the model, requests without one are rejected
export const EmptyPlaceholder: Story = {
  render: () => <Harness fetchStub={answering({ ...BASE, enabled: true })} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const field = await canvas.findByLabelText("Default model");
    await expect(field.getAttribute("placeholder")).toMatch(/reject requests/i);
    await expect(field).not.toHaveAttribute("placeholder", "provider default");
  },
};

// the field offers the served models and sends the one picked
export const PicksAModel: Story = {
  render: () => <Harness fetchStub={recordingStub} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const field = await canvas.findByLabelText("Default model");
    await userEvent.click(field);
    await userEvent.click(
      await within(document.body).findByRole("option", { name: "claude-sonnet" }),
    );
    await userEvent.click(canvas.getByRole("button", { name: "Save changes" }));
    await expectToast(canvasElement, /model settings updated/i);
    await expect(JSON.parse(String(sent[sent.length - 1]))).toMatchObject({
      default_model: "claude-sonnet",
    });
  },
};

// a group address is not a route, so it is typed rather than picked
export const AcceptsATypedGroupAddress: Story = {
  render: () => <Harness fetchStub={recordingStub} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const field = await canvas.findByLabelText("Default model");
    await userEvent.type(field, "edge/gpt-4o");
    await userEvent.click(
      await within(document.body).findByRole("option", { name: /edge\/gpt-4o/ }),
    );
    await userEvent.click(canvas.getByRole("button", { name: "Save changes" }));
    await expectToast(canvasElement, /model settings updated/i);
    await expect(JSON.parse(String(sent[sent.length - 1]))).toMatchObject({
      default_model: "edge/gpt-4o",
    });
  },
};

// no routes yet: nothing to pick, and the typed address still works
export const NoModelsServed: Story = {
  render: () => <Harness fetchStub={answering({ ...BASE, enabled: true }, [])} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const field = await canvas.findByLabelText("Default model");
    await userEvent.click(field);
    await expect(await within(document.body).findByText(/No options match/)).toBeVisible();
  },
};

// the model list is still on its way: the rest of the screen is usable
export const ModelsLoading: Story = {
  render: () => (
    <Harness
      fetchStub={(input, init) =>
        String(input).includes("/api/v1/models")
          ? new Promise<Response>(() => {})
          : answering({ ...BASE, enabled: true })(input, init)
      }
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByLabelText("Default model")).toBeEnabled();
  },
};

// the model list failed: say so, and keep the field typeable
export const ModelsFailedToLoad: Story = {
  render: () => (
    <Harness
      fetchStub={answering({ ...BASE, enabled: true }, { error: { message: "boom" } }, 500)}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(/Couldn’t load the list of models/)).toBeVisible();
    await expect(canvas.getByLabelText("Default model")).toBeEnabled();
  },
};

// a non-superadmin principal gets 403
export const Forbidden: Story = {
  render: () => <Harness fetchStub={async () => json({ error: { message: "forbidden" } }, 403)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() =>
      expect(canvas.getByText(/You do not have access to model settings/)).toBeVisible(),
    );
  },
};

// enabled with nothing filled in is a no-op, and the screen says so rather
// than implying traffic is being changed
export const EnabledWithNoDefaults: Story = {
  render: () => <Harness fetchStub={answering({ ...BASE, enabled: true })} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("inactive")).toBeVisible());
    await expect(canvas.getByText(/No defaults are set yet/)).toBeVisible();
  },
};

// the value is forwarded to the provider, so an out-of-range one is blocked
// before it can fail every request
export const RejectsAnOutOfRangeTemperature: Story = {
  render: () => <Harness fetchStub={answering(CONFIGURED)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const temperature = await canvas.findByLabelText("Temperature");
    await userEvent.clear(temperature);
    await userEvent.type(temperature, "3");
    await waitFor(() => expect(temperature).toHaveAttribute("aria-invalid", "true"));
    await expect(temperature).toHaveAccessibleDescription("Temperature must be between 0 and 2.");
    await expect(canvas.getByRole("button", { name: "Save changes" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
  },
};

// every failing field is marked at once, each with its own message, and a
// press on Save moves focus to the first one instead of doing nothing (#2651)
export const MarksEveryInvalidField: Story = {
  render: () => <Harness fetchStub={answering(CONFIGURED)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const topP = await canvas.findByLabelText("Top-p");
    const maxTokens = canvas.getByLabelText("Max tokens");
    await userEvent.clear(topP);
    await userEvent.type(topP, "2");
    await userEvent.clear(maxTokens);
    await userEvent.type(maxTokens, "0");
    await waitFor(() => {
      expect(topP).toHaveAttribute("aria-invalid", "true");
      expect(maxTokens).toHaveAttribute("aria-invalid", "true");
    });
    await expect(topP).toHaveAccessibleDescription("Top-p must be between 0 and 1.");
    await expect(maxTokens).toHaveAccessibleDescription(
      "Max tokens must be a whole number between 1 and 1000000.",
    );
    await expect(canvas.getByLabelText("Temperature")).not.toHaveAttribute("aria-invalid");
    await expect(canvas.getByText("2 fields need attention")).toBeVisible();
    await userEvent.click(canvas.getByRole("button", { name: "Save changes" }));
    await expect(topP).toHaveFocus();
  },
};

// a 400 that names a field lands on that field, with focus, not in a toast
export const ServerRejectionLandsOnTheField: Story = {
  render: () => {
    const stub: FetchStub = async (input, init) => {
      if (init?.method === "PUT") {
        return json({ error: { message: "default_model must be at most 256 characters" } }, 400);
      }
      return answering(CONFIGURED)(input, init);
    };
    return <Harness fetchStub={stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const model = await canvas.findByLabelText("Default model");
    // a typed model is taken on Enter, and that edit is what enables Save
    await userEvent.type(model, "x{Enter}");
    await userEvent.click(canvas.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(model).toHaveAttribute("aria-invalid", "true"));
    await expect(model).toHaveAccessibleDescription("default_model must be at most 256 characters");
    await waitFor(() => expect(model).toHaveFocus());
  },
};

// the fields are disabled while the defaults are off, so an out-of-range
// stored value must neither show an error nor block saving (#2645); turning
// the defaults back on validates it again
export const DefaultsOffIgnoresAnOutOfRangeTemperature: Story = {
  render: () => <Harness fetchStub={answering({ ...BASE, default_temperature: 3 })} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByLabelText("Temperature")).toBeDisabled());
    await expect(canvas.getByLabelText("Temperature")).toHaveValue("3");
    await expect(canvas.queryByText("Temperature must be between 0 and 2.")).toBeNull();
    await expect(canvas.queryByText(/attention/)).toBeNull();
    await userEvent.click(canvas.getByRole("switch", { name: "Apply defaults" }));
    await waitFor(() => expect(canvas.getByText(/attention/)).toBeVisible());
    await expect(canvas.getByLabelText("Temperature")).toHaveAccessibleDescription(
      "Temperature must be between 0 and 2.",
    );
  },
};

const switchingOff = recording(answering(CONFIGURED));

// a bad draft left behind by switching the defaults off is not what gets
// saved: the field cannot be reached any more, so the stored value is kept
export const SwitchingOffKeepsTheStoredValue: Story = {
  render: () => <Harness fetchStub={switchingOff.stub} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const temperature = await canvas.findByLabelText("Temperature");
    await userEvent.clear(temperature);
    await userEvent.type(temperature, "5");
    await userEvent.click(canvas.getByRole("switch", { name: "Apply defaults" }));
    const save = canvas.getByRole("button", { name: "Save changes" });
    await expect(save).not.toHaveAttribute("aria-disabled");
    await userEvent.click(save);
    const body = await switchingOff.expectSentBody<ModelDefaultsDto>("PUT", "model-defaults");
    await expect(body.enabled).toBe(false);
    await expect(body.default_temperature).toBe(CONFIGURED.default_temperature);
  },
};

const typedModel = recording(answering(CONFIGURED));

// a model name typed and not confirmed with Enter is still the value once focus
// moves on, so clicking Save sends it rather than saving nothing (#3244)
export const TypedModelIsSavedWithoutPressingEnter: Story = {
  render: () => <Harness fetchStub={typedModel.stub} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const model = await canvas.findByLabelText("Default model");
    await userEvent.clear(model);
    await userEvent.type(model, "my-fine-tune");
    await userEvent.click(canvas.getByRole("button", { name: "Save changes" }));
    const body = await typedModel.expectSentBody<ModelDefaultsDto>("PUT", "model-defaults");
    await expect(body.default_model).toBe("my-fine-tune");
  },
};

// interaction: a valid edit round-trips and confirms
export const SavesChanges: Story = {
  render: () => {
    return <Harness fetchStub={answering(CONFIGURED)} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const tokens = await canvas.findByLabelText("Max tokens");
    await userEvent.clear(tokens);
    await userEvent.type(tokens, "4096");
    await userEvent.click(canvas.getByRole("button", { name: "Save changes" }));
    await expectToast(canvasElement, /model settings updated/i);
    await expect(canvas.getByLabelText("Max tokens")).toHaveValue("4096");
  },
};

/**
 * The save is refused (#1607).
 *
 * `SavesChanges` covers the answer; this covers the other one. The refusal
 * reaches the toast queue, and the form keeps the value that was typed rather
 * than snapping back to the setting the server last confirmed — a settings
 * screen that reverts on a rejected save loses the edit without saying so.
 */
export const SaveRejectedByTheServer: Story = {
  render: () => {
    const stub: FetchStub = async (input, init) => {
      if (init?.method === "PUT") {
        return json({ error: { message: "4096 is above the ceiling this gateway enforces" } }, 422);
      }
      return answering(CONFIGURED)(input, init);
    };
    return <Harness fetchStub={stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const field = await canvas.findByLabelText("Max tokens");
    await userEvent.clear(field);
    await userEvent.type(field, "4096");
    await userEvent.click(canvas.getByRole("button", { name: "Save changes" }));
    await expectToast(canvasElement, /above the ceiling/, "error");
    await waitFor(() => expect(canvas.getByLabelText("Max tokens")).toHaveValue("4096"));
  },
};

// What a non-superadmin gets, which is the screen refused before it asks
// (#1606).
//
// The model defaults are is a deployment-scoped resource, so `superadminOnly` never mounts the
// screen for an org role however high. The stub answers the screen's own
// request with a perfectly good payload on purpose: if the wrapper is dropped
// the screen renders that payload and this story fails, which the `Forbidden`
// story cannot do — it stubs the 403 itself, so it passes either way.
export const RefusedToAnAdmin: Story = {
  render: () => <Harness fetchStub={answering(BASE)} role="admin" />,
  play: async ({ canvasElement }) => {
    await expectForbidden(canvasElement);
  },
};

export const RefusedToAViewer: Story = {
  render: () => <Harness fetchStub={answering(BASE)} role="viewer" />,
  play: async ({ canvasElement }) => {
    await expectForbidden(canvasElement);
  },
};

// the same screen at a phone's width in both languages: Russian runs a third
// longer than English and overflowed twice as many screens (#2004)
const modelFits = phoneFits({
  render: () => <Harness fetchStub={answering(CONFIGURED)} />,
  // the label in the language the story is drawn in: a screen already in Russian
  // never shows the English one, and only a screen that beats the catalog swap to
  // its first paint does. which of the two a run gets is timing, not the story
  ready: (canvas, locale) =>
    canvas.findByLabelText((locale === "ru" ? ru : en).pages.modelSettings.model.defaultModel),
});
export const MobileFits: Story = modelFits("mobile", "en");
export const SmallPhone: Story = modelFits("small", "en");
export const SmallPhoneInRussian: Story = modelFits("small", "ru");

// the control plane answers 5xx: the retry is on offer, reaches the network,
// and the screen comes back once the control plane does (#2220)
const modelSettingsDown = failingThenRecovering("/api/v1/model-defaults", answering(CONFIGURED));
export const LoadFailsAndRetries: Story = {
  render: () => {
    modelSettingsDown.reset();
    return <Harness fetchStub={modelSettingsDown.stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectRetryRecovers(
      canvasElement,
      modelSettingsDown,
      /failed to return model settings/,
      async () =>
        await expect(await canvas.findByLabelText("Default model")).toHaveValue("gpt-4o-mini"),
    );
  },
};

// the switch looks like it applies at once; it waits for Save, and the panel says so (#2214)
export const SwitchWaitsForSave: Story = {
  render: () => <Harness fetchStub={answering(BASE)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const apply = await canvas.findByRole("switch", { name: "Apply defaults" });
    await expect(canvas.getByRole("button", { name: "Save changes" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    await userEvent.click(apply);
    await expect(canvas.getByText("Not saved")).toBeVisible();
    await expect(canvas.getByRole("button", { name: "Save changes" })).not.toHaveAttribute(
      "aria-disabled",
    );
    await userEvent.click(canvas.getByRole("button", { name: "Discard changes" }));
    await expect(apply).toHaveAttribute("aria-checked", "false");
    await expect(canvas.queryByText("Not saved")).toBeNull();
  },
};
