import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import LogErasure from "./LogErasure";
import {
  confirmation,
  expectAllowed,
  expectEmptyState,
  expectForbidden,
  expectListTable,
  expectLoadError,
  expectNoFalseEmpty,
  expectRefused,
  expectSkeleton,
  Harness,
  json,
  NEEDS_SUPERADMIN,
  ORG,
  pending,
  pickOption,
  recording,
  scoped,
  Toasted,
  type FetchStub,
} from "./story-harness";
import { ErasePayloadAction } from "@/components/LogErasure";
import type { InvocationRow, LogErasureRow } from "@/lib/api";

const ORG_UUID = "5b0b3c1e-0d6c-4c0e-8f53-6a1e2d3c4b5a";
const KEY_ID = "0c8c0e6e-6a3b-4b1d-9f0a-2d7a5b6c7d8e";

const erasure = (over: Partial<LogErasureRow> = {}): LogErasureRow => ({
  id: "er-1",
  scope_kind: "virtual_key",
  scope_id: KEY_ID,
  org_id: null,
  request_ts: null,
  include_logs: true,
  reason: "erasure request DSR-1042",
  requested_by: "user-1",
  status: "completed",
  statements: 2,
  error: null,
  created_at: "2026-10-27T12:00:00Z",
  completed_at: "2026-10-27T12:03:00Z",
  ...over,
});

const RECENT: LogErasureRow[] = [
  erasure({ id: "er-done" }),
  erasure({
    id: "er-bodies",
    scope_kind: "request",
    scope_id: "req_8f2c",
    org_id: ORG.id,
    include_logs: false,
    reason: "secret pasted into prompt, ticket SEC-77",
    status: "failed",
    statements: 0,
    error: "clickhouse refused the lookup of older payload rows",
    completed_at: null,
  }),
  erasure({
    id: "er-stale",
    scope_kind: "org",
    scope_id: ORG.id,
    reason: "offboarding",
    status: "unverified",
    completed_at: null,
  }),
];

function api({
  list = () => json({ data: RECENT }),
  create,
  read,
}: {
  list?: () => Response | Promise<Response>;
  create?: (body: Record<string, unknown>) => Response | Promise<Response>;
  read?: (id: string) => Response | Promise<Response>;
} = {}): FetchStub {
  return scoped(async (input, init) => {
    const url = new URL(String(input), "http://localhost");
    const method = (init?.method ?? "GET").toUpperCase();
    const one = url.pathname.match(/\/log-erasures\/([^/]+)$/);
    if (one) return read ? read(one[1]) : json(erasure({ id: one[1] }));
    if (url.pathname.endsWith("/log-erasures")) {
      if (method === "POST") {
        const body = JSON.parse(String(init?.body)) as Record<string, unknown>;
        return create
          ? create(body)
          : json(erasure({ id: "er-new", status: "pending", completed_at: null }), 202);
      }
      return list();
    }
    return json([]);
  });
}

const meta = {
  title: "Screens/LogErasure",
  component: LogErasure,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof LogErasure>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = {
  render: () => (
    <Harness fetchStub={api()}>
      <LogErasure />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectListTable(canvasElement, "Log Erasure");
    await waitFor(() => expect(canvas.getByText("erasure request DSR-1042")).toBeVisible());
    // each of the four outcomes is told apart; nothing but `completed` says erased
    await expect(canvas.getByText("Erased")).toBeVisible();
    await expect(canvas.getByText("Failed")).toBeVisible();
    await expect(canvas.getByText("Unverified")).toBeVisible();
    await expect(
      canvas.getByText("clickhouse refused the lookup of older payload rows"),
    ).toBeVisible();
    await expect(canvas.getByText("Bodies only")).toBeVisible();
  },
};

export const Loading: Story = {
  render: () => (
    <Harness fetchStub={pending}>
      <LogErasure />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
    await expectNoFalseEmpty(canvasElement, /Nothing has been erased/);
  },
};

export const Empty: Story = {
  render: () => (
    <Harness fetchStub={api({ list: () => json({ data: [] }) })}>
      <LogErasure />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectEmptyState(canvasElement, /Nothing has been erased/);
  },
};

export const LoadFailed: Story = {
  render: () => (
    <Harness
      fetchStub={api({ list: () => json({ error: { message: "erasure store is down" } }, 500) })}
    >
      <LogErasure />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /erasure store is down/);
    await expectNoFalseEmpty(canvasElement, /Nothing has been erased/);
  },
};

export const RefusedToAnAdmin: Story = {
  render: () => (
    <Harness fetchStub={api()} role="admin">
      <LogErasure />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectForbidden(canvasElement);
  },
};

const submits = recording(api());

