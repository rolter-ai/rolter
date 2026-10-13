import type { Meta, StoryObj } from "@storybook/react-vite";
import * as React from "react";
import { expect, fn, userEvent, waitFor, within } from "storybook/test";

import {
  ChangesPanel,
  DeleteBySlugDialog,
  MakeLiveDialog,
  MakeLiveLabel,
  RepositoryIndex,
  VersionBadge,
  VersionMeta,
  VersionRail,
  liveAction,
} from "./versioned-repository";
import { GatedButton } from "@/components/GatedButton";
import { Button } from "@/components/ui/button";
import { FieldDiff } from "@/components/ui/text-diff";
import { Harness, expectAllowed, expectRefused, routes } from "@/pages/story-harness";
import { diffFields } from "@/lib/text-diff";
import { atMobile } from "@/lib/story-viewport";

// the pieces the Prompt Repository and the Skills Repository are both built
// from (#2221), drawn once with plain data. Each screen's own stories cover it
// in place; these hold the contract of the kit itself: which verb a version
// offers, what folds on a phone, what stays locked until the slug is typed

interface Row {
  version: number;
  created_at: string;
}

const versions: Row[] = [
  { version: 3, created_at: "2026-08-01T10:00:00Z" },
  { version: 2, created_at: "2026-07-30T12:15:00Z" },
  { version: 1, created_at: "2026-07-28T09:05:00Z" },
];

const stub = routes([]);

function makeLive(onMakeLive: (version: number) => void, retired = false) {
  return (version: number, action: "publish" | "rollback") =>
    retired ? null : action === "publish" ? (
      <GatedButton
        gate="skill:update"
        control="rail-publish"
        variant="ghost"
        onClick={() => onMakeLive(version)}
      >
        <MakeLiveLabel action={action} version={version} size="sm" />
      </GatedButton>
    ) : (
      <GatedButton
        gate="skill:update"
        control="rail-rollback"
        variant="ghost"
        onClick={() => onMakeLive(version)}
      >
        <MakeLiveLabel action={action} version={version} size="sm" />
      </GatedButton>
    );
}

function Rail({
  published = 2,
  retired = false,
  loading = false,
  rows = versions,
  role = "admin",
  onSelect = () => {},
  onMakeLive = () => {},
}: {
  published?: number | null;
  retired?: boolean;
  loading?: boolean;
  rows?: Row[];
  role?: "admin" | "viewer";
  onSelect?: (version: number) => void;
  onMakeLive?: (version: number) => void;
}) {
  return (
    <Harness fetchStub={stub} role={role}>
      <VersionRail
        label="Version history"
        hint="Saved versions cannot be edited."
        versions={rows}
        publishedVersion={published}
        selectedVersion={2}
        loading={loading}
        loadingTestId="rail-loading"
        emptyText="No saved versions"
        renderMeta={(version) => <VersionMeta>{`meta ${version.version}`}</VersionMeta>}
        renderMakeLive={makeLive(onMakeLive, retired)}
        onSelect={onSelect}
      />
    </Harness>
  );
}

const meta = {
  title: "Screens/VersionedRepository",
  parameters: { layout: "padded" },
} satisfies Meta;

export default meta;
type Story = StoryObj<typeof meta>;

export const LiveAction: Story = {
  render: () => <div />,
  play: async () => {
    // newer than the live version is published, older is rolled back to, and
    // with nothing live every version is a publish
    await expect(liveAction(3, 2)).toBe("publish");
    await expect(liveAction(1, 2)).toBe("rollback");
    await expect(liveAction(1, null)).toBe("publish");
    await expect(liveAction(1, undefined)).toBe("publish");
  },
};

/** a publish on the newer version, a roll back on the older, neither on the live one */
export const RailOffersTheVerbThatFollowsTheDirection: Story = {
  render: () => <Rail />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectAllowed(canvasElement, "Publish v3");
    await expectAllowed(canvasElement, "Roll back to v1");
    await expect(canvas.queryByRole("button", { name: "Publish v2" })).toBeNull();
    await expect(canvas.queryByRole("button", { name: "Roll back to v2" })).toBeNull();
    await expect(canvas.getByText("Published")).toBeVisible();
    await expect(canvas.getByText("meta 3")).toBeVisible();
    await expect(canvas.getByRole("button", { name: /^v2\b/ })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
  },
};

