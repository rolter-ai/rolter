import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import ClientSettings from "./ClientSettings";
import {
  expectForbidden,
  expectLoadError,
  expectSkeleton,
  expectToast,
  Harness as ScreenHarness,
  expectRetryRecovers,
  failingThenRecovering,
  json,
  Toasted,
  type FetchStub,
  type StoryRole,
} from "./story-harness";
import type { ClientSettingsDto } from "@/lib/api";
import en from "@/lib/i18n/locales/en.json";
import ru from "@/lib/i18n/locales/ru.json";
import { phoneFits } from "@/lib/story-viewport";

const RESERVED = ["authorization", "x-api-key", "host", "cookie"];

const BASE: ClientSettingsDto = {
  public_base_url: null,
  forwarded_headers: [],
  injected_headers: {},
  request_id_header: "x-request-id",
  updated_at: "2026-08-05T12:00:00Z",
  always_propagated: ["traceparent", "tracestate", "b3"],
  reserved: RESERVED,
  request_id_refused: ["authorization", "cookie", "set-cookie", "te", "x-goog-api-key"],
};

const CONFIGURED: ClientSettingsDto = {
  ...BASE,
  public_base_url: "https://gateway.example.com",
  forwarded_headers: ["x-tenant-id"],
  injected_headers: { "x-partner-id": "acme" },
};

/**
 * The screen under the shared fetch-stub harness, with a role to render as.
 *
 * `role` is what a story needs to mount a `CapabilityProvider` at all: with no
 * provider above it `can()` answers "unknown", the `superadminOnly` wrapper
 * never blocks, and a story can only reach the 403 by stubbing one — which
 * tests the screen's own error path rather than the gate (#1606).
 */
function Harness({ fetchStub, role }: { fetchStub: FetchStub; role?: StoryRole }) {
  return (
    <ScreenHarness fetchStub={fetchStub} role={role}>
      <Toasted>
        <ClientSettings />
      </Toasted>
    </ScreenHarness>
  );
}

const meta = {
  title: "Screens/ClientSettings",
  component: ClientSettings,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof ClientSettings>;

export default meta;
type Story = StoryObj<typeof meta>;

/**
 * The example request's text. The highlighter splits it across token spans,
 * so it is the region's text that carries the command, not any one node.
 */
const example = (canvasElement: HTMLElement) =>
  within(canvasElement).getByRole("region", {
    name: new RegExp(`^${en.pages.clientSettings.exampleRequest} `),
  }).textContent ?? "";

/**
 * The shipped default: no base URL override, no header policy.
 *
 * With no address saved there is no example request: the dashboard's `/gw`
 * proxy needs a dashboard session, so it is no address for a client (#2486).
 * The field's placeholder is only a shape.
 */
export const Empty: Story = {
  render: () => <Harness fetchStub={async () => json(BASE)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText(/No headers injected/)).toBeVisible());
    await expect(canvas.getByRole("note")).toHaveTextContent(en.common.gatewayBasePrompt);
    await expect(canvasElement.textContent ?? "").not.toContain("/gw/v1");
    await expect(canvas.getByLabelText("Public base URL")).toHaveAttribute(
      "placeholder",
      "https://gateway.example.com",
    );
    await expect(canvas.getByText(en.pages.clientSettings.baseUrlHint)).toBeVisible();
    for (const key of ["baseUrl", "forwarded", "injected", "correlation"] as const) {
      await expect(
        canvas.getByRole("heading", { level: 2, name: en.pages.clientSettings[key] }),
      ).toBeVisible();
    }
  },
};

/** A saved public base URL: the example calls the gateway there, not through `/gw`. */
export const Configured: Story = {
  render: () => <Harness fetchStub={async () => json(CONFIGURED)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByLabelText("Public base URL")).toHaveValue(
      "https://gateway.example.com",
    );
    await expect(canvas.getByLabelText("Injected header name 1")).toHaveValue("x-partner-id");
    await expect(example(canvasElement)).toContain(
      "curl https://gateway.example.com/v1/chat/completions",
    );
    await expect(example(canvasElement)).not.toContain("/gw/");
  },
};

/**
 * The example follows the field as it is typed, before anything is saved, and
 * an address pasted the way the OpenAI SDKs document theirs — with `/v1` on
 * the end — does not come out as `/v1/v1/chat/completions` (#2218).
 */
export const ExampleFollowsTheTypedAddress: Story = {
  render: () => <Harness fetchStub={async () => json(BASE)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.type(
      await canvas.findByLabelText("Public base URL"),
      "https://edge.example.com/v1/",
    );
    await waitFor(() =>
      expect(example(canvasElement)).toContain("curl https://edge.example.com/v1/chat/completions"),
    );
    await expect(example(canvasElement)).not.toContain("/v1/v1/");
  },
};

