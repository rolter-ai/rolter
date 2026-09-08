import * as React from "react";
import { MemoryRouter } from "react-router";

import App from "@/App";
import type { SubsystemStability } from "@/lib/api";
import { ToastProvider } from "@/lib/toast";
import {
  Harness,
  ORG,
  StaleSession,
  routes,
  withCapabilities,
  type FetchStub,
} from "./story-harness";

// Mounts the assembled shell — rail + header + screen, signed in (#1239).
//
// `story-harness.tsx` renders a screen on its own: a QueryClient, a fetch stub
// and nothing above them. The shell needs four more things before `App` will
// render at all — a router, a live session, the capability answer the rail is
// built from, and a stub broad enough for whatever screen the route lands on —
// and it needs them in the order `main.tsx` mounts them, or the composition
// under test is not the one that ships.
//
// A sibling fixture rather than more of `story-harness.tsx`: that module is
// imported by every screen story, and importing `App` into it would pull every
// page into every one of those bundles.
//
// Not a `.stories.tsx` file, so Storybook does not try to render it as a screen.

const USER = {
  id: "user-1",
  email: "anya@acme.co",
  is_superadmin: true,
  created_at: "2026-01-01T00:00:00Z",
};

const ME = {
  user: USER,
  memberships: [
    {
      id: "membership-1",
      user_id: USER.id,
      org_id: ORG.id,
      team_id: null,
      project_id: null,
      role: "admin",
      created_at: "2026-01-01T00:00:00Z",
      source: "manual",
    },
  ],
};

// no newer release: the footer then renders the plain version, which is the
// state a story asserting the rail's shape wants — the update pill is the nav
// sidebar's own story, not the shell's
const VERSION = {
  current: "1.0.0",
  latest: null,
  release_url: null,
  update_available: false,
  checked_at: null,
  enabled: false,
  // nothing experimental by default: the marker is the exception, so the
  // stories that are not about it get the rail every other build renders
  experimental: [] as SubsystemStability[],
};

/**
 * One subsystem the build ships as experimental, mapped onto a nav leaf (#1386).
 *
 * `plugins` rather than one of the subsystems actually listed in
 * `crates/rolter-core/src/stability.rs`: those sit under a collapsed parent,
 * and the story is about the shell reading `nav_keys` at all, not about which
 * corners of this particular build are unfinished. Which list the marker comes
 * from is the control plane's business, and the stub is standing in for it.
 */
export const EXPERIMENTAL_SUBSYSTEM: SubsystemStability = {
  id: "plugins",
  stability: "experimental",
  note: "plugin manifests are stored but the gateway does not load them yet",
  nav_keys: ["plugins"],
};

/** The shell's chain with `experimental` answering with `subsystems`. */
export function shellStubWithStability(subsystems: SubsystemStability[]): FetchStub {
  return shellStub([["/api/v1/version", () => ({ ...VERSION, experimental: subsystems })]]);
}

const SUMMARY = {
  requests: 132,
  tokens: 1_284_000,
  prompt_tokens: 900_000,
  completion_tokens: 384_000,
  cost_usd: 41.27,
  unpriced_requests: 0,
  unpriced_models: 0,
  errors: 7,
  p50_latency_ms: 210,
  p95_latency_ms: 980,
};

/**
 * Everything the shell asks for before a screen has been chosen, plus enough
 * of the landing screen's own data that it settles instead of hanging in a
 * skeleton.
 *
 * `extra` is prepended, so a story can answer a route of its own without
 * restating the shell's chain. Anything unmatched falls through to `[]`, which
 * is what puts a screen the drawer navigated to into its empty state rather
 * than into an error.
 */
export function shellStub(extra: [string, () => unknown][] = []): FetchStub {
  return withCapabilities(
    "superadmin",
    routes([
      ...extra,
      ["/api/v1/auth/me", () => ME],
      ["/api/v1/analytics/summary", () => ({ data: [SUMMARY] })],
      ["/api/v1/analytics", () => ({ data: [] })],
      ["/api/v1/currency", () => ({ base: "USD", codes: ["USD"], rates: {} })],
      ["/api/v1/version", () => VERSION],
    ]),
  );
}

/**
 * The whole dashboard at `route`, with a session already in localStorage.
 *
 * The provider order mirrors `main.tsx`: query client, toasts, session,
 * router, `App`. `MemoryRouter` rather than `BrowserRouter` because the
 * Storybook iframe's URL belongs to Storybook — a story that pushed onto it
 * would navigate the runner instead of the shell.
 */
export function AppShell({
  route = "/dashboard",
  fetchStub = shellStub(),
}: {
  route?: string;
  fetchStub?: FetchStub;
}) {
  // the rail remembers its dragged width per browser, so a story that ran
  // after one which resized it would start somewhere else entirely
  React.useState(() => {
    localStorage.removeItem("rolter.nav.width");
    return null;
  });
  return (
    <Harness fetchStub={fetchStub}>
      <ToastProvider>
        <StaleSession email={USER.email}>
          <MemoryRouter initialEntries={[route]}>
            <App />
          </MemoryRouter>
        </StaleSession>
      </ToastProvider>
    </Harness>
  );
}
