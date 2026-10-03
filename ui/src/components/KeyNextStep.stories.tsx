import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, waitFor, within } from "storybook/test";

import { KeyNextStep } from "./KeyNextStep";
import type { ClientSettingsDto } from "@/lib/api";
import ru from "@/lib/i18n/locales/ru.json";
import { expectGateAnswered, Harness, json, recording, routes } from "@/pages/story-harness";

const SAVED: ClientSettingsDto = {
  public_base_url: "https://gateway.example.com/v1/",
  forwarded_headers: [],
  injected_headers: {},
  request_id_header: "x-request-id",
  updated_at: "2026-08-05T12:00:00Z",
  always_propagated: ["traceparent", "tracestate", "b3"],
  reserved: ["authorization"],
};

const meta = {
  title: "Components/KeyNextStep",
  component: KeyNextStep,
  parameters: { layout: "padded" },
  args: { models: [] },
  // no role, so no capability provider and no client-settings read: the
  // address falls back to the dashboard's /gw proxy, as it does for a caller
  // whose gate has not answered yet
  render: (args) => (
    <Harness fetchStub={routes([])}>
      <KeyNextStep {...args} />
    </Harness>
  ),
} satisfies Meta<typeof KeyNextStep>;

export default meta;
type Story = StoryObj<typeof meta>;

const address = (canvasElement: HTMLElement) =>
  within(canvasElement).findByRole("region", { name: /Gateway URL/ });
const request = (canvasElement: HTMLElement) =>
  within(canvasElement).findByRole("region", { name: /First request/ });

/**
 * With no public base URL saved the address is the dashboard's own `/gw` proxy,
 * the one the dashboard can prove works. The bare origin would be a 404 (#2075).
 * A key that may reach every route is shown the built-in model, and the key is
 * referenced through an environment variable rather than written out.
 */
export const Default: Story = {
  play: async ({ canvasElement }) => {
    const origin = window.location.origin;
    await expect(await address(canvasElement)).toHaveTextContent(`${origin}/gw/v1`);
    const snippet = await request(canvasElement);
    await waitFor(() => expect(snippet).toHaveTextContent(`curl ${origin}/gw/v1/chat/completions`));
    await expect(snippet).toHaveTextContent(`"model":"fake-llm"`);
    await expect(snippet).toHaveTextContent(`Authorization: Bearer $ROLTER_API_KEY`);
    // the proxy is the control plane's port, and the snippet says so
    await expect(snippet).toHaveTextContent(/dashboard's gateway proxy/);
    await expect(
      within(canvasElement).getByRole("heading", { name: "Make your first request" }),
    ).toBeVisible();
    await expect(canvasElement.textContent).toContain("ROLTER_API_KEY");
  },
};

/** A key limited to some models sends its first request to the first of them. */
export const NamesTheFirstModelTheKeyMayReach: Story = {
  args: { models: ["claude-sonnet", "gpt-4o"] },
  play: async ({ canvasElement }) => {
    const snippet = await request(canvasElement);
    await waitFor(() => expect(snippet).toHaveTextContent(`"model":"claude-sonnet"`));
    await expect(snippet).not.toHaveTextContent("fake-llm");
  },
};

const asSuperadmin = recording(async (input) =>
  String(input).includes("/api/v1/client-settings") ? json(SAVED) : json([]),
);

/**
 * A superadmin, who may read client settings, is handed the saved public base
 * URL, trimmed of a trailing slash and of the `/v1` an operator pasted from an
 * SDK example, and the snippet drops its note about the proxy (#2218).
 */
export const UsesTheSavedPublicBaseUrl: Story = {
  render: (args) => (
    <Harness fetchStub={asSuperadmin.stub} role="superadmin">
      <KeyNextStep {...args} />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await waitFor(async () =>
      expect(await address(canvasElement)).toHaveTextContent("https://gateway.example.com/v1"),
    );
    const snippet = await request(canvasElement);
    await waitFor(() =>
      expect(snippet).toHaveTextContent("curl https://gateway.example.com/v1/chat/completions"),
    );
    await expect(snippet).not.toHaveTextContent("/v1/v1");
    await expect(canvasElement.textContent ?? "").not.toContain("/gw/");
    await expect(snippet).not.toHaveTextContent(/dashboard's gateway proxy/);
    await asSuperadmin.expectSent("GET", "/api/v1/client-settings");
  },
};

const asAdmin = recording(async (input) =>
  String(input).includes("/api/v1/client-settings") ? json(SAVED) : json([]),
);

/**
 * Client settings are superadmin-only, so an org admin never asks for them and
 * gets the proxy, even on a deployment that saved a public base URL.
 */
export const AnAdminKeepsTheProxy: Story = {
  render: (args) => (
    <Harness fetchStub={asAdmin.stub} role="admin">
      <KeyNextStep {...args} />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectGateAnswered();
    await expect(await address(canvasElement)).toHaveTextContent("/gw/v1");
    asAdmin.expectNotSent("GET", "/api/v1/client-settings");
  },
};

/** The heading, the lead and the regions' names follow the locale. */
export const InRussian: Story = {
  globals: { locale: "ru" },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(
      await canvas.findByRole("heading", { name: ru.common.secret.nextStep.title }),
    ).toBeVisible();
    await expect(
      await canvas.findByRole("region", {
        name: new RegExp(ru.common.secret.nextStep.gatewayUrl),
      }),
    ).toBeVisible();
    await expect(
      canvas.getByRole("region", { name: new RegExp(ru.common.secret.nextStep.request) }),
    ).toBeVisible();
  },
};