export const Loading: Story = {
  render: () => <Harness fetchStub={() => new Promise<Response>(() => {})} />,
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
  },
};

// a 500 or an unreachable control plane is a failure to load, said as one and
// with a retry, not an empty form a save would overwrite the settings from
export const LoadErrorRetries: Story = {
  render: () => (
    <Harness fetchStub={async () => json({ error: { message: "store unavailable" } }, 500)} />
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /client settings/i);
    await expect(within(canvasElement).getByRole("button", { name: /try again/i })).toBeVisible();
    await expect(within(canvasElement).queryByLabelText("Public base URL")).toBeNull();
  },
};

/**
 * Save waits for a change, the bar says one is pending, and undoing the edit
 * is no change at all (#2076). The screen changes gateway behaviour live, so
 * what is pending versus applied has to be on screen.
 */
export const TracksUnsavedChanges: Story = {
  render: () => <Harness fetchStub={async () => json(CONFIGURED)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const url = await canvas.findByLabelText("Public base URL");
    const save = canvas.getByRole("button", { name: "Save changes" });
    await expect(save).toHaveAttribute("aria-disabled", "true");
    await expect(canvas.queryByText(/change not saved yet/)).toBeNull();
    await userEvent.type(url, "/edge");
    await waitFor(() => expect(save).not.toHaveAttribute("aria-disabled"));
    await expect(canvas.getByText(/1 change not saved yet/)).toHaveAttribute("role", "status");
    await userEvent.clear(url);
    await userEvent.type(url, "https://gateway.example.com");
    await waitFor(() => expect(save).toHaveAttribute("aria-disabled", "true"));
  },
};

// a non-superadmin principal gets 403
export const Forbidden: Story = {
  render: () => <Harness fetchStub={async () => json({ error: { message: "forbidden" } }, 403)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() =>
      expect(canvas.getByText(/You do not have access to client settings/)).toBeVisible(),
    );
  },
};

// forwarding the caller's Authorization header upstream would hand a provider
// credential to whoever asked, so the server rejects it and so does the screen
export const RejectsAReservedHeader: Story = {
  render: () => <Harness fetchStub={async () => json(BASE)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const forwarded = await canvas.findByLabelText("Forwarded request headers");
    await userEvent.type(forwarded, "authorization");
    await waitFor(() => expect(forwarded).toHaveAttribute("aria-invalid", "true"));
    await expect(forwarded).toHaveAccessibleDescription(
      "'authorization' is managed by the gateway.",
    );
    await expect(canvas.getByRole("button", { name: "Save changes" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
  },
};

export const RejectsANonHttpBaseUrl: Story = {
  render: () => <Harness fetchStub={async () => json(BASE)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const url = await canvas.findByLabelText("Public base URL");
    await userEvent.type(url, "gateway.example.com");
    await waitFor(() => expect(url).toHaveAttribute("aria-invalid", "true"));
    await expect(url).toHaveAccessibleDescription("Base URL must start with http:// or https://.");
  },
};

// every failing field is marked at once, each with its own message, and a
// press on Save moves focus to the first one instead of doing nothing (#2651)
export const MarksEveryInvalidField: Story = {
  render: () => <Harness fetchStub={async () => json(BASE)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "Add header" }));
    const name = canvas.getByLabelText("Injected header name 1");
    const requestId = canvas.getByLabelText("Request ID header");
    await userEvent.type(name, "bad header");
    await userEvent.clear(requestId);
    await userEvent.type(requestId, "x request");
    await waitFor(() => {
      expect(name).toHaveAttribute("aria-invalid", "true");
      expect(requestId).toHaveAttribute("aria-invalid", "true");
    });
    await expect(name).toHaveAccessibleDescription("'bad header' is not a valid header name.");
    await expect(requestId).toHaveAccessibleDescription(
      "Request ID header is not a valid header name.",
    );
    await expect(canvas.getByLabelText("Injected header value 1")).not.toHaveAttribute(
      "aria-invalid",
    );
    await expect(canvas.getByText("2 fields need attention")).toBeVisible();
    await userEvent.click(canvas.getByRole("button", { name: "Save changes" }));
    await expect(name).toHaveFocus();
  },
};

// a name the gateway refuses for the request id is caught inline, before any
// save, from the list the API returns (#2970)
export const RefusesACredentialHeaderAsTheRequestId: Story = {
  render: () => <Harness fetchStub={async () => json(BASE)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const requestId = await canvas.findByLabelText("Request ID header");
    await userEvent.clear(requestId);
    await userEvent.type(requestId, "X-Goog-Api-Key");
    await waitFor(() => expect(requestId).toHaveAttribute("aria-invalid", "true"));
    await expect(requestId).toHaveAccessibleDescription(
      "'x-goog-api-key' carries a credential, session state or message framing, so it cannot be the request ID header.",
    );
  },
};

