import type { Meta, StoryObj } from "@storybook/react-vite";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { ProviderGroupSheet } from "./ProviderGroupSheet";
import {
  Harness,
  ORG,
  expectClosesWithoutPrompting,
  expectSheetClosed,
  json,
  recording,
  scoped,
  sheet,
  answerDiscardPrompt,
  pickOption,
} from "@/pages/story-harness";
import en from "@/lib/i18n/locales/en.json";
import { expectFieldHints } from "@/lib/story-hints";
import type { ProviderGroupRow, ProviderRow } from "@/lib/api";

const provider = (id: string, name: string): ProviderRow => ({
  id,
  org_id: ORG.id,
  name,
  slug: name,
  kind: "openai_compatible",
  api_base: `http://${name}.internal:8000`,
  egress_proxies: [],
  created_at: "2026-01-01T00:00:00Z",
});

const PROVIDERS: ProviderRow[] = [provider("prov-1", "vllm-a"), provider("prov-2", "vllm-b")];

const GROUP: ProviderGroupRow = {
  id: "grp-1",
  org_id: ORG.id,
  name: "vllm-cluster",
  slug: "vllm-cluster",
  strategy: "cache_aware",
  created_at: "2026-03-02T09:00:00Z",
  members: [
    { provider_id: "prov-1", upstream_model: "llama-3.1-70b", weight: 2 },
    { provider_id: "prov-2", upstream_model: null, weight: 1 },
  ] as ProviderGroupRow["members"],
};

/** the recorder the story under way installed, read back by its play function */
let calls: ReturnType<typeof recording>;

function Stage({
  mode,
  group,
  providers = PROVIDERS,
  answer = async () => json(GROUP),
}: {
  mode: "add" | "edit";
  group?: ProviderGroupRow | null;
  providers?: ProviderRow[];
  answer?: () => Promise<Response>;
}) {
  const [open, setOpen] = React.useState(true);
  // a ref, not `useMemo`: `answer` is a default parameter, so its identity
  // changes on every render and a memo keyed on it would hand each render a
  // fresh recorder — with the requests the last one saw thrown away
  const recorder = React.useRef<ReturnType<typeof recording> | null>(null);
  if (!recorder.current) {
    // the scope picker reads the org's teams and projects as the sheet opens
    recorder.current = recording(scoped(answer));
    calls = recorder.current;
  }
  return (
    <Harness fetchStub={recorder.current.stub}>
      <ProviderGroupSheet
        open={open}
        mode={mode}
        onOpenChange={setOpen}
        orgId={ORG.id}
        providers={providers}
        group={group}
        onDone={() => {}}
      />
    </Harness>
  );
}

const meta = {
  title: "Overlays/ProviderGroupSheet",
  component: ProviderGroupSheet,
  parameters: { layout: "fullscreen" },
  // every story renders through `Stage`, which owns the props; these satisfy
  // the required-prop contract for the docs page
  args: {
    open: true,
    mode: "add" as const,
    onOpenChange: () => {},
    orgId: ORG.id,
    providers: PROVIDERS,
    onDone: () => {},
  },
} satisfies Meta<typeof ProviderGroupSheet>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Add: Story = {
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await waitFor(() => expect(dialog.getByText("Add provider group")).toBeVisible());
    // a group with nothing in it resolves to nothing, and says so rather than
    // leaving an empty area where members would be
    await expect(dialog.getByText(/A group with no members resolves to nothing/)).toBeVisible();
  },
};

/** Editing seeds from the stored group: name, slug, strategy and members. */
export const Edit: Story = {
  render: () => <Stage mode="edit" group={GROUP} />,
  play: async () => {
    const dialog = within(sheet());
    // the draft is seeded in an effect, so the first read has to wait for it
    await waitFor(() => expect(dialog.getByLabelText("Name")).toHaveValue("vllm-cluster"));
    await expect(dialog.getByLabelText("Slug")).toHaveValue("vllm-cluster");
    await expect(dialog.getByLabelText("Strategy")).toHaveValue("cache_aware");
    await expect(dialog.getAllByLabelText("Provider")).toHaveLength(2);
  },
};