const selects = fn();
const makesLive = fn();

export const RailSelectsAndMakesLive: Story = {
  render: () => <Rail onSelect={selects} onMakeLive={makesLive} />,
  play: async ({ canvasElement }) => {
    selects.mockClear();
    makesLive.mockClear();
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "Publish v3" }));
    await expect(makesLive).toHaveBeenCalledWith(3);
    await userEvent.click(canvas.getByRole("button", { name: "Roll back to v1" }));
    await expect(makesLive).toHaveBeenLastCalledWith(1);
    await userEvent.click(canvas.getByRole("button", { name: /^v1\b/ }));
    await expect(selects).toHaveBeenCalledWith(1);
    // choosing a version is not making it live
    await expect(makesLive).toHaveBeenCalledTimes(2);
  },
};

/** nothing can be made live on a retired row, so the rail offers neither verb */
export const RetiredOffersNoVerb: Story = {
  render: () => <Rail retired />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByRole("button", { name: /^v3\b/ });
    await expect(canvas.queryByRole("button", { name: /^Publish/ })).toBeNull();
    await expect(canvas.queryByRole("button", { name: /^Roll back/ })).toBeNull();
  },
};

export const RailRefusedToAViewer: Story = {
  render: () => <Rail role="viewer" />,
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, "Publish v3");
    await expectRefused(canvasElement, "Roll back to v1");
  },
};

export const RailLoading: Story = {
  render: () => <Rail loading />,
  play: async ({ canvasElement }) => {
    await expect(await within(canvasElement).findByTestId("rail-loading")).toBeDefined();
  },
};

export const RailEmpty: Story = {
  render: () => <Rail rows={[]} published={null} />,
  play: async ({ canvasElement }) => {
    await expect(await within(canvasElement).findByText("No saved versions")).toBeVisible();
  },
};

/** under `lg` the history is one line that opens the list; from `lg` it is always open */
export const RailFoldsOnAPhone: Story = {
  ...atMobile,
  render: () => <Rail />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const toggle = await canvas.findByRole("button", { name: /Version history/ });
    await expect(toggle).toHaveAttribute("aria-expanded", "false");
    await expect(toggle).toHaveTextContent("3 versions · v2 selected");
    await expect(canvas.queryByRole("button", { name: /^v3\b/ })).toBeNull();
    await userEvent.click(toggle);
    await expect(toggle).toHaveAttribute("aria-expanded", "true");
    await expect(await canvas.findByRole("button", { name: /^v3\b/ })).toBeVisible();
    await userEvent.click(toggle);
    await waitFor(() => expect(canvas.queryByRole("button", { name: /^v3\b/ })).toBeNull());
  },
};

export const IndexSelectsAndCreates: Story = {
  render: () => {
    const Index = () => {
      const [selected, setSelected] = React.useState("a");
      return (
        <Harness fetchStub={stub} role="admin">
          <RepositoryIndex
            label="Templates"
            count="2 in this org"
            empty="No templates yet"
            create={
              <GatedButton
                gate="skill:create"
                control="repo-new"
                variant="ghost"
                aria-label="Create template"
              />
            }
            items={[
              {
                id: "a",
                name: "Support concierge",
                slug: "support-concierge",
                badge: <VersionBadge version={2} draft="draft" />,
              },
              {
                id: "b",
                name: "Billing helper",
                slug: "billing-helper",
                badge: <VersionBadge version={null} draft="draft" />,
              },
            ]}
            selectedId={selected}
            onSelect={setSelected}
          />
        </Harness>
      );
    };
    return <Index />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const index = within(await canvas.findByRole("complementary", { name: "Templates" }));
    await expect(index.getByRole("button", { name: /Support concierge/ })).toHaveAttribute(
      "aria-current",
      "true",
    );
    await waitFor(() => expect(index.getByText("v2")).toBeVisible());
    await expect(index.getByText("draft")).toBeVisible();
    await userEvent.click(index.getByRole("button", { name: /Billing helper/ }));
    await expect(index.getByRole("button", { name: /Billing helper/ })).toHaveAttribute(
      "aria-current",
      "true",
    );
    await expectAllowed(canvasElement, "Create template");
  },
};

