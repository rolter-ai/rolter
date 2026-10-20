import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { LabelSheet } from "./Labels";
import type { LabelRow } from "@/lib/api";
import en from "@/lib/i18n/locales/en.json";
import { Harness, json, scoped } from "@/pages/story-harness";

const meta = {
  title: "Forms/LabelSheet",
  component: LabelSheet,
  args: {
    open: true,
    onOpenChange: () => {},
    orgId: "org-1",
    subjectType: "provider",
    subjectId: "provider-1",
    subjectName: "openai-prod",
  },
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof LabelSheet>;
export default meta;
type Story = StoryObj<typeof meta>;

let finishDelete: () => void;
let deletes = 0;

export const RemoveCustomLabel: Story = {
  render: () => {
    deletes = 0;
    let removed = false;
    const row: LabelRow = {
      id: "label-1",
      subject_type: "provider",
      subject_id: "provider-1",
      key: "region",
      value: "eu",
      source: "custom",
      created_at: "2026-10-01T00:00:00Z",
      updated_at: "2026-10-01T00:00:00Z",
    };
    const stub = scoped(async (_input, init) => {
      if (init?.method === "DELETE") {
        deletes += 1;
        await new Promise<void>((resolve) => {
          finishDelete = resolve;
        });
        removed = true;
        return new Response(null, { status: 204 });
      }
      return json(removed ? [] : [row]);
    });
    return (
      <Harness fetchStub={stub}>
        <LabelSheet
          open
          onOpenChange={() => {}}
          orgId="org-1"
          subjectType="provider"
          subjectId="provider-1"
          subjectName="openai-prod"
        />
      </Harness>
    );
  },
  play: async () => {
    const screen = within(document.body);
    const remove = await screen.findByRole("button", {
      name: en.labels.removeOne.replace("{{label}}", "region=eu"),
    });
    await userEvent.click(remove);
    const dialog = await screen.findByRole("dialog", {
      name: en.labels.confirm.title.replace("{{label}}", "region=eu"),
    });
    await waitFor(() => expect(dialog).toBeVisible());
    await expect(deletes).toBe(0);
    await userEvent.click(within(dialog).getByRole("button", { name: en.common.delete }));
    await waitFor(() => expect(deletes).toBe(1));
    await waitFor(() => expect(remove).toBeDisabled());
    await expect(remove.querySelector(".lucide-loader-circle")).not.toBeNull();
    finishDelete();
    await waitFor(() => expect(remove).not.toBeInTheDocument());
  },
};