/**
 * A weight only means something under a strategy that reads it. GROUP is
 * `cache_aware`, so the inputs stay (the stored weights are kept) but are
 * disabled and the sheet says why; switching to `weighted` enables them (#2140).
 */
export const WeightsAreOffUnderCacheAware: Story = {
  render: () => <Stage mode="edit" group={GROUP} />,
  play: async () => {
    const dialog = within(sheet());
    const picker = await dialog.findByLabelText("Strategy");
    await waitFor(() => expect(picker).toHaveValue("cache_aware"));
    const weights = await dialog.findAllByLabelText("Relative weight");
    await expect(weights).toHaveLength(2);
    for (const w of weights) await expect(w).toBeDisabled();
    await waitFor(() =>
      expect(dialog.getByText(/cache_aware does not read weights/)).toBeVisible(),
    );
    await expect(dialog.getByText(/Leave Upstream model empty/)).toBeVisible();
    await pickOption(picker, /^weighted\b/);
    await waitFor(() => expect(dialog.getAllByLabelText("Relative weight")[0]).toBeEnabled());
    await expect(dialog.queryByText(/does not read weights/)).toBeNull();
  },
};

/**
 * A group with no members fails every request, so saving one is refused with an
 * inline error instead of a request; adding a member lets it through (#2140).
 */
export const EmptyGroupCannotBeSaved: Story = {
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await userEvent.type(dialog.getByLabelText("Name"), "vllm-cluster");
    await userEvent.click(dialog.getByRole("button", { name: "Create group" }));
    await waitFor(() => expect(dialog.getByText(/Add at least one member/)).toBeVisible());
    calls.expectNotSent("POST", `/orgs/${ORG.id}/provider-groups`);
    await userEvent.click(dialog.getByRole("button", { name: /add member/i }));
    await waitFor(() => expect(dialog.queryByText(/Add at least one member/)).toBeNull());
    await userEvent.click(dialog.getByRole("button", { name: "Create group" }));
    await calls.expectSentBody("POST", `/orgs/${ORG.id}/provider-groups`);
  },
};

/**
 * The strategy's own description stays under the picker, replacing the generic
 * "how requests are balanced" line, and adaptive can be chosen (#2136).
 */
export const StrategyDescriptionFollowsTheChoice: Story = {
  render: () => <Stage mode="edit" group={GROUP} />,
  play: async () => {
    const dialog = within(sheet());
    const picker = await dialog.findByLabelText("Strategy");
    await waitFor(() => expect(picker).toHaveValue("cache_aware"));
    await waitFor(() =>
      expect(dialog.getByText(en.strategies.descriptions.cacheAware)).toBeVisible(),
    );
    await expect(dialog.queryByText(/how requests are balanced/i)).toBeNull();
    await pickOption(picker, /^adaptive\b/);
    await waitFor(() => expect(picker).toHaveValue("adaptive"));
    await expect(await dialog.findByText(en.strategies.descriptions.adaptive)).toBeVisible();
    await expect(dialog.getByRole("link", { name: "Adaptive Routing settings" })).toHaveAttribute(
      "href",
      "/adaptive-settings",
    );
  },
};

/**
 * No providers in the org yet. A group is a fleet of providers, so the sheet
 * points at the thing that has to exist first rather than offering an empty
 * dropdown.
 */
export const NoProvidersYet: Story = {
  render: () => <Stage mode="add" providers={[]} />,
  play: async () => {
    const dialog = within(sheet());
    await waitFor(() => expect(dialog.getByText(/add providers first/)).toBeVisible());
    await expect(dialog.getByRole("button", { name: /add member/i })).toBeDisabled();
  },
};

/** Nothing to create until the group has a name. */
export const CannotCreateWithoutAName: Story = {
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await expect(dialog.getByRole("button", { name: "Create group" })).toBeDisabled();
  },
};

