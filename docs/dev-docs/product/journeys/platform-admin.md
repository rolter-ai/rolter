# Platform operator and org admin

The person who stands rolter up and hands it to everyone else. In a small shop
that is one person holding **superadmin**; in a larger one it splits into a
**platform operator** (superadmin: the deployment, its policies and its fleet) and
an **org admin** (admin at the org: its providers, people, keys and budgets). The
scripts mark which of the two a step needs.

**Goal:** from nothing to "our people and our applications call our models
through rolter, inside limits we chose, and we can see what they did."

**Dogfood accounts:** `dev@rolter.local` (superadmin), `orgadmin@rolter.local`
(admin at org `default`). See [dogfood data](../user-journeys.md#dogfood-data).

**The observer watches:** `audit_log` for every mutation below, `ui_events` for
the Getting started card and the sheets, and **Cluster Config** for the config
version reaching the gateway after each change.

## A0 — choose the install shape

The first fork, decided by one question: **who will use it, and from where?**

| branch | when                                             | shape                                                                                           |
| ------ | ------------------------------------------------ | ----------------------------------------------------------------------------------------------- |
| A0-a   | "I want to see it work" — one person, one laptop | `rolter easy-up`: no database, no keys, loopback only                                           |
| A0-b   | a team on one host                               | `docker compose -f docker/docker-compose.yml up -d`: Postgres, Redis, ClickHouse, both planes   |
| A0-c   | production                                       | Helm (`charts/rolter`), managed Postgres/Redis/ClickHouse, TLS in front, secrets from a manager |
| A0-d   | no internet at all                               | A0-b or A0-c from mirrors ([air-gapped](../../deployment/air-gapped.md))                        |

### A0-a — five minutes, no credentials

| #    | step                          | where                                                 | expect                                                                            | status   |
| ---- | ----------------------------- | ----------------------------------------------------- | --------------------------------------------------------------------------------- | -------- |
| A0.1 | start everything              | `rolter easy-up`                                      | both planes up; the startup banner prints a ready-to-run `curl` for `fake-llm`    | works    |
| A0.2 | call the built-in model       | the printed `curl`, or the dashboard's **Playground** | a deterministic lorem-ipsum completion; no provider or key configured             | verified |
| A0.3 | see the request               | **Observability → LLM Logs** (needs ClickHouse)       | the row, with model `fake-llm` and a latency                                      | works    |
| A0.4 | notice the deployment is open | the dashboard banner                                  | an open-mode banner: no admin token, so anyone who reaches the port is superadmin | works    |

### A0-b / A0-c — a real deployment

| #     | step                                       | where                                                                              | expect                                                                        | status                                                   |
| ----- | ------------------------------------------ | ---------------------------------------------------------------------------------- | ----------------------------------------------------------------------------- | -------------------------------------------------------- |
| A0.5  | generate the deployment secrets            | `rolter init` ([preflight](../../deployment/preflight-validation.md))              | admin token, KEK and both peppers written somewhere they outlive a restart    | works                                                    |
| A0.6  | bring the stack up                         | `docker compose … up -d`, or `helm install`                                        | Postgres, Redis, ClickHouse healthy; migrations applied on control-plane boot | verified (compose); partial on constrained hosts — #1819 |
| A0.7  | check the configuration before trusting it | `rolter check`                                                                     | no open mode on a non-loopback bind, KEK present, pepper present              | works                                                    |
| A0.8  | create the first account                   | `rolter-seed --admin-email … --admin-password …`                                   | a superadmin that can sign in; the org, team and project `default` exist      | verified                                                 |
| A0.9  | sign in and enrol a second factor          | dashboard sign-in, then **Settings → My Virtual Keys → Two-factor authentication** | TOTP enrolled, ten recovery codes shown once                                  | works                                                    |
| A0.10 | confirm the planes agree                   | **Cluster Config**                                                                 | every gateway node live and converged on the current config version           | verified                                                 |

## A1 — decide how people sign in

The second fork: **does the company have an identity provider rolter should
trust?** Branches A1-a to A1-c are alternatives; A1-d and A1-e combine with any.

### A1-a — no identity provider: invitations and passwords

| #     | step                                       | where                                                                        | expect                                                                                     | status          |
| ----- | ------------------------------------------ | ---------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------ | --------------- |
| A1a.1 | invite a colleague at a scope and role     | **Governance → Users → Invite user** · `POST /api/v1/orgs/{org}/invitations` | a one-time link, valid for days, naming the role and scope it grants                       | verified        |
| A1a.2 | get the link to them                       | copy the link into chat or email by hand                                     | the invitee receives it                                                                    | partial — #1828 |
| A1a.3 | the invitee accepts and chooses a password | the link opens **Accept invitation**                                         | an account with exactly the invited role; the link is dead once used                       | verified        |
| A1a.4 | require a second factor for the org        | **Governance → Single Sign-On → Org sign-in policy** (`mfa_policy`)          | members without a factor are refused a session until they enrol (the confirmation says so) | partial — #1852 |

### A1-b — an OIDC identity provider (Okta, Entra ID, Google, Keycloak)

| #     | step                                           | where                                                                     | expect                                                                             | status      |
| ----- | ---------------------------------------------- | ------------------------------------------------------------------------- | ---------------------------------------------------------------------------------- | ----------- |
| A1b.1 | create an OIDC web app in the IdP              | the IdP, redirect URI `https://<control>/auth/sso/<slug>/callback`        | a client id and secret                                                             | works       |
| A1b.2 | register it                                    | **Governance → Single Sign-On** · `POST /api/v1/orgs/{org}/sso-providers` | the provider card; the login screen offers **Continue with …**                     | works       |
| A1b.3 | map IdP groups to roles at scopes              | the provider card's **Group mappings**                                    | a user in `platform-eng` becomes admin of team `platform`, and nothing else        | works       |
| A1b.4 | decide what an unmapped user gets              | the provider's **default role** (empty refuses them)                      | an unmapped user is refused, or lands as the default role at the org               | works       |
| A1b.5 | turn passwords off for the org                 | **Org sign-in policy**: single sign-on only                               | one button on the login screen; the superadmin keeps a password as the break-glass | works       |
| A1b.6 | sign in from a private window as a mapped user | the login screen                                                          | the dashboard, scoped to what the mapping granted                                  | works       |
| A1b.x | the IdP speaks only SAML                       | —                                                                         | —                                                                                  | gap — #1827 |

### A1-c — an LDAP / Active Directory directory

| #     | step                                         | where | expect | status      |
| ----- | -------------------------------------------- | ----- | ------ | ----------- |
| A1c.1 | point rolter at the directory and bind users | —     | —      | gap — #1826 |

### A1-d — provisioning from the IdP (SCIM), with any of the above

| #     | step                                   | where                                            | expect                                                                | status      |
| ----- | -------------------------------------- | ------------------------------------------------ | --------------------------------------------------------------------- | ----------- |
| A1d.1 | issue an org-scoped provisioning token | **Governance → User Provisioning → Issue token** | the token, shown once, named after the IdP connector                  | verified    |
| A1d.2 | configure the IdP's SCIM connector     | the IdP                                          | a test user is created in rolter as an org **viewer**, nothing more   | verified    |
| A1d.3 | map IdP groups to teams and roles      | **Group mappings** on the same screen            | group membership in the IdP becomes a role in rolter on the next sync | works       |
| A1d.4 | deprovision the test user in the IdP   | the IdP                                          | the account is deactivated, its sessions dropped                      | verified    |
| A1d.5 | their personal keys stop working too   | the gateway                                      | a key the leaver minted is refused                                    | bug — #1841 |

### A1-e — custom roles and access profiles (optional)

| #     | step                                                        | where                                | expect                                                           | status   |
| ----- | ----------------------------------------------------------- | ------------------------------------ | ---------------------------------------------------------------- | -------- |
| A1e.1 | define a role that widens a built-in one by specific grants | **Governance → Roles & Permissions** | e.g. "viewer, plus create routes in project X"                   | verified |
| A1e.2 | hand it to people, with an optional model/route policy      | **Governance → Access Profiles**     | the grant shows in `GET /api/v1/rbac/effective` for those people | verified |

## A2 — lay out the tenancy

| #    | step                                                           | where                                                                                | expect                                                                        | status   |
| ---- | -------------------------------------------------------------- | ------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------- | -------- |
| A2.1 | create teams and projects that match how money and access flow | the scope switcher's **+** · `POST /api/v1/orgs/{org}/teams`, `/teams/{id}/projects` | the chain appears in every screen's scope                                     | verified |
| A2.2 | decide who reads prompts, per project                          | the gear beside the project → **Viewers can read captured payloads**                 | off by default: members and admins read bodies, viewers see rows only (#1820) | verified |
| A2.3 | create business units and customers for attribution            | **Governance → Business Units**, **Customers**                                       | slugs spend can be recorded against                                           | verified |

## A3 — connect models

The third fork, and the one with the most branches: **how many models, from how
many places, and does traffic need spreading?**

| branch | shape                                                       | balancing                                                         |
| ------ | ----------------------------------------------------------- | ----------------------------------------------------------------- |
| A3-a   | one hosted provider, one or two models                      | none — one target per route                                       |
| A3-b   | the same model from two providers                           | failover: retries and cooldowns move traffic off a failing target |
| A3-c   | several vendors behind named routes                         | none or failover, per route                                       |
| A3-d   | a self-hosted fleet, many replicas                          | load-balanced: a strategy per route                               |
| A3-e   | a hosted API reached through a proxy or a re-implementation | as A3-a, with the host pin relaxed                                |

### A3-a — one provider, one route

| #    | step                                      | where                                                                               | expect                                                                                      | status                                                 |
| ---- | ----------------------------------------- | ----------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------- | ------------------------------------------------------ |
| A3.1 | add the provider with its key             | **Models → Model Providers → + Add provider** · `POST /api/v1/orgs/{org}/providers` | the key is sealed with the KEK and never shown again                                        | verified                                               |
| A3.2 | check it before anything depends on it    | the provider's **Test connection**                                                  | a model list from the upstream, or the reason there is none                                 | verified                                               |
| A3.3 | add a route: public name → provider/model | **Models → Routing Rules** · `POST /api/v1/projects/{id}/routes`                    | the public name appears in **Model Catalog** and in `/v1/models` for keys that may reach it | verified (seed)                                        |
| A3.4 | try it                                    | **Playground**, the new model                                                       | an answer; a row in **LLM Logs** naming the provider and the cost                           | bug — #1853; #1847 for a superadmin with no membership |

### A3-b — the same model from two providers, with failover

| #    | step                              | where                                                  | expect                                                                          | status                                                                       |
| ---- | --------------------------------- | ------------------------------------------------------ | ------------------------------------------------------------------------------- | ---------------------------------------------------------------------------- |
| A3.5 | give the route two targets        | **Routing Rules**, the route's targets                 | both targets listed with weights                                                | verified                                                                     |
| A3.6 | set the retry budget and cooldown | **Settings → Performance Tuning**, **Circuit Breaker** | a transient 5xx on one target is retried on the other before the caller sees it | verified (dogfood `deepseek-r1`: `vllm-spot-01` 503s never reached a caller) |
| A3.7 | watch a target trip               | **Circuit Breaker**, **Observability → Dashboard**     | the failing target's breaker opens, traffic moves, and it is probed back in     | works                                                                        |

### A3-c — several vendors behind named routes

| #     | step                                                         | where                                    | expect                                                                              | status   |
| ----- | ------------------------------------------------------------ | ---------------------------------------- | ----------------------------------------------------------------------------------- | -------- |
| A3.8  | one provider per vendor, one route per public model name     | as A3-a, repeated                        | clients ask for `gpt-4o` or `claude-sonnet-4` and never learn which vendor answered | verified |
| A3.9  | pin defaults on a route and decide what callers may override | the route's params and **param policy**  | `temperature` defaulted, `max_tokens` capped, overrides allowed or denied           | works    |
| A3.10 | fill in what callers omit, deployment-wide                   | **Models → Model Settings** (superadmin) | a default temperature and fallback model, never overwriting a caller's value        | works    |

### A3-d — a self-hosted fleet, load-balanced

| #     | step                                   | where                                                     | expect                                                                                                                                                                                            | status                                                                            |
| ----- | -------------------------------------- | --------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------- |
| A3.11 | describe the fleet as a file           | `rolter.toml`: providers, provider groups, routes, prices | —                                                                                                                                                                                                 | works                                                                             |
| A3.12 | apply it as desired state              | `rolter-seed --import rolter.toml`                        | rows created or updated to match the file; the gateway picks them up within its poll interval                                                                                                     | verified (4 s)                                                                    |
| A3.13 | notice what the import did not apply   | the import's output                                       | sections it does not import (`[adaptive_routing]`, `[retry]`, budgets, keys) named                                                                                                                | gap — #1818                                                                       |
| A3.14 | choose a strategy per route            | the route's **strategy**                                  | round robin for identical replicas, `cache_aware` for prefix-heavy chat, `fastest`/`predicted_latency` for latency, `weighted` for a canary, `adaptive` to let live latency, cost and load decide | verified, except `cache_aware`: it sends every request to one replica — bug #1851 |
| A3.15 | address a whole group                  | `vllm-a100/meta-llama/Llama-3.1-8B-Instruct`              | the group's members share the traffic                                                                                                                                                             | verified                                                                          |
| A3.16 | turn adaptive routing on               | **Adaptive Routing → Settings** (superadmin)              | `deepseek-r1` engages once it has samples                                                                                                                                                         | works; dogfood never engages — #1817                                              |
| A3.17 | take the live state back out as a file | `rolter config export --output rolter.toml`               | an importable file with no credential in it                                                                                                                                                       | verified                                                                          |

### A3-e — a hosted API behind a proxy or a local re-implementation

| #     | step                                                    | where                          | expect                           | status                                                                                               |
| ----- | ------------------------------------------------------- | ------------------------------ | -------------------------------- | ---------------------------------------------------------------------------------------------------- |
| A3.18 | point a hosted kind (e.g. `openrouter`) at another host | `allow_custom_api_base = true` | the provider reaches the gateway | works in a gateway-only file; gap in the database — #1133 (the dogfood `claude-sonnet-4` route 404s) |

## A4 — give access to people and applications

| #    | step                                     | where                                                                     | expect                                                                                          | status                                                          |
| ---- | ---------------------------------------- | ------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------- | --------------------------------------------------------------- |
| A4.1 | make engineers members of their projects | A1-a invitations, A1-b mappings or A1-d groups                            | they can mint their own keys (see [engineer E2](engineer.md))                                   | works                                                           |
| A4.2 | mint a shared key for an application     | **Governance → Virtual Keys** · `POST /api/v1/projects/{id}/virtual-keys` | shown once; models and providers allow-lists, expiry, business unit and customer set on the key | verified                                                        |
| A4.3 | rotate that key later                    | —                                                                         | a new secret on the same key with an overlap window                                             | partial — #1837 (mint a second key, redeploy, delete the first) |
| A4.4 | automate the control plane from CI       | —                                                                         | a token scoped to one project                                                                   | gap — #1836 (the admin token is superadmin)                     |

## A5 — spend and budgets

| #    | step                                                    | where                                                                | expect                                                         | status                      |
| ---- | ------------------------------------------------------- | -------------------------------------------------------------------- | -------------------------------------------------------------- | --------------------------- |
| A5.1 | price every model that costs money                      | **Models → Pricing Overrides** (superadmin), or `[[model_prices]]`   | `cost_usd` on every row; nothing counts as free by accident    | works                       |
| A5.2 | find unpriced traffic                                   | **Dashboard** (unpriced share), **LLM Logs** (unpriced flag)         | the dogfood fleet shows 12 unpriced models — every fake route  | verified                    |
| A5.3 | cap spend per org, team, project, key, unit or customer | **Models → Budgets & Limits → Add budget** (admin at that scope)     | the next request past the cap gets HTTP 402; counters in Redis | verified                    |
| A5.4 | cap throughput                                          | **Add rate limit**                                                   | HTTP 429 with `Retry-After`                                    | verified                    |
| A5.5 | hear about it before the cap                            | —                                                                    | a warning at a threshold                                       | gap — #337                  |
| A5.6 | alert on spend velocity                                 | **Alerting → Rules**, `spend_velocity` (superadmin, deployment-wide) | a webhook when spend per hour crosses the line                 | works; scoped rules — #1829 |

## A6 and A7 — run it, and keep it safe

Day-2 operations (health, upgrades, drains, backups) are the
[DevOps script](devops.md); identity hardening, payload redaction and audit are
the [SecOps script](secops.md). An admin who is both walks those next.
