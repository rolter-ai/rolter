import type { Meta, StoryObj } from "@storybook/react-vite";
import * as React from "react";
import { expect, userEvent, within } from "storybook/test";

import {
  DEFAULT_KEY_TTL_DAYS,
  KeyCacheField,
  KeyExpiryField,
  KeyModelsField,
  KeyNameField,
  KeyReachSummary,
  MAX_KEY_NAME_LEN,
  NEVER,
  type CacheMode,
} from "./KeyMintFields";
import { ApiError } from "@/lib/api";

/**
 * The mint block as both screens assemble it — the admin editor on Keys and
 * the self-service one on Account. It is one component precisely so the rules
 * #945 introduced (a name and an expiry are required) cannot hold on one
 * screen and not the other.
 */
/** the models a project with routes offers as ticks */
const ROUTED = ["claude-sonnet", "fake-llm", "gpt-4o"];

function MintForm({
  initialName = "",
  initialTtl = String(DEFAULT_KEY_TTL_DAYS),
  initialModels = [],
  options = ROUTED,
  loading = false,
  error,
}: {
  initialName?: string;
  initialTtl?: string;
  initialModels?: string[];
  /** what the project's routes offer; empty stands for a project with none */
  options?: string[];
  loading?: boolean;
  error?: unknown;
}) {
  const [name, setName] = React.useState(initialName);
  const [ttl, setTtl] = React.useState(initialTtl);
  const [models, setModels] = React.useState(initialModels);
  const [cache, setCache] = React.useState<CacheMode>("inherit");
  return (
    <div className="max-w-md space-y-4">
      <KeyNameField value={name} onChange={setName} />
      <KeyExpiryField value={ttl} onChange={setTtl} />
      <KeyModelsField
        value={models}
        onChange={setModels}
        options={options}
        loading={loading}
        error={error}
      />
      <KeyCacheField value={cache} onChange={setCache} />
      <KeyReachSummary project="Gateway" models={models} ttl={ttl} />
    </div>
  );
}

const meta = {
  title: "Components/KeyMintFields",
  component: KeyNameField,
  parameters: { layout: "padded" },
  args: { value: "", onChange: () => {} },
} satisfies Meta<typeof KeyNameField>;

export default meta;
type Story = StoryObj<typeof meta>;

/** What an operator gets before touching anything: 30 days, no allow-list. */
export const Default: Story = {
  render: () => <MintForm />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByLabelText("Expires")).toHaveValue(
      String(DEFAULT_KEY_TTL_DAYS),
    );
    await expect(canvas.getByLabelText("Response cache")).toHaveValue("inherit");
  },
};

/** A filled-in draft, narrowed to two models. */
export const Filled: Story = {
  render: () => (
    <MintForm initialName="ci-runner" initialModels={["gpt-4o", "claude-sonnet"]} />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByText(/2 models: gpt-4o, claude-sonnet/)).toBeVisible();
  },
};

/**
 * The name limit is the control plane's (`MAX_KEY_NAME_LEN` in `me.rs`),
 * checked here so the operator learns it before spending a round trip.
 */
export const NameTooLong: Story = {
  render: () => <MintForm initialName={"k".repeat(MAX_KEY_NAME_LEN + 1)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByRole("alert")).toHaveTextContent(
      `At most ${MAX_KEY_NAME_LEN} characters`,
    );
    await expect(canvas.getByLabelText("Name")).toHaveAttribute("aria-invalid", "true");
  },
};

/**
 * "Never" is a deliberate choice with its own consequence, so the hint changes
 * to say so rather than staying on the generic rotation advice.
 */
export const NeverExpires: Story = {
  render: () => <MintForm initialTtl={NEVER} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByText(/until someone revokes it/)).toBeVisible();
    await expect(canvas.getByText("Forever, until revoked")).toBeVisible();
  },
};

/**
 * The summary is the answer to "did I just mint something narrow, or something
 * that can spend money against every provider?" — asked while it can still be
 * changed, because the secret is shown exactly once.
 */
export const ReachSummaryTracksTheDraft: Story = {
  render: () => <MintForm />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByText("Every model this project can route to")).toBeVisible();
    await userEvent.click(canvas.getByRole("checkbox", { name: "gpt-4o" }));
    await expect(canvas.getByText("One model: gpt-4o")).toBeVisible();
    await expect(
      canvas.queryByText("Every model this project can route to"),
    ).not.toBeInTheDocument();
  },
};