export const CreatesAGroup: Story = {
  render: () => <Stage mode="add" />,
  play: async () => {
    const dialog = within(sheet());
    await userEvent.type(dialog.getByLabelText("Name"), "vllm-cluster");
    await userEvent.click(dialog.getByRole("button", { name: /add member/i }));
    await userEvent.click(dialog.getByRole("button", { name: "Create group" }));
    const body = (await calls.expectSentBody("POST", `/orgs/${ORG.id}/provider-groups`)) as {
      name: string;
      members: { provider_id: string; weight: number }[];
    };
    await expect(body.name).toBe("vllm-cluster");
    // a member added with the defaults still carries a usable weight
    await expect(body.members).toEqual([{ provider_id: "prov-1", weight: 1 }]);
  },
};

/**
 * The slug is the group's address, so editing it is behind a switch: changing
 * it breaks every client already calling `group-slug/model`.
 */
export const SlugIsLockedUntilUnlocked: Story = {
  render: () => <Stage mode="edit" group={GROUP} />,
  play: async () => {
    const dialog = within(sheet());
    await waitFor(() => expect(dialog.getByLabelText("Slug")).toHaveValue("vllm-cluster"));
    await expect(dialog.getByLabelText("Slug")).toBeDisabled();
    await userEvent.click(dialog.getByRole("switch"));
    await expect(dialog.getByLabelText("Slug")).toBeEnabled();
    await waitFor(() => expect(dialog.getByText(/breaks any client using the old/)).toBeVisible());
  },
};

/** A rejected save keeps the draft and says why. */
export const SaveRejected: Story = {
  render: () => (
    <Stage
      mode="add"
      answer={async () => json({ error: { message: "slug 'vllm-cluster' is already taken" } }, 409)}
    />
  ),
  play: async () => {
    const dialog = within(sheet());
    await userEvent.type(dialog.getByLabelText("Name"), "vllm-cluster");
    await userEvent.click(dialog.getByRole("button", { name: /add member/i }));
    await userEvent.click(dialog.getByRole("button", { name: "Create group" }));
    await waitFor(() => expect(dialog.getByText(/already taken/)).toBeVisible());
    await expect(dialog.getByLabelText("Name")).toHaveValue("vllm-cluster");
  },
};

/**
 * An untouched draft closes without asking. A confirm on a form nobody edited
 * is what trains people to click through the one that matters (#868).
 */
export const ClosesCleanWithoutPrompting: Story = {
  render: () => <Stage mode="edit" group={GROUP} />,
  play: async () => {
    const dialog = within(sheet());
    await waitFor(() => expect(dialog.getByLabelText("Name")).toHaveValue("vllm-cluster"));
    await expectClosesWithoutPrompting();
  },
};

/** A dirty draft asks, and "cancel" keeps the sheet open with the edit intact. */
export const DiscardGuardKeepsTheDraft: Story = {
  render: () => <Stage mode="edit" group={GROUP} />,
  play: async () => {
    const dialog = within(sheet());
    // wait for the seed: an edit that lands before it would be overwritten
    await waitFor(() => expect(dialog.getByLabelText("Name")).toHaveValue("vllm-cluster"));
    await userEvent.type(dialog.getByLabelText("Name"), "-eu");
    await userEvent.click(dialog.getByRole("button", { name: /close/i }));
    await answerDiscardPrompt(false);
    await expect(dialog.getByLabelText("Name")).toHaveValue("vllm-cluster-eu");
  },
};

/** And "discard" closes it. */
export const DiscardGuardThrowsItAway: Story = {
  render: () => <Stage mode="edit" group={GROUP} />,
  play: async () => {
    const dialog = within(sheet());
    await waitFor(() => expect(dialog.getByLabelText("Name")).toHaveValue("vllm-cluster"));
    await userEvent.type(dialog.getByLabelText("Name"), "-eu");
    await userEvent.click(dialog.getByRole("button", { name: /close/i }));
    await answerDiscardPrompt(true);
    await expectSheetClosed();
  },
};

/** The strategy explains itself in an (i) (#3285). */
export const StrategyIsExplainedInAHint: Story = {
  render: () => <Stage mode="add" />,
  play: async () => {
    const f = en.providerGroupSheet.fields;
    await expectFieldHints(sheet(), en.common.aboutField, [[f.strategy, f.strategyInfo]]);
  },
};
