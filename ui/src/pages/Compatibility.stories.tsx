import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import Compatibility from "./Compatibility";
import { Toasted, expectSkeleton, expectToast, expectForbidden } from "./story-harness";
import type { CompatibilityPolicyDto } from "@/lib/api";

const BASE: CompatibilityPolicyDto = {
  anthropic_version: "2023-06-01",
  default_max_tokens: 4096,
  updated_at: "2026-07-30T12:00:00Z",
  restart_required: [],
};

type FetchStub = (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });

// the stub is installed during render, not in an effect: child effects run
// before the parent's, so an effect would let the first real fetch through
function Harness({ fetchStub }: { fetchStub: FetchStub }) {
  const original = React.useRef<typeof globalThis.fetch | null>(null);
  const client = React.useMemo(() => {
    original.current ??= globalThis.fetch;
    globalThis.fetch = fetchStub as typeof globalThis.fetch;
    return new QueryClient({ defaultOptions: { queries: { retry: false } } });
  }, [fetchStub]);
  React.useEffect(
    () => () => {
      if (original.current) globalThis.fetch = original.current;
    },
    [],
  );
  return (
    <QueryClientProvider client={client}>
      <Toasted>
        <Compatibility />
      </Toasted>
    </QueryClientProvider>
  );
}

const meta = {
  title: "Screens/Compatibility",
  component: Compatibility,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof Compatibility>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = {
  render: () => <Harness fetchStub={async () => json(BASE)} />,
};

export const Loading: Story = {
  render: () => <Harness fetchStub={() => new Promise<Response>(() => {})} />,
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
  },
};

// a non-superadmin principal gets 403
export const Forbidden: Story = {
  render: () => (
    <Harness fetchStub={async () => json({ error: { message: "forbidden" } }, 403)} />
  ),
  play: async ({ canvasElement }) => {
    await expectForbidden(canvasElement);
  },
};

// the server owns the restart list, so the notice appears whenever it is
// non-empty without the screen knowing which fields are on it
export const RestartRequired: Story = {
  render: () => (
    <Harness
      fetchStub={async () =>
        json({ ...BASE, restart_required: ["anthropic_version"] })
      }
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("RESTART")).toBeVisible());
    await expect(canvas.getByText("anthropic_version")).toBeVisible();
  },
};

// the version is forwarded verbatim as the anthropic-version header, so a
// non-dated value is blocked before it can fail every Anthropic call upstream
export const RejectsAnUndatedVersion: Story = {
  render: () => <Harness fetchStub={async () => json(BASE)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const version = await canvas.findByLabelText("Anthropic API version");
    await userEvent.clear(version);
    await userEvent.type(version, "latest");
    await waitFor(() =>
      expect(
        canvas.getByText("Anthropic version must be a dated release like 2023-06-01."),
      ).toBeVisible(),
    );
    await expect(canvas.getByRole("button", { name: "Save Changes" })).toBeDisabled();
  },
};

// interaction: a valid edit round-trips and confirms
export const SavesChanges: Story = {
  render: () => {
    const stub: FetchStub = async (_input, init) => {
      if (init?.method === "PUT") {
        return json({ ...BASE, ...JSON.parse(String(init.body)) });
      }
      return json(BASE);
    };
    return <Harness fetchStub={stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const tokens = await canvas.findByLabelText("Default max tokens");
    await userEvent.clear(tokens);
    await userEvent.type(tokens, "8192");
    await userEvent.click(canvas.getByRole("button", { name: "Save Changes" }));
    await expectToast(canvasElement, /compatibility settings updated/i);
    await expect(canvas.getByLabelText("Default max tokens")).toHaveValue("8192");
  },
};
