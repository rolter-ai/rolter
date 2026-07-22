// seed a tenant + admin user against a running rolter control plane, using the
// operator admin token. mirrors integration/e2e bootstrap.new_tenant so the
// browser tests start from "a fresh tenant with a real login".

const CONTROL_URL = process.env.E2E_CONTROL_URL || "http://localhost:4001";
const ADMIN_TOKEN = process.env.E2E_ADMIN_TOKEN || "e2e-superadmin-token";

export interface SeededTenant {
  orgId: string;
  teamId: string;
  projectId: string;
  email: string;
  password: string;
  token: string;
}

function rand(prefix: string): string {
  return `${prefix}-${Math.random().toString(36).slice(2, 8)}`;
}

async function api<T>(method: string, path: string, body?: unknown, token = ADMIN_TOKEN): Promise<T> {
  const res = await fetch(`${CONTROL_URL}${path}`, {
    method,
    headers: {
      "content-type": "application/json",
      ...(token ? { authorization: `Bearer ${token}` } : {}),
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`${method} ${path} -> ${res.status}: ${text.slice(0, 300)}`);
  }
  return (await res.json()) as T;
}

type WithId = { id: string };

// seed org -> team -> project -> admin user, then log that user in to obtain a
// real session token (what the self-service /me/* endpoints and the UI need).
export async function seedTenant(): Promise<SeededTenant> {
  const slug = rand("e2e");
  const org = await api<WithId>("POST", "/api/v1/orgs", { name: rand("e2e-org"), slug });
  const team = await api<WithId>("POST", `/api/v1/orgs/${org.id}/teams`, { name: rand("e2e-team") });
  const project = await api<WithId>("POST", `/api/v1/teams/${team.id}/projects`, {
    name: rand("e2e-proj"),
  });

  const email = `${rand("e2e-admin")}@e2e.test`;
  const password = `e2e-${Math.random().toString(36).slice(2, 12)}`;
  await api("POST", `/api/v1/orgs/${org.id}/users`, { email, password, role: "admin" });

  // real login → session token (no admin token; this is the user's own session)
  const auth = await api<{ token: string }>(
    "POST",
    "/api/v1/auth/login",
    { email, password },
    "",
  );

  return {
    orgId: org.id,
    teamId: team.id,
    projectId: project.id,
    email,
    password,
    token: auth.token,
  };
}
