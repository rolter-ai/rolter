import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { useAuth } from "@/lib/auth";
import type { PreferencesResponse } from "@/lib/api";
import { useFormat } from "@/lib/i18n/format";
import { setChartTimeZone } from "@/lib/i18n/format";
import { currentLocale, setLocale } from "@/lib/i18n";
import { PreferencesProvider } from "@/lib/preferences";
import { PREFERENCES_CACHE_KEY } from "@/lib/preferences-cache";
import { useScope } from "@/lib/scope";
import { Harness, StaleSession, json, recording, type FetchStub } from "@/pages/story-harness";

// the preferences provider has three jobs no single screen shows: paint from
// the cached copy before the fetch answers, let the server's answer win when it
// does, and forget the account on sign-out. the unit suite has no DOM, so a
// probe renders what the dashboard would use and each story stubs a different
// answer from `/api/v1/me/preferences`

const MOMENT = "2026-01-01T00:30:00Z";

function Probe() {
  const fmt = useFormat();
  const scope = useScope();
  const auth = useAuth();
  return (
    <dl className="grid grid-cols-[10rem_1fr] gap-1 font-mono text-sm">
      <dt>language</dt>
      <dd data-testid="language">{currentLocale()}</dd>
      <dt>time</dt>
      <dd data-testid="time">{fmt.timeShort(MOMENT)}</dd>
      <dt>project</dt>
      <dd data-testid="project">{scope.projectId ?? "—"}</dd>
      <dt>
        <button type="button" onClick={auth.signOut}>
          sign out
        </button>
      </dt>
      <dd data-testid="session">{auth.token ? "in" : "out"}</dd>
    </dl>
  );
}

const meta = {
  title: "Session/Preferences",
  component: Probe,
  parameters: { layout: "padded" },
} satisfies Meta<typeof Probe>;

export default meta;
type Story = StoryObj<typeof meta>;

const org = { id: "org-1", name: "Acme", slug: "acme", created_at: "2026-01-01T00:00:00Z" };
const team = {
  id: "team-1",
  org_id: "org-1",
  name: "Platform",
  created_at: "2026-01-01T00:00:00Z",
};
const projects = [
  { id: "project-1", team_id: "team-1", name: "Gateway", created_at: "2026-01-01T00:00:00Z" },
  { id: "project-2", team_id: "team-1", name: "Batch", created_at: "2026-01-01T00:00:00Z" },
];

const doc = (patch: Partial<PreferencesResponse> = {}): PreferencesResponse => ({
  language: null,
  default_org_id: null,
  default_team_id: null,
  default_project_id: null,
  default_playground_model: null,
  chart_time_zone: null,
  effective_default_scope: null,
  ...patch,
});

/** the scope chain, `/auth/me`, and `preferences` as the story answers it */
function server(preferences: () => Promise<Response>): FetchStub {
  return async (input, init) => {
    const url = String(input);
    const path = new URL(url, "http://localhost").pathname;
    if (path === "/api/v1/me/preferences") {
      return init?.method === "PUT"
        ? json(JSON.parse(String(init.body)) as unknown)
        : preferences();
    }
    if (path === "/api/v1/auth/me") {
      return json({
        user: { id: "u1", email: "anya@acme.co", is_superadmin: false },
        memberships: [],
      });
    }
    if (path === "/api/v1/orgs") return json([org]);
    if (/\/orgs\/[^/]+\/teams$/.test(path)) return json([team]);
    if (/\/teams\/[^/]+\/projects$/.test(path)) return json(projects);
    return json([]);
  };
}

/** seeds localStorage before the provider reads it, and restores the page on unmount */
function Seeded({
  cache,
  extra = {},
  children,
}: {
  cache?: PreferencesResponse;
  extra?: Record<string, string>;
  children: React.ReactNode;
}) {
  React.useState(() => {
    localStorage.removeItem(PREFERENCES_CACHE_KEY);
    localStorage.removeItem("rolter.locale");
    if (cache) localStorage.setItem(PREFERENCES_CACHE_KEY, JSON.stringify(cache));
    for (const [key, value] of Object.entries(extra)) localStorage.setItem(key, value);
    return null;
  });
  // the storybook toolbar decorator re-applies `en` once the story mounts, and
  // that write lands on `rolter.locale`; a language this story wants kept in
  // the browser is written after it
  React.useEffect(() => {
    const later = extra["rolter.locale"];
    if (!later) return;
    const id = setTimeout(() => localStorage.setItem("rolter.locale", later), 50);
    return () => clearTimeout(id);
  }, [extra]);
  React.useEffect(
    () => () => {
      localStorage.removeItem(PREFERENCES_CACHE_KEY);
      localStorage.removeItem("rolter.locale");
      localStorage.removeItem("rolter.scope");
      setChartTimeZone(null);
      // only a story that switched language has anything to put back; a
      // write from an unconditional reset would land in the next story's seed
      if (currentLocale() !== "en") {
        void setLocale("en").then(() => localStorage.removeItem("rolter.locale"));
      }
    },
    [],
  );
  return <>{children}</>;
}

function Screen({
  stub,
  cache,
  extra,
}: {
  stub: FetchStub;
  cache?: PreferencesResponse;
  extra?: Record<string, string>;
}) {
  return (
    <Harness fetchStub={stub}>
      <Seeded cache={cache} extra={extra}>
        <StaleSession>
          <PreferencesProvider>
            <Probe />
          </PreferencesProvider>
        </StaleSession>
      </Seeded>
    </Harness>
  );
}

const never = () => new Promise<Response>(() => {});

