import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { Meta, StoryObj } from "@storybook/react-vite";
import * as React from "react";
import { Link, RouterProvider, createMemoryRouter, useLocation } from "react-router";
import { expect, userEvent, waitFor, within } from "storybook/test";

import SkillsRepository from "./SkillsRepository";
import {
  Toasted,
  expectInStatusRegion,
  expectEmptyState,
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
import type { SkillRow, SkillVersionRow } from "@/lib/api";
import { CapabilityProvider } from "@/lib/can";
import ru from "@/lib/i18n/locales/ru.json";
import { atMobile, expectNoHorizontalOverflow } from "@/lib/story-viewport";
import { UxScreenProvider } from "@/lib/ux-react";

const ORG = "00000000-0000-4000-8000-000000000011";
const TEAM = "00000000-0000-4000-8000-000000000012";
const PROJECT = "00000000-0000-4000-8000-000000000013";
const SKILL = "00000000-0000-4000-8000-000000000014";
const CREATED = "00000000-0000-4000-8000-0000000000bb";

const skill: SkillRow = {
  id: SKILL,
  org_id: ORG,
  name: "Incident coordinator",
  slug: "incident-coordinator",
  description: "Guides agents through evidence gathering, severity, and handoff.",
  retired_at: null,
  published_version: 2,
  allowed_team_ids: [TEAM],
  minimum_role: "member",
  created_at: "2026-07-26T09:00:00Z",
};

const versions: SkillVersionRow[] = [
  {
    skill_id: SKILL,
    version: 2,
    content:
      "---\nname: incident-coordinator\ndescription: Coordinate incidents\n---\n\n# Workflow\n\n1. Collect evidence.\n2. Assign severity.\n3. Record the handoff.\n",
    content_ref: null,
    metadata: { runtime: "agent", capabilities: ["logs", "handoff"] },
    created_at: "2026-07-30T12:15:00Z",
  },
  {
    skill_id: SKILL,
    version: 1,
    content: null,
    content_ref: "oci://registry.example/skills/incident-coordinator@sha256:abc123",
    metadata: { source: "platform-runbooks" },
    created_at: "2026-07-26T09:05:00Z",
  },
];

type FetchStub = (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;

function json(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

/** `live` is the published version the skill starts with */
function loadedStub(live = skill.published_version): FetchStub {
  let currentSkill = { ...skill, published_version: live };
  let currentVersions = [...versions];
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
    if (url.endsWith(`/orgs/${ORG}/skills`) && init?.method === "POST") {
      const body = JSON.parse(String(init.body)) as { name: string };
      return json({
        ...skill,
        id: CREATED,
        name: body.name,
        slug: "billing-triage",
        description: "",
        published_version: null,
        allowed_team_ids: [],
      });
    }
    if (url.endsWith(`/skills/${CREATED}/versions`)) return json([]);
    if (url.endsWith(`/orgs/${ORG}/skills`)) return json(deleted ? [] : [currentSkill]);
    if (url.endsWith(`/skills/${SKILL}/versions`) && init?.method === "POST") {
      const body = JSON.parse(String(init.body)) as Partial<SkillVersionRow>;
      const created: SkillVersionRow = {
        skill_id: SKILL,
        version: 3,
        content: body.content ?? null,
        content_ref: body.content_ref ?? null,
        metadata: body.metadata ?? {},
        created_at: "2026-08-02T08:00:00Z",
      };
      currentVersions = [created, ...currentVersions];
      return json(created);
    }
    if (url.endsWith(`/skills/${SKILL}/versions`)) return json(currentVersions);
    if (url.endsWith(`/skills/${SKILL}/publish`)) {
      currentSkill = { ...currentSkill, published_version: JSON.parse(String(init?.body)).version };
      return json(currentSkill);
    }
    if (url.endsWith(`/skills/${SKILL}/rollback`)) {
      currentSkill = { ...currentSkill, published_version: JSON.parse(String(init?.body)).version };
      return json(currentSkill);
    }
    if (url.endsWith(`/skills/${SKILL}`) && init?.method === "DELETE") {
      deleted = true;
      return new Response(null, { status: 204 });
    }
    if (url.endsWith(`/skills/${SKILL}`) && init?.method === "PUT") {
      const body = JSON.parse(String(init.body));
      currentSkill = {
        ...currentSkill,
        ...body,
        retired_at: body.retired ? "2026-08-02T08:30:00Z" : null,
      };
      return json(currentSkill);
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
      <SkillsRepository />
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
  title: "Screens/SkillsRepository",
  component: SkillsRepository,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof SkillsRepository>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = {
  render: () => <Harness fetchStub={loadedStub()} />,
  play: async ({ canvasElement }) => {
    await within(canvasElement).findAllByText("Incident coordinator");
    // who may use the skill is named, not counted
    await expect(await within(canvasElement).findByText("Teams: Platform")).toBeVisible();
    // every saved version is immutable, so the badge says nothing and is gone
    await expect(within(canvasElement).queryByText("Immutable")).toBeNull();
    // the shell's ScreenHeader owns the page's only h1 (#2241)
    await expect(within(canvasElement).queryAllByRole("heading", { level: 1 })).toHaveLength(0);
    // DeleteIconButton control present in repository header
    await expect(
      within(canvasElement).getByRole("button", { name: "Delete Incident coordinator" }),
    ).toBeVisible();
  },
};

// the role in "minimum role: …" is the dashboard's word for it, as in the
// settings dialog, not the stored identifier (#2098)
export const LoadedInRussian: Story = {
  globals: { locale: "ru" },
  render: () => <Harness fetchStub={loadedStub()} />,
  play: async ({ canvasElement }) => {
    const { skillsRepo } = ru.pages;
    await within(canvasElement).findAllByText("Incident coordinator");
    await expect(
      await within(canvasElement).findByText(
        skillsRepo.minimumRoleMeta.replace("{{role}}", skillsRepo.roleMember),
      ),
    ).toBeVisible();
  },
};

export const Empty: Story = {
  render: () => {
    const stub = loadedStub();
    return (
      <Harness
        fetchStub={async (input, init) =>
          String(input).endsWith(`/orgs/${ORG}/skills`) ? json([]) : stub(input, init)
        }
      />
    );
  },
  play: async ({ canvasElement }) => {
    await expectEmptyState(canvasElement, /No skills yet/, /Create skill/);
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
          String(input).endsWith(`/orgs/${ORG}/skills`)
            ? json({ error: { message: "database is unavailable" } }, 503)
            : stub(input, init)
        }
      />
    );
  },
  play: async ({ canvasElement }) => expectLoadError(canvasElement, /skills/i),
};

// the skill list loads and the *versions* request is the one in flight: the
// workbench is a second load state one level down, and bare `Skeleton`s are
// aria-hidden, so before #1618 a screen reader heard nothing here at all
export const VersionsLoading: Story = {
  render: () => {
    const stub = loadedStub();
    return (
      <Harness
        fetchStub={async (input, init) =>
          String(input).endsWith(`/skills/${SKILL}/versions`) && init?.method !== "POST"
            ? new Promise<Response>(() => {})
            : stub(input, init)
        }
      />
    );
  },
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
    // and specifically *here*: the workbench skeletons sit inside a status
    // region. asserting a loading label anywhere on the screen would pass with
    // the bare aria-hidden Skeletons this story exists for
    await expectInStatusRegion(canvasElement, "skill-workbench-loading");
    await expectInStatusRegion(canvasElement, "skill-versions-loading");
  },
};

// and a versions request that is refused: the hand-rolled panel offered a
// "Try again" that a 403 can only fail again, which is the bug #1259 fixed one
// level up (#1618)
export const VersionsError: Story = {
  render: () => {
    const stub = loadedStub();
    return (
      <Harness
        fetchStub={async (input, init) =>
          String(input).endsWith(`/skills/${SKILL}/versions`) && init?.method !== "POST"
            ? json({ error: { message: "forbidden" } }, 403)
            : stub(input, init)
        }
      />
    );
  },
  play: async ({ canvasElement }) => expectLoadError(canvasElement, /skill/i),
};

export const SavesImmutableVersion: Story = {
  render: () => <Harness fetchStub={loadedStub()} />,
  play: async ({ canvas, canvasElement }) => {
    const content = await canvas.findByRole("textbox", { name: "SKILL.md content" });
    await userEvent.type(content, "\n4. Schedule the follow-up.");
    await userEvent.click(canvas.getByRole("button", { name: /Save new version/ }));
    await expectToast(canvasElement, /Version v3 saved/);
  },
};

/**
 * The version is refused (#1607).
 *
 * `ValidatesArtifactReference` is the client-side guard; this is the server's
 * answer, which is a different path. A skill version is the whole SKILL.md, so
 * the editor keeping what was typed is the assertion that matters.
 *
 * The refusal is reported twice, in the toast queue and inline in the editor's
 * meta row, and both are asserted: either could stop reporting on its own.
 */
export const SaveVersionRejectedByTheServer: Story = {
  render: () => {
    const stub = loadedStub();
    return (
      <Harness
        fetchStub={async (input, init) =>
          String(input).endsWith(`/skills/${SKILL}/versions`) && init?.method === "POST"
            ? json({ error: { message: "SKILL.md is larger than the 64 KiB limit" } }, 413)
            : stub(input, init)
        }
      />
    );
  },
  play: async ({ canvas, canvasElement }) => {
    const content = await canvas.findByRole("textbox", { name: "SKILL.md content" });
    await userEvent.type(content, "\n4. Schedule the follow-up.");
    await userEvent.click(canvas.getByRole("button", { name: /Save new version/ }));

    await expectToast(canvasElement, /larger than the 64 KiB limit/, "error");
    // the inline copy in the meta row, matched by excluding the toast's own
    // live region since both carry the same words
    await waitFor(() =>
      expect(
        canvas
          .getAllByText(/larger than the 64 KiB limit/)
          .some((node) => node.getAttribute("role") === "alert" && node.tagName === "SPAN"),
      ).toBe(true),
    );
    await expect((content as HTMLTextAreaElement).value).toContain("4. Schedule the follow-up.");
  },
};

export const ValidatesArtifactReference: Story = {
  render: () => <Harness fetchStub={loadedStub()} />,
  play: async ({ canvas }) => {
    await userEvent.click(await canvas.findByRole("radio", { name: /Artifact reference/ }));
    const reference = await canvas.findByRole("textbox", { name: "Content reference" });
    await userEvent.type(reference, "ftp://user:secret@example.com/skill");
    await expect(canvas.getByText("Use an https, git+https, oci, or s3 reference.")).toBeVisible();
    await expect(canvas.getByRole("button", { name: /Save new version/ })).toBeDisabled();
  },
};

/**
 * Retiring is a lifecycle action of its own, not a checkbox in Settings: it
 * hides the skill from every team, so it has its own button and its own
 * confirmation, and sends the flag alone, leaving the access policy untouched.
 * Restoring is the same move back.
 */
let retirements: Recorder;
export const RetiresThenRestoresSkill: Story = {
  beforeEach: recordUxEvents,
  render: () => {
    retirements = recording(loadedStub());
    return <Harness fetchStub={retirements.stub} screen="skills-repo" />;
  },
  play: async ({ canvas, canvasElement }) => {
    // not a field of the settings form any more
    await userEvent.click(await canvas.findByRole("button", { name: "Settings" }));
    const page = within(canvasElement.ownerDocument.body);
    await expect(page.queryByRole("checkbox", { name: /Retire this skill/ })).toBeNull();
    await userEvent.click(page.getByRole("button", { name: "Cancel" }));
    await expectSheetClosed();

    await userEvent.click(canvas.getByRole("button", { name: "Retire" }));
    await expect(
      await page.findByRole("heading", { name: "Retire skill Incident coordinator?" }),
    ).toBeVisible();
    await cancelConfirmation();
    retirements.expectNotSent("PUT", `/skills/${SKILL}`);

    await userEvent.click(canvas.getByRole("button", { name: "Retire" }));
    await confirmDestructive(/restore the skill later/, "Retire skill");
    const sent = await retirements.expectSentBody<Record<string, unknown>>(
      "PUT",
      `/skills/${SKILL}`,
    );
    await expect(sent).toEqual({ retired: true });
    await expectToast(canvasElement, /no longer resolves/);
    await waitFor(() => expect(canvas.getAllByText("Retired")).toHaveLength(2));
    await expectUxEvent("save_confirmed", "skill-retire");

    await userEvent.click(canvas.getByRole("button", { name: "Restore" }));
    await expect(
      await page.findByRole("heading", { name: "Restore skill Incident coordinator?" }),
    ).toBeVisible();
    await confirmDestructive(/resolve its published version again/, "Restore skill");
    await expectToast(canvasElement, /Skill restored/);
    await waitFor(() => expect(canvas.queryByText("Retired")).toBeNull());
  },
};

export const ConfirmsRollback: Story = {
  render: () => <Harness fetchStub={loadedStub()} />,
  play: async ({ canvas, canvasElement }) => {
    await userEvent.click(await canvas.findByRole("button", { name: "Roll back to v1" }));
    const page = within(canvasElement.ownerDocument.body);
    await expect(page.getByRole("heading", { name: "Roll back to v1" })).toBeVisible();
    await expect(page.getByText(/changes the resolved version from v2/)).toBeVisible();
  },
};

/**
 * Publishing asks like rolling back does, and the verb follows the direction
 * (#2120): with v1 live, the newer v2 is published, never "rolled back to",
 * and the older one offers no publish at all.
 */
let publishes: Recorder;
export const PublishConfirmsThenGoesLive: Story = {
  beforeEach: recordUxEvents,
  render: () => {
    publishes = recording(loadedStub(1));
    return <Harness fetchStub={publishes.stub} screen="skills-repo" />;
  },
  play: async ({ canvas, canvasElement }) => {
    const rail = within(await canvas.findByRole("complementary", { name: "Version history" }));
    const railPublish = await rail.findByRole("button", { name: "Publish v2" });
    await waitFor(() => expect(railPublish).toBeVisible());
    // newer than the live version: never offered as a roll back
    await expect(rail.queryByRole("button", { name: "Roll back to v2" })).toBeNull();

    await userEvent.click(
      within(canvas.getByRole("main")).getByRole("button", { name: "Publish v2" }),
    );
    const page = within(canvasElement.ownerDocument.body);
    await expect(
      await page.findByRole("heading", { name: "Publish v2 of Incident coordinator?" }),
    ).toBeVisible();
    // asking sent nothing
    publishes.expectNotSent("PUT", "/publish");
    await expect(page.getByText(/immutable v2 instead of v1/)).toBeVisible();
    await userEvent.click(
      within(await page.findByRole("dialog")).getByRole("button", { name: "Publish v2" }),
    );
    const sent = await publishes.expectSentBody<{ version: number }>("PUT", "/publish");
    await expect(sent.version).toBe(2);
    publishes.expectNotSent("PUT", "/rollback");
    await expectToast(canvasElement, /v2 is now the resolved version/);
    await expectUxEvent("save_confirmed", "skill-publish");
    // v2 is live, so v1 is now the older one
    await waitFor(() =>
      expect(rail.getByRole("button", { name: "Roll back to v1" })).toBeVisible(),
    );
  },
};

/**
 * The content source is a radio group that behaves like one (#2119): a single
 * Tab stop, the arrow keys moving the selection, and the hint under the group
 * following the choice. Driven from the keyboard alone.
 */
export const SourceSwitchesFromTheKeyboard: Story = {
  render: () => <Harness fetchStub={loadedStub()} />,
  play: async ({ canvas }) => {
    const group = await canvas.findByRole("radiogroup", { name: "Skill content source" });
    await expect(group).toHaveAccessibleDescription("Version the instructions in rolter");
    const inline = within(group).getByRole("radio", { name: "Inline SKILL.md" });
    const reference = within(group).getByRole("radio", { name: "Artifact reference" });
    await expect(inline).toBeChecked();
    // one tab stop: the checked option, not both
    await expect(inline).toHaveAttribute("tabindex", "0");
    await expect(reference).toHaveAttribute("tabindex", "-1");

    inline.focus();
    await userEvent.keyboard("{ArrowRight}");
    await waitFor(() => expect(reference).toBeChecked());
    await waitFor(() => expect(reference).toHaveFocus());
    await expect(group).toHaveAccessibleDescription("HTTPS, Git, OCI, or S3");
    await expect(canvas.getByRole("textbox", { name: "Content reference" })).toBeVisible();

    await userEvent.keyboard("{ArrowDown}");
    await waitFor(() => expect(inline).toBeChecked());
    await expect(canvas.getByRole("textbox", { name: "SKILL.md content" })).toBeVisible();
    // Tab leaves the group rather than walking to the other option
    await userEvent.tab();
    await expect(reference).not.toHaveFocus();
    await expect(inline).not.toHaveFocus();
  },
};

/**
 * Switching version or skill with edits in the workbench asks first (#2115).
 *
 * Cancel keeps the edits where they were; Discard replaces them with the
 * version chosen; a clean workbench switches without a question, and so does
 * a click on the version already open.
 */
export const KeepsTheDraftWhenSwitchingVersions: Story = {
  render: () => <Harness fetchStub={loadedStub()} />,
  play: async ({ canvas }) => {
    const rail = within(await canvas.findByRole("complementary", { name: "Version history" }));
    await canvas.findByRole("textbox", { name: "SKILL.md content" });
    await expect(canvas.queryByText("Unsaved changes")).toBeNull();
    // a clean workbench switches at once
    await userEvent.click(rail.getByRole("button", { name: /^v1\b/ }));
    await userEvent.click(await rail.findByRole("button", { name: /^v2\b/ }));
    await waitFor(() => expect(canvas.getByText("editing from immutable v2")).toBeVisible());
    await expect(within(document.body).queryByRole("dialog")).toBeNull();

    // v1 holds a reference, so the editor was swapped out and back in
    const content = await canvas.findByRole("textbox", { name: "SKILL.md content" });
    await userEvent.type(content, "\n4. Schedule the follow-up.");
    await expect(canvas.getByText("Unsaved changes")).toBeVisible();
    // the version already open is not a switch
    await userEvent.click(rail.getByRole("button", { name: /^v2\b/ }));
    await expect(within(document.body).queryByRole("dialog")).toBeNull();

    await userEvent.click(rail.getByRole("button", { name: /^v1\b/ }));
    await expect(await discardPrompt()).toBeVisible();
    await answerDiscardPrompt(false);
    await expect(canvas.getByText("editing from immutable v2")).toBeVisible();
    await expect((content as HTMLTextAreaElement).value).toContain("4. Schedule the follow-up.");

    await userEvent.click(rail.getByRole("button", { name: /^v1\b/ }));
    await answerDiscardPrompt(true);
    await expect(await canvas.findByRole("textbox", { name: "Content reference" })).toHaveValue(
      "oci://registry.example/skills/incident-coordinator@sha256:abc123",
    );
    await expect(canvas.getByText("editing from immutable v1")).toBeVisible();
    await expect(canvas.queryByText("Unsaved changes")).toBeNull();
  },
};

/** Leaving the screen with edits asks too; a clean workbench lets go at once. */
export const LeavingWithADraftAsksFirst: Story = {
  render: () => <Harness fetchStub={loadedStub()} />,
  play: async ({ canvas }) => {
    const content = await canvas.findByRole("textbox", { name: "SKILL.md content" });
    await userEvent.type(content, "!");
    await userEvent.click(canvas.getByRole("link", { name: "Go elsewhere" }));
    await expect(await discardPrompt()).toBeVisible();
    await expect(canvas.getByTestId("pathname")).toHaveTextContent("/");
    await answerDiscardPrompt(false);
    await expect((content as HTMLTextAreaElement).value).toContain("!");
    await userEvent.click(canvas.getByRole("link", { name: "Go elsewhere" }));
    await answerDiscardPrompt(true);
    await waitFor(() => expect(canvas.getByTestId("pathname")).toHaveTextContent("/elsewhere"));
  },
};

/**
 * Deleting confirms through the shared `ConfirmDialog` (#1760), with the slug
 * typed back as its `children`: the title names the row, a cancel sends
 * nothing and is an abandon, and the confirm stays locked until the slug
 * matches, then sends the DELETE and lands as `save_confirmed`.
 */
let deletes: Recorder;
export const RequiresSlugToDeleteSkill: Story = {
  beforeEach: recordUxEvents,
  render: () => {
    deletes = recording(loadedStub());
    return <Harness fetchStub={deletes.stub} screen="skills-repo" />;
  },
  play: async ({ canvas, canvasElement }) => {
    const remove = await canvas.findByRole("button", { name: "Delete Incident coordinator" });
    await userEvent.click(remove);
    const page = within(canvasElement.ownerDocument.body);
    await expect(
      await page.findByRole("heading", { name: "Delete skill Incident coordinator?" }),
    ).toBeVisible();
    await cancelConfirmation();
    deletes.expectNotSent("DELETE", "/skills/");
    const abandon = await expectUxEvent("form_abandon", "skill-delete");
    await expect(abandon.screen).toBe("skills-repo");
    await expect(abandon.outcome).toBe("cancelled");
    expectNoUxEvent("form_submit", "skill-delete");

    await userEvent.click(remove);
    const dialog = within(await page.findByRole("dialog"));
    const confirm = dialog.getByRole("button", { name: "Delete skill" });
    // the destructive action stays locked until the slug is typed back
    await expect(confirm).toBeDisabled();
    await userEvent.type(dialog.getByRole("textbox"), "incident-coordinator");
    await confirmDestructive(/Retire it instead/, "Delete skill");
    await deletes.expectSent("DELETE", `/skills/${SKILL}`);
    await waitFor(() => expect(canvas.getByText("Share your first skill")).toBeVisible());
    const submit = await expectUxEvent("form_submit", "skill-delete");
    await expect(submit.outcome).toBe("ok");
    await expectUxEvent("save_confirmed", "skill-delete");
  },
};

// The same for skills: a viewer reads the library and writes none of it.
// Settings is the skill's access policy, which is a `skill:update` like the
// rest — only the delete asks for `skill:delete` — so Admin is the role every
// refusal here names.
export const AsViewer: Story = {
  render: () => <Harness fetchStub={loadedStub()} role="viewer" />,
  play: async ({ canvas, canvasElement }) => {
    await expectRefused(canvasElement, "Save new version");
    await expectRefused(canvasElement, "Settings");
    await expectRefused(canvasElement, "Delete Incident coordinator");
    await expectRefused(canvasElement, "Roll back to v1");
    await expectRefused(canvasElement, "Retire");
    await userEvent.click(await canvas.findByRole("button", { name: /^v1\b/ }));
    // v1 is older than the live v2, so the header offers the same roll back
    await waitFor(() =>
      expect(canvas.getAllByRole("button", { name: "Roll back to v1" })).toHaveLength(2),
    );
    await waitFor(() => {
      for (const button of canvas.getAllByRole("button", { name: "Roll back to v1" })) {
        expect(button).toBeDisabled();
      }
    });
    // reading is untouched: the slug is on screen in the index and the header
    await expect(canvas.getAllByText("incident-coordinator").length).toBeGreaterThan(0);
  },
};

async function railOf(canvas: ReturnType<typeof within>) {
  return within(await canvas.findByRole("complementary", { name: "Version history" }));
}

/**
 * Selecting a version offers its comparison with the live one (#2112): the
 * source and the metadata keys as fields, the content as lines. v1 is an
 * artifact reference and v2, the live one, is inline content, so the source
 * moved, every inline line is gone from v1 and its metadata is another set.
 */
export const ComparesAVersionWithTheLiveOne: Story = {
  render: () => <Harness fetchStub={loadedStub()} />,
  play: async ({ canvas, canvasElement }) => {
    const rail = await railOf(canvas);
    await userEvent.click(await rail.findByRole("button", { name: /^v1\b/ }));
    await userEvent.click(
      await within(canvas.getByRole("main")).findByRole("button", {
        name: "Compare with live v2",
      }),
    );
    const dialog = within(await within(canvasElement.ownerDocument.body).findByRole("dialog"));
    await waitFor(() =>
      expect(dialog.getByRole("heading", { name: "v1 compared with live v2" })).toBeVisible(),
    );

    const source = within(dialog.getByRole("list", { name: "Content source" }));
    await expect(source.getByText("Source").closest("li")).toHaveTextContent(/changed/);
    await expect(source.getByText("Content reference").closest("li")).toHaveTextContent(
      "oci://registry.example/skills/incident-coordinator@sha256:abc123",
    );

    const content = within(dialog.getByRole("region", { name: "SKILL.md content" }));
    await expect(content.getByText("1. Collect evidence.").closest("div")).toHaveTextContent(
      /Removed:/,
    );

    const metadata = within(dialog.getByRole("list", { name: "Version metadata" }));
    await expect(metadata.getByText("runtime").closest("li")).toHaveTextContent(/removed/);
    await expect(metadata.getByText("source").closest("li")).toHaveTextContent(/added/);
    await expect(metadata.getByText("source").closest("li")).toHaveTextContent(
      '"platform-runbooks"',
    );

    await userEvent.click(dialog.getByRole("radio", { name: "Side by side" }));
    await expect(
      within(dialog.getByRole("region", { name: "SKILL.md content" })).getByText(
        "1. Collect evidence.",
      ),
    ).toBeVisible();
  },
};

/** the roll back dialog says what rolling back changes, not only that it will */
export const RollbackShowsWhatChanged: Story = {
  render: () => <Harness fetchStub={loadedStub()} />,
  play: async ({ canvas, canvasElement }) => {
    await userEvent.click(await canvas.findByRole("button", { name: "Roll back to v1" }));
    const dialog = within(await within(canvasElement.ownerDocument.body).findByRole("dialog"));
    const changes = within(dialog.getByRole("region", { name: /Changes from live v2 to v1/ }));
    await expect(changes.getByText(/\d+ differences/)).toBeVisible();
    await expect(changes.queryByRole("list", { name: "Version metadata" })).toBeNull();
    await userEvent.click(changes.getByRole("button", { name: "Show changes" }));
    await expect(changes.getByRole("list", { name: "Version metadata" })).toBeVisible();
  },
};

/**
 * Enter in the create dialog submits it (#3242), and an empty name refuses the
 * submit by naming the field instead of leaving Create silently dead.
 */
export const CreateDialogNamesTheMissingName: Story = {
  render: () => <Harness fetchStub={loadedStub()} />,
  play: async ({ canvas, canvasElement }) => {
    const index = within(await canvas.findByRole("complementary", { name: "Org skills" }));
    await canvas.findByRole("textbox", { name: "SKILL.md content" });
    await userEvent.click(index.getByRole("button", { name: "Create skill" }));
    const dialog = within(await within(canvasElement.ownerDocument.body).findByRole("dialog"));
    const name = dialog.getByRole("textbox", { name: /^Name/ });
    await expect(name).toBeRequired();
    await expect(dialog.queryByText("Required")).toBeNull();

    await userEvent.click(name);
    await userEvent.keyboard("{Enter}");
    await waitFor(async () => expect(await dialog.findByText("Required")).toBeVisible());
    await waitFor(() => expect(name).toHaveFocus());
    await expect(index.queryByRole("button", { name: /Billing triage/ })).toBeNull();

    await userEvent.type(name, "Billing triage{Enter}");
    await waitFor(() =>
      expect(index.getByRole("button", { name: /Billing triage/ })).toBeVisible(),
    );
  },
};

export const CreatingOverAnUnsavedDraftAsksFirst: Story = {
  render: () => <Harness fetchStub={loadedStub()} />,
  play: async ({ canvas, canvasElement }) => {
    const index = within(await canvas.findByRole("complementary", { name: "Org skills" }));
    const content = await canvas.findByRole("textbox", { name: "SKILL.md content" });
    await userEvent.type(content, "\n4. Schedule the follow-up.");
    await userEvent.click(index.getByRole("button", { name: "Create skill" }));
    const dialog = within(await within(canvasElement.ownerDocument.body).findByRole("dialog"));
    await userEvent.type(dialog.getByRole("textbox", { name: "Name" }), "Billing triage");
    await userEvent.click(dialog.getByRole("button", { name: "Create skill" }));

    await expect(await discardPrompt()).toBeVisible();
    await answerDiscardPrompt(false);
    // created and listed, not selected, and the edits are still there
    await waitFor(() =>
      expect(index.getByRole("button", { name: /Billing triage/ })).toBeVisible(),
    );
    await expect(index.getByRole("button", { name: /Incident coordinator/ })).toHaveAttribute(
      "aria-current",
      "true",
    );
    await expect((content as HTMLTextAreaElement).value).toContain("4. Schedule the follow-up.");
    await expect(canvas.getByText("Unsaved changes")).toBeVisible();
  },
};

export const CreatingAfterDiscardingSelectsTheNewSkill: Story = {
  render: () => <Harness fetchStub={loadedStub()} />,
  play: async ({ canvas, canvasElement }) => {
    const index = within(await canvas.findByRole("complementary", { name: "Org skills" }));
    await userEvent.type(
      await canvas.findByRole("textbox", { name: "SKILL.md content" }),
      "\n4. Schedule the follow-up.",
    );
    await userEvent.click(index.getByRole("button", { name: "Create skill" }));
    const dialog = within(await within(canvasElement.ownerDocument.body).findByRole("dialog"));
    await userEvent.type(dialog.getByRole("textbox", { name: "Name" }), "Billing triage");
    await userEvent.click(dialog.getByRole("button", { name: "Create skill" }));
    await answerDiscardPrompt(true);
    await waitFor(() =>
      expect(index.getByRole("button", { name: /Billing triage/ })).toHaveAttribute(
        "aria-current",
        "true",
      ),
    );
    await expect(canvas.queryByText("Unsaved changes")).toBeNull();
  },
};

export const CreatingOverACleanDraftSelectsAtOnce: Story = {
  render: () => <Harness fetchStub={loadedStub()} />,
  play: async ({ canvas, canvasElement }) => {
    const index = within(await canvas.findByRole("complementary", { name: "Org skills" }));
    await canvas.findByRole("textbox", { name: "SKILL.md content" });
    await userEvent.click(index.getByRole("button", { name: "Create skill" }));
    const dialog = within(await within(canvasElement.ownerDocument.body).findByRole("dialog"));
    await userEvent.type(dialog.getByRole("textbox", { name: "Name" }), "Billing triage");
    await userEvent.click(dialog.getByRole("button", { name: "Create skill" }));
    await waitFor(() =>
      expect(index.getByRole("button", { name: /Billing triage/ })).toHaveAttribute(
        "aria-current",
        "true",
      ),
    );
    await expect(within(document.body).queryByRole("dialog")).toBeNull();
  },
};

/**
 * On a phone the version history is a folded line above the editor instead of
 * a list below all of it (#3118), and opens with one tap.
 */
export const HistoryFoldsOnAPhone: Story = {
  ...atMobile,
  render: () => <Harness fetchStub={loadedStub()} />,
  play: async ({ canvas }) => {
    const rail = await railOf(canvas);
    const toggle = rail.getByRole("button", { name: /Version history/ });
    await expect(toggle).toHaveAttribute("aria-expanded", "false");
    await waitFor(() => expect(toggle).toHaveTextContent("2 versions · v2 selected"));
    await expect(rail.queryByRole("button", { name: /^v1\b/ })).toBeNull();
    const editor = await canvas.findByRole("main");
    await expect(toggle.getBoundingClientRect().top).toBeLessThan(
      editor.getBoundingClientRect().top,
    );

    await userEvent.click(toggle);
    await expect(toggle).toHaveAttribute("aria-expanded", "true");
    await userEvent.click(await rail.findByRole("button", { name: /^v1\b/ }));
    await waitFor(() => expect(canvas.getByText("editing from immutable v1")).toBeVisible());
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
