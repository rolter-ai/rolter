# Viewer

**Viewer at a project, a team or the org.** A stakeholder, product manager,
support engineer or auditor: reads the dashboard, the request log and the
configuration, changes nothing that belongs to anyone else. Two things are the
viewer's own, and the product should let them manage both: **their account**
(profile, preferences, second factor) and **their view** of the data (the
filters they keep coming back to).

**Goal:** "I can see what I am responsible for, in the shape I need it, and keep
that shape — without being able to break anything."

**Dogfood account:** `viewer@rolter.local`, viewer at project `default/default`.
`finops@rolter.local` is an org-level viewer for comparison. See
[dogfood data](../user-journeys.md#dogfood-data).

**The observer watches:** `refused_click` in `ui_events`. A viewer reaching for a
disabled control is fine once — reaching for it twice means the screen did not
say clearly enough that it is read-only.

## V1 — see the project

| #    | step                                        | where                                                           | expect                                                                           | status           |
| ---- | ------------------------------------------- | --------------------------------------------------------------- | -------------------------------------------------------------------------------- | ---------------- |
| V1.1 | sign in                                     | login screen                                                    | the dashboard for project `default/default`                                      | bug — #1846      |
| V1.2 | traffic, spend and latency                  | **Observability → Dashboard**                                   | the project's numbers only                                                       | verified         |
| V1.3 | a request in detail                         | **LLM Logs** → a row                                            | status, tokens, cost, target; the bodies say they are hidden for the viewer role | verified         |
| V1.4 | the bodies too, where the project allows it | a project admin turns on **Viewers can read captured payloads** | the same row now shows the request and the response                              | verified (#1820) |

## V2 — own the account

| #    | step                         | where                                                      | expect                                                  | status                        |
| ---- | ---------------------------- | ---------------------------------------------------------- | ------------------------------------------------------- | ----------------------------- |
| V2.1 | set a display name and a bio | —                                                          | others see the name; the bio says who to ask about what | gap — #1823                   |
| V2.2 | enrol a second factor        | **Settings → My Virtual Keys → Two-factor authentication** | TOTP and recovery codes, same as any role               | verified (as a member, E10.4) |

## V3 — preferences

| #    | step                                    | where               | expect                                                           | status          |
| ---- | --------------------------------------- | ------------------- | ---------------------------------------------------------------- | --------------- |
| V3.1 | switch the dashboard language           | the language picker | the whole dashboard in Russian — remembered in this browser only | partial — #1824 |
| V3.2 | land on the right scope on a new laptop | the scope switcher  | the last scope, from the server rather than `localStorage`       | partial — #1824 |

## V4 — read the configuration without changing it

| #    | step                                      | where                                | expect                                                                                                                       | status                                                   |
| ---- | ----------------------------------------- | ------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------- |
| V4.1 | which models, routes and strategies exist | **Model Catalog**, **Routing Rules** | readable; every edit, delete and toggle disabled, naming the role it needs                                                   | bug — #1846 (no project in scope, so the screen refuses) |
| V4.2 | which keys exist and what they may reach  | **Governance → Virtual Keys**        | names, prefixes, allow-lists; never a secret                                                                                 | bug — #1846                                              |
| V4.3 | which budgets and limits apply            | **Budgets & Limits**                 | readable, not editable                                                                                                       | bug — #1846                                              |
| V4.4 | who may do what                           | **Governance → Roles & Permissions** | the published matrix, including this session's `analytics`, `request_payload`, `provider_health` and `project_settings` rows | bug — #1846 (the API publishes all four rows)            |

## V5 — keep my own view

| #    | step                                                | where | expect                                               | status      |
| ---- | --------------------------------------------------- | ----- | ---------------------------------------------------- | ----------- |
| V5.1 | save "errors on gpt-4o this week" as a named filter | —     | one click back to it on any browser                  | gap — #1825 |
| V5.2 | the saved filter is mine alone                      | —     | nobody else sees it; it never widens what I can read | gap — #1825 |