/** The id must be a UUID and a reason is required before the erase can be asked for. */
export const SubmitsAfterAConfirmationAndListsThePendingRecord: Story = {
  render: () => (
    <Harness fetchStub={submits.stub} role="superadmin">
      <Toasted>
        <LogErasure />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const erase = await canvas.findByRole("button", { name: "Erase…" });
    // story-wait-allow: disabled by the form's own state until an id and a reason are given
    await expect(erase).toBeDisabled();

    await userEvent.type(canvas.getByLabelText("Subject id"), "not-an-id");
    await expect(canvas.getByText("This is not an id. Paste the full UUID.")).toBeVisible();
    await userEvent.clear(canvas.getByLabelText("Subject id"));
    await userEvent.type(canvas.getByLabelText("Subject id"), KEY_ID);
    // story-wait-allow: still disabled by the form's own state, with no reason given
    await expect(erase).toBeDisabled();
    await userEvent.type(canvas.getByLabelText("Reason"), "erasure request DSR-1042");
    await expectAllowed(canvasElement, "Erase…");

    await userEvent.click(erase);
    submits.expectNotSent("POST", "/api/v1/log-erasures");
    const dialog = within(await confirmation());
    await waitFor(() =>
      expect(
        dialog.getByRole("heading", { name: `Erase the logs of virtual key ${KEY_ID}?` }),
      ).toBeVisible(),
    );
    await userEvent.click(dialog.getByRole("button", { name: "Erase" }));

    const body = await submits.expectSentBody<Record<string, unknown>>(
      "POST",
      "/api/v1/log-erasures",
    );
    await expect(body).toEqual({
      scope: { kind: "virtual_key", id: KEY_ID },
      reason: "erasure request DSR-1042",
      include_logs: true,
    });
  },
};

const bodiesOnly = recording(api());

export const CanKeepTheLogRowsAndDeleteTheBodiesOnly: Story = {
  render: () => (
    <Harness fetchStub={bodiesOnly.stub} role="superadmin">
      <LogErasure />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await pickOption(await canvas.findByLabelText("Subject"), /^Organization/);
    await userEvent.type(canvas.getByLabelText("Subject id"), ORG_UUID);
    await userEvent.type(canvas.getByLabelText("Reason"), "offboarding");
    await userEvent.click(canvas.getByRole("switch", { name: "Delete the log rows too" }));
    await expect(
      canvas.getByText(/Deletes the captured prompt and response bodies only/),
    ).toBeVisible();
    const erase = canvas.getByRole("button", { name: "Erase…" });
    await expectAllowed(canvasElement, "Erase…");
    await userEvent.click(erase);
    await userEvent.click(within(await confirmation()).getByRole("button", { name: "Erase" }));
    const body = await bodiesOnly.expectSentBody<Record<string, unknown>>(
      "POST",
      "/api/v1/log-erasures",
    );
    await expect(body.include_logs).toBe(false);
    await expect((body.scope as { kind: string }).kind).toBe("org");
  },
};

/** No ClickHouse behind the control plane: the 503 stays in the dialog with its words. */
export const NoClickHouseRefusesTheErasure: Story = {
  render: () => (
    <Harness
      fetchStub={api({
        create: () =>
          json({ error: { message: "analytics is not configured: set clickhouse_url" } }, 503),
      })}
      role="superadmin"
    >
      <LogErasure />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.type(await canvas.findByLabelText("Subject id"), KEY_ID);
    await userEvent.type(canvas.getByLabelText("Reason"), "ticket 1");
    const erase = canvas.getByRole("button", { name: "Erase…" });
    await expectAllowed(canvasElement, "Erase…");
    await userEvent.click(erase);
    const dialog = within(await confirmation());
    await userEvent.click(dialog.getByRole("button", { name: "Erase" }));
    await waitFor(() =>
      expect(dialog.getByRole("alert")).toHaveTextContent(/analytics is not configured/),
    );
  },
};

// ------------------------------------------------- the action on an LLM Logs row

const logRow = (over: Partial<InvocationRow> = {}): InvocationRow =>
  ({
    ts: "2026-10-27 12:30:45.123",
    request_id: "req_8f2c",
    org_id: ORG.id,
    ...over,
  }) as InvocationRow;

function Action({ row = logRow(), pollMs }: { row?: InvocationRow; pollMs?: number }) {
  return <ErasePayloadAction row={row} pollMs={pollMs} />;
}

const actionMeta = recording(
  api({
    read: () =>
      json(erasure({ id: "er-new", status: "pending", mutations_done: 0, completed_at: null })),
  }),
);