/**
 * The per-key cache override has three states, not two: "inherit" is the
 * absence of a decision, and collapsing it into a switch would turn "I did not
 * choose" into "I chose no".
 */
export const CacheIsThreeState: Story = {
  render: () => <MintForm />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const select = canvas.getByLabelText("Response cache");
    await expect(within(select).getAllByRole("option")).toHaveLength(3);
    await userEvent.selectOptions(select, "off");
    await expect(select).toHaveValue("off");
  },
};

/** A key narrowed to one provider, summarised beside a fixed expiry date. */
export const ScopedToProviders: Story = {
  render: () => (
    <div className="max-w-md">
      <KeyReachSummary
        project="Gateway"
        models={["gpt-4o"]}
        providers={["openai-prod"]}
        ttl="7"
      />
    </div>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByText("One provider: openai-prod")).toBeVisible();
    await expect(canvas.getByText(/^Until /)).toBeVisible();
  },
};

/**
 * The allow-list is the project's own routes, ticked off (#1345). Typing a
 * model address used to be the only way to fill this in, and a typo produced
 * an allow-list matching nothing — silently, since the strings are stored as
 * given.
 */
export const ModelsFromRoutes: Story = {
  render: () => <MintForm />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getAllByRole("checkbox")).toHaveLength(ROUTED.length);
    await userEvent.click(canvas.getByRole("checkbox", { name: "claude-sonnet" }));
    await userEvent.click(canvas.getByRole("checkbox", { name: "fake-llm" }));
    await expect(canvas.getByText("2 models allowed")).toBeVisible();
    // the chip removes the same entry the tick added, and names it while
    // doing so — "Remove" alone tells a screen reader nothing about which
    await userEvent.click(
      canvas.getByRole("button", { name: "Remove fake-llm from the allow-list" }),
    );
    await expect(canvas.getByRole("checkbox", { name: "fake-llm" })).toHaveAttribute(
      "aria-checked",
      "false",
    );
    await expect(canvas.getByText("1 model allowed")).toBeVisible();
  },
};

/**
 * A project with no routes yet still has to be able to mint a narrowed key:
 * the address may be one a route about to be created will serve, so the field
 * falls back to free-form entry rather than locking the operator out.
 */
export const ModelsWithoutRoutes: Story = {
  render: () => <MintForm options={[]} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByText("No routes in this project yet")).toBeVisible();
    await userEvent.type(canvas.getByLabelText(/model allow-list/i), "gpt-4o-mini");
    await userEvent.click(canvas.getByRole("button", { name: "Add" }));
    await expect(canvas.getByText("One model: gpt-4o-mini")).toBeVisible();
  },
};

/**
 * An allow-list already naming an address no route offers keeps it. Dropping
 * it because it is not on the list would quietly widen the key, which is the
 * one change nobody would notice until the bill arrived.
 */
export const ModelsKeepUnknownEntry: Story = {
  render: () => <MintForm initialModels={["gpt-4o", "legacy-davinci"]} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const kept = canvas.getByRole("checkbox", { name: /legacy-davinci/ });
    await expect(kept).toHaveAttribute("aria-checked", "true");
    await expect(within(kept).getByText("Custom")).toBeVisible();
    await expect(canvas.getByText("2 models allowed")).toBeVisible();
  },
};

/** While the route lookup is in flight, held open at the height of the list. */
export const ModelsLoading: Story = {
  render: () => <MintForm loading />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getAllByLabelText("Loading…").length).toBeGreaterThan(0);
  },
};

/**
 * A member without route read access gets a 403 that asking again will not
 * fix. The field says so and keeps free-form entry, rather than pretending the
 * project routes nothing.
 */
export const ModelsLoadFailed: Story = {
  render: () => <MintForm error={new ApiError("forbidden", 403)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByRole("alert")).toHaveTextContent(
      "You do not have access to routes",
    );
    await userEvent.type(canvas.getByLabelText(/model allow-list/i), "gpt-4o");
    await userEvent.click(canvas.getByRole("button", { name: "Add" }));
    await expect(canvas.getByText("One model: gpt-4o")).toBeVisible();
  },
};