// a 400 that names a field lands on that field, with focus, not in a toast
export const ServerRejectionLandsOnTheField: Story = {
  render: () => {
    const stub: FetchStub = async (_input, init) => {
      if (init?.method === "PUT") {
        return json({ error: { message: "public_base_url must be at most 2048 characters" } }, 400);
      }
      return json(BASE);
    };
    return <Harness fetchStub={stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const url = await canvas.findByLabelText("Public base URL");
    await userEvent.clear(url);
    await userEvent.type(url, "https://gateway.example.com");
    await userEvent.click(canvas.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(url).toHaveAttribute("aria-invalid", "true"));
    await expect(url).toHaveAccessibleDescription(
      "public_base_url must be at most 2048 characters",
    );
    await waitFor(() => expect(url).toHaveFocus());
  },
};

// interaction: adding an injected header round-trips and confirms
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
    await userEvent.click(await canvas.findByRole("button", { name: "Add header" }));
    await userEvent.type(canvas.getByLabelText("Injected header name 1"), "x-partner-id");
    await userEvent.type(canvas.getByLabelText("Injected header value 1"), "acme");
    await userEvent.click(canvas.getByRole("button", { name: "Save changes" }));
    await expectToast(canvasElement, /without a restart/i);
    await expect(canvas.getByLabelText("Injected header value 1")).toHaveValue("acme");
  },
};

/**
 * The save is refused (#1607).
 *
 * `SavesChanges` covers the answer; this covers the other one. An injected
 * header is a name/value pair typed by hand, so a form that reverted on a
 * rejected save would lose both without saying so.
 */
export const SaveRejectedByTheServer: Story = {
  render: () => {
    const stub: FetchStub = async (_input, init) => {
      if (init?.method === "PUT") {
        return json({ error: { message: "x-partner-id is reserved by the gateway" } }, 422);
      }
      return json(BASE);
    };
    return <Harness fetchStub={stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "Add header" }));
    await userEvent.type(canvas.getByLabelText("Injected header name 1"), "x-partner-id");
    await userEvent.type(canvas.getByLabelText("Injected header value 1"), "acme");
    await userEvent.click(canvas.getByRole("button", { name: "Save changes" }));

    await expectToast(canvasElement, /reserved by the gateway/, "error");
    await waitFor(() =>
      expect(canvas.getByLabelText("Injected header value 1")).toHaveValue("acme"),
    );
    await expect(canvas.getByLabelText("Injected header name 1")).toHaveValue("x-partner-id");
  },
};

// What a non-superadmin gets, which is the screen refused before it asks
// (#1606).
//
// `the client settings` is a deployment-scoped resource, so `superadminOnly` never mounts
// the screen for an org role however high. The stub answers the screen's own
// request with a perfectly good payload on purpose: if the wrapper is dropped
// the screen renders that payload and this story fails, which the `Forbidden`
// story below cannot do — it stubs the 403 itself, so it passes either way.
export const RefusedToAnAdmin: Story = {
  render: () => <Harness fetchStub={async () => json(BASE)} role="admin" />,
  play: async ({ canvasElement }) => {
    await expectForbidden(canvasElement);
  },
};

export const RefusedToAViewer: Story = {
  render: () => <Harness fetchStub={async () => json(BASE)} role="viewer" />,
  play: async ({ canvasElement }) => {
    await expectForbidden(canvasElement);
  },
};

// the same screen at a phone's width in both languages: Russian runs a third
// longer than English and overflowed twice as many screens (#2004)
const clientFits = phoneFits({
  render: () => <Harness fetchStub={async () => json(CONFIGURED)} />,
  // the label in the language the story is drawn in: a screen already in Russian
  // never shows the English one, and only a screen that beats the catalog swap to
  // its first paint does. which of the two a run gets is timing, not the story
  ready: (canvas, locale) =>
    canvas.findByLabelText((locale === "ru" ? ru : en).pages.clientSettings.publicBaseUrl),
});
// the base-url field held a 320px floor: 63px past the edge of a 320px phone
export const MobileFits: Story = clientFits("mobile", "en");
export const SmallPhone: Story = clientFits("small", "en");
export const SmallPhoneInRussian: Story = clientFits("small", "ru");

// the control plane answers 5xx: the retry is on offer, reaches the network,
// and the screen comes back once the control plane does (#2220)
const clientSettingsDown = failingThenRecovering("/api/v1/client-settings", async () =>
  json(CONFIGURED),
);
export const LoadFailsAndRetries: Story = {
  render: () => {
    clientSettingsDown.reset();
    return <Harness fetchStub={clientSettingsDown.stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectRetryRecovers(
      canvasElement,
      clientSettingsDown,
      /failed to return client settings/,
      async () =>
        await expect(await canvas.findByLabelText("Injected header name 1")).toHaveValue(
          "x-partner-id",
        ),
    );
  },
};
