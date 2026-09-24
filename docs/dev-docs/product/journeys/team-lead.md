# Team lead

**Admin at a team.** Owns the team's projects, the people in them, what they may
call and what it costs — and nothing outside the team. Providers are the org's:
a team lead routes to them but does not add them (provider mutations take admin
at the org, by design).

**Goal:** "my team has the models it needs, inside a budget I set, and I can see
who spent what — without asking the platform team for every change."

**Dogfood account:** `lead@rolter.local`, admin at team `default` (which holds the
fleet's project `default`). See [dogfood data](../user-journeys.md#dogfood-data).

**The observer watches:** `refused_click` in `ui_events` — every control a team
lead reaches for that is an org admin's is a finding about what the screen
offered, even when the refusal is correct.

## T1 — arrive

| #    | step                               | where                                         | expect                                                                                              | status                                               |
| ---- | ---------------------------------- | --------------------------------------------- | --------------------------------------------------------------------------------------------------- | ---------------------------------------------------- |
| T1.1 | sign in (SSO or password)          | login screen                                  | the dashboard, scoped to team `default`                                                             | bug — #1846                                          |
| T1.2 | see what the team is already doing | **Observability → Dashboard**, **LLM Logs**   | the team's traffic and spend, with captured bodies (admin at the team), and nothing of other teams' | verified                                             |
| T1.3 | see what the team may call         | **Models → Model Catalog**, **Routing Rules** | the routes of the team's projects                                                                   | bug — #1846 (Routing Rules refuses; the API answers) |

## T2 — bring the team in

| #    | step                                               | where                                                                     | expect                                                                  | status                                           |
| ---- | -------------------------------------------------- | ------------------------------------------------------------------------- | ----------------------------------------------------------------------- | ------------------------------------------------ |
| T2.1 | create a project for a new workstream              | scope switcher **+** under the team · `POST /api/v1/teams/{id}/projects`  | the project, owned by the team                                          | verified (API); the switcher's **+** needs #1846 |
| T2.2 | invite engineers as members of that project        | **Governance → Users → Invite user**, scope = the project, role = member  | a link per person; authorized at the project, so no org admin is needed | verified                                         |
| T2.3 | get the links to people                            | by hand                                                                   | —                                                                       | partial — #1828                                  |
| T2.4 | with SSO or SCIM instead, map the team's IdP group | done by an org admin once ([platform-admin A1-b/A1-d](platform-admin.md)) | new joiners land in the project with no invitation                      | works                                            |

## T3 — give the team models

| #    | step                                              | where                                   | expect                                                                                 | status               |
| ---- | ------------------------------------------------- | --------------------------------------- | -------------------------------------------------------------------------------------- | -------------------- |
| T3.1 | route a public model name to an existing provider | **Routing Rules** in the team's project | allowed: routes are project-scoped and the lead is admin above the project             | verified             |
| T3.2 | add a new provider                                | **Model Providers → + Add provider**    | refused, naming the Admin role at the org — a request to the platform team             | verified (by design) |
| T3.3 | mint a shared key for the team's service          | **Governance → Virtual Keys**           | as [platform-admin A4.2](platform-admin.md#a4--give-access-to-people-and-applications) | verified             |

## T4 — keep it inside a budget

| #    | step                                            | where                                                                        | expect                                                                                                      | status      |
| ---- | ----------------------------------------------- | ---------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------- | ----------- |
| T4.1 | cap the team's monthly spend                    | **Budgets & Limits → Add budget**, scope = the team · `POST /api/v1/budgets` | the team's requests get HTTP 402 once the month's spend reaches the cap                                     | verified    |
| T4.2 | cap each project's throughput                   | **Add rate limit**, scope = the project                                      | HTTP 429 with `Retry-After` past the cap                                                                    | verified    |
| T4.3 | give every engineer the same personal allowance | —                                                                            | "$50 a month each, however many keys they mint"                                                             | gap — #1830 |
| T4.4 | raise the budget mid-month                      | the budget row                                                               | budgets cannot be edited: delete and recreate (the counter is keyed by scope, so spend so far still counts) | partial     |
| T4.5 | hear before the team hits the cap               | —                                                                            | a warning at 80%                                                                                            | gap — #337  |

## T5 — see what happened

| #    | step                                             | where                                       | expect                                                                       | status      |
| ---- | ------------------------------------------------ | ------------------------------------------- | ---------------------------------------------------------------------------- | ----------- |
| T5.1 | spend by model and by key over the month         | **Dashboard**, **LLM Logs** filtered by key | the team's rows only                                                         | verified    |
| T5.2 | is an upstream the reason the team's calls fail? | **Circuit Breaker**, health rollups         | provider health is readable only with an org-level role — the lead sees none | gap — #1833 |
| T5.3 | an alert on the team's error rate                | —                                           | a rule the lead owns, over the team's traffic                                | gap — #1829 |

## T6 — someone leaves

| #    | step                                             | where                                                    | expect                                                   | status                                                    |
| ---- | ------------------------------------------------ | -------------------------------------------------------- | -------------------------------------------------------- | --------------------------------------------------------- |
| T6.1 | remove their membership                          | **Governance → Users**, the person's role at the project | they lose the project in the dashboard at once           | partial — #1850 (the lead cannot list the team's members) |
| T6.2 | their personal keys stop working                 | the gateway                                              | a key they minted for themselves is refused              | bug — #1841                                               |
| T6.3 | shared keys they minted as an admin keep working | the gateway                                              | the team's services do not go down because a person left | verified                                                  |