function ChangesDialog({ changed = 3 }: { changed?: number }) {
  const [open, setOpen] = React.useState(false);
  return (
    <Harness fetchStub={stub}>
      <Button onClick={() => setOpen(true)}>Publish v3</Button>
      <MakeLiveDialog
        uxName="story"
        open={open}
        action="publish"
        version={3}
        title="Publish v3 of Support concierge?"
        description="Callers get v3 instead of v2."
        pending={false}
        error={undefined}
        onOpenChange={setOpen}
        onConfirm={() => setOpen(false)}
      >
        <ChangesPanel live={2} target={3} changed={changed} collapsible>
          <FieldDiff
            label="Metadata"
            changes={diffFields({ runtime: '"node"' }, { runtime: '"bun"' })}
          />
        </ChangesPanel>
      </MakeLiveDialog>
    </Harness>
  );
}

/** the comparison is folded behind a button, and says how much there is to read first */
export const ChangesFoldBehindAButton: Story = {
  render: () => <ChangesDialog />,
  play: async ({ canvasElement }) => {
    await userEvent.click(await within(canvasElement).findByRole("button", { name: "Publish v3" }));
    const dialog = within(await within(document.body).findByRole("dialog"));
    await waitFor(() =>
      expect(
        dialog.getByRole("heading", { name: "Publish v3 of Support concierge?" }),
      ).toBeVisible(),
    );
    const changes = within(dialog.getByRole("region", { name: "Changes from live v2 to v3" }));
    await expect(changes.getByText("3 differences")).toBeVisible();
    await expect(changes.queryByRole("list", { name: "Metadata" })).toBeNull();
    await userEvent.click(changes.getByRole("button", { name: "Show changes" }));
    await expect(changes.getByRole("list", { name: "Metadata" })).toBeVisible();
    await userEvent.click(changes.getByRole("button", { name: "Hide changes" }));
    await expect(changes.queryByRole("list", { name: "Metadata" })).toBeNull();
    // the confirm names the direction it was opened with
    await expect(dialog.getByRole("button", { name: "Publish v3" })).toBeVisible();
  },
};

export const ChangesNoneSaysIdentical: Story = {
  render: () => <ChangesDialog changed={0} />,
  play: async ({ canvasElement }) => {
    await userEvent.click(await within(canvasElement).findByRole("button", { name: "Publish v3" }));
    const dialog = within(await within(document.body).findByRole("dialog"));
    await waitFor(() => expect(dialog.getByText("v3 is identical to live v2.")).toBeVisible());
    await expect(dialog.queryByRole("button", { name: "Show changes" })).toBeNull();
  },
};

function DeleteDialog({ onConfirm }: { onConfirm: () => void }) {
  const [open, setOpen] = React.useState(false);
  return (
    <Harness fetchStub={stub}>
      <Button onClick={() => setOpen(true)}>Delete</Button>
      <DeleteBySlugDialog
        uxName="story-delete"
        open={open}
        slug="support-concierge"
        title="Delete template Support concierge?"
        description="Every version is deleted with it."
        slugLabel="Type support-concierge to confirm"
        confirmLabel="Delete template"
        pending={false}
        error={null}
        onOpenChange={setOpen}
        onConfirm={onConfirm}
      />
    </Harness>
  );
}

/** the destructive button stays locked until the slug is typed back, and the box is empty on the next open */
export const DeleteStaysLockedUntilTheSlugMatches: Story = {
  render: () => <DeleteDialog onConfirm={fn()} />,
  play: async ({ canvasElement }) => {
    await userEvent.click(await within(canvasElement).findByRole("button", { name: "Delete" }));
    let dialog = within(await within(document.body).findByRole("dialog"));
    const confirm = dialog.getByRole("button", { name: "Delete template" });
    await expect(confirm).toBeDisabled();
    const box = dialog.getByRole("textbox", { name: "Type support-concierge to confirm" });
    await userEvent.type(box, "support");
    await expect(confirm).toBeDisabled();
    await userEvent.type(box, "-concierge");
    await expect(confirm).toBeEnabled();
    await userEvent.click(dialog.getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(within(document.body).queryByRole("dialog")).toBeNull());
    await userEvent.click(within(canvasElement).getByRole("button", { name: "Delete" }));
    dialog = within(await within(document.body).findByRole("dialog"));
    await expect(
      dialog.getByRole("textbox", { name: "Type support-concierge to confirm" }),
    ).toHaveValue("");
  },
};
