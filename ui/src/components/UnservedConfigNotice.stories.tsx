import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, within } from "storybook/test";

import { UnservedConfigNotice } from "./UnservedConfigNotice";
import type { ConfigFinding, ConfigProblems } from "@/lib/api";
import { Harness, ORG, routes } from "@/pages/story-harness";
import { atWide, expectNoHorizontalOverflow, phoneFits } from "@/lib/story-viewport";

const meta = {
  title: "Components/UnservedConfigNotice",
  component: UnservedConfigNotice,
  render: (args) => (
    <Harness fetchStub={routes([["/api/v1/orgs", () => [ORG]]])}>
      <UnservedConfigNotice {...args} />
    </Harness>
  ),
} satisfies Meta<typeof UnservedConfigNotice>;

export default meta;
type Story = StoryObj<typeof meta>;

const OMITTED_PROVIDER =
  "provider 'openrouter-edge' omitted from the snapshot: openrouter provider 'openrouter-edge' api_base must be https://openrouter.ai/api/v1 unless the provider sets allow_custom_api_base = true";

/** The #926 case: one provider dropped from an otherwise healthy fleet. */
export const OneProblem: Story = {
  args: {
    report: {
      problems: [OMITTED_PROVIDER],
      omitted: [
        {
          kind: "provider",
          name: "openrouter-edge",
          reason: "api_base must be https://openrouter.ai/api/v1",
          index: 0,
        },
      ],
      findings: [{ kind: "omitted", message: OMITTED_PROVIDER }],
    },
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(/1 config entry/)).toBeInTheDocument();
    // one line per kind, folded; the rows it is about open on demand
    const line = canvas.getByRole("button", { name: "1 provider is not served" });
    await expect(line).toHaveAttribute("aria-expanded", "false");
    await userEvent.click(line);
    await expect(line).toHaveAttribute("aria-expanded", "true");
    await expect(canvas.getByText("openrouter-edge")).toBeVisible();
  },
};

/** A control plane older than `findings`: its sentences, folded under one line. */
export const AnOlderControlPlane: Story = {
  args: {
    report: {
      problems: [
        "route 'gpt-4o-mini' omitted from the snapshot: it has no target that references a known provider with a positive weight",
        "budget 'team-cap' on team 'research' has period '7d', which the gateway does not recognise, so it is enforced as a monthly cap; set it to one of daily, weekly, monthly",
      ],
      omitted: [],
    },
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(
      await canvas.findByText("2 config entries are not being served as written"),
    ).toBeInTheDocument();
    await userEvent.click(canvas.getByRole("button", { name: "2 other findings" }));
    await expect(canvas.getByText(/enforced as a monthly cap/)).toBeVisible();
  },
};

/** The healthy fleet: the notice must take up no space at all. */
export const Healthy: Story = {
  args: { report: { problems: [], omitted: [] } },
  play: async ({ canvasElement }) => {
    await expect(within(canvasElement).queryByTestId("unserved-config")).toBeNull();
  },
};

const shadow = (kind: string, name: string): ConfigFinding => ({
  kind: "bootstrap_shadow",
  message: `warning: organization ${ORG.id}'s ${kind} '${name}' shadows the readonly ${kind} of the same name in the control plane's config file for that organization's keys; every other key is served the file's ${kind} (#3516)`,
  org_id: ORG.id,
  subject: { kind, name },
});

const KEY_LINE =
  "virtual key 'local-dev' omitted from the snapshot: a database deployment mints keys in the store; remove the [[virtual_keys]] entry from the control plane's config file";

/**
 * #3646: the first page of a fresh `easy-up --database-url` before #3645.
 * Thirteen English sentences, each with the org's UUID and an issue number,
 * filled the screen. They are two lines now: the twelve shadows under the
 * org's name, and the one key left out.
 */
const FIRST_RUN: ConfigProblems = (() => {
  const findings = [
    ...["openai", "anthropic", "openrouter", "ollama-cloud", "vllm-1", "vllm-2", "ollama"].map(
      (n) => shadow("provider", n),
    ),
    ...["gpt-4o", "claude", "llama", "llama-weighted", "llama-pipeline"].map((n) =>
      shadow("route", n),
    ),
    { kind: "omitted", message: KEY_LINE },
  ];
  return {
    problems: findings.map((f) => f.message),
    omitted: [{ kind: "virtual_key", name: "local-dev", reason: "minted in the store", index: 0 }],
    findings,
  };
})();

export const AFreshInstallReadsInTwoLines: Story = {
  ...atWide,
  args: { report: FIRST_RUN },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(/13 config entries/)).toBeVisible();
    const lines = canvas.getAllByRole("button");
    await expect(lines).toHaveLength(2);
    const shadows = await canvas.findByRole("button", { name: /12 rows of Rolter replace/ });
    await expect(
      canvas.getByRole("button", { name: "1 config-file key is not served" }),
    ).toBeVisible();
    // no UUID, no issue number, no `warning:` lead on screen
    await expect(canvasElement.textContent).not.toContain(ORG.id);
    await expect(canvasElement.textContent).not.toMatch(/#3516|warning:/);
    await userEvent.click(shadows);
    await expect(canvas.getByText("provider anthropic")).toBeVisible();
    await expect(canvas.getByText("route llama-pipeline")).toBeVisible();
  },
};

export const AFreshInstallInRussian: Story = {
  ...atWide,
  globals: { ...atWide.globals, locale: "ru" },
  args: { report: FIRST_RUN },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(/13 записей конфигурации/)).toBeVisible();
    await expect(
      await canvas.findByRole("button", { name: /12 записей организации Rolter/ }),
    ).toBeVisible();
    await expect(
      canvas.getByRole("button", { name: /1 ключ из файла конфигурации/ }),
    ).toBeVisible();
    await expect(canvasElement.textContent).not.toMatch(/shadows|omitted from the snapshot/);
    await expectNoHorizontalOverflow();
  },
};

const fits = phoneFits({
  render: () => (
    <Harness fetchStub={routes([["/api/v1/orgs", () => [ORG]]])}>
      <UnservedConfigNotice report={FIRST_RUN} />
    </Harness>
  ),
  ready: async (canvas) => {
    const buttons = await canvas.findAllByRole("button");
    for (const button of buttons) await userEvent.click(button);
  },
});
export const AFreshInstallOnAPhoneInRussian: Story = {
  ...fits("mobile", "ru"),
  args: { report: FIRST_RUN },
};
