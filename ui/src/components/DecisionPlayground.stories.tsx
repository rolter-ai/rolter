import type { Meta, StoryObj } from "@storybook/react";
import { expect, fireEvent, userEvent, waitFor, within } from "storybook/test";

import Playground from "@/pages/Playground";
import { Harness, json, recording, routes } from "@/pages/story-harness";
import { setPlaygroundKey } from "@/lib/gateway";
import en from "@/lib/i18n/locales/en.json";

const models = {
  data: [
    { id: "fake-llm", rolter_capabilities: { decisions: true, chat: true } },
    { id: "openjev", rolter_capabilities: { decisions: true, chat: false } },
    { id: "chat-only", rolter_capabilities: { decisions: false, chat: true } },
  ],
};
const answer = {
  answers: {
    category: {
      type: "choice",
      choice: "damaged",
      probabilities: { damaged: 0.9, intact: 0.1 },
      confidence: 0.8,
    },
    visible: { type: "noul", noul: 0.9 },
    severity: {
      type: "score",
      score: 1.8,
      legend: { "0": "minor", "1": "major" },
      probabilities: { "0": 0.1, "1": 0.9 },
    },
  },
};
const paths = {
  "/gw/v1/models": models,
  "/api/v1/models": [],
  "/api/v1/config/problems": { problems: [], omitted: [] },
  "/gw/v1/systemone": answer,
};
const router = (values: Record<string, unknown>) =>
  routes(Object.entries(values).map(([path, value]) => [path, () => value]));
const sent = recording(router(paths));
const meta = {
  title: "Screens/Playground/Decisions",
  component: Playground,
  parameters: { layout: "fullscreen" },
  render: () => {
    sent.calls.length = 0;
    setPlaygroundKey("sk-test-decision");
    return (
      <Harness fetchStub={sent.stub}>
        <Playground />
      </Harness>
    );
  },
} satisfies Meta<typeof Playground>;
export default meta;
type Story = StoryObj<typeof meta>;

async function open(canvasElement: HTMLElement) {
  const canvas = within(canvasElement);
  await userEvent.click(
    await canvas.findByRole("tab", { name: en.pages.playground.modes.decisions }),
  );
  return canvas;
}

export const Empty: Story = {
  play: async ({ canvasElement }) => {
    const canvas = await open(canvasElement);
    await expect(await canvas.findByText(en.pages.playground.decisions.empty)).toBeVisible();
  },
};

export const TypedAnswers: Story = {
  play: async ({ canvasElement }) => {
    const canvas = await open(canvasElement);
    await userEvent.type(
      canvas.getByLabelText(en.pages.playground.decisions.state),
      "parcel arrived damaged",
    );
    await waitFor(() =>
      expect(canvas.getByRole("button", { name: en.pages.playground.decisions.run })).toBeEnabled(),
    );
    await userEvent.click(canvas.getByRole("button", { name: en.pages.playground.decisions.run }));
    await expect(await canvas.findByRole("region", { name: "category" })).toHaveTextContent(
      "damaged",
    );
    await expect(await canvas.findByRole("region", { name: "visible" })).toHaveTextContent("90");
    await expect(await canvas.findByRole("region", { name: "severity" })).toHaveTextContent(
      "major",
    );
    await sent.expectSent("POST", "/gw/v1/systemone");
  },
};

export const ImageUsesTheOpenJevWireFormat: Story = {
  play: async ({ canvasElement }) => {
    const canvas = await open(canvasElement);
    const input = canvas.getByLabelText(en.pages.playground.decisions.image) as HTMLInputElement;
    const bytes = Uint8Array.from(
      atob(
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==",
      ),
      (c) => c.charCodeAt(0),
    );
    fireEvent.change(input, {
      target: { files: [new File([bytes], "premise.png", { type: "image/png" })] },
    });
    await canvas.findByRole("img", { name: en.pages.playground.decisions.image });
    await userEvent.type(
      canvas.getByLabelText(en.pages.playground.decisions.state),
      "An image: <<IMG>>",
    );
    await userEvent.click(canvas.getByRole("button", { name: en.pages.playground.decisions.run }));
    const body = await sent.expectSentBody<{ image_data: string; state: string }>(
      "POST",
      "/gw/v1/systemone",
    );
    await expect(body.image_data).toMatch(/^data:image\/png;base64,/);
    await expect(body.state).toBe("An image: <<IMG>>");
  },
};

