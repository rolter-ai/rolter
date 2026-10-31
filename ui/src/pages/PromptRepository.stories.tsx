import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { Meta, StoryObj } from "@storybook/react-vite";
import * as React from "react";
import { Link, RouterProvider, createMemoryRouter, useLocation } from "react-router";
import { expect, userEvent, waitFor, within } from "storybook/test";

import PromptRepository from "./PromptRepository";
import {
  Toasted,
  expectEmptyState,
  expectInStatusRegion,
  expectLoadError,
  expectRefused,
  expectSheetClosed,
  expectSkeleton,
  expectToast,
  withCapabilities,
  answerDiscardPrompt,
  cancelConfirmation,
  confirmDestructive,
  discardPrompt,
  expectNoUxEvent,
  expectUxEvent,
  recordUxEvents,
  recording,
  type Recorder,
  type StoryRole,
} from "./story-harness";
import type {
  PromptTemplateRow,
  PromptTemplateScopeRow,
  PromptTemplateVersionRow,
} from "@/lib/api";
import { CapabilityProvider } from "@/lib/can";
import ru from "@/lib/i18n/locales/ru.json";
import {
  atMobile,
  atShort,
  expectInViewport,
  expectNoHorizontalOverflow,
} from "@/lib/story-viewport";
import { UxScreenProvider } from "@/lib/ux-react";

const ORG = "00000000-0000-4000-8000-000000000001";
const TEAM = "00000000-0000-4000-8000-000000000002";
const PROJECT = "00000000-0000-4000-8000-000000000003";
const TEMPLATE = "00000000-0000-4000-8000-000000000004";
const ROUTE = "00000000-0000-4000-8000-000000000005";
const KEY = "00000000-0000-4000-8000-000000000006";
const CREATED = "00000000-0000-4000-8000-0000000000aa";

const template: PromptTemplateRow = {
  id: TEMPLATE,
  org_id: ORG,
  name: "Support concierge",
  slug: "support-concierge",
  description: "Sets a consistent support voice and adds escalation context.",
  published_version: 2,
  created_at: "2026-07-28T09:00:00Z",
};

const versions: PromptTemplateVersionRow[] = [
  {
    template_id: TEMPLATE,
    version: 2,
    variables: [
      { name: "customer_name", required: true },
      { name: "tone", required: false, default: "calm and direct" },
    ],
    decorators: [
      {
        role: "system",
        position: "prepend",
        content: "You support {{ customer_name }}. Keep the tone {{ tone }}.",
      },
      {
        role: "assistant",
        position: "append",
        content: "If the issue remains unresolved, summarize the next action.",
      },
    ],
    created_at: "2026-07-30T12:15:00Z",
  },
  {
    template_id: TEMPLATE,
    version: 1,
    variables: [{ name: "customer_name", required: true }],
    decorators: [
      {
        role: "system",
        position: "prepend",
        content: "You are the support concierge for {{ customer_name }}.",
      },
    ],
    created_at: "2026-07-28T09:05:00Z",
  },
];

const projectScope = (version: number): PromptTemplateScopeRow => ({
  template_id: TEMPLATE,
  version,
  scope_type: "project",
  scope_id: PROJECT,
  created_at: "2026-07-30T12:15:00Z",
});

const routeScope = (version: number): PromptTemplateScopeRow => ({
  template_id: TEMPLATE,
  version,
  scope_type: "route",
  scope_id: ROUTE,
  created_at: "2026-08-01T10:00:00Z",
});

const scopes: PromptTemplateScopeRow[] = [projectScope(2)];

/** a draft newer than the live v2 that asks callers for nothing v2 did not */
const rewordedV3: PromptTemplateVersionRow = {
  template_id: TEMPLATE,
  version: 3,
  variables: versions[0].variables,
  decorators: [
    {
      role: "system",
      position: "prepend",
      content: "You are helping {{ customer_name }}. Keep the tone {{ tone }}.",
    },
  ],
  created_at: "2026-08-01T10:00:00Z",
};

/**
 * A draft newer than the live v2 that callers of v2 cannot satisfy: it needs
 * `ticket_id`, which nobody sends yet, and no longer declares `tone`, which
 * they do — and it widens from the project to a route as well.
 */
const breakingV3: PromptTemplateVersionRow = {
  template_id: TEMPLATE,
  version: 3,
  variables: [
    { name: "customer_name", required: true },
    { name: "ticket_id", required: true },
  ],
  decorators: [
    {
      role: "system",
      position: "prepend",
      content: "You support {{ customer_name }} on ticket {{ ticket_id }}.",
    },
  ],
  created_at: "2026-08-01T10:00:00Z",
};

