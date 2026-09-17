import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import Playground from "./Playground";
import { atMobile, expectNoHorizontalOverflow } from "@/lib/story-viewport";

const KEY_STORAGE = "rolter.playground.key";

/** What the gateway serves: a route, a provider pin, and a provider group. */
const GATEWAY_MODELS = {
  data: [
    { id: "minicpm5-1b", object: "model", owned_by: "rolter" },
    { id: "fake-llm", object: "model", owned_by: "rolter" },
    { id: "gpustack/minicpm5-1b", object: "model", owned_by: "vllm-test" },
    { id: "abc/minicpm5-1b", object: "model", owned_by: "abc" },
  ],
};

/** What the control plane serves: bare route ids, nothing else. */
const ROUTES = [{ id: "r-1", model: "minicpm5-1b", strategy: "round_robin" }];

type FetchStub = (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });

const url = (input: RequestInfo | URL): string =>
  typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;

/**
 * Routes always resolve; the gateway's `/v1/models` answers according to
 * `gateway`, so a story can put the picker in each of its three states.
 */
function stubFor(gateway: () => Promise<Response>): FetchStub {
  return async (input) => {
    if (url(input).includes("/gw/v1/models")) return gateway();
    return json(ROUTES);
  };
}

// installed during render, not in an effect: child effects run before the
// parent's, so an effect would let the first real fetch through
function Harness({ fetchStub, playgroundKey }: { fetchStub: FetchStub; playgroundKey: string }) {
  const original = React.useRef<typeof globalThis.fetch | null>(null);
  const client = React.useMemo(() => {
    original.current ??= globalThis.fetch;
    globalThis.fetch = fetchStub as typeof globalThis.fetch;
    try {
      if (playgroundKey) localStorage.setItem(KEY_STORAGE, playgroundKey);
      else localStorage.removeItem(KEY_STORAGE);
    } catch {
      // localStorage unavailable in this runner; the no-key path is the default
    }
    return new QueryClient({ defaultOptions: { queries: { retry: false } } });
  }, [fetchStub, playgroundKey]);
  React.useEffect(
    () => () => {
      if (original.current) globalThis.fetch = original.current;
      try {
        localStorage.removeItem(KEY_STORAGE);
      } catch {
        // nothing to clean up
      }
    },
    [],
  );
  return (
    <QueryClientProvider client={client}>
      <Playground />
    </QueryClientProvider>
  );
}

const meta = {
  title: "Screens/Playground",
  component: Playground,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof Playground>;

export default meta;
type Story = StoryObj<typeof meta>;

/**
 * With a working key the picker lists what the gateway actually serves, and
 * groups it by owner so a route, a provider pin and a provider group are told
 * apart rather than all reading as bare strings (#946).
 */
export const GatewayModels: Story = {
  render: () => (
    <Harness playgroundKey="rolter-test-key" fetchStub={stubFor(async () => json(GATEWAY_MODELS))} />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const picker = await canvas.findByRole("combobox", { name: "Model" });
    await userEvent.click(picker);
    const listbox = canvas.getByRole("listbox");
    await waitFor(() =>
      expect(within(listbox).getByRole("option", { name: "abc/minicpm5-1b" })).toBeTruthy(),
    );
    // the group address is selectable — the whole point of the screen
    await expect(
      within(listbox).getByRole("option", { name: "gpustack/minicpm5-1b" }),
    ).toBeTruthy();

    // grouped by owner, so the three kinds of address are visually distinct
    const groups = within(listbox)
      .getAllByRole("group")
      .map((g) => g.getAttribute("aria-label"));
    await expect(groups).toContain("vllm-test");
    await expect(groups).toContain("abc");
    await userEvent.keyboard("{Escape}");

    // nothing to explain when the list is the gateway's own
    await expect(canvas.queryByText(/Showing configured routes/)).toBeNull();
  },
};

/**
 * No key yet: the fallback route list is still shown, but the picker says what
 * it is showing and what is missing from it. Silently substituting a strictly
 * smaller list is what made a provider group look like it did not exist.
 */
export const NoKeySaysWhatIsMissing: Story = {
  render: () => (
    <Harness playgroundKey="" fetchStub={stubFor(async () => json({ data: [] }))} />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText(/Showing configured routes/)).toBeVisible());
    await userEvent.click(canvas.getByRole("combobox", { name: "Model" }));
    const listbox = canvas.getByRole("listbox");
    // the picker stays usable rather than emptying out
    await expect(within(listbox).getByRole("option", { name: "fake-llm" })).toBeTruthy();
    // and the gateway-only addresses are genuinely absent, as the notice says
    await expect(
      within(listbox).queryByRole("option", { name: "abc/minicpm5-1b" }),
    ).toBeNull();
    await userEvent.keyboard("{Escape}");
  },
};

/**
 * A key the gateway rejects is a different message from no key at all: one is
 * "you have not set one", the other "the one you set did not work".
 */
export const RejectedKeySaysSo: Story = {
  render: () => (
    <Harness
      playgroundKey="rolter-bad-key"
      fetchStub={stubFor(async () => json({ error: { message: "invalid key" } }, 401))}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() =>
      expect(canvas.getByText(/Could not read the gateway's model list/)).toBeVisible(),
    );
    await expect(canvas.queryByText(/Showing configured routes/)).toBeNull();
  },
};

// the model picker row and the composer wrap on a phone (#1242)
export const Mobile: Story = {
  ...atMobile,
  render: () => (
    <Harness playgroundKey="rolter-test-key" fetchStub={stubFor(async () => json(GATEWAY_MODELS))} />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvasElement.querySelector('[role="combobox"]')).toBeTruthy());
    void canvas;
    await expectNoHorizontalOverflow();
  },
};