const TOKYO = doc({
  chart_time_zone: "Asia/Tokyo",
  effective_default_scope: { org_id: "org-1", team_id: "team-1", project_id: "project-2" },
});

/**
 * The fetch has not answered, and the page already uses the cached document:
 * the zone the clock is drawn in and the scope it opens on.
 */
export const FirstPaintFromCache: Story = {
  render: () => <Screen stub={server(never)} cache={TOKYO} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByTestId("project")).toHaveTextContent("project-2"));
    // 00:30 UTC is 09:30 in Tokyo
    await expect(canvas.getByTestId("time")).toHaveTextContent("09:30");
  },
};

/** The server's answer replaces the cached one, and the cache follows it. */
export const ServerWins: Story = {
  render: () => (
    <Screen
      stub={server(async () =>
        json(
          doc({
            chart_time_zone: "UTC",
            effective_default_scope: {
              org_id: "org-1",
              team_id: "team-1",
              project_id: "project-1",
            },
          }),
        ),
      )}
      cache={TOKYO}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByTestId("time")).toHaveTextContent("00:30"));
    await waitFor(() => expect(canvas.getByTestId("project")).toHaveTextContent("project-1"));
    await waitFor(() =>
      expect(JSON.parse(localStorage.getItem(PREFERENCES_CACHE_KEY) ?? "{}")).toMatchObject({
        chart_time_zone: "UTC",
      }),
    );
  },
};

/**
 * The saved default names a scope the account lost access to. The server sends
 * no effective scope while the raw id is still stored; the dashboard follows the
 * computed one and never the raw id, so it opens on the first project it can.
 */
export const LostDefaultScope: Story = {
  render: () => (
    <Screen
      stub={server(async () =>
        json(doc({ default_project_id: "project-gone", effective_default_scope: null })),
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByTestId("project")).toHaveTextContent("project-1"));
    await expect(canvas.getByTestId("project")).not.toHaveTextContent("project-gone");
  },
};

/** A zone this engine does not know falls back to the local zone instead of throwing. */
export const UnknownZoneFallsBack: Story = {
  render: () => (
    <Screen stub={server(never)} cache={doc({ chart_time_zone: "Mars/Olympus_Mons" })} />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const local = new Intl.DateTimeFormat("en", {
      hourCycle: "h23",
      hour: "2-digit",
      minute: "2-digit",
    }).format(new Date(MOMENT));
    await waitFor(() => expect(canvas.getByTestId("time")).toHaveTextContent(local));
  },
};

/** Signing out drops the cached document, so the next account does not paint with it. */
export const SignOutDropsTheCache: Story = {
  render: () => <Screen stub={server(never)} cache={TOKYO} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(localStorage.getItem(PREFERENCES_CACHE_KEY)).not.toBeNull());
    await userEvent.click(canvas.getByRole("button", { name: "sign out" }));
    await waitFor(() => expect(localStorage.getItem(PREFERENCES_CACHE_KEY)).toBeNull());
    await expect(canvas.getByTestId("session")).toHaveTextContent("out");
    // and the zone is the browser's again
    const local = new Intl.DateTimeFormat("en", {
      hourCycle: "h23",
      hour: "2-digit",
      minute: "2-digit",
    }).format(new Date(MOMENT));
    await waitFor(() => expect(canvas.getByTestId("time")).toHaveTextContent(local));
  },
};

/**
 * An account whose document is empty gets this browser's language and scope
 * moved into it, once. The server holds them from then on.
 */
export const MovesLocalStorageIntoAnEmptyDocument: Story = {
  render: () => {
    // answered after the browser's own language has been written (see `Seeded`)
    const recorder = recording(
      server(async () => {
        await new Promise((resolve) => setTimeout(resolve, 300));
        return json(doc());
      }),
    );
    (globalThis as { __recorder?: typeof recorder }).__recorder = recorder;
    return (
      <Screen
        stub={recorder.stub}
        extra={{
          "rolter.locale": "ru",
          "rolter.scope": JSON.stringify({
            orgId: "org-1",
            teamId: "team-1",
            projectId: "project-2",
          }),
        }}
      />
    );
  },
  play: async () => {
    const recorder = (globalThis as { __recorder?: ReturnType<typeof recording> }).__recorder!;
    const body = await recorder.expectSentBody<Record<string, unknown>>("PUT", "/me/preferences");
    await expect(body).toEqual({
      language: "ru",
      default_org_id: "org-1",
      default_team_id: "team-1",
      default_project_id: "project-2",
      default_playground_model: null,
      chart_time_zone: null,
    });
    // leave the page as found, before the next story seeds its own
    await setLocale("en");
    localStorage.removeItem("rolter.locale");
  },
};

/** A document that already holds something is never overwritten by what this browser kept. */
export const DoesNotOverwriteAServerDocument: Story = {
  render: () => {
    const recorder = recording(
      server(async () => {
        await new Promise((resolve) => setTimeout(resolve, 300));
        return json(doc({ language: "en" }));
      }),
    );
    (globalThis as { __recorder?: typeof recorder }).__recorder = recorder;
    return <Screen stub={recorder.stub} extra={{ "rolter.locale": "ru" }} />;
  },
  play: async ({ canvasElement }) => {
    const recorder = (globalThis as { __recorder?: ReturnType<typeof recording> }).__recorder!;
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByTestId("language")).toHaveTextContent("en"));
    recorder.expectNotSent("PUT", "/me/preferences");
    await setLocale("en");
    localStorage.removeItem("rolter.locale");
  },
};
