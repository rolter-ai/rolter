# User journeys

How the people who use rolter get from "I have a job to do" to "it is done", written
as scripts: one per person and goal, step by step, with the branches a real
deployment takes. They exist for three reasons at once:

- **Planning.** Each script describes the journey as it _should_ go. Where the
  product does not support a step yet, the step says so and links the issue, so
  the gaps read as a list in the [gap register](#gap-register) instead of being
  rediscovered by every new user.
- **Testing.** The scripts are what the pre-1.0 live pass (#1789) walks, on the
  [dogfooding fleet](../development/dogfooding-fleet.md): headless by an agent
  driving the dashboard in Chromium plus the API, and by a real person while an
  observer watches the telemetry. See [running a script](#running-a-script).
- **Onboarding.** The steps that already work end to end are also the public
  [guides](#public-guides), minus the gap tracking.

The user-facing docs describe _features_; these describe _people_. When the two
disagree about what a person can do, one of them is wrong — file it.

## The people

rolter has three scoped roles — **viewer**, **member**, **admin** — granted at an
org, a team or a project, plus the deployment-wide **superadmin** (the admin token,
or an account with the superadmin bit). Custom roles and access profiles can only
widen a scoped role (see [RBAC & auth](../architecture/rbac-and-auth.md)). Every
persona below is one of those, at one scope:

| Persona                           | Role today                          | Script                                          | Their question                                                               |
| --------------------------------- | ----------------------------------- | ----------------------------------------------- | ---------------------------------------------------------------------------- |
| Platform operator / org admin     | superadmin, or admin at the org     | [platform-admin.md](journeys/platform-admin.md) | "How do I stand this up, connect our models and hand it to people?"          |
| Team lead                         | admin at a team                     | [team-lead.md](journeys/team-lead.md)           | "How do I give my team models, keep them inside a budget, and see the bill?" |
| LLM / ML / software engineer      | member at a project                 | [engineer.md](journeys/engineer.md)             | "How do I get a key, call models, and see why my call failed?"               |
| Application / service account     | a virtual key (no human)            | [app-service.md](journeys/app-service.md)       | "What happens to my traffic, my limits and my key over time?"                |
| Finance / FinOps                  | viewer at the org (admin to cap)    | [finops.md](journeys/finops.md)                 | "What did we spend, on what, for whom — and can it stop at a limit?"         |
| SecOps                            | superadmin (a narrower role: #1834) | [secops.md](journeys/secops.md)                 | "Can I prove who reads what, and that secrets never land in a log?"          |
| DevOps / SRE (maintenance only)   | superadmin (a narrower role: #1834) | [devops.md](journeys/devops.md)                 | "Is it healthy, and can I upgrade, drain and restore it without a surprise?" |
| Viewer (stakeholder, PM, auditor) | viewer at a project, team or org    | [viewer.md](journeys/viewer.md)                 | "What is happening here — and can I keep my own view of it?"                 |

SecOps and DevOps are superadmin today only because nothing narrower reaches the
deployment-wide settings they need. Both scripts start from that and mark it as
the first gap.

## The simplest paths

Most people only ever need one of these. Every longer script is a branch off one
of them.

1. **Five minutes, no credentials.** `rolter easy-up`, open the dashboard, send a
   Playground message to the built-in `fake-llm` model, open **LLM Logs** and see
   the row. Proves the gateway, the dashboard and the log pipeline before a
   single provider key exists. ([platform-admin A0-a](journeys/platform-admin.md#a0--choose-the-install-shape))
2. **One hour to a team deployment.** Install with Postgres, Redis and ClickHouse,
   sign in as the seeded admin, follow the dashboard's **Getting started** card —
   connect a provider, add a route, mint a key — and hand people either an
   invitation or single sign-on. ([platform-admin A0-b to A4](journeys/platform-admin.md))
3. **Configuration as code.** Keep providers, groups, routes, prices and prompt
   templates in a `rolter.toml`, apply it with `rolter-seed --import` (desired
   state), and read the live state back with `rolter config export`. What the
   file cannot carry — keys, budgets, SSO, policy — stays in the dashboard or the
   API. ([platform-admin A3-d](journeys/platform-admin.md#a3--connect-models))
4. **An engineer's first call.** Sign in, **Settings → My Virtual Keys → Generate virtual key**,
   point the OpenAI or Anthropic SDK (or a coding agent) at the gateway with that
   key. ([engineer E1–E4](journeys/engineer.md)) Until #1846, the dashboard half
   works only for someone who also holds a role at the org: an invitation to a
   project grants nothing above it.

## How to read a script

Each script is a table of steps. A step names what the person does, where they do
it — the dashboard path first, the API equivalent second, so the same step can be
run by hand, by a browser driver or by `curl` — what they should see, and its
status:

| status       | meaning                                                                                         |
| ------------ | ----------------------------------------------------------------------------------------------- |
| **verified** | passed in a recorded dogfood run (the [run log](#run-log) names the date)                       |
| **works**    | the product supports it and nothing is known to be wrong, but no recorded run has walked it yet |
| **partial**  | it can be done, with a workaround the step spells out; the linked issue removes the workaround  |
| **gap**      | the product cannot do this yet; the linked issue is the plan                                    |
| **bug**      | it should work and does not; the linked issue is the bug                                        |

Where the path forks — no identity provider or OIDC, one model or a fleet — the
script names the fork, the question that decides it, and gives each branch its
own steps (`A1-a`, `A1-b`, …). A branch is walked in a run only when the run is
testing that branch; the rest are still read, because a branch nobody walks
rots.

## Dogfood data

Every script runs against the stack `just dogfood` brings up
([the dogfooding fleet](../development/dogfooding-fleet.md)). Seed the fleet and
the personas once:

```bash
just dogfood                                    # the stack, SigNoz, the sheet
just dogfood-seed                               # 15 fake providers, 3 groups, 11 routes
just dogfood-personas                           # one account per persona
```

`personas.sh` is idempotent and prints the roster. Every account shares the
dogfood password from `integration/dogfood/creds.env`:

| account                  | role   | scope                      | persona                            |
| ------------------------ | ------ | -------------------------- | ---------------------------------- |
| `dev@rolter.local`       | super  | deployment                 | platform operator, DevOps, SecOps  |
| `orgadmin@rolter.local`  | admin  | org `default`              | org admin                          |
| `lead@rolter.local`      | admin  | team `default`             | team lead                          |
| `engineer@rolter.local`  | member | project `default/default`  | engineer                           |
| `engineer2@rolter.local` | member | project `research/sandbox` | an engineer on a different project |
| `viewer@rolter.local`    | viewer | project `default/default`  | viewer                             |
| `finops@rolter.local`    | viewer | org `default`              | FinOps                             |

The fleet lives in project `default/default`: routes `gpt-4o`, `gpt-4o-mini`,
`claude-sonnet-4`, `llama-3.1-8b` (round robin), `llama-3.1-8b-cached`
(cache-aware), `llama-3.1-8b-fastest`, `qwen-32b` (power of two), `mistral-7b`
(predicted latency), `gemma-27b` (weighted), `deepseek-r1` (adaptive) and
`text-embedding`, plus group addresses such as `vllm-a100/…`. Three targets are
bad on purpose — `vllm-a100-03` is slow, `vllm-spot-01` answers 503 a quarter of
the time, `vllm-spot-02` takes 1.4 s to first token — so failure, retry and
latency screens have something to show. `research/sandbox` has no routes; it
exists so one project's people can be shown not seeing another's traffic.

## Running a script

### Headless — an agent drives it

This is how a script runs when nobody can open the dashboard: the stack is in a
cloud container, or the run is a regression pass.

1. Bring the stack up and seed it (above). Mint nothing by hand: every key and
   account a script needs is made by its own steps or by `personas.sh`.
2. Walk the steps as the persona. **Dashboard steps** are driven in headless
   Chromium (Playwright) signed in as the persona's account — a screenshot of
   each step's end state is kept, because "the button was there" is not the same
   claim as "the button was usable". **API steps** use `curl` with the persona's
   session (`POST /api/v1/auth/login`) or the virtual key the script minted.
3. Check every **Expect** against the source of truth rather than the screen:
   the API's answer, a `request_logs` row in ClickHouse, an `audit_log` row in
   Postgres.
4. Record the result per step in the [run log](#run-log). A failed step is either
   a bug in rolter (file it), a wrong script (fix the script in the same PR as
   the run log), or a known gap (already linked).

`just dogfood-journeys` does steps 2 and 3 for every script: one runner per
persona in `integration/dogfood/journeys/`, results and screenshots in
`integration/dogfood/.journeys/`, and a `summary.md` to copy into the run log.
`just dogfood-screens` adds the persona × screen matrix. See
[walking it as someone else](../development/dogfooding-fleet.md#walking-it-as-someone-else).
A runner step and its script step share an id; when a script changes, change
its runner in the same PR.

### Watched — a person drives it, an observer watches

This is the #1789 pass: someone who did not build the screen uses it, and the
struggle is the finding.

1. The person runs the stack locally (`just dogfood`) or on a shared host and
   signs in as the persona. They get the script's **goal**, not its steps — the
   steps are what the observer checks them against.
2. The observer runs `just dogfood-watch` (live `ui_events` and gateway traffic),
   keeps SigNoz open on `rolter · dashboard UX`, and notes every place the
   person hesitates, backs out, reaches for a refused control (`refused_click`)
   or abandons a form (`form_abandon`).
3. Afterwards the two walk the script together and mark each step. A step the
   person completed by a route the script did not foresee is a finding too: the
   script, or the screen, is wrong about how people think.

### What the observer watches

| signal                    | where                                                  | tells you                                                       |
| ------------------------- | ------------------------------------------------------ | --------------------------------------------------------------- |
| screen views and struggle | ClickHouse `ui_events`, `just dogfood-watch`           | which screen, how long to interactive, refused clicks, abandons |
| requests                  | ClickHouse `request_logs`, **LLM Logs**                | what reached the gateway, the status, the target, the cost      |
| who did what              | Postgres `audit_log`, **Governance → Audit Logs**      | every mutation a step claims to make                            |
| traces                    | SigNoz, `rolter · overview`                            | where a slow step spent its time                                |
| the gateway's own view    | `GET :4000/metrics`, **Cluster**, **Adaptive Routing** | config version, breaker state, queue, adaptive engagement       |

## Run log

| date       | mode     | scripts                                                         | result                                                                                                                                                                                                                                       |
| ---------- | -------- | --------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 2026-09-24 | headless | smoke over A0, A3, P2, P5 (API only)                            | stack up; `fake-llm` on every dialect; 11/12 fleet routes answer; found #1815, #1816, #1817, #1818, #1819, and the unauthenticated analytics read fixed in #1820                                                                             |
| 2026-09-24 | headless | request-log visibility for every persona                        | after #1820: org admin all rows and bodies; team lead and engineer their project's rows and bodies, no provider health (#1833); `engineer2` nothing; viewer and FinOps rows with bodies withheld                                             |
| 2026-09-24 | headless | every screen as every persona account (49 screens × 7 accounts) | org admin 32 usable, FinOps 28; team lead, both engineers and the viewer 5, because their scope switcher stops at the org (#1846); the superadmin cannot open the Playground (#1847); non-admins get the setup checklist as an error (#1848) |
| 2026-09-24 | headless | all eight scripts, 94 steps, `just dogfood-journeys`            | 59 pass, 17 bug, 10 partial, 8 gap. New: #1844–#1856. Script claims the run disproved were corrected: A1a.4, S2.1, P1.2, P2.4, A3.14                                                                                                         |

Add a row per run. The full walk of every script is the #1789 pass.

## Gap register

Every step marked **partial**, **gap** or **bug**, in one place. Milestones and
priorities live on the issues.

| issue | what is missing                                                                      | blocks                                         |
| ----- | ------------------------------------------------------------------------------------ | ---------------------------------------------- |
| #1846 | team- and project-scoped members cannot select their own scope in the dashboard      | E1–E3, T1, V1, V4: every persona below the org |
| #1844 | a route with no visibility list is served to virtual keys of every org               | A3, A4 on any multi-org deployment             |
| #1845 | a same-named route in a second project freezes config for every gateway              | A3, T3                                         |
| #1847 | a superadmin with no membership cannot open the Playground or mint a personal key    | A3.4                                           |
| #1853 | the Playground asks for models before its key is live, then defaults to a dead route | A3.4, E3.1                                     |
| #1851 | `cache_aware` sends every request to one replica and never checks load               | A3-d, P2.4, P5                                 |
| #1850 | a team admin cannot list their own team's members                                    | T2, T6                                         |
| #1852 | no second-factor enrolment at sign-in when an org requires one                       | A1a.4, S2.1                                    |
| #1854 | sign-in and second-factor audit rows are written with no org, so nothing reads them  | S2, S6                                         |
| #1856 | a flag stored on but unavailable blocks every feature-flag change                    | D3.3                                           |
| #1848 | the Dashboard's setup checklist shows an access error to everyone below admin        | E1, V1, F1                                     |
| #1849 | no lookup by the `x-request-id` a client received                                    | E5.2                                           |
| #1395 | requests refused before routing (401, 402, 403, 429) never reach LLM Logs            | E5, P5.4                                       |
| #1855 | no per-provider queue depth or in-flight metric                                      | D2.4, D3.1                                     |
| #1815 | the provider queue serialises every provider to one in-flight request                | P5, D3 — any concurrent traffic                |
| #1822 | self-service account area (epic)                                                     | E10, V2, V3, V5                                |
| #1823 | display name and bio, editable by the account itself                                 | V2, E10                                        |
| #1824 | preferences and defaults stored server-side                                          | V3, E10                                        |
| #1825 | saved filter presets on LLM Logs and the Dashboard                                   | V5, F2                                         |
| #1831 | members cannot read their own MCP tool-call logs                                     | E6                                             |
| #1841 | a deprovisioned person's own virtual keys keep working                               | T6, S2                                         |
| #1840 | anonymous `/api/v1/config` still describes the upstream topology                     | S1.3                                           |
| #1133 | a database-backed provider cannot opt out of the hosted host pin                     | A3-e                                           |
| #1826 | LDAP is implemented but not reachable from sign-in                                   | A1-c                                           |
| #1827 | no SAML single sign-on                                                               | A1-b (SAML-only IdPs)                          |
| #1828 | invitations are links an admin passes on by hand                                     | A1-a, T2                                       |
| #1829 | alert rules are superadmin-only and deployment-wide                                  | A5, T5, F4                                     |
| #337  | no warning before a budget blocks                                                    | A5, T4, F4                                     |
| #1830 | no budget per person across their keys                                               | T4, F3, E9                                     |
| #1833 | project members cannot see the health of their own routes' providers                 | E5, T5                                         |
| #1834 | no operator or security-auditor role short of superadmin                             | D0, S0                                         |
| #1835 | payload redaction matches key names, not secrets inside text; no preview             | S3                                             |
| #1085 | one subject's logs and captured payloads cannot be erased on request                 | S3.6                                           |
| #1836 | no scoped control-plane token for automation                                         | P4, D6                                         |
| #1837 | shared project keys cannot be rotated in place                                       | P3                                             |
| #1838 | no CSV export of spend or logs                                                       | F5                                             |
| #1816 | a request abandoned while waiting on its upstream is logged with no provider         | D4, E5                                         |
| #1817 | the dogfood fleet's adaptive route never engages                                     | A3-d (adaptive), D2                            |
| #1818 | `rolter-seed --import` is silent about sections it skips                             | A3-d                                           |
| #1819 | compose ClickHouse fails where `nofile` cannot reach 262144                          | A0-b on constrained hosts                      |
| #1832 | raw MCP traffic inspection for local servers (mcp-snoop)                             | E7                                             |
| #1839 | a member cannot route their own local model through rolter (idea)                    | E8                                             |

## Public guides

The paths that already work end to end are published as user-facing guides in
`docs/user-docs/guides/`, without the gap tracking: the admin's first hour,
connecting models, an engineer's first call, and tracking spend. When a gap here
closes, the matching guide gains the step.