type FetchStub = (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;

function json(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

function loadedStub({
  versions: seeded = versions,
  scopes: byVersion = { 2: scopes },
}: {
  versions?: PromptTemplateVersionRow[];
  /** each version's scope rows; a version missing here has none */
  scopes?: Record<number, PromptTemplateScopeRow[]>;
} = {}): FetchStub {
  let currentVersions = [...seeded];
  let published = template.published_version;
  let deleted = false;
  return async (input, init) => {
    const url = String(input);
    if (url === "/api/v1/orgs")
      return json([
        { id: ORG, name: "Northstar", slug: "northstar", created_at: "2026-01-01T00:00:00Z" },
      ]);
    if (url.endsWith(`/orgs/${ORG}/teams`))
      return json([
        { id: TEAM, org_id: ORG, name: "Platform", created_at: "2026-01-01T00:00:00Z" },
      ]);
    if (url.endsWith(`/teams/${TEAM}/projects`))
      return json([
        { id: PROJECT, team_id: TEAM, name: "Production", created_at: "2026-01-01T00:00:00Z" },
      ]);
    if (url.endsWith(`/orgs/${ORG}/prompt-templates`) && init?.method === "POST") {
      const body = JSON.parse(String(init.body)) as { name: string };
      return json({
        id: CREATED,
        org_id: ORG,
        name: body.name,
        slug: "billing-helper",
        description: "",
        published_version: null,
        created_at: "2026-08-03T09:00:00Z",
      });
    }
    if (url.endsWith(`/prompt-templates/${CREATED}/versions`)) return json([]);
    if (url.endsWith(`/orgs/${ORG}/prompt-templates`))
      return json(deleted ? [] : [{ ...template, published_version: published }]);
    if (url.endsWith(`/projects/${PROJECT}/routes`))
      return json([
        {
          id: ROUTE,
          project_id: PROJECT,
          model: "support",
          strategy: "round_robin",
          enabled: true,
          params: {},
          param_policy: {},
          created_at: "2026-01-01T00:00:00Z",
        },
      ]);
    if (url.endsWith(`/projects/${PROJECT}/virtual-keys`))
      return json([
        {
          id: KEY,
          project_id: PROJECT,
          key_prefix: "rlt_prod",
          name: "Support app",
          models: ["support"],
          disabled: false,
          created_at: "2026-01-01T00:00:00Z",
        },
      ]);
    if (url.endsWith(`/prompt-templates/${TEMPLATE}/versions`) && init?.method === "POST") {
      const body = JSON.parse(String(init.body)) as Pick<
        PromptTemplateVersionRow,
        "variables" | "decorators"
      >;
      const created = {
        template_id: TEMPLATE,
        version: Math.max(...currentVersions.map((row) => row.version)) + 1,
        ...body,
        created_at: "2026-08-02T08:00:00Z",
      };
      currentVersions = [created, ...currentVersions];
      return json(created);
    }
    if (url.endsWith(`/prompt-templates/${TEMPLATE}/versions`)) return json(currentVersions);
    if (url.includes(`/prompt-templates/${TEMPLATE}/versions/`) && url.endsWith("/scopes")) {
      if (init?.method === "PUT") return json(JSON.parse(String(init.body)).scopes);
      const version = Number(url.split("/versions/")[1].split("/")[0]);
      return json(byVersion[version] ?? []);
    }
    if (url.endsWith(`/prompt-templates/${TEMPLATE}`) && init?.method === "PUT") {
      const body = JSON.parse(String(init.body)) as { name?: string; description?: string };
      return json({ ...template, ...body });
    }
    if (url.endsWith(`/prompt-templates/${TEMPLATE}`) && init?.method === "DELETE") {
      deleted = true;
      return new Response(null, { status: 204 });
    }
    if (
      url.endsWith(`/prompt-templates/${TEMPLATE}/publish`) ||
      url.endsWith(`/prompt-templates/${TEMPLATE}/rollback`)
    ) {
      published = JSON.parse(String(init?.body)).version;
      return json({ ...template, published_version: published });
    }
    return json({ error: { message: `unhandled story request: ${url}` } }, 500);
  };
}

function Harness({
  fetchStub,
  role,
  screen: uxScreen,
}: {
  fetchStub: FetchStub;
  role?: StoryRole;
  /** the key the app shell's UxScreenProvider supplies, for a story asserting the UX stream */
  screen?: string;
}) {
  const original = React.useRef<typeof globalThis.fetch | null>(null);
  const client = React.useMemo(() => {
    original.current ??= globalThis.fetch;
    globalThis.fetch = (
      role ? withCapabilities(role, fetchStub) : fetchStub
    ) as typeof globalThis.fetch;
    localStorage.removeItem("rolter.scope");
    return new QueryClient({ defaultOptions: { queries: { retry: false } } });
  }, [fetchStub, role]);
  React.useEffect(
    () => () => {
      if (original.current) globalThis.fetch = original.current;
    },
    [],
  );
  const page = (
    <div className="h-screen bg-[color:var(--surface-app)]">
      <PromptRepository />
    </div>
  );
  const screen = uxScreen ? <UxScreenProvider screen={uxScreen}>{page}</UxScreenProvider> : page;
  const tree = (
    <QueryClientProvider client={client}>
      <Toasted>{role ? <CapabilityProvider>{screen}</CapabilityProvider> : screen}</Toasted>
      <Elsewhere />
    </QueryClientProvider>
  );
  // a data router, because the screen guards leaving with `useBlocker`
  const router = React.useMemo(
    () => createMemoryRouter([{ path: "*", element: <HarnessBody /> }]),
    [],
  );
  return (
    <HarnessBodyContext.Provider value={tree}>
      <RouterProvider router={router} />
    </HarnessBodyContext.Provider>
  );
}

const HarnessBodyContext = React.createContext<React.ReactNode>(null);
function HarnessBody() {
  return <>{React.useContext(HarnessBodyContext)}</>;
}

// stands in for the nav: a link away from the screen and a readout of where
// the router ended up
function Elsewhere() {
  const { pathname } = useLocation();
  return (
    <div className="fixed bottom-1 right-1 text-xs">
      <Link to="/elsewhere">Go elsewhere</Link>
      <output data-testid="pathname">{pathname}</output>
    </div>
  );
}

const meta = {
  title: "Screens/PromptRepository",
  component: PromptRepository,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof PromptRepository>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = {
  render: () => <Harness fetchStub={loadedStub()} />,
  play: async ({ canvasElement }) => {
    await within(canvasElement).findAllByText("Support concierge");
    // the shell's ScreenHeader owns the page's only h1 (#2241)
    await expect(within(canvasElement).queryAllByRole("heading", { level: 1 })).toHaveLength(0);
    const canvas = within(canvasElement);
    // counts take their plural form, and every version being immutable, the
    // badge that said so on all but the live one is gone (#2117)
    await expect(await canvas.findByText("1 variable · 1 decorator")).toBeVisible();
    await expect(canvas.getByText("2 variables · 2 decorators")).toBeVisible();
    await expect(canvas.queryByText("Immutable")).toBeNull();
    // a heading is read once, and the list selects within the screen
    await expect(canvas.getAllByRole("heading", { name: "Variables" })).toHaveLength(1);
    await expect(canvas.getByRole("region", { name: "Variables" })).toBeVisible();
    await expect(
      within(canvas.getByRole("complementary", { name: "Templates" })).getByRole("button", {
        name: /Support concierge/,
      }),
    ).toHaveAttribute("aria-current", "true");
    // the preview sits between the decorators and the scopes, not under the
    // whole editor
    const decorators = canvas.getByRole("heading", { name: "Decorators" });
    const preview = canvas.getByRole("complementary", { name: "Preview" });
    const scopes = canvas.getByRole("heading", { name: "Assignment scopes" });
    await expect(
      decorators.compareDocumentPosition(preview) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    await expect(
      preview.compareDocumentPosition(scopes) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    // DeleteIconButton controls present in repository header and section editors
    await expect(canvas.getByRole("button", { name: "Delete Support concierge" })).toBeVisible();
    await expect(canvas.getByRole("button", { name: "Remove decorator 1" })).toBeVisible();
  },
};

/**
 * Switching version with edits in the workbench asks first (#2115).
 *
 * Cancel keeps the edits where they were; Discard replaces them with the
 * version chosen; a clean workbench switches without a question, and so does
 * a click on the version already open. Leaving the screen asks as well.
 */
export const KeepsTheDraftWhenSwitchingVersions: Story = {
  render: () => <Harness fetchStub={loadedStub()} />,
  play: async ({ canvas, canvasElement }) => {
    const rail = await versionRail(canvasElement);
    const decorator = await canvas.findByRole("textbox", { name: /Decorator 1 content/ });
    await expect(canvas.queryByText("Unsaved changes")).toBeNull();
    // a clean workbench switches at once. the rail's versions load beside the
    // draft, so wait for them rather than read them on the next line
    await userEvent.click(await rail.findByRole("button", { name: /^v1\b/ }));
    await waitFor(() => expect(canvas.getByText(/editing from immutable v1/)).toBeVisible());
    await userEvent.click(rail.getByRole("button", { name: /^v2\b/ }));
    await waitFor(() => expect(canvas.getByText(/editing from immutable v2/)).toBeVisible());
    await expect(within(document.body).queryByRole("dialog")).toBeNull();

    await userEvent.type(decorator, " Extra.");
    await expect(canvas.getByText("Unsaved changes")).toBeVisible();
    // the version already open is not a switch
    await userEvent.click(rail.getByRole("button", { name: /^v2\b/ }));
    await expect(within(document.body).queryByRole("dialog")).toBeNull();

    await userEvent.click(rail.getByRole("button", { name: /^v1\b/ }));
    await expect(await discardPrompt()).toBeVisible();
    await answerDiscardPrompt(false);
    await expect(canvas.getByText(/editing from immutable v2/)).toBeVisible();
    await expect((decorator as HTMLTextAreaElement).value).toContain("Extra.");

    // leaving the screen asks too
    await userEvent.click(canvas.getByRole("link", { name: "Go elsewhere" }));
    await expect(await discardPrompt()).toBeVisible();
    await expect(canvas.getByTestId("pathname")).toHaveTextContent("/");
    await answerDiscardPrompt(false);

    await userEvent.click(rail.getByRole("button", { name: /^v1\b/ }));
    await answerDiscardPrompt(true);
    await waitFor(() => expect(canvas.getByText(/editing from immutable v1/)).toBeVisible());
    await waitFor(() =>
      expect(canvas.getByRole("textbox", { name: /Decorator 1 content/ })).toHaveValue(
        "You are the support concierge for {{ customer_name }}.",
      ),
    );
    await expect(canvas.queryByText("Unsaved changes")).toBeNull();
  },
};

// the preview names each decorator's role and position the way the editor above it does
export const PreviewInRussian: Story = {
  globals: { locale: "ru" },
  render: () => <Harness fetchStub={loadedStub()} />,
  play: async ({ canvasElement }) => {
    const { promptRepo } = ru.pages;
    const preview = await within(canvasElement).findByLabelText(promptRepo.renderedPreviewAria);
    await expect(within(preview).getByText(promptRepo.roleSystem)).toBeVisible();
    await expect(within(preview).getByText(promptRepo.positionPrepend)).toBeVisible();
    await expect(within(preview).queryByText("prepend")).toBeNull();
  },
};

export const Empty: Story = {
  render: () => {
    const stub = loadedStub();
    return (
      <Harness
        fetchStub={async (input, init) =>
          String(input).endsWith(`/orgs/${ORG}/prompt-templates`) ? json([]) : stub(input, init)
        }
      />
    );
  },
  play: async ({ canvasElement }) => {
    await expectEmptyState(canvasElement, /No templates yet/, /Create template/);
  },
};

export const Loading: Story = {
  render: () => <Harness fetchStub={() => new Promise<Response>(() => {})} />,
  play: async ({ canvasElement }) => expectSkeleton(canvasElement),
};

export const Error: Story = {
  render: () => {
    const stub = loadedStub();
    return (
      <Harness
        fetchStub={async (input, init) =>
          String(input).endsWith(`/orgs/${ORG}/prompt-templates`)
            ? json({ error: { message: "database is unavailable" } }, 503)
            : stub(input, init)
        }
      />
    );
  },
  play: async ({ canvasElement }) => expectLoadError(canvasElement, /prompt templates/i),
};

// the template list loads and the version history is the request still in
// flight. it rendered a bare `Skeleton`, which is aria-hidden, so the panel was
// silent to a screen reader while it filled (#1618)
export const VersionHistoryLoading: Story = {
  render: () => {
    const stub = loadedStub();
    return (
      <Harness
        fetchStub={async (input, init) =>
          String(input).endsWith(`/prompt-templates/${TEMPLATE}/versions`) &&
          init?.method !== "POST"
            ? new Promise<Response>(() => {})
            : stub(input, init)
        }
      />
    );
  },
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
    // a loading label anywhere on the screen would pass with the bare
    // aria-hidden Skeleton this story exists for, so name the panel
    await expectInStatusRegion(canvasElement, "prompt-versions-loading");
  },
};

export const RendersSamplesAndSavesDraft: Story = {
  render: () => <Harness fetchStub={loadedStub()} />,
  play: async ({ canvas, canvasElement }) => {
    const sample = await canvas.findByRole("textbox", { name: "Sample value for customer_name" });
    await userEvent.type(sample, "Aster Labs");
    await expect(canvas.getByText(/You support Aster Labs/)).toBeVisible();
    // the request a caller sends, beside the render it produces (#2110): the
    // sample fills the required variable and the default stands in for tone
    const request = canvas.getByRole("region", { name: /What callers send/ });
    // the block is highlighted from a lazy chunk, one keystroke behind the
    // field it mirrors, so the whole shape is awaited together rather than
    // read on the frame after the last key (#3067)
    await waitFor(() => {
      expect(request).toHaveTextContent(/"rolter_template_vars"/);
      expect(request).toHaveTextContent(/"customer_name": "Aster Labs"/);
      expect(request).toHaveTextContent(/"tone": "calm and direct"/);
    });
    await expect(canvas.getAllByText("invalid_prompt_template")[0]).toBeVisible();
    await userEvent.click(canvas.getByRole("button", { name: /Save as new draft/ }));
    await expectToast(canvasElement, /Draft v3 saved/);
  },
};

/**
 * The draft is refused (#1607).
 *
 * There is no sheet here — the workbench *is* the draft — so "the draft
 * survives" means the edited decorator and the sample value are still on screen
 * after the refusal rather than reset to the published version.
 *
 * The refusal is reported twice, in the toast queue and inline beside the save
 * button, and both are asserted: either could stop reporting on its own.
 */
export const SaveDraftRejectedByTheServer: Story = {
  render: () => {
    const stub = loadedStub();
    return (
      <Harness
        fetchStub={async (input, init) =>
          String(input).endsWith(`/prompt-templates/${TEMPLATE}/versions`) &&
          init?.method === "POST"
            ? json({ error: { message: "tone is referenced but never declared" } }, 422)
            : stub(input, init)
        }
      />
    );
  },
  play: async ({ canvas, canvasElement }) => {
    const sample = await canvas.findByRole("textbox", {
      name: "Sample value for customer_name",
    });
    await userEvent.type(sample, "Aster Labs");
    await userEvent.click(canvas.getByRole("button", { name: /Save as new draft/ }));

    await expectToast(canvasElement, /referenced but never declared/, "error");
    // the inline copy beside the save button, matched by excluding the toast's
    // own live region since both carry the same words
    await waitFor(() =>
      expect(
        canvas
          .getAllByText(/referenced but never declared/)
          .some((node) => node.getAttribute("role") === "alert" && node.tagName === "SPAN"),
      ).toBe(true),
    );
    await expect(sample).toHaveValue("Aster Labs");
    // and the preview it feeds, which is the draft the operator was working on
    await expect(canvas.getByText(/You support Aster Labs/)).toBeVisible();
  },
};

/** the workbench, whose header repeats the rail's action for the selected version */
async function workbench(canvasElement: HTMLElement) {
  return within(await within(canvasElement).findByRole("main"));
}

async function versionRail(canvasElement: HTMLElement) {
  return within(
    await within(canvasElement).findByRole("complementary", { name: "Version history" }),
  );
}

async function openConfirmation(canvasElement: HTMLElement) {
  return within(await within(canvasElement.ownerDocument.body).findByRole("dialog"));
}

/**
 * Publishing asks first (#2110), through the same `ConfirmDialog` as rolling
 * back. This v3 asks callers for nothing v2 did not, so the dialog names its
 * scope and its one required variable and raises no warning. A cancel sends
 * nothing and is an abandon; a confirm sends the publish, lands as
 * `save_confirmed` and moves the live badge.
 */
let publishes: Recorder;
export const PublishConfirmsThenGoesLive: Story = {
  beforeEach: recordUxEvents,
  render: () => {
    publishes = recording(
      loadedStub({
        versions: [rewordedV3, ...versions],
        scopes: { 2: [projectScope(2)], 3: [projectScope(3)] },
      }),
    );
    return <Harness fetchStub={publishes.stub} screen="prompt-repo" />;
  },
  play: async ({ canvas, canvasElement }) => {
    const publish = await (
      await workbench(canvasElement)
    ).findByRole("button", {
      name: "Publish v3",
    });
    await userEvent.click(publish);
    let dialog = await openConfirmation(canvasElement);
    await expect(
      dialog.getByRole("heading", { name: "Publish v3 of Support concierge?" }),
    ).toBeVisible();
    await expect(dialog.getByText(/applies v3 instead of v2/)).toBeVisible();
    // the scope by the name the operator knows it by, never a bare id
    await expect(await dialog.findByText("Production")).toBeVisible();
    await expect(dialog.getByText("customer_name")).toBeVisible();
    // nothing new is asked of anyone, so nothing is flagged
    await expect(dialog.queryByText("new")).toBeNull();
    await expect(dialog.queryByText(/may be refused/)).toBeNull();
    await expect(dialog.getByText("rolter_template_vars")).toBeVisible();

    await cancelConfirmation();
    publishes.expectNotSent("PUT", "/publish");
    const abandon = await expectUxEvent("form_abandon", "prompt-template-publish");
    await expect(abandon.screen).toBe("prompt-repo");
    await expect(abandon.outcome).toBe("cancelled");
    expectNoUxEvent("form_submit", "prompt-template-publish");

    await userEvent.click(publish);
    dialog = await openConfirmation(canvasElement);
    await userEvent.click(dialog.getByRole("button", { name: "Publish v3" }));
    const sent = await publishes.expectSentBody<{ version: number }>(
      "PUT",
      `/prompt-templates/${TEMPLATE}/publish`,
    );
    await expect(sent.version).toBe(3);
    // forward is a publish in the audit log, never a roll back
    publishes.expectNotSent("PUT", "/rollback");
    await expectToast(canvasElement, /v3 is now live/);
    await expectSheetClosed();
    await waitFor(() => expect(canvas.getByText("v3 live")).toBeVisible());
    const submit = await expectUxEvent("form_submit", "prompt-template-publish");
    await expect(submit.outcome).toBe("ok");
    await expectUxEvent("save_confirmed", "prompt-template-publish");
  },
};

/**
 * The version callers of v2 cannot satisfy (#2110). The dialog marks what is
 * new — `ticket_id`, which nobody sends yet, and the route the version widens
 * to — lists `tone`, which v2's callers send and v3 no longer declares, and
 * says how callers pass variables and what the gateway answers when they do
 * not. It warns and still lets the operator decide.
 */
export const PublishWarnsAboutANewRequiredVariable: Story = {
  render: () => (
    <Harness
      fetchStub={loadedStub({
        versions: [breakingV3, ...versions],
        scopes: { 2: [projectScope(2)], 3: [projectScope(3), routeScope(3)] },
      })}
    />
  ),
  play: async ({ canvasElement }) => {
    await userEvent.click(
      await (await workbench(canvasElement)).findByRole("button", { name: "Publish v3" }),
    );
    const dialog = await openConfirmation(canvasElement);

    const required = within(dialog.getByRole("region", { name: "Variables a request must send" }));
    await expect(required.getByText("ticket_id").closest("li")).toHaveTextContent(/new$/);
    await expect(required.getByText("customer_name").closest("li")).not.toHaveTextContent(/new$/);

    const dropped = within(dialog.getByRole("region", { name: "No longer declared" }));
    await expect(dropped.getByText("tone")).toBeVisible();

    const reach = within(dialog.getByRole("region", { name: "Applies to" }));
    await expect((await reach.findByText("support")).closest("li")).toHaveTextContent(/new$/);
    await expect(reach.getByText("Production").closest("li")).not.toHaveTextContent(/new$/);

    await expect(dialog.getByText("Requests that work with v2 may be refused.")).toBeVisible();
    await expect(dialog.getByText("rolter_template_vars")).toBeVisible();
    await expect(dialog.getByText("invalid_prompt_template")).toBeVisible();
    // a warning, not a lock: the same dialog backs a roll back mid-incident
    await expect(dialog.getByRole("button", { name: "Publish v3" })).toBeEnabled();
  },
};

/**
 * The rail offers "Publish" on a version newer than the live one and "Roll
 * back" only on an older one (#2110), and both open the same confirmation.
 * Rolling back sends the roll back, and once v1 is live both later versions
 * turn into publishes.
 */
let rollbacks: Recorder;
export const RailPublishesNewerAndRollsBackOlder: Story = {
  beforeEach: recordUxEvents,
  render: () => {
    rollbacks = recording(
      loadedStub({
        versions: [rewordedV3, ...versions],
        scopes: { 1: [projectScope(1)], 2: [projectScope(2)], 3: [projectScope(3)] },
      }),
    );
    return <Harness fetchStub={rollbacks.stub} screen="prompt-repo" />;
  },
  play: async ({ canvasElement }) => {
    const rail = await versionRail(canvasElement);
    await expect(await rail.findByRole("button", { name: "Publish v3" })).toBeVisible();
    await expect(rail.getByRole("button", { name: "Roll back to v1" })).toBeVisible();
    await expect(rail.queryByRole("button", { name: "Roll back to v3" })).toBeNull();
    // the live version offers neither
    await expect(rail.queryByRole("button", { name: "Publish v2" })).toBeNull();
    await expect(rail.queryByRole("button", { name: "Roll back to v2" })).toBeNull();

    await userEvent.click(rail.getByRole("button", { name: "Roll back to v1" }));
    const dialog = await openConfirmation(canvasElement);
    await expect(
      dialog.getByRole("heading", { name: "Roll back Support concierge to v1?" }),
    ).toBeVisible();
    await expect(dialog.getByText(/goes back to v1 instead of v2/)).toBeVisible();
    // v1 never declared the tone v2's callers send
    await expect(
      within(dialog.getByRole("region", { name: "No longer declared" })).getByText("tone"),
    ).toBeVisible();
    await userEvent.click(dialog.getByRole("button", { name: "Roll back to v1" }));

    const sent = await rollbacks.expectSentBody<{ version: number }>(
      "PUT",
      `/prompt-templates/${TEMPLATE}/rollback`,
    );
    await expect(sent.version).toBe(1);
    rollbacks.expectNotSent("PUT", "/publish");
    await expectToast(canvasElement, /Rolled back: v1 is live again/);
    await expectSheetClosed();
    await waitFor(() => expect(rail.getByRole("button", { name: "Publish v2" })).toBeVisible());
    await expect(rail.getByRole("button", { name: "Publish v3" })).toBeVisible();
    await expect(rail.queryByRole("button", { name: /^Roll back/ })).toBeNull();
    // the landing is reported under the direction it was opened with, although
    // by the render that closes the dialog v1 is the live version
    await expectUxEvent("save_confirmed", "prompt-template-rollback");
    expectNoUxEvent("save_confirmed", "prompt-template-publish");
  },
};

/** everything but the scopes of v3, which `answer` decides */
function scopesOfV3(answer: () => Promise<Response>): FetchStub {
  const stub = loadedStub({ versions: [rewordedV3, ...versions] });
  return async (input, init) =>
    String(input).endsWith("/versions/3/scopes") && init?.method !== "PUT"
      ? answer()
      : stub(input, init);
}

// the variables are the version's own and on screen at once; only the scope
// list is a request behind the dialog, and it says so while it is out
export const PublishScopesLoading: Story = {
  render: () => <Harness fetchStub={scopesOfV3(() => new Promise<Response>(() => {}))} />,
  play: async ({ canvasElement }) => {
    await userEvent.click(
      await (await workbench(canvasElement)).findByRole("button", { name: "Publish v3" }),
    );
    const dialog = await openConfirmation(canvasElement);
    await expectInStatusRegion(canvasElement.ownerDocument.body, "prompt-publish-scopes-loading");
    await expect(dialog.getByText("customer_name")).toBeVisible();
  },
};

// a scope list that failed to load is reported where it would have been, and
// the confirm stays live: a roll back mid-incident must never wait on it
export const PublishScopesFailToLoad: Story = {
  render: () => (
    <Harness
      fetchStub={scopesOfV3(async () =>
        json({ error: { message: "database is unavailable" } }, 503),
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    await userEvent.click(
      await (await workbench(canvasElement)).findByRole("button", { name: "Publish v3" }),
    );
    const dialog = await openConfirmation(canvasElement);
    await expectLoadError(canvasElement.ownerDocument.body, /the version's scopes/);
    await expect(dialog.getByRole("button", { name: "Publish v3" })).toBeEnabled();
  },
};

export const RenamesTemplateKeepingSlug: Story = {
  render: () => <Harness fetchStub={loadedStub()} />,
  play: async ({ canvas, canvasElement }) => {
    await userEvent.click(await canvas.findByRole("button", { name: "Rename Support concierge" }));
    const page = within(canvasElement.ownerDocument.body);
    const dialog = within(await page.findByRole("dialog"));
    await waitFor(() =>
      expect(dialog.getByRole("heading", { name: "Rename prompt template" })).toBeVisible(),
    );
    // the slug is the stable identity: it is stated, never offered as a field
    await expect(dialog.getByText(/support-concierge/)).toBeVisible();
    const [name] = dialog.getAllByRole("textbox");
    await userEvent.clear(name);
    await userEvent.type(name, "Support desk");
    await userEvent.click(dialog.getByRole("button", { name: "Save details" }));
    await expectToast(canvasElement, /Template details updated/);
  },
};

/**
 * The create dialog in a 640x360 window (#2225): its fields sit in a
 * `DialogBody`, so the body scrolls between a header and a footer that stay
 * on screen, and the primary action is still reachable.
 */
export const CreateDialogFitsAShortScreen: Story = {
  ...atShort,
  render: () => <Harness fetchStub={loadedStub()} />,
  play: async ({ canvas, canvasElement }) => {
    await userEvent.click(await canvas.findByRole("button", { name: /Create template/ }));
    const page = within(canvasElement.ownerDocument.body);
    const dialog = within(await page.findByRole("dialog"));
    await expectInViewport(dialog.getByRole("heading", { name: /Create/ }));
    await expectInViewport(dialog.getByRole("button", { name: "Close" }));
    const submit = dialog.getByRole("button", { name: "Create template" });
    await expectInViewport(submit);
    const body = dialog.getAllByRole("textbox")[0].closest<HTMLElement>("[data-slot=dialog-body]")!;
    await expect(body).not.toBeNull();
    await expect(body.scrollHeight).toBeGreaterThan(body.clientHeight);
  },
};

/**
 * Deleting confirms through the shared `ConfirmDialog` (#1760), with the slug
 * typed back as its `children`: the title names the row, a cancel sends
 * nothing and is an abandon, and the confirm stays locked until the slug
 * matches, then sends the DELETE and lands as `save_confirmed`.
 */
let deletes: Recorder;
export const RequiresSlugToDeleteTemplate: Story = {
  beforeEach: recordUxEvents,
  render: () => {
    deletes = recording(loadedStub());
    return <Harness fetchStub={deletes.stub} screen="prompt-repo" />;
  },
  play: async ({ canvas, canvasElement }) => {
    const remove = await canvas.findByRole("button", { name: "Delete Support concierge" });
    await userEvent.click(remove);
    const page = within(canvasElement.ownerDocument.body);
    await expect(
      await page.findByRole("heading", { name: "Delete template Support concierge?" }),
    ).toBeVisible();
    await cancelConfirmation();
    deletes.expectNotSent("DELETE", "/prompt-templates/");
    const abandon = await expectUxEvent("form_abandon", "prompt-template-delete");
    await expect(abandon.screen).toBe("prompt-repo");
    await expect(abandon.outcome).toBe("cancelled");
    expectNoUxEvent("form_submit", "prompt-template-delete");

    await userEvent.click(remove);
    const dialog = within(await page.findByRole("dialog"));
    const confirm = dialog.getByRole("button", { name: "Delete template" });
    // the destructive action stays locked until the slug is typed back
    await expect(confirm).toBeDisabled();
    await userEvent.type(dialog.getByRole("textbox"), "support-concierge");
    await confirmDestructive(/v2 is live/, "Delete template");
    await deletes.expectSent("DELETE", `/prompt-templates/${TEMPLATE}`);
    await waitFor(() => expect(canvas.getByText("Start with a prompt template")).toBeVisible());
    const submit = await expectUxEvent("form_submit", "prompt-template-delete");
    await expect(submit.outcome).toBe("ok");
    await expectUxEvent("save_confirmed", "prompt-template-delete");
  },
};

// A viewer reaches the same workbench — `prompt_template:read` is a viewer's —
// and every control on it that writes is refused before the click rather than
// after the 403. Publishing, rolling back and saving a version are all one
// `prompt_template:update` guard in crates/rolter-control/src/crud/prompts.rs, so the
// role that would allow them is Admin.
export const AsViewer: Story = {
  render: () => (
    <Harness fetchStub={loadedStub({ versions: [rewordedV3, ...versions] })} role="viewer" />
  ),
  play: async ({ canvas, canvasElement }) => {
    await expectRefused(canvasElement, "Save as new draft");
    await expectRefused(canvasElement, "Rename Support concierge");
    await expectRefused(canvasElement, "Delete Support concierge");
    // v3 is newer than the live v2, so the header and the rail both offer to
    // publish it, and the rail offers to roll back to v1; every copy is refused
    await expectRefused(canvasElement, "Publish v3");
    await expectRefused(canvasElement, "Roll back to v1");
    // the header offers an action only for a selected version that is not the
    // live one, so selecting v1 brings a second roll back control
    await userEvent.click(await canvas.findByRole("button", { name: /^v1\b/ }));
    await waitFor(() =>
      expect(canvas.getAllByRole("button", { name: "Roll back to v1" })).toHaveLength(2),
    );
    await expectRefused(canvasElement, "Roll back to v1");
    // reading is untouched: the slug is on screen in the index and the header
    await expect(canvas.getAllByText("support-concierge").length).toBeGreaterThan(0);
  },
};

/**
 * Selecting a version offers its comparison with the live one (#2112): the
 * variables by name, each added, removed or changed, and the decorators as
 * lines. A change is never only a colour: every line and row carries a mark and
 * a visually hidden "Added" / "Removed". The two layouts show the same lines.
 */
export const ComparesAVersionWithTheLiveOne: Story = {
  render: () => (
    <Harness
      fetchStub={loadedStub({
        versions: [breakingV3, ...versions],
        scopes: { 2: [projectScope(2)], 3: [projectScope(3)] },
      })}
    />
  ),
  play: async ({ canvasElement }) => {
    const main = await workbench(canvasElement);
    await userEvent.click(await main.findByRole("button", { name: "Compare with live v2" }));
    const dialog = await openConfirmation(canvasElement);
    await expect(
      await dialog.findByRole("heading", { name: "v3 compared with live v2" }),
    ).toBeVisible();

    const variables = within(dialog.getByRole("list", { name: "Variables" }));
    await expect(variables.getByText("ticket_id").closest("li")).toHaveTextContent(/added/);
    await expect(variables.getByText("tone").closest("li")).toHaveTextContent(/removed/);
    // the unchanged variable is not repeated
    await expect(variables.queryByText("customer_name")).toBeNull();

    const decorators = within(dialog.getByRole("region", { name: "Decorators" }));
    const added = decorators.getByText(
      "You support {{ customer_name }} on ticket {{ ticket_id }}.",
    );
    await expect(added.closest("div")).toHaveTextContent(/Added:/);
    await expect(
      decorators
        .getByText("You support {{ customer_name }}. Keep the tone {{ tone }}.")
        .closest("div"),
    ).toHaveTextContent(/Removed:/);

    await userEvent.click(dialog.getByRole("radio", { name: "Side by side" }));
    await expect(
      within(dialog.getByRole("region", { name: "Decorators" })).getByText(
        "You support {{ customer_name }} on ticket {{ ticket_id }}.",
      ),
    ).toBeVisible();
    await userEvent.click(dialog.getByRole("button", { name: "Close" }));
    await waitFor(() => expect(within(document.body).queryByRole("dialog")).toBeNull());
  },
};

/** the live version has nothing to compare with itself, so it offers no comparison */
export const TheLiveVersionOffersNoComparison: Story = {
  render: () => <Harness fetchStub={loadedStub()} />,
  play: async ({ canvasElement }) => {
    const main = await workbench(canvasElement);
    await main.findByText(/editing from immutable v2/);
    await expect(main.queryByRole("button", { name: /^Compare with live/ })).toBeNull();
  },
};

/**
 * The publish confirmation carries the comparison, folded behind a button so
 * the impact on callers stays the first thing read (#2112). The count says
 * how many differences there are before it is opened.
 */
export const PublishShowsWhatChanged: Story = {
  render: () => (
    <Harness
      fetchStub={loadedStub({
        versions: [breakingV3, ...versions],
        scopes: { 2: [projectScope(2)], 3: [projectScope(3)] },
      })}
    />
  ),
  play: async ({ canvasElement }) => {
    await userEvent.click(
      await (await workbench(canvasElement)).findByRole("button", { name: "Publish v3" }),
    );
    const dialog = await openConfirmation(canvasElement);
    const changes = within(await dialog.findByRole("region", { name: /Changes from live v2/ }));
    await expect(changes.getByText(/\d+ differences/)).toBeVisible();
    await expect(changes.queryByRole("region", { name: "Decorators" })).toBeNull();
    const show = changes.getByRole("button", { name: "Show changes" });
    await expect(show).toHaveAttribute("aria-expanded", "false");
    await userEvent.click(show);
    await expect(changes.getByRole("button", { name: "Hide changes" })).toHaveAttribute(
      "aria-expanded",
      "true",
    );
    await expect(changes.getByRole("region", { name: "Decorators" })).toBeVisible();
    await expect(changes.getByRole("list", { name: "Variables" })).toBeVisible();
  },
};

/** a version that changed nothing says so, and shows no empty diff */
export const IdenticalVersionsSayNothingChanged: Story = {
  render: () => {
    const copy: PromptTemplateVersionRow = { ...versions[0], version: 3 };
    return (
      <Harness
        fetchStub={loadedStub({
          versions: [copy, ...versions],
          scopes: { 2: [projectScope(2)], 3: [projectScope(3)] },
        })}
      />
    );
  },
  play: async ({ canvasElement }) => {
    await userEvent.click(
      await (await workbench(canvasElement)).findByRole("button", { name: "Publish v3" }),
    );
    const dialog = await openConfirmation(canvasElement);
    await expect(await dialog.findByText("v3 is identical to live v2.")).toBeVisible();
    await expect(dialog.queryByRole("button", { name: "Show changes" })).toBeNull();
  },
};

/** every version in the rail names the scopes it is assigned to (#2112) */
export const RailListsEachVersionsScopes: Story = {
  render: () => (
    <Harness fetchStub={loadedStub({ scopes: { 2: [projectScope(2), routeScope(2)] } })} />
  ),
  play: async ({ canvasElement }) => {
    const rail = await versionRail(canvasElement);
    await expect(await rail.findByText("Scopes: Production, support")).toBeVisible();
    await expect(rail.getByText("No scopes")).toBeVisible();
  },
};

/**
 * Enter in the create dialog submits it (#3242), and an empty name refuses the
 * submit by naming the field instead of leaving Create silently dead.
 */
export const CreateDialogNamesTheMissingName: Story = {
  render: () => <Harness fetchStub={loadedStub()} />,
  play: async ({ canvas, canvasElement }) => {
    const index = within(await canvas.findByRole("complementary", { name: "Templates" }));
    await canvas.findByRole("textbox", { name: /Decorator 1 content/ });
    await userEvent.click(index.getByRole("button", { name: "Create template" }));
    const dialog = await openConfirmation(canvasElement);
    const name = dialog.getByRole("textbox", { name: /^Name/ });
    await expect(name).toBeRequired();
    await expect(dialog.queryByText("Required")).toBeNull();

    await userEvent.click(name);
    await userEvent.keyboard("{Enter}");
    await waitFor(async () => expect(await dialog.findByText("Required")).toBeVisible());
    await waitFor(() => expect(name).toHaveFocus());
    await expect(index.queryByRole("button", { name: /Billing helper/ })).toBeNull();

    await userEvent.type(name, "Billing helper{Enter}");
    await waitFor(() =>
      expect(index.getByRole("button", { name: /Billing helper/ })).toBeVisible(),
    );
  },
};

/**
 * Creating a template while the open draft has unsaved edits goes through the
 * same guard as switching (#3118). The template is created either way; Cancel
 * leaves the edits and the selection where they were.
 */
export const CreatingOverAnUnsavedDraftAsksFirst: Story = {
  render: () => <Harness fetchStub={loadedStub()} />,
  play: async ({ canvas, canvasElement }) => {
    const index = within(await canvas.findByRole("complementary", { name: "Templates" }));
    const decorator = await canvas.findByRole("textbox", { name: /Decorator 1 content/ });
    await userEvent.type(decorator, " Extra.");
    await userEvent.click(index.getByRole("button", { name: "Create template" }));
    const dialog = await openConfirmation(canvasElement);
    await userEvent.type(dialog.getByRole("textbox", { name: "Name" }), "Billing helper");
    await userEvent.click(dialog.getByRole("button", { name: "Create template" }));

    await expect(await discardPrompt()).toBeVisible();
    await answerDiscardPrompt(false);
    // created and listed, not selected, and the edits are still there
    await waitFor(() =>
      expect(index.getByRole("button", { name: /Billing helper/ })).toBeVisible(),
    );
    await expect(index.getByRole("button", { name: /Support concierge/ })).toHaveAttribute(
      "aria-current",
      "true",
    );
    await expect((decorator as HTMLTextAreaElement).value).toContain("Extra.");
    await expect(canvas.getByText("Unsaved changes")).toBeVisible();
  },
};

export const CreatingAfterDiscardingSelectsTheNewTemplate: Story = {
  render: () => <Harness fetchStub={loadedStub()} />,
  play: async ({ canvas, canvasElement }) => {
    const index = within(await canvas.findByRole("complementary", { name: "Templates" }));
    await userEvent.type(
      await canvas.findByRole("textbox", { name: /Decorator 1 content/ }),
      " Extra.",
    );
    await userEvent.click(index.getByRole("button", { name: "Create template" }));
    const dialog = await openConfirmation(canvasElement);
    await userEvent.type(dialog.getByRole("textbox", { name: "Name" }), "Billing helper");
    await userEvent.click(dialog.getByRole("button", { name: "Create template" }));
    await answerDiscardPrompt(true);
    await waitFor(() =>
      expect(index.getByRole("button", { name: /Billing helper/ })).toHaveAttribute(
        "aria-current",
        "true",
      ),
    );
    await expect(canvas.queryByText("Unsaved changes")).toBeNull();
  },
};

/** a clean draft is not asked about: the new template is selected at once */
export const CreatingOverACleanDraftSelectsAtOnce: Story = {
  render: () => <Harness fetchStub={loadedStub()} />,
  play: async ({ canvas, canvasElement }) => {
    const index = within(await canvas.findByRole("complementary", { name: "Templates" }));
    await canvas.findByRole("textbox", { name: /Decorator 1 content/ });
    await userEvent.click(index.getByRole("button", { name: "Create template" }));
    const dialog = await openConfirmation(canvasElement);
    await userEvent.type(dialog.getByRole("textbox", { name: "Name" }), "Billing helper");
    await userEvent.click(dialog.getByRole("button", { name: "Create template" }));
    await waitFor(() =>
      expect(index.getByRole("button", { name: /Billing helper/ })).toHaveAttribute(
        "aria-current",
        "true",
      ),
    );
    await expect(within(document.body).queryByRole("dialog")).toBeNull();
  },
};

/**
 * On a phone the version history is a folded line above the editor instead of
 * a list below all of it (#3118): reachable without scrolling past the
 * editor, and opened with one tap.
 */
export const HistoryFoldsOnAPhone: Story = {
  ...atMobile,
  render: () => <Harness fetchStub={loadedStub()} />,
  play: async ({ canvas, canvasElement }) => {
    const rail = await versionRail(canvasElement);
    const toggle = rail.getByRole("button", { name: /Version history/ });
    await expect(toggle).toHaveAttribute("aria-expanded", "false");
    await waitFor(() => expect(toggle).toHaveTextContent("2 versions · v2 selected"));
    await expect(rail.queryByRole("button", { name: /^v1\b/ })).toBeNull();
    // above the editor on screen, though after it in the document
    const editor = await canvas.findByRole("main");
    await expect(
      rail.getByRole("button", { name: /Version history/ }).getBoundingClientRect().top,
    ).toBeLessThan(editor.getBoundingClientRect().top);

    await userEvent.click(toggle);
    await expect(toggle).toHaveAttribute("aria-expanded", "true");
    await userEvent.click(await rail.findByRole("button", { name: /^v1\b/ }));
    await waitFor(() => expect(canvas.getByText(/editing from immutable v1/)).toBeVisible());
    await expectNoHorizontalOverflow();
  },
};

export const HistoryFoldsOnAPhoneInRussian: Story = {
  ...atMobile,
  globals: { ...atMobile.globals, locale: "ru" },
  render: () => <Harness fetchStub={loadedStub()} />,
  play: async ({ canvasElement }) => {
    const history = within(
      await within(canvasElement).findByRole("complementary", { name: ru.repository.history }),
    );
    const toggle = history.getByRole("button", { name: new RegExp(ru.repository.history) });
    await expect(toggle).toHaveAttribute("aria-expanded", "false");
    await waitFor(() => expect(toggle).toHaveTextContent("2 версии · выбрана v2"));
    await userEvent.click(toggle);
    await waitFor(() => expect(history.getByRole("button", { name: /^v1\b/ })).toBeVisible());
    await expectNoHorizontalOverflow();
  },
};
