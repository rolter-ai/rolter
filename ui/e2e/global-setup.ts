import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { seedTenant } from "./seed";

// playwright global-setup: seed a fresh tenant + admin user against the running
// control plane, then write (a) an authenticated storageState so specs start
// logged in, and (b) a fixtures file with the tenant creds/ids for specs that
// need them (e.g. the login-UI spec, which logs in by hand).
const HERE = path.dirname(fileURLToPath(import.meta.url));
const AUTH_DIR = path.join(HERE, ".auth");
const STATE_FILE = path.join(AUTH_DIR, "state.json");
const FIXTURE_FILE = path.join(AUTH_DIR, "tenant.json");

const BASE_URL = process.env.E2E_BASE_URL || "http://localhost:3000";

export default async function globalSetup(): Promise<void> {
  const tenant = await seedTenant();
  mkdirSync(AUTH_DIR, { recursive: true });

  // the SPA reads the session from localStorage (see src/lib/auth.tsx); seed it
  // directly so authenticated specs skip the login form
  const origin = new URL(BASE_URL).origin;
  const storageState = {
    cookies: [],
    origins: [
      {
        origin,
        localStorage: [
          { name: "rolter.session.token", value: tenant.token },
          { name: "rolter.session.email", value: tenant.email },
          // pin the org/team/project scope to the seeded tenant. useScope
          // otherwise defaults to the first org returned by the API, which on a
          // shared e2e database is some other test's org (with no project), so
          // the create controls would be disabled
          {
            name: "rolter.scope",
            value: JSON.stringify({
              orgId: tenant.orgId,
              teamId: tenant.teamId,
              projectId: tenant.projectId,
            }),
          },
        ],
      },
    ],
  };
  writeFileSync(STATE_FILE, JSON.stringify(storageState, null, 2));
  writeFileSync(FIXTURE_FILE, JSON.stringify(tenant, null, 2));
}