export const ErasesOneRequestsPayload: Story = {
  render: () => (
    <Harness fetchStub={actionMeta.stub} role="superadmin">
      <Action />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const button = await canvas.findByRole("button", { name: "Erase captured payload" });
    await expectAllowed(canvasElement, "Erase captured payload");
    await userEvent.click(button);
    const dialog = within(await confirmation());
    await waitFor(() =>
      expect(
        dialog.getByRole("heading", { name: /Erase the captured payload of req_8f2c\?/ }),
      ).toBeVisible(),
    );
    // the audit trail keeps a reason, so there is no erasing without one
    const confirm = dialog.getByRole("button", { name: "Erase payload" });
    // story-wait-allow: disabled by confirmDisabled until a reason is typed
    await expect(confirm).toBeDisabled();
    await userEvent.type(
      dialog.getByLabelText("Reason"),
      "secret pasted into prompt, ticket SEC-77",
    );
    await waitFor(() => expect(confirm).not.toBeDisabled());
    await userEvent.click(confirm);

    const body = await actionMeta.expectSentBody<Record<string, unknown>>(
      "POST",
      "/api/v1/log-erasures",
    );
    // the bodies only, one request, at the instant the row shows (UTC)
    await expect(body).toEqual({
      scope: {
        kind: "request",
        request_id: "req_8f2c",
        org_id: ORG.id,
        ts: "2026-10-27T12:30:45.123Z",
      },
      reason: "secret pasted into prompt, ticket SEC-77",
      include_logs: false,
    });
    // submitted, not erased: the record is pending until ClickHouse says so
    await waitFor(() => expect(canvas.getByRole("status")).toHaveTextContent("Pending"));
  },
};

let reads = 0;
const progressing = api({
  create: () =>
    json(erasure({ id: "er-live", status: "pending", statements: 2, completed_at: null }), 202),
  read: () => {
    reads += 1;
    return reads === 1
      ? json(
          erasure({
            id: "er-live",
            status: "pending",
            statements: 2,
            mutations_done: 1,
            completed_at: null,
          }),
        )
      : json(erasure({ id: "er-live", status: "completed" }));
  },
});

/** The pending record is asked again until ClickHouse reports every mutation done. */
export const PendingBecomesCompleted: Story = {
  render: () => {
    reads = 0;
    return (
      <Harness fetchStub={progressing} role="superadmin">
        <Action pollMs={300} />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "Erase captured payload" }));
    const dialog = within(await confirmation());
    await userEvent.type(dialog.getByLabelText("Reason"), "ticket");
    await userEvent.click(dialog.getByRole("button", { name: "Erase payload" }));
    await waitFor(() =>
      expect(canvas.getByRole("status")).toHaveTextContent("1 of 2 deletions done"),
    );
    await waitFor(() => expect(canvas.getByRole("status")).toHaveTextContent("Erased"));
    await expect(canvas.queryByText(/Pending/)).toBeNull();
    await expect(canvas.getByText(/ClickHouse reports every deletion done/)).toBeVisible();
  },
};

export const SubmissionFailureIsSaidAsFailed: Story = {
  render: () => (
    <Harness
      fetchStub={api({
        create: () =>
          json(
            erasure({
              id: "er-bad",
              status: "failed",
              statements: 0,
              error: "clickhouse refused the statement",
              completed_at: null,
            }),
            202,
          ),
      })}
      role="superadmin"
    >
      <Action />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "Erase captured payload" }));
    const dialog = within(await confirmation());
    await userEvent.type(dialog.getByLabelText("Reason"), "ticket");
    await userEvent.click(dialog.getByRole("button", { name: "Erase payload" }));
    await waitFor(() => expect(canvas.getByRole("status")).toHaveTextContent("Failed"));
    await expect(canvas.getByText("clickhouse refused the statement")).toBeVisible();
  },
};

export const ActionRefusedToAnAdmin: Story = {
  render: () => (
    <Harness fetchStub={api()} role="admin">
      <Action />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, "Erase captured payload", NEEDS_SUPERADMIN);
  },
};

export const ARowWithNoOrganizationCannotBeAddressed: Story = {
  render: () => (
    <Harness fetchStub={api()} role="superadmin">
      <Action row={logRow({ org_id: "" })} />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const button = await within(canvasElement).findByRole("button", {
      name: "Erase captured payload",
    });
    // story-wait-allow: disabled by its own prop from the first paint
    await expect(button).toBeDisabled();
    await expect(button).toHaveAttribute(
      "title",
      expect.stringContaining("records no organization"),
    );
  },
};

/** A time that does not parse would erase every row sharing the id, so the action is not offered. */
export const ARowWhoseTimeCannotBeReadCannotBeAddressed: Story = {
  render: () => (
    <Harness fetchStub={api()} role="superadmin">
      <Action row={logRow({ ts: "yesterday" })} />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const button = await within(canvasElement).findByRole("button", {
      name: "Erase captured payload",
    });
    // story-wait-allow: disabled by its own prop from the first paint
    await expect(button).toBeDisabled();
    await expect(button).toHaveAttribute(
      "title",
      expect.stringContaining("time could not be read"),
    );
  },
};