export const PerQuestionError: Story = {
  render: () => {
    sent.calls.length = 0;
    setPlaygroundKey("sk-test-decision");
    return (
      <Harness
        fetchStub={router({
          ...paths,
          "/gw/v1/systemone": {
            answers: {
              decision: { error: { code: "invalid_question", message: "unsupported type" } },
            },
          },
        })}
      >
        <Playground />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = await open(canvasElement);
    await waitFor(() =>
      expect(canvas.getByRole("button", { name: en.pages.playground.decisions.run })).toBeEnabled(),
    );
    await userEvent.click(canvas.getByRole("button", { name: en.pages.playground.decisions.run }));
    await expect(
      await canvas.findByRole("region", {
        name: new RegExp(en.pages.playground.decisions.questionError),
      }),
    ).toHaveTextContent("invalid_question");
  },
};

export const ModelCapabilitiesAndJsonState: Story = {
  play: async ({ canvasElement }) => {
    const canvas = await open(canvasElement);
    const picker = canvas.getByRole("combobox", { name: en.pages.playground.decisions.model });
    await userEvent.click(picker);
    let list = within(await canvas.findByRole("listbox"));
    await expect(await list.findByRole("option", { name: "openjev" })).toBeVisible();
    await expect(list.queryByRole("option", { name: "chat-only" })).toBeNull();
    await userEvent.keyboard("openjev{ArrowDown}{Enter}");
    await expect(picker).toHaveValue("openjev");
    const format = canvas.getByRole("radiogroup", {
      name: en.pages.playground.decisions.stateFormat,
    });
    await expect(within(format).getByRole("radio", { name: "JSON" })).not.toBeChecked();
    await userEvent.click(within(format).getByRole("radio", { name: "JSON" }));
    await expect(within(format).getByRole("radio", { name: "JSON" })).toBeChecked();
    fireEvent.change(canvas.getByLabelText(en.pages.playground.decisions.state), {
      target: { value: JSON.stringify({ text: "parcel", metadata: [1, 2] }) },
    });
    await userEvent.click(canvas.getByRole("button", { name: en.pages.playground.decisions.run }));
    const body = await sent.expectSentBody<{ model: string; state: unknown }>(
      "POST",
      "/gw/v1/systemone",
    );
    await expect(body.model).toBe("openjev");
    await expect(body.state).toEqual({ text: "parcel", metadata: [1, 2] });
    await userEvent.click(canvas.getByRole("tab", { name: en.pages.playground.modes.chat }));
    await userEvent.click(canvas.getByRole("combobox", { name: "Model" }));
    list = within(await canvas.findByRole("listbox"));
    await expect(list.queryByRole("option", { name: "openjev" })).toBeNull();
    await expect(list.getByRole("option", { name: "chat-only" })).toBeVisible();
    await userEvent.keyboard("{Escape}");
    // a decision-only model is no more use to Embeddings than to Chat (#3220)
    await userEvent.click(canvas.getByRole("tab", { name: en.pages.playground.modes.embeddings }));
    await userEvent.click(canvas.getByRole("combobox", { name: "Model" }));
    list = within(await canvas.findByRole("listbox"));
    await expect(list.queryByRole("option", { name: "openjev" })).toBeNull();
    await expect(list.getByRole("option", { name: "chat-only" })).toBeVisible();
    await userEvent.keyboard("{Escape}");
  },
};

const shadowed = router({
  ...paths,
  "/gw/v1/models": {
    data: [
      // a configured chat route named fake-llm shadows the builtin
      { id: "fake-llm", rolter_capabilities: { decisions: false, chat: true } },
      { id: "openjev", rolter_capabilities: { decisions: true, chat: false } },
    ],
  },
});

export const ShadowedFakeLlmStartsOnTheFirstDecisionModel: Story = {
  render: () => {
    sent.calls.length = 0;
    setPlaygroundKey("sk-test-decision");
    return (
      <Harness fetchStub={recording(shadowed).stub}>
        <Playground />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = await open(canvasElement);
    const picker = canvas.getByRole("combobox", { name: en.pages.playground.decisions.model });
    await waitFor(() => expect(picker).toHaveValue("openjev"));
    await waitFor(() =>
      expect(canvas.getByRole("button", { name: en.pages.playground.decisions.run })).toBeEnabled(),
    );
  },
};

export const NoDecisionModelShowsTheEmptyState: Story = {
  render: () => {
    setPlaygroundKey("sk-test-decision");
    return (
      <Harness
        fetchStub={router({
          ...paths,
          "/gw/v1/models": {
            // the shadowing route is all there is, and it does not serve decisions
            data: [{ id: "fake-llm", rolter_capabilities: { decisions: false, chat: true } }],
          },
        })}
      >
        <Playground />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = await open(canvasElement);
    await expect(await canvas.findByText(en.pages.playground.decisions.noModels)).toBeVisible();
    await expect(
      canvas.queryByRole("button", { name: en.pages.playground.decisions.run }),
    ).toBeNull();
  },
};

export const RejectsInvalidImagesBeforeSending: Story = {
  play: async ({ canvasElement }) => {
    const canvas = await open(canvasElement);
    const input = canvas.getByLabelText(en.pages.playground.decisions.image);
    for (const file of [
      new File(["not an image"], "premise.svg", { type: "image/svg+xml" }),
      new File([new Uint8Array(4 * 1024 * 1024 + 1)], "large.png", { type: "image/png" }),
      new File(["broken png"], "broken.png", { type: "image/png" }),
    ]) {
      fireEvent.change(input, { target: { files: [file] } });
      await expect(await canvas.findByRole("alert")).toHaveTextContent(
        en.pages.playground.decisions.imageInvalid
          .replace("{{size}}", "4")
          .replace("{{pixels}}", "12"),
      );
      // story-wait-allow: invalid file validation disables this control before the assertion
      await expect(
        canvas.getByRole("button", { name: en.pages.playground.decisions.run }),
      ).toBeDisabled();
    }
    await expect(sent.calls.filter((c) => c.url === "/gw/v1/systemone")).toHaveLength(0);
    fireEvent.change(input, { target: { files: [] } });
    await waitFor(() =>
      expect(canvas.getByRole("button", { name: en.pages.playground.decisions.run })).toBeEnabled(),
    );
  },
};

let cancelled = false;
export const PendingCanBeCancelled: Story = {
  render: () => {
    cancelled = false;
    setPlaygroundKey("sk-test-decision");
    return (
      <Harness
        fetchStub={(input, init) =>
          String(input).endsWith("/v1/systemone")
            ? new Promise<Response>((_resolve, reject) => {
                init?.signal?.addEventListener(
                  "abort",
                  () => {
                    cancelled = true;
                    reject(new DOMException("Aborted", "AbortError"));
                  },
                  { once: true },
                );
              })
            : router(paths)(input, init)
        }
      >
        <Playground />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = await open(canvasElement);
    await waitFor(() =>
      expect(canvas.getByRole("button", { name: en.pages.playground.decisions.run })).toBeEnabled(),
    );
    await userEvent.click(canvas.getByRole("button", { name: en.pages.playground.decisions.run }));
    const pending = await canvas.findByRole("button", {
      name: en.pages.playground.decisions.running,
    });
    // story-wait-allow: this is the component's own synchronous request state, not RBAC
    await expect(pending).toBeDisabled();
    await userEvent.click(canvas.getByRole("button", { name: en.pages.playground.decisions.stop }));
    await waitFor(() => expect(cancelled).toBe(true));
    await waitFor(() =>
      expect(canvas.getByRole("button", { name: en.pages.playground.decisions.run })).toBeEnabled(),
    );
    await expect(canvas.queryByRole("alert")).toBeNull();
  },
};

export const Refused: Story = {
  render: () => {
    sent.calls.length = 0;
    setPlaygroundKey("sk-test-decision");
    return (
      <Harness
        fetchStub={async (input, init) =>
          String(input).endsWith("/v1/systemone")
            ? json({ error: { message: "decision request refused" } }, 403)
            : router(paths)(input, init)
        }
      >
        <Playground />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = await open(canvasElement);
    await waitFor(() =>
      expect(canvas.getByRole("button", { name: en.pages.playground.decisions.run })).toBeEnabled(),
    );
    await userEvent.click(canvas.getByRole("button", { name: en.pages.playground.decisions.run }));
    await expect(await canvas.findByRole("alert")).toHaveTextContent(en.errors.api.generic);
  },
};
